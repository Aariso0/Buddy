mod common;

use buddy_capture::chunks::read_chunk_index;
use buddy_capture::CaptureError;

#[test]
fn truncated_chunk_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audio")).unwrap();
    let path = dir.path().join("audio/mic-0000.wav");
    std::fs::write(&path, b"RIFF\x00\x00\x00\x00WAVE").unwrap();
    assert!(matches!(
        read_chunk_index(dir.path()),
        Err(CaptureError::InvalidChunk(_))
    ));
}
