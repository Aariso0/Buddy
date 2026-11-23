use buddy_capture::chunks::*;
use buddy_capture::engine::*;
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{AudioSource, CaptureError, DeviceInfo, TrackKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn fake_source_delivers_samples_without_allocating_in_caller() {
    let mut src = FakeAudioSource::from_samples(TrackKind::System, 48_000, vec![0.25; 512]);
    let mut buf = [0.0f32; 256];
    assert_eq!(src.read(&mut buf).unwrap(), 256);
    assert_eq!(buf, [0.25; 256]);
    assert_eq!(src.frames_read(), 256);
}

#[test]
fn fake_source_reports_injected_device_loss_as_typed_error() {
    let mut src = FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.0; 16]);
    src.fail_next_read_with(CaptureError::DeviceLost("mic-1".into()));
    assert!(matches!(
        src.read(&mut [0.0; 16]),
        Err(CaptureError::DeviceLost(_))
    ));
    // The injected error is one-shot: the script resumes on the next read, so
    // the engine can recover without opening a different source.
    assert_eq!(src.read(&mut [0.0; 16]).unwrap(), 16);
    assert_eq!(src.frames_read(), 16);
}

/// Decode the data chunk of an IEEE-float WAV file. Test-only: the public API
/// deliberately exposes only chunk metadata, not a decoder.
fn read_chunk_samples(dir: &Path, info: &ChunkInfo) -> std::io::Result<Vec<f32>> {
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

#[test]
fn chunk_round_trip_preserves_float_samples() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..96_000).map(|i| i as f32 / 96_000.0).collect();
    let info = write_chunk(dir.path(), TrackKind::System, 0, &samples, 48_000, 2).unwrap();
    assert_eq!(info.frames, 96_000);
    assert_eq!(read_chunk_index(dir.path()).unwrap().len(), 1);
    assert_eq!(read_chunk_samples(dir.path(), &info).unwrap(), samples);
}

#[test]
fn gap_markers_round_trip_as_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    append_gap(
        dir.path(),
        &GapMarker {
            start_ms: 1200,
            end_ms: 1350,
            reason: GapReason::DeviceLoss,
        },
    )
    .unwrap();
    let gaps = read_gaps(dir.path()).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].start_ms, 1200);
    assert!(matches!(gaps[0].reason, GapReason::DeviceLoss));
}

#[test]
fn truncated_chunk_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audio")).unwrap();
    let path = dir.path().join("audio/mic-0000.wav");
    std::fs::write(&path, b"RIFF\x00\x00\x00\x00WAVE").unwrap();
    assert!(matches!(
        read_chunk_index(dir.path()),
        Err(CaptureError::InvalidChunk(_))
    ));
}

/// Handles shared with a `ControlledSource` so a test can inject errors while
/// the engine's reader thread is running.
#[derive(Clone)]
struct SourceHandle {
    injection: Arc<Mutex<Option<CaptureError>>>,
    reads: Arc<AtomicUsize>,
    reconnects: Arc<AtomicUsize>,
    pauses: Arc<AtomicUsize>,
    closes: Arc<AtomicUsize>,
    value: Arc<Mutex<f32>>,
}

