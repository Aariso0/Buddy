//! Shared fixtures for the per-test capture contract integration tests.
//!
//! Every test in this directory is its own Cargo integration-test crate; this
//! module holds the helpers and fixtures they share so a test's code stays
//! small and a failure points at one named test file.

#![allow(dead_code)]

use buddy_capture::chunks::*;
use buddy_capture::engine::*;
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{AudioSource, CaptureError, DeviceInfo, TrackKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Unbounded test sources must not outrun the writer on a loaded runner: each
/// unread batch becomes an overrun marker, and a zero-delay read loop can fill
/// the bounded gap queue before the writer can fsync its markers.
pub const SOURCE_PACE: Duration = Duration::from_millis(2);

/// Decode the data chunk of an IEEE-float WAV file. Test-only: the public API
/// deliberately exposes only chunk metadata, not a decoder.
pub fn read_chunk_samples(dir: &Path, info: &ChunkInfo) -> std::io::Result<Vec<f32>> {
    assert!(info.path.starts_with(dir));
    let bytes = std::fs::read(&info.path)?;
    let mut pos = 12usize; // skip RIFF size and WAVE magic
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let start = pos + 8;
        if start + size > bytes.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "truncated WAV chunk",
            ));
        }
        if id == b"data" {
            return Ok(bytes[start..start + size]
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect());
        }
        pos = start + size + (size % 2);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "missing data chunk",
    ))
}

/// Handles shared with a `ControlledSource` so a test can inject errors while
/// the engine's reader thread is running.
#[derive(Clone)]
pub struct SourceHandle {
    injection: Arc<Mutex<Option<CaptureError>>>,
    reads: Arc<AtomicUsize>,
    reconnects: Arc<AtomicUsize>,
    pauses: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    value: Arc<Mutex<f32>>,
    delay: Arc<Mutex<Duration>>,
    reconnect_delay: Arc<Mutex<Duration>>,
    burst: Arc<AtomicUsize>,
    // A backend-shaped stall: reads fail until resume() re-arms the stream.
    stalled: Arc<AtomicBool>,
}

impl SourceHandle {
    pub fn inject(&self, error: CaptureError) {
        *self.injection.lock().unwrap() = Some(error);
    }

    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub fn reconnects(&self) -> usize {
        self.reconnects.load(Ordering::SeqCst)
    }

    pub fn pauses(&self) -> usize {
        self.pauses.load(Ordering::SeqCst)
    }

    pub fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    pub fn set_value(&self, value: f32) {
        *self.value.lock().unwrap() = value;
    }

    /// Let the next `reads` calls skip `delay` so a tiny ring overruns without
    /// producing an unbounded storm of gap markers.
    pub fn set_burst(&self, reads: usize) {
        self.burst.store(reads, Ordering::SeqCst);
    }

    /// Delay re-arming so a device-loss gap stays open while the writer runs.
    pub fn set_reconnect_delay(&self, delay: Duration) {
        *self.reconnect_delay.lock().unwrap() = delay;
    }
}

/// Test source with controllable error injection, a configurable per-read
/// delay for distinguishable gap timestamps, and observable pause/reconnect
/// calls. `frames_total` can be effectively unbounded to keep the source alive
/// until `stop()`.
pub struct ControlledSource {
    kind: TrackKind,
    frames_total: usize,
    position: usize,
    handle: SourceHandle,
}

impl ControlledSource {
    pub fn new(kind: TrackKind, frames_total: usize, delay: Duration) -> (Self, SourceHandle) {
        let handle = SourceHandle {
            injection: Arc::new(Mutex::new(None)),
            reads: Arc::new(AtomicUsize::new(0)),
            reconnects: Arc::new(AtomicUsize::new(0)),
            pauses: Arc::new(AtomicUsize::new(0)),
            closes: Arc::new(AtomicUsize::new(0)),
            value: Arc::new(Mutex::new(if kind == TrackKind::System {
                0.5
            } else {
                0.25
            })),
            delay: Arc::new(Mutex::new(delay)),
            reconnect_delay: Arc::new(Mutex::new(Duration::ZERO)),
            burst: Arc::new(AtomicUsize::new(0)),
            stalled: Arc::new(AtomicBool::new(false)),
        };
        (
            Self {
                kind,
                frames_total,
                position: 0,
                handle: handle.clone(),
            },
            handle,
        )
    }
}

impl AudioSource for ControlledSource {
    fn kind(&self) -> TrackKind {
        self.kind.clone()
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn list_devices(_kind: TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
        Ok(Vec::new())
    }

    fn open(_device_id: &str) -> Result<Self, CaptureError> {
        Err(CaptureError::Backend(
            "test source has no device id".to_owned(),
        ))
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, CaptureError> {
        self.handle.reads.fetch_add(1, Ordering::SeqCst);
        if self.handle.stalled.load(Ordering::SeqCst) {
            return Err(CaptureError::Backend("paused stream stalled".to_owned()));
        }
        let delay = if self.handle.burst.load(Ordering::SeqCst) > 0 {
            self.handle.burst.fetch_sub(1, Ordering::SeqCst);
            Duration::ZERO
        } else {
            *self.handle.delay.lock().unwrap()
        };
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        if let Some(error) = self.handle.injection.lock().unwrap().take() {
            return Err(error);
        }
        let count = (self.frames_total - self.position).min(out.len());
        let value = *self.handle.value.lock().unwrap();
        out[..count].fill(value);
        out[count..].fill(0.0);
        self.position += count;
        Ok(count)
    }

    fn pause(&mut self) -> Result<(), CaptureError> {
        self.handle.pauses.fetch_add(1, Ordering::SeqCst);
        self.handle.stalled.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn resume(&mut self) -> Result<(), CaptureError> {
        self.handle.stalled.store(false, Ordering::SeqCst);
        Ok(())
    }

    fn reconnect(&mut self) -> Result<(), CaptureError> {
        let delay = *self.handle.reconnect_delay.lock().unwrap();
        if !delay.is_zero() {
            thread::sleep(delay);
        }
        // Clear the consumed error before publishing completion: observers use
        // this counter as a signal that they may inject the next one.
        *self.handle.injection.lock().unwrap() = None;
        self.handle.reconnects.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn close(&mut self) -> Result<(), CaptureError> {
        self.handle.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

pub const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

/// Reserve a real session beneath `root` and return its full directory. The
/// engine is constructed with this directory in the app too, so stop() always
/// has a manifest to finalize.
pub fn session_dir_under(root: &Path) -> PathBuf {
    let sessions_root = root.join("sessions");
    buddy_core::session::create_session(
        &sessions_root,
        SESSION_ID,
        chrono::Utc::now(),
        buddy_core::session::Preset::Lecture,
    )
    .unwrap();
    sessions_root.join(SESSION_ID)
}

pub fn wait_for(predicate: impl Fn() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "timed out waiting for condition"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

pub fn engine_with(system: Vec<f32>, mic: Vec<f32>, session_dir: &Path) -> CaptureEngine {
    CaptureEngine::new(
        SESSION_ID,
        session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            system,
        )),
        Box::new(FakeAudioSource::from_samples(TrackKind::Mic, 48_000, mic)),
        CaptureConfig::default(),
    )
    .unwrap()
}
