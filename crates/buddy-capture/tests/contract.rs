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
const SOURCE_PACE: Duration = Duration::from_millis(2);

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
            track: TrackKind::Mic,
            start_ms: 1200,
            end_ms: 1350,
            reason: GapReason::DeviceLoss,
        },
    )
    .unwrap();
    let gaps = read_gaps(dir.path()).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].track, TrackKind::Mic);
    assert_eq!(gaps[0].start_ms, 1200);
    assert_eq!(gaps[0].end_ms, 1350);
    assert!(matches!(gaps[0].reason, GapReason::DeviceLoss));
}

#[test]
fn an_incomplete_interleaved_frame_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        write_chunk(dir.path(), TrackKind::Mic, 0, &[0.0, 1.0, 2.0], 48_000, 2),
        Err(CaptureError::InvalidChunk(_))
    ));
    // Complete stereo and mono frames stay valid.
    assert!(write_chunk(dir.path(), TrackKind::Mic, 0, &[0.0, 1.0], 48_000, 2).is_ok());
    assert!(write_chunk(dir.path(), TrackKind::System, 0, &[0.0], 48_000, 1).is_ok());
}

#[test]
fn parsing_rejects_an_incomplete_interleaved_frame() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audio")).unwrap();
    let data_bytes = 12u32; // three f32 values: one and a half stereo frames
    let mut bytes = vec![0u8; 44];
    bytes[0..4].copy_from_slice(b"RIFF");
    bytes[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes[8..12].copy_from_slice(b"WAVE");
    bytes[12..16].copy_from_slice(b"fmt ");
    bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&3u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&2u16.to_le_bytes());
    bytes[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&(48_000u32 * 8).to_le_bytes());
    bytes[32..34].copy_from_slice(&8u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&32u16.to_le_bytes());
    bytes[36..40].copy_from_slice(b"data");
    bytes[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    for value in [0.0f32, 1.0, 2.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(dir.path().join("audio/system-0000.wav"), bytes).unwrap();
    let error = read_chunk_index(dir.path()).expect_err("half a frame must not parse");
    assert!(
        matches!(&error, CaptureError::InvalidChunk(message) if message.contains("incomplete interleaved frame")),
        "unexpected error: {error}"
    );
}

#[test]
fn failed_publication_removes_the_staging_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audio/system-0000.wav")).unwrap();
    assert!(write_chunk(dir.path(), TrackKind::System, 0, &[0.5, 0.5], 48_000, 2).is_err());
    let staged: Vec<_> = std::fs::read_dir(dir.path().join("audio"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(staged.is_empty(), "staging files leaked: {staged:?}");
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
    delay: Arc<Mutex<Duration>>,
    reconnect_delay: Arc<Mutex<Duration>>,
    burst: Arc<AtomicUsize>,
    // A backend-shaped stall: reads fail until resume() re-arms the stream.
    stalled: Arc<AtomicBool>,
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

    /// Let the next `reads` calls skip `delay` so a tiny ring overruns without
    /// producing an unbounded storm of gap markers.
    fn set_burst(&self, reads: usize) {
        self.burst.store(reads, Ordering::SeqCst);
    }

    /// Delay re-arming so a device-loss gap stays open while the writer runs.
    fn set_reconnect_delay(&self, delay: Duration) {
        *self.reconnect_delay.lock().unwrap() = delay;
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
fn a_source_sample_rate_mismatch_is_rejected_before_start() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            44_100,
            vec![0.5; 16],
        )),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::Mic,
            48_000,
            vec![0.25; 16],
        )),
        CaptureConfig::default(),
    )
    .unwrap();
    let error = engine
        .start()
        .expect_err("a 44.1 kHz source must not be labelled 48 kHz");
    assert!(
        matches!(&error, CaptureError::Backend(message) if message.contains("sample rate")),
        "unexpected error: {error}"
    );
    assert!(
        matches!(engine.stop(), Err(CaptureError::Backend(message)) if message.contains("before start")),
        "a rejected start must not leave a writer running"
    );
}

