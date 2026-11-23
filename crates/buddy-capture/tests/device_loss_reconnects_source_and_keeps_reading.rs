mod common;

use buddy_capture::chunks::{read_chunk_index, read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

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
