mod common;

use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{AudioSource, CaptureError, TrackKind};

#[test]
fn fake_source_reports_injected_device_loss_as_typed_error() {
    let mut src = FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.0; 16]);
    src.fail_next_read_with(CaptureError::DeviceLost("mic-1".into()));
    assert!(matches!(
        src.read(&mut [0.0; 16]),
        Err(CaptureError::DeviceLost(_))
    ));
    // The injected error is one-shot: the script resumes on the next read, so
    // the engine can recover without opening a different source.
    assert_eq!(src.read(&mut [0.0; 16]).unwrap(), 16);
    assert_eq!(src.frames_read(), 16);
}
