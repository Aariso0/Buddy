mod common;

use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};
use common::*;

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