impl SourceHandle {
    fn inject(&self, error: CaptureError) {
        *self.injection.lock().unwrap() = Some(error);
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn reconnects(&self) -> usize {
        self.reconnects.load(Ordering::SeqCst)
    }

    fn pauses(&self) -> usize {
        self.pauses.load(Ordering::SeqCst)
    }

    fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    fn set_value(&self, value: f32) {
        *self.value.lock().unwrap() = value;
    }
}

/// Test source with controllable error injection, a configurable per-read
/// delay for distinguishable gap timestamps, and observable pause/reconnect
/// calls. `frames_total` can be effectively unbounded to keep the source alive
/// until `stop()`.
struct ControlledSource {
    kind: TrackKind,
    frames_total: usize,
    position: usize,
    delay: Duration,
    handle: SourceHandle,
}

impl ControlledSource {
    fn new(kind: TrackKind, frames_total: usize, delay: Duration) -> (Self, SourceHandle) {
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
        };
        (
            Self {
                kind,
                frames_total,
                position: 0,
                delay,
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
        if !self.delay.is_zero() {
            thread::sleep(self.delay);
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
        Ok(())
    }

    fn resume(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }

    fn reconnect(&mut self) -> Result<(), CaptureError> {
        self.handle.reconnects.fetch_add(1, Ordering::SeqCst);
        *self.handle.injection.lock().unwrap() = None;
        Ok(())
    }

    fn close(&mut self) -> Result<(), CaptureError> {
        self.handle.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

const SESSION_ID: &str = "11111111-1111-4111-8111-111111111111";

/// Reserve a real session beneath `root` and return its full directory. The
/// engine is constructed with this directory in the app too, so stop() always
/// has a manifest to finalize.
fn session_dir_under(root: &Path) -> PathBuf {
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

fn wait_for(predicate: impl Fn() -> bool) {
    let start = Instant::now();
    while !predicate() {
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "timed out waiting for condition"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

fn engine_with(system: Vec<f32>, mic: Vec<f32>, session_dir: &Path) -> CaptureEngine {
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

#[test]
fn both_tracks_record_simultaneously() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut engine = engine_with(vec![0.5; 96_000], vec![0.25; 96_000], &session_dir);
    engine.start().unwrap();
    engine.stop().unwrap();
    let kinds: Vec<_> = read_chunk_index(&session_dir)
        .unwrap()
        .into_iter()
        .map(|c| c.track)
        .collect();
    assert!(kinds.contains(&TrackKind::System));
    assert!(kinds.contains(&TrackKind::Mic));
}

#[test]
fn pause_resume_keeps_monotonic_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::ZERO);
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 144_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    engine.pause().unwrap();
    // Wait until the reader has actually applied pause, then capture the
    // boundary on the same clock that stamps gap markers.
    wait_for(|| handle.pauses() >= 1);
    let pause_boundary_ms = engine.elapsed_ms();
    handle.inject(CaptureError::DeviceLost("mic-after-resume".into()));
    engine.resume().unwrap();
    // Only assert the boundary once the reader has really re-armed after the
    // injected loss; stopping earlier can race with the gap being stamped.
    wait_for(|| handle.reconnects() >= 1);
    engine.stop().unwrap();

    let gaps = read_gaps(&session_dir).unwrap();
    let loss_starts: Vec<u64> = gaps
        .iter()
        .filter(|gap| matches!(gap.reason, GapReason::DeviceLoss))
        .map(|gap| gap.start_ms)
        .collect();
    assert!(
        !loss_starts.is_empty(),
        "the injected device-loss gap must actually be recorded: {gaps:?}"
    );
    assert!(
        loss_starts
            .iter()
            .any(|&start| start >= pause_boundary_ms),
        "a DeviceLoss gap injected after resume must not be stamped before the pause boundary ({pause_boundary_ms}): {loss_starts:?}"
    );
    let all_starts: Vec<u64> = gaps.iter().map(|gap| gap.start_ms).collect();
    assert!(all_starts.windows(2).all(|w| w[0] <= w[1]));
    assert!(read_chunk_index(&session_dir)
        .unwrap()
        .iter()
        .all(|c| c.frames > 0));
}

#[test]
fn device_loss_records_gap_and_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut mic = FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.25; 96_000]);
    mic.fail_next_read_with(CaptureError::DeviceLost("mic-1".into()));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    engine.stop().unwrap();
    let gaps = read_gaps(&session_dir).unwrap();
    assert_eq!(
        gaps.iter()
            .filter(|g| matches!(g.reason, GapReason::DeviceLoss))
            .count(),
        1
    );
    assert!(read_chunk_index(&session_dir)
        .unwrap()
        .iter()
        .any(|c| c.track == TrackKind::Mic));
}

#[test]
fn overrun_records_gap_and_new_samples_reach_disk() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let config = CaptureConfig {
        ring_frames: 8,
        ..CaptureConfig::default()
    };
    // A short per-read delay keeps the overrun rate below the writer's gap
    // drain rate; with no delay the 16k-slot gap queue overflows before the
    // writer can publish the first marker.
    let (mic, handle) =
        ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::from_micros(250));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        config,
    )
    .unwrap();
    engine.start().unwrap();
    // Wait for the published marker, not for a fixed sleep: the writer flushes
    // gaps.jsonl as it drains them, so this is observable while capture runs.
    wait_for(|| {
        read_gaps(&session_dir)
            .map(|gaps| gaps.iter().any(|g| matches!(g.reason, GapReason::Overrun)))
            .unwrap_or(false)
    });
    // Switch to a distinguishable value only after the overrun is known, then
    // let the reader push several more batches before stopping.
    handle.set_value(0.75);
    let reads_at_switch = handle.reads();
    wait_for(|| handle.reads() >= reads_at_switch + 8);
    engine.stop().unwrap();

