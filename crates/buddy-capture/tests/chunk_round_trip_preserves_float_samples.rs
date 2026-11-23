mod common;

use buddy_capture::chunks::{read_chunk_index, write_chunk};
use buddy_capture::TrackKind;
use common::*;

#[test]
fn chunk_round_trip_preserves_float_samples() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..96_000).map(|i| i as f32 / 96_000.0).collect();
    let info = write_chunk(dir.path(), TrackKind::System, 0, &samples, 48_000, 2).unwrap();
    assert_eq!(info.frames, 96_000);
    assert_eq!(read_chunk_index(dir.path()).unwrap().len(), 1);
    assert_eq!(read_chunk_samples(dir.path(), &info).unwrap(), samples);
}
