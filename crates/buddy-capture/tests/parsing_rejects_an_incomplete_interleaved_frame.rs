mod common;

use buddy_capture::chunks::read_chunk_index;
use buddy_capture::CaptureError;

#[test]
fn parsing_rejects_an_incomplete_interleaved_frame() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audio")).unwrap();
    let data_bytes = 12u32; // three f32 values: one and a half stereo frames
    let mut bytes = vec![0u8; 44];
    bytes[0..4].copy_from_slice(b"RIFF");
    bytes[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    bytes[8..12].copy_from_slice(b"WAVE");
    bytes[12..16].copy_from_slice(b"fmt ");
    bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
    bytes[20..22].copy_from_slice(&3u16.to_le_bytes());
    bytes[22..24].copy_from_slice(&2u16.to_le_bytes());
    bytes[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    bytes[28..32].copy_from_slice(&(48_000u32 * 8).to_le_bytes());
    bytes[32..34].copy_from_slice(&8u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&32u16.to_le_bytes());
    bytes[36..40].copy_from_slice(b"data");
    bytes[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    for value in [0.0f32, 1.0, 2.0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    std::fs::write(dir.path().join("audio/system-0000.wav"), bytes).unwrap();
    let error = read_chunk_index(dir.path()).expect_err("half a frame must not parse");
    assert!(
        matches!(&error, CaptureError::InvalidChunk(message) if message.contains("incomplete interleaved frame")),
        "unexpected error: {error}"
    );
}