    assert!(read_gaps(&session_dir)
        .unwrap()
        .iter()
        .any(|g| matches!(g.reason, GapReason::Overrun)));
    let mic_samples: Vec<f32> = read_chunk_index(&session_dir)
        .unwrap()
        .into_iter()
        .filter(|chunk| chunk.track == TrackKind::Mic)
        .flat_map(|chunk| read_chunk_samples(&session_dir, &chunk).unwrap())
        .collect();
    assert!(
        mic_samples.iter().any(|&sample| sample == 0.75),
        "samples captured after the overrun must reach disk: {mic_samples:?}"
    );
}

#[test]
fn writer_preserves_samples_across_chunk_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    // 20_000-value chunks do not align with the 16_384-value drain batch, so
    // the writer must carry the remainder of each batch into the next chunk.
    let config = CaptureConfig {
        chunk_frames: 10_000,
        ring_frames: 48_000,
        ..CaptureConfig::default()
    };
    let samples: Vec<f32> = (0..96_000).map(|i| i as f32).collect();
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            samples.clone(),
        )),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::Mic,
            48_000,
            vec![0.25; 96_000],
        )),
        config,
    )
    .unwrap();
    engine.start().unwrap();
    engine.stop().unwrap();

    let chunks: Vec<_> = read_chunk_index(&session_dir)
        .unwrap()
        .into_iter()
        .filter(|c| c.track == TrackKind::System)
        .collect();
    let total: usize = chunks.iter().map(|c| c.frames as usize).sum();
    assert_eq!(total, samples.len());
    let mut restored = Vec::new();
    for chunk in &chunks {
        restored.extend(read_chunk_samples(&session_dir, chunk).unwrap());
    }
    assert_eq!(restored, samples);
}

#[test]
fn gap_events_keep_per_event_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::from_millis(6));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| handle.reads() >= 1);
    for i in 0..3 {
        let before = handle.reconnects();
        handle.inject(CaptureError::DeviceLost(format!("mic-{i}")));
        wait_for(|| handle.reconnects() > before);
    }
    engine.stop().unwrap();

    let losses: Vec<_> = read_gaps(&session_dir)
        .unwrap()
        .into_iter()
        .filter(|gap| matches!(gap.reason, GapReason::DeviceLoss))
        .collect();
    assert!(
        losses.len() >= 3,
        "expected one marker per loss event, got {}",
        losses.len()
    );
    let starts: Vec<u64> = losses.iter().map(|gap| gap.start_ms).collect();
    assert!(
        starts.windows(2).all(|pair| pair[0] <= pair[1]),
        "gap timestamps must be monotonic: {starts:?}"
    );
    assert!(
        starts.iter().any(|&start| start > starts[0]),
        "each gap must keep its own detection time, not a shared overwritten slot: {starts:?}"
    );
}

#[test]
fn generic_read_error_surfaces_from_stop_and_records_gap() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::ZERO);
    handle.inject(CaptureError::Backend("mic decoder exploded".to_owned()));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| handle.reads() >= 1);
    let result = engine.stop();
    assert!(
        matches!(
            &result,
            Err(CaptureError::Backend(message)) if message.contains("mic decoder exploded")
        ),
        "a failed track must not be reported as clean success: {result:?}"
    );
    let gaps = read_gaps(&session_dir).unwrap();
    assert_eq!(
        gaps.iter()
            .filter(|gap| matches!(gap.reason, GapReason::DeviceLoss))
            .count(),
        1
    );
}

