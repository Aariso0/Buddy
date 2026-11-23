mod common;

use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

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
