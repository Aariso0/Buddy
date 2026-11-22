//! Capture engine: one reader thread per source and one writer thread for all
//! durable output. Reads only touch reused buffers and atomics; chunk and gap
//! publication happens on the writer thread.

use crate::chunks::{append_gap, write_chunk, GapMarker, GapReason};
use crate::ring::RingBuffer;
use crate::{AudioSource, CaptureError, TrackKind};
use buddy_core::clock::SessionClock;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const RUNNING: u8 = 0;
const PAUSED: u8 = 1;
const STOPPING: u8 = 2;

// A stop drains whatever the source has already buffered before its reader
// exits. The bound keeps a misbehaving backend from hanging stop() forever.
const STOP_DRAIN_LIMIT: Duration = Duration::from_millis(250);
const WRITER_IDLE_SLEEP: Duration = Duration::from_millis(1);
const MAX_DRAIN_SAMPLES: usize = 16_384;

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub chunk_frames: u32,
    pub ring_frames: usize,
    pub channels: u16,
    pub sample_rate: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            chunk_frames: 48_000,
            ring_frames: 8192,
            channels: 2,
            sample_rate: 48_000,
        }
    }
}

fn track_at(index: usize) -> TrackKind {
    match index {
        0 => TrackKind::System,
        _ => TrackKind::Mic,
    }
}

struct Control {
    state: AtomicU8,
    writer_finish: AtomicBool,
    overruns: [AtomicU64; 2],
    device_losses: [AtomicU64; 2],
    last_gap_start_ms: [AtomicU64; 2],
    // Only the writer touches this, and only when publication fails; the
    // capture read path never reaches a lock.
    writer_error: Mutex<Option<CaptureError>>,
}

impl Control {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            writer_finish: AtomicBool::new(false),
            overruns: std::array::from_fn(|_| AtomicU64::new(0)),
            device_losses: std::array::from_fn(|_| AtomicU64::new(0)),
            last_gap_start_ms: std::array::from_fn(|_| AtomicU64::new(0)),
            writer_error: Mutex::new(None),
        }
    }

    fn record_error(&self, error: CaptureError) {
        let mut guard = self
            .writer_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.is_none() {
            *guard = Some(error);
        }
    }
}

pub struct CaptureEngine {
    // Kept for Task 3's finalizer, which marks this session complete on a
    // clean stop.
    session_id: String,
    session_dir: PathBuf,
    config: CaptureConfig,
    clock: Arc<SessionClock>,
    control: Arc<Control>,
    system: Option<Box<dyn AudioSource>>,
    mic: Option<Box<dyn AudioSource>>,
    rings: [Arc<RingBuffer>; 2],
    readers: Vec<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    started: bool,
}

impl CaptureEngine {
    pub fn new(
        session_id: &str,
        session_dir: &Path,
        clock: SessionClock,
        system: Box<dyn AudioSource>,
        mic: Box<dyn AudioSource>,
        config: CaptureConfig,
    ) -> Self {
        let ring_frames = config.ring_frames;
        let channels = config.channels as usize;
        let ring = || Arc::new(RingBuffer::new(ring_frames, channels));
        Self {
            session_id: session_id.to_owned(),
            session_dir: session_dir.to_owned(),
            config,
            clock: Arc::new(clock),
            control: Arc::new(Control::new()),
            system: Some(system),
            mic: Some(mic),
            rings: [ring(), ring()],
            readers: Vec::new(),
            writer: None,
            started: false,
        }
    }

