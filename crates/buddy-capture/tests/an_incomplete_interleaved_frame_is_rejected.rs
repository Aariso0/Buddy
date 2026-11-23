mod common;

use buddy_capture::chunks::write_chunk;
use buddy_capture::{CaptureError, TrackKind};

#[test]
fn an_incomplete_interleaved_frame_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        write_chunk(dir.path(), TrackKind::Mic, 0, &[0.0, 1.0, 2.0], 48_000, 2),
        Err(CaptureError::InvalidChunk(_))
    ));
    // Complete stereo and mono frames stay valid.
    assert!(write_chunk(dir.path(), TrackKind::Mic, 0, &[0.0, 1.0], 48_000, 2).is_ok());
    assert!(write_chunk(dir.path(), TrackKind::System, 0, &[0.0], 48_000, 1).is_ok());
}