#[test]
fn device_loss_reconnects_source_and_keeps_reading() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::ZERO);
    handle.inject(CaptureError::DeviceLost("mic-1".into()));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| handle.reconnects() >= 1);
    engine.stop().unwrap();
    assert!(
        handle.reconnects() >= 1,
        "the DeviceLost path must call reconnect(), not resume()"
    );
    assert!(
        handle.reads() > 1,
        "the reconnected source must keep reading"
    );
    let gaps = read_gaps(&session_dir).unwrap();
    assert_eq!(
        gaps.iter()
            .filter(|gap| matches!(gap.reason, GapReason::DeviceLoss))
            .count(),
        1
    );
    assert!(read_chunk_index(&session_dir)
        .unwrap()
        .iter()
        .any(|chunk| chunk.track == TrackKind::Mic));
}

#[test]
fn clean_stop_finalizes_the_real_session_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let sessions_root = session_dir.parent().unwrap();
    let mut engine = engine_with(vec![0.5; 9_600], vec![0.25; 9_600], &session_dir);

    engine.start().unwrap();
    engine.stop().unwrap();

    // The manifest must be the one inside sessions/<id>, not a sibling of it.
    assert!(session_dir.join("session.json").is_file());
    let manifest = buddy_core::session::read_session(sessions_root, SESSION_ID).unwrap();
    assert!(
        matches!(manifest.state, buddy_core::session::SessionState::Complete),
        "a clean stop must mark the session Complete: {manifest:?}"
    );
    assert!(manifest.ended_at.is_some());
    assert!(
        !buddy_core::session::recover_session(sessions_root, SESSION_ID)
            .unwrap()
            .needs_repair
    );
}

#[test]
fn failed_stop_does_not_finalize_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let sessions_root = session_dir.parent().unwrap();
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::ZERO);
    handle.inject(CaptureError::Backend("mic decoder exploded".to_owned()));
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 9_600],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();

    engine.start().unwrap();
    wait_for(|| handle.reads() >= 1);
    let result = engine.stop();
    assert!(
        matches!(
            &result,
            Err(CaptureError::Backend(message)) if message.contains("mic decoder exploded")
        ),
        "the original typed reader error must survive finalization: {result:?}"
    );

    let recovery = buddy_core::session::recover_session(sessions_root, SESSION_ID).unwrap();
    assert!(
        matches!(
            recovery.manifest.state,
            buddy_core::session::SessionState::Recording
        ),
        "a failed stop must not mark the session Complete: {:?}",
        recovery.manifest
    );
    assert!(recovery.needs_repair);
}

#[test]
fn dropping_a_started_engine_closes_sources_without_finalizing() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let sessions_root = dir.path().join("sessions");
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::ZERO);
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| handle.reads() >= 1);

    drop(engine);

    assert!(
        handle.closes() >= 1,
        "Drop must close the source owned by the reader thread"
    );
    let recovery = buddy_core::session::recover_session(&sessions_root, SESSION_ID).unwrap();
    assert!(
        matches!(
            recovery.manifest.state,
            buddy_core::session::SessionState::Recording
        ),
        "Drop is not a clean stop and must not complete the manifest"
    );
}

#[test]
fn session_dir_must_end_with_the_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let wrong_dir = dir.path().join("another-session");
    let result = CaptureEngine::new(
        SESSION_ID,
        &wrong_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            Vec::new(),
        )),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::Mic,
            48_000,
            Vec::new(),
        )),
        CaptureConfig::default(),
    );
    let error = result
        .err()
        .expect("a mismatched directory must be rejected");
    assert!(
        matches!(&error, CaptureError::Backend(message) if message.contains(SESSION_ID)),
        "a mismatched directory must be rejected before any thread starts: {error}"
    );
}

#[test]
fn ring_reports_full_instead_of_overwriting() {
    let ring = buddy_capture::ring::RingBuffer::new(4, 2);
    assert_eq!(ring.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]), Ok(()));
    assert!(matches!(
        ring.push(&[9.0, 10.0]),
        Err(buddy_capture::ring::RingError::Full)
    ));
    let mut out = [0.0; 3];
    assert_eq!(ring.pop(&mut out), Some(3));
    assert_eq!(out, [1.0, 2.0, 3.0]);
    assert_eq!(ring.push(&[9.0, 10.0, 11.0]), Ok(()));
    let mut rest = [0.0; 8];
    assert_eq!(ring.pop(&mut rest), Some(8));
    assert_eq!(&rest[..8], &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]);
    assert_eq!(ring.pop(&mut rest), None);
}