    pub fn start(&mut self) -> Result<(), CaptureError> {
        if self.started {
            return Err(CaptureError::Backend(format!(
                "capture already started for session {}",
                self.session_id
            )));
        }
        fs::create_dir_all(self.session_dir.join("audio"))?;
        self.control.state.store(RUNNING, Ordering::Release);
        self.control.writer_finish.store(false, Ordering::Release);

        self.writer = Some(thread::spawn({
            let session_dir = self.session_dir.clone();
            let config = self.config.clone();
            let clock = Arc::clone(&self.clock);
            let control = Arc::clone(&self.control);
            let rings = self.rings.clone();
            move || writer_loop(session_dir, config, clock, control, rings)
        }));

        let sources = [
            self.system.take().ok_or_else(|| {
                CaptureError::Backend("system source already consumed".to_owned())
            })?,
            self.mic
                .take()
                .ok_or_else(|| CaptureError::Backend("mic source already consumed".to_owned()))?,
        ];
        for (index, source) in sources.into_iter().enumerate() {
            self.readers.push(thread::spawn({
                let ring = Arc::clone(&self.rings[index]);
                let clock = Arc::clone(&self.clock);
                let control = Arc::clone(&self.control);
                move || reader_loop(source, ring, clock, control, index)
            }));
        }
        self.started = true;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), CaptureError> {
        self.require_started("pause")?;
        self.control.state.store(PAUSED, Ordering::Release);
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), CaptureError> {
        self.require_started("resume")?;
        self.control.state.store(RUNNING, Ordering::Release);
        Ok(())
    }

