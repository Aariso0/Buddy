//! Capture engine: one reader thread per source and one writer thread for all
//! durable output. Reads only touch reused buffers and atomics; chunk and gap
//! publication happens on the writer thread.

use crate::chunks::{append_gap, write_chunk, GapMarker, GapReason};
use crate::ring::RingBuffer;
use crate::{AudioSource, CaptureError, TrackKind};
use buddy_core::clock::SessionClock;
use buddy_core::session::finalize_session;
use chrono::Utc;
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
// Power of two, preallocated once per track. One reader produces and the
// writer consumes, so each queue stays single-producer/single-consumer.
const GAP_QUEUE_SLOTS: usize = 16_384;

#[derive(Clone, Copy)]
struct GapEvent {
    track: usize,
    reason: GapReason,
    start_ms: u64,
}

struct GapSlot {
    sequence: AtomicU64,
    start_ms: AtomicU64,
    // bit 0: track (0 system, 1 mic); bit 1: reason (0 overrun, 1 device loss)
    meta: AtomicU8,
}

struct GapQueue {
    slots: Box<[GapSlot]>,
    mask: u64,
    head: AtomicU64,
    tail: AtomicU64,
}

impl GapQueue {
    fn new() -> Self {
        Self {
            slots: (0..GAP_QUEUE_SLOTS)
                .map(|index| GapSlot {
                    sequence: AtomicU64::new(index as u64),
                    start_ms: AtomicU64::new(0),
                    meta: AtomicU8::new(0),
                })
                .collect(),
            mask: (GAP_QUEUE_SLOTS - 1) as u64,
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
        }
    }

    /// False means the bounded queue is full; the caller surfaces that as a
    /// typed error rather than silently dropping a gap marker.
    fn push(&self, event: GapEvent) -> bool {
        let tail = self.tail.load(Ordering::Relaxed);
        let slot = &self.slots[(tail & self.mask) as usize];
        if slot.sequence.load(Ordering::Acquire) != tail {
            return false;
        }
        let reason_bit = match event.reason {
            GapReason::Overrun => 0,
            GapReason::DeviceLoss => 1,
        };
        slot.start_ms.store(event.start_ms, Ordering::Relaxed);
        slot.meta
            .store((event.track as u8) | (reason_bit << 1), Ordering::Relaxed);
        slot.sequence.store(tail + 1, Ordering::Release);
        self.tail.store(tail + 1, Ordering::Relaxed);
        true
    }

    fn pop(&self) -> Option<GapEvent> {
        let head = self.head.load(Ordering::Relaxed);
        let slot = &self.slots[(head & self.mask) as usize];
        if slot.sequence.load(Ordering::Acquire) != head + 1 {
            return None;
        }
        let meta = slot.meta.load(Ordering::Relaxed);
        let event = GapEvent {
            track: (meta & 1) as usize,
            reason: if meta & 2 == 0 {
                GapReason::Overrun
            } else {
                GapReason::DeviceLoss
            },
            start_ms: slot.start_ms.load(Ordering::Relaxed),
        };
        slot.sequence
            .store(head + GAP_QUEUE_SLOTS as u64, Ordering::Release);
        self.head.store(head + 1, Ordering::Relaxed);
        Some(event)
    }
}

fn record_error(slot: &Mutex<Option<CaptureError>>, error: CaptureError) {
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(error);
    }
}

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
    gap_queues: [GapQueue; 2],
    // Both locks are touched only on failure paths (reader errors, writer
    // publication errors and stop after joins), never on a successful sample.
    reader_error: Mutex<Option<CaptureError>>,
    writer_error: Mutex<Option<CaptureError>>,
}

impl Control {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(RUNNING),
            writer_finish: AtomicBool::new(false),
            gap_queues: [GapQueue::new(), GapQueue::new()],
            reader_error: Mutex::new(None),
            writer_error: Mutex::new(None),
        }
    }
}

