use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{AudioSource, CaptureError, TrackKind};

#[test]
fn fake_source_delivers_samples_without_allocating_in_caller() {
    let mut src = FakeAudioSource::from_samples(TrackKind::System, 48_000, vec![0.25; 512]);
    let mut buf = [0.0f32; 256];
    assert_eq!(src.read(&mut buf).unwrap(), 256);
    assert_eq!(buf, [0.25; 256]);
    assert_eq!(src.frames_read(), 256);
}

#[test]
fn fake_source_reports_injected_device_loss_as_typed_error() {
    let mut src = FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.0; 16]);
    src.fail_next_read_with(CaptureError::DeviceLost("mic-1".into()));
    assert!(matches!(
        src.read(&mut [0.0; 16]),
        Err(CaptureError::DeviceLost(_))
    ));
}
