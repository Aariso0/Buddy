mod common;

use buddy_capture::chunks::write_chunk;
use buddy_capture::TrackKind;

#[test]
fn failed_publication_removes_the_staging_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("audio/system-0000.wav")).unwrap();
    assert!(write_chunk(dir.path(), TrackKind::System, 0, &[0.5, 0.5], 48_000, 2).is_err());
    let staged: Vec<_> = std::fs::read_dir(dir.path().join("audio"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(staged.is_empty(), "staging files leaked: {staged:?}");
}
