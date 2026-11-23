mod common;

use buddy_capture::engine::{CaptureConfig, CaptureEngine};
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::TrackKind;
use common::*;

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
