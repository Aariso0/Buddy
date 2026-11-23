mod common;

use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

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
