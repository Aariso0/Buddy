mod common;

use buddy_capture::chunks::read_chunk_index;
use buddy_capture::TrackKind;
use common::*;

#[test]
fn both_tracks_record_simultaneously() {
    let dir = tempfile::tempdir().unwrap();
    let session_dir = session_dir_under(dir.path());
    let mut engine = engine_with(vec![0.5; 96_000], vec![0.25; 96_000], &session_dir);
    engine.start().unwrap();
    engine.stop().unwrap();
    let kinds: Vec<_> = read_chunk_index(&session_dir)
        .unwrap()
        .into_iter()
        .map(|c| c.track)
        .collect();
    assert!(kinds.contains(&TrackKind::System));
    assert!(kinds.contains(&TrackKind::Mic));
}
