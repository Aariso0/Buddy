mod common;

use buddy_capture::chunks::{read_gaps, GapReason};
use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::TrackKind;
use common::*;

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