fn enqueue_gap(control: &Control, track: usize, reason: GapReason, clock: &SessionClock) {
    let event = GapEvent {
        track,
        reason,
        // Captured by the reader at the moment the gap is detected; the writer
        // never substitutes its own time for it.
        start_ms: clock.now().as_millis() as u64,
    };
    if !control.gap_queues[track].push(event) {
        record_error(
            &control.reader_error,
            CaptureError::Backend("gap event queue full".to_owned()),
        );
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
    ) -> Result<Self, CaptureError> {
        // stop() finalizes using the id with this directory's parent, so a
        // mismatched final component would complete a different session.
        if session_dir.file_name().and_then(|name| name.to_str()) != Some(session_id) {
            return Err(CaptureError::Backend(format!(
                "session directory must end with session id {session_id}: {}",
                session_dir.display()
            )));
        }
        let ring_frames = config.ring_frames;
        let channels = config.channels as usize;
        let ring = || Arc::new(RingBuffer::new(ring_frames, channels));
        Ok(Self {
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
        })
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

        // A failed capture track takes precedence over a publication failure:
        // it is the root cause the caller needs to see.
        let reader_error = self
            .control
            .reader_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(error) = reader_error {
            return Err(error);
        }
        let writer_error = self
            .control
            .writer_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(error) = writer_error {
            return Err(error);
        }

        // Only a clean stop may complete the session. A failed track leaves the
        // recording state in place so recovery still reports needs_repair.
        // session_dir is the full sessions/<id> directory, so the finalizer
        // needs its parent root and joins the id back on itself.
        let root = self
            .session_dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        finalize_session(root, &self.session_id, Utc::now())
            .map_err(|err| CaptureError::Backend(format!("finalize session: {err}")))?;
        Ok(())
    }

    /// Elapsed time on the session clock, in milliseconds. Shared by gap
    /// timestamps and by callers that display recording time.
    pub fn elapsed_ms(&self) -> u64 {
        self.clock.now().as_millis() as u64
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

impl Drop for CaptureEngine {
    fn drop(&mut self) {
        // An abandoned engine must not leave capture threads spinning with the
        // backend handles they own. Finalization stays in stop(): dropping the
        // engine is not a clean stop, so the session manifest is untouched.
        self.control.state.store(STOPPING, Ordering::Release);
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
        // Like stop(): only signal the writer once no reader can still publish
        // samples or gaps during its shutdown drain.
        self.control.writer_finish.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        // Sources not handed to a reader (engine dropped before start) still
        // need their backend handles released.
        if let Some(mut source) = self.system.take() {
            let _ = source.close();
        }
        if let Some(mut source) = self.mic.take() {
            let _ = source.close();
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
                    enqueue_gap(&control, index, GapReason::Overrun, &clock);
                }
            }
            Err(CaptureError::DeviceLost(_)) => {
                enqueue_gap(&control, index, GapReason::DeviceLoss, &clock);
                // The backend remembers its own device id and re-arms the
                // hardware here. A failed reconnect is a typed error and ends
                // this track rather than silently continuing on a dead device.
                if let Err(error) = source.reconnect() {
                    record_error(&control.reader_error, error);
                    break;
                }
            }
            Err(error) => {
                // Keep GapReason unchanged: any other read failure also opens
                // a DeviceLoss gap for the rest of this track, but the caller
                // sees the original typed error from stop().
                enqueue_gap(&control, index, GapReason::DeviceLoss, &clock);
                record_error(&control.reader_error, error);
                break;
            }
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

/// Drain pending gap events and return how many were published. The count lets
/// the writer avoid exiting on the same pass that observed a new gap.
fn drain_gaps(
    control: &Control,
    session_dir: &Path,
    clock: &SessionClock,
) -> Result<usize, CaptureError> {
    let mut events = Vec::new();
    for index in 0..2 {
        while let Some(event) = control.gap_queues[index].pop() {
            events.push(event);
        }
    }
    // The two readers detect gaps concurrently, so their queues can hold
    // events from different instants. Publish one pass in timestamp order to
    // keep gaps.jsonl globally monotonic.
    events.sort_by_key(|event| event.start_ms);
    let count = events.len();
    for event in events {
        if event.track >= 2 {
            return Err(CaptureError::Backend("invalid gap track".to_owned()));
        }
        append_gap(
            session_dir,
            &GapMarker {
                // Preserve the reader's detection time; only the end of the
                // gap is observed here on the writer thread.
                start_ms: event.start_ms,
                end_ms: clock.now().as_millis() as u64,
                reason: event.reason,
            },
        )?;
    }
    Ok(count)
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

    loop {
        let mut drained = false;
        // Observe the stop flag before draining either output queue so the
        // final empty check covers everything published before completion.
        let finishing = control.writer_finish.load(Ordering::Acquire);
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
                            record_error(&control.writer_error, err);
                        }
                    }
                }
            }
        }
        // Draining a gap defers the exit by one pass, so nothing published on
        // this pass can be missed by an early break.
        match drain_gaps(&control, &session_dir, &clock) {
            Ok(count) => drained |= count > 0,
            Err(err) => record_error(&control.writer_error, err),
        }

        if finishing && !drained {
            for index in 0..2 {
                if !pending[index].is_empty() {
                    if let Err(err) = publish_chunk(
                        &session_dir,
                        &config,
                        index,
                        &mut next_chunk[index],
                        &mut pending[index],
                    ) {
                        record_error(&control.writer_error, err);
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
