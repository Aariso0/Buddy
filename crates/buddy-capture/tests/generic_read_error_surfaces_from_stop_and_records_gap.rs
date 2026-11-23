mod common;

use buddy_capture::chunks::{read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

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
