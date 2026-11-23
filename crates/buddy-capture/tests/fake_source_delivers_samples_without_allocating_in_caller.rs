mod common;

use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{AudioSource, TrackKind};

#[test]
fn fake_source_delivers_samples_without_allocating_in_caller() {
    let mut src = FakeAudioSource::from_samples(TrackKind::System, 48_000, vec![0.25; 512]);
    let mut buf = [0.0f32; 256];
    assert_eq!(src.read(&mut buf).unwrap(), 256);
    assert_eq!(buf, [0.25; 256]);
    assert_eq!(src.frames_read(), 256);
}