#[test]
fn restarting_a_consumed_engine_is_rejected_before_state_changes() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut engine = engine_with(vec![0.5; 128], vec![0.25; 128], &session_dir);
    engine.start().unwrap();
    engine.stop().unwrap();
    let error = engine
        .start()
        .expect_err("sources handed to the first start cannot be reused");
    assert!(
        matches!(&error, CaptureError::Backend(message) if message.contains("already consumed")),
        "unexpected error: {error}"
    );
    assert!(
        matches!(engine.stop(), Err(CaptureError::Backend(message)) if message.contains("before start")),
        "a rejected restart must remain safely stoppable"
    );
}

#[test]
fn dead_reader_tracks_are_visible_before_stop() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (system, _system_handle) =
        ControlledSource::new(TrackKind::System, usize::MAX, SOURCE_PACE);
    let (mic, _mic_handle) = ControlledSource::new(TrackKind::Mic, 8, Duration::ZERO);
    let mut engine = CaptureEngine::new(
        SESSION_ID,
        &session_dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(system),
        Box::new(mic),
        CaptureConfig::default(),
    )
    .unwrap();
    engine.start().unwrap();
    wait_for(|| !engine.track_is_alive(&TrackKind::Mic));
    assert!(engine.track_is_alive(&TrackKind::System));
    engine.stop().unwrap();
    assert!(!engine.track_is_alive(&TrackKind::Mic));
}

#[test]
fn stopping_a_paused_recording_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
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
    engine.pause().unwrap();
    wait_for(|| handle.pauses() >= 1);
    engine
        .stop()
        .expect("a normal pause-then-stop must not report a backend stall");
    assert!(
        read_gaps(&session_dir)
            .unwrap()
            .iter()
            .all(|gap| matches!(gap.reason, GapReason::Overrun)),
        "a clean pause-then-stop must not record a track failure"
    );
}

#[test]
fn pause_resume_keeps_monotonic_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
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
    let losses: Vec<_> = gaps
        .iter()
        .filter(|g| matches!(g.reason, GapReason::DeviceLoss))
        .collect();
    assert_eq!(losses.len(), 1);
    assert_eq!(
        losses[0].track,
        TrackKind::Mic,
        "the gap must name the track that lost samples: {losses:?}"
    );
    assert!(read_chunk_index(&session_dir)
        .unwrap()
        .iter()
        .any(|c| c.track == TrackKind::Mic));
}

#[test]
fn device_loss_gap_ends_at_reader_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
    handle.set_reconnect_delay(Duration::from_millis(300));
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
    handle.inject(CaptureError::DeviceLost("slow-recovery".to_owned()));
    wait_for(|| handle.reconnects() >= 1);
    engine.stop().unwrap();

    let loss = read_gaps(&session_dir)
        .unwrap()
        .into_iter()
        .find(|gap| matches!(gap.reason, GapReason::DeviceLoss))
        .expect("a device-loss gap must be published");
    assert_eq!(loss.track, TrackKind::Mic);
    assert!(
        loss.end_ms.saturating_sub(loss.start_ms) >= 250,
        "the gap must span the recovery delay, not end when the writer drains it: {loss:?}"
    );
}

#[test]
fn overrun_records_gap_and_new_samples_reach_disk() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let config = CaptureConfig {
        chunk_frames: 64,
        ring_frames: 8,
        ..CaptureConfig::default()
    };
    // Two back-to-back reads are enough to overrun the tiny ring. Slow reads
    // afterwards let the writer drain; an unbounded fast source floods
    // gaps.jsonl on slow CI filesystems and starves later samples.
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, Duration::from_millis(5));
    handle.set_burst(2);
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
    // The third read cannot start until the second has already overrun. Waiting
    // on reads avoids polling gaps.jsonl, which the writer fsyncs per marker.
    wait_for(|| handle.reads() >= 3);
    let reads_at_switch = handle.reads();
    handle.set_value(0.75);
    wait_for(|| handle.reads() >= reads_at_switch + 8);
    // stop() joins the readers, then drains the rings and flushes pending
    // samples, so the distinguishable batch is on disk without polling chunks.
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
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
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
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
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
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
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
    let (mic, handle) = ControlledSource::new(TrackKind::Mic, usize::MAX, SOURCE_PACE);
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
