mod common;

use common::*;

#[test]
fn clean_stop_finalizes_the_real_session_manifest() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let sessions_root = session_dir.parent().unwrap();
    let mut engine = engine_with(vec![0.5; 9_600], vec![0.25; 9_600], &session_dir);

    engine.start().unwrap();
    engine.stop().unwrap();

    // The manifest must be the one inside sessions/<id>, not a sibling of it.
    assert!(session_dir.join("session.json").is_file());
    let manifest = buddy_core::session::read_session(sessions_root, SESSION_ID).unwrap();
    assert!(
        matches!(manifest.state, buddy_core::session::SessionState::Complete),
        "a clean stop must mark the session Complete: {manifest:?}"
    );
    assert!(manifest.ended_at.is_some());
    assert!(
        !buddy_core::session::recover_session(sessions_root, SESSION_ID)
            .unwrap()
            .needs_repair
    );
}
