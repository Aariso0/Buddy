mod common;

use buddy_capture::chunks::read_chunk_index;
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::TrackKind;
use common::*;

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
