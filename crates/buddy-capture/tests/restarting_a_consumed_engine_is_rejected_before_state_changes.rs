mod common;

use buddy_capture::CaptureError;
use common::*;

#[test]
fn restarting_a_consumed_engine_is_rejected_before_state_changes() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut engine = engine_with(vec![0.5; 128], vec![0.25; 128], &session_dir);
    engine.start().unwrap();
    engine.stop().unwrap();
    let error = engine
        .start()
        .expect_err("sources handed to the first start cannot be reused");
    assert!(
        matches!(&error, CaptureError::Backend(message) if message.contains("already consumed")),
        "unexpected error: {error}"
    );
    assert!(
        matches!(engine.stop(), Err(CaptureError::Backend(message)) if message.contains("before start")),
        "a rejected restart must remain safely stoppable"
    );
}