    /// Join both readers, then tell the writer to drain and publish. Returning
    /// only after the writer joins is what guarantees flushed, synced files on
    /// disk when the caller sees Ok.
    pub fn stop(&mut self) -> Result<(), CaptureError> {
        self.require_started("stop")?;
        self.control.state.store(STOPPING, Ordering::Release);
        for reader in std::mem::take(&mut self.readers) {
            reader
                .join()
                .map_err(|_| CaptureError::Backend("capture reader thread panicked".to_owned()))?;
        }
        self.control.writer_finish.store(true, Ordering::Release);
        let writer = self
            .writer
            .take()
            .ok_or_else(|| CaptureError::Backend("capture writer thread missing".to_owned()))?;
        writer
            .join()
            .map_err(|_| CaptureError::Backend("capture writer thread panicked".to_owned()))?;
        self.started = false;

        let error = self
            .control
            .writer_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn require_started(&self, operation: &str) -> Result<(), CaptureError> {
        if self.started {
            Ok(())
        } else {
            Err(CaptureError::Backend(format!(
                "cannot {operation} session {} before start",
                self.session_id
            )))
        }
    }
}

fn reader_loop(
    mut source: Box<dyn AudioSource>,
    ring: Arc<RingBuffer>,
    clock: Arc<SessionClock>,
    control: Arc<Control>,
    index: usize,
) {
    // One allocation when the thread starts, never inside the read loop.
    let mut buf = vec![0.0f32; ring.capacity_samples()];
    let mut paused = false;
    let mut stop_started: Option<std::time::Duration> = None;

    loop {
        match control.state.load(Ordering::Acquire) {
            PAUSED => {
                if !paused {
                    paused = source.pause().is_ok();
                }
                thread::sleep(WRITER_IDLE_SLEEP);
                continue;
            }
            STOPPING => {
                stop_started.get_or_insert_with(|| clock.now());
                if clock.now().saturating_sub(stop_started.unwrap()) > STOP_DRAIN_LIMIT {
                    break;
                }
            }
            _ => {
                if paused {
                    let _ = source.resume();
                    paused = false;
                }
            }
        }

        match source.read(&mut buf) {
            Ok(0) => break,
            Ok(count) => {
                let count = count.min(buf.len());
                if ring.push(&buf[..count]).is_err() {
                    let now_ms = clock.now().as_millis() as u64;
                    control.last_gap_start_ms[index].store(now_ms, Ordering::Release);
                    control.overruns[index].fetch_add(1, Ordering::AcqRel);
                }
            }
            Err(CaptureError::DeviceLost(_)) => {
                let now_ms = clock.now().as_millis() as u64;
                control.last_gap_start_ms[index].store(now_ms, Ordering::Release);
                control.device_losses[index].fetch_add(1, Ordering::AcqRel);
                // The source handle is owned by this thread. Re-arm it in
                // place: `open` is object-unsafe and the trait carries no
                // device id, so a backend-specific reopen belongs to the
                // backend task.
                let _ = source.resume();
            }
            Err(_) => break,
        }
    }
    let _ = source.close();
}

fn publish_chunk(
    session_dir: &Path,
    config: &CaptureConfig,
    index: usize,
    next_chunk: &mut u32,
    pending: &mut Vec<f32>,
) -> Result<(), CaptureError> {
    let result = write_chunk(
        session_dir,
        track_at(index),
        *next_chunk,
        pending,
        config.sample_rate,
        config.channels,
    );
    // Drop the bad batch rather than splicing stale samples into the next one.
    pending.clear();
    result?;
    *next_chunk += 1;
    Ok(())
}

fn drain_gap_counter(
    control: &Control,
    session_dir: &Path,
    clock: &SessionClock,
    index: usize,
    counter: &AtomicU64,
    reason: GapReason,
    seen: &mut u64,
) -> Result<(), CaptureError> {
    let current = counter.load(Ordering::Acquire);
    while *seen < current {
        append_gap(
            session_dir,
            &GapMarker {
                start_ms: control.last_gap_start_ms[index].load(Ordering::Acquire),
                end_ms: clock.now().as_millis() as u64,
                reason,
            },
        )?;
        *seen += 1;
    }
    Ok(())
}

fn writer_loop(
    session_dir: PathBuf,
    config: CaptureConfig,
    clock: Arc<SessionClock>,
    control: Arc<Control>,
    rings: [Arc<RingBuffer>; 2],
) {
    let chunk_samples = (config.chunk_frames as usize)
        .saturating_mul(config.channels as usize)
        .max(1);
    let mut pending = [
        Vec::with_capacity(chunk_samples),
        Vec::with_capacity(chunk_samples),
    ];
    let mut scratch = vec![0.0f32; chunk_samples.min(MAX_DRAIN_SAMPLES)];
    let mut next_chunk = [0u32; 2];
    let mut seen_overruns = [0u64; 2];
    let mut seen_losses = [0u64; 2];

    loop {
        let mut drained = false;
        for index in 0..2 {
            while let Some(count) = rings[index].pop(&mut scratch) {
                drained = true;
                // A drained batch can straddle a chunk boundary: publish the
                // full part and carry the remainder into the next chunk so no
                // samples are dropped.
                let mut taken = 0;
                while taken < count {
                    let room = chunk_samples - pending[index].len();
                    let batch = room.min(count - taken);
                    pending[index].extend_from_slice(&scratch[taken..taken + batch]);
                    taken += batch;
                    if pending[index].len() == chunk_samples {
                        if let Err(err) = publish_chunk(
                            &session_dir,
                            &config,
                            index,
                            &mut next_chunk[index],
                            &mut pending[index],
                        ) {
                            control.record_error(err);
                        }
                    }
                }
            }
            if let Err(err) = drain_gap_counter(
                &control,
                &session_dir,
                &clock,
                index,
                &control.overruns[index],
                GapReason::Overrun,
                &mut seen_overruns[index],
            ) {
                control.record_error(err);
            }
            if let Err(err) = drain_gap_counter(
                &control,
                &session_dir,
                &clock,
                index,
                &control.device_losses[index],
                GapReason::DeviceLoss,
                &mut seen_losses[index],
            ) {
                control.record_error(err);
            }
        }

        if control.writer_finish.load(Ordering::Acquire) && !drained {
            for index in 0..2 {
                if !pending[index].is_empty() {
                    if let Err(err) = publish_chunk(
                        &session_dir,
                        &config,
                        index,
                        &mut next_chunk[index],
                        &mut pending[index],
                    ) {
                        control.record_error(err);
                    }
                }
            }
            break;
        }
        if !drained {
            thread::sleep(WRITER_IDLE_SLEEP);
        }
    }
}
