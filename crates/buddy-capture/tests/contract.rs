use buddy_capture::chunks::*;
use buddy_capture::engine::*;
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{AudioSource, CaptureError, TrackKind};
use std::path::Path;

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

fn engine_with(system: Vec<f32>, mic: Vec<f32>, dir: &Path) -> CaptureEngine {
    CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            system,
        )),
        Box::new(FakeAudioSource::from_samples(TrackKind::Mic, 48_000, mic)),
        CaptureConfig::default(),
    )
}

#[test]
fn both_tracks_record_simultaneously() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = engine_with(vec![0.5; 96_000], vec![0.25; 96_000], dir.path());
    engine.start().unwrap();
    engine.stop().unwrap();
    let kinds: Vec<_> = read_chunk_index(dir.path())
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
    let mut engine = engine_with(vec![0.5; 144_000], vec![0.25; 144_000], dir.path());
    engine.start().unwrap();
    engine.pause().unwrap();
    engine.resume().unwrap();
    engine.stop().unwrap();
    let starts: Vec<u64> = read_gaps(dir.path())
        .unwrap()
        .iter()
        .map(|g| g.start_ms)
        .collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    assert!(read_chunk_index(dir.path())
        .unwrap()
        .iter()
        .all(|c| c.frames > 0));
}

#[test]
fn device_loss_records_gap_and_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let mut mic = FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.25; 96_000]);
    mic.fail_next_read_with(CaptureError::DeviceLost("mic-1".into()));
    let mut engine = CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir.path(),
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(mic),
        CaptureConfig::default(),
    );
    engine.start().unwrap();
    engine.stop().unwrap();
    let gaps = read_gaps(dir.path()).unwrap();
    assert_eq!(
        gaps.iter()
            .filter(|g| matches!(g.reason, GapReason::DeviceLoss))
            .count(),
        1
    );
    assert!(read_chunk_index(dir.path())
        .unwrap()
        .iter()
        .any(|c| c.track == TrackKind::Mic));
}

#[test]
fn overrun_records_gap_and_keeps_capturing() {
    let dir = tempfile::tempdir().unwrap();
    let config = CaptureConfig {
        ring_frames: 8,
        ..CaptureConfig::default()
    };
    let mut engine = CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir.path(),
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::System,
            48_000,
            vec![0.5; 96_000],
        )),
        Box::new(FakeAudioSource::from_samples(
            TrackKind::Mic,
            48_000,
            vec![0.25; 96_000],
        )),
        config,
    );
    engine.start().unwrap();
    engine.stop().unwrap();
    assert!(read_gaps(dir.path())
        .unwrap()
        .iter()
        .any(|g| matches!(g.reason, GapReason::Overrun)));
    assert!(read_chunk_index(dir.path())
        .unwrap()
        .iter()
        .all(|c| c.frames > 0));
}

#[test]
fn writer_preserves_samples_across_chunk_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    // 20_000-value chunks do not align with the 16_384-value drain batch, so
    // the writer must carry the remainder of each batch into the next chunk.
    let config = CaptureConfig {
        chunk_frames: 10_000,
        ring_frames: 48_000,
        ..CaptureConfig::default()
    };
    let samples: Vec<f32> = (0..96_000).map(|i| i as f32).collect();
    let mut engine = CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir.path(),
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
    );
    engine.start().unwrap();
    engine.stop().unwrap();

    let chunks: Vec<_> = read_chunk_index(dir.path())
        .unwrap()
        .into_iter()
        .filter(|c| c.track == TrackKind::System)
        .collect();
    let total: usize = chunks.iter().map(|c| c.frames as usize).sum();
    assert_eq!(total, samples.len());
    let mut restored = Vec::new();
    for chunk in &chunks {
        restored.extend(read_chunk_samples(dir.path(), chunk).unwrap());
    }
    assert_eq!(restored, samples);
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
