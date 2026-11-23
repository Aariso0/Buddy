mod common;

use buddy_capture::chunks::{read_chunk_index, read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

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
