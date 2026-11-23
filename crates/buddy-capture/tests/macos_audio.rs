//! macOS CoreAudio backend contract. These tests only run on macOS. A Linux
//! host can cross-check their compilation with
//! `cargo check --target x86_64-apple-darwin --all-targets`, but cannot execute
//! them; real capture remains UNVERIFIED here.
//!
//! `BUDDY_AUDIO_SOAK=1` turns every environment skip into a hard failure, so a
//! device-less or permission-less runner cannot pass by skipping the live
//! assertions. Ordinary runs keep the device-independent checks mandatory.

#![cfg(target_os = "macos")]

use buddy_capture::macos::CoreAudioSource;
use buddy_capture::{AudioSource, CaptureError, DeviceInfo, TrackKind};

const DEFAULT_ID: &str = "default";
const PROCESS_PREFIX: &str = "process:";
const SOAK_ENV: &str = "BUDDY_AUDIO_SOAK";

fn soak_enabled() -> bool {
    std::env::var(SOAK_ENV)
        .map(|value| value == "1")
        .unwrap_or(false)
}

fn skip_or_fail(reason: &str) {
    if soak_enabled() {
        panic!("{SOAK_ENV}=1 requires {reason}");
    }
    eprintln!("SKIP: {reason}; set {SOAK_ENV}=1 to make this a hard failure");
}

fn listed_devices() -> Option<Vec<DeviceInfo>> {
    // Enumeration is device-independent: a backend regression here is always a
    // failure, not an environment skip.
    let devices = CoreAudioSource::list_devices(TrackKind::System)
        .unwrap_or_else(|error| panic!("CoreAudio device enumeration failed: {error}"));
    assert!(
        devices.iter().any(|device| device.id == DEFAULT_ID),
        "system devices must include the default process-tap source: {devices:?}"
    );
    assert!(devices
        .iter()
        .all(|device| device.kind == TrackKind::System));
    // `default` is always injected, so a listed process id is the only signal
    // that a real tap source might exist. It is not proof the host can arm one:
    // hosted runners list CoreAudio clients but have no recording grant.
    if !devices
        .iter()
        .any(|device| device.id.starts_with(PROCESS_PREFIX))
    {
        skip_or_fail("a real CoreAudio process to be listed for system audio");
        return None;
    }
    Some(devices)
}

/// Opening a live tap needs a recording grant and a playing process. Both are
/// absent on hosted runners, so a failed `open` there is an environment limit.
/// Soak runs keep the same failure as a hard backend regression.
fn open_or_skip(device_id: &str) -> Option<CoreAudioSource> {
    match CoreAudioSource::open(device_id) {
        Ok(source) => Some(source),
        Err(error) if !soak_enabled() => {
            skip_or_fail(&format!(
                "a host that can arm a live CoreAudio tap ({error})"
            ));
            None
        }
        Err(error) => panic!("listed System source but open({device_id}) failed: {error}"),
    }
}

fn unknown_id_is_typed_error() {
    match CoreAudioSource::open("process:definitely-not-a-coreaudio-process") {
        Ok(_) => panic!("unknown device must not open successfully"),
        Err(CaptureError::DeviceLost(_)) | Err(CaptureError::Backend(_)) => {}
        Err(error) => panic!("unexpected error for unknown id: {error}"),
    }
}

#[test]
fn core_audio_source_implements_contract() {
    let Some(_devices) = listed_devices() else {
        return;
    };
    let Some(mut source) = open_or_skip(DEFAULT_ID) else {
        return;
    };
    assert_eq!(source.kind(), TrackKind::System);
    assert!(source.sample_rate() > 0, "sample rate must be positive");

    // A quiet desktop cannot prove a live read. Keep ordinary runs to startup
    // and teardown; strict runs stay responsible for real signal verification.
    if !soak_enabled() {
        eprintln!("SKIP: live system-audio samples; set {SOAK_ENV}=1 on a host with playback");
        source.close().expect("close must be typed and succeed");
        return;
    }

    let mut buffer = vec![0.0f32; 64];
    let count = source.read(&mut buffer).expect("live read must not fail");
    assert!(count > 0, "a live source must deliver samples");
    assert!(count <= buffer.len());

    // The remembered device id must survive a re-arm.
    source
        .reconnect()
        .expect("reconnect must re-arm the same source");
    assert_eq!(source.kind(), TrackKind::System);
    assert!(source.sample_rate() > 0);
    source.close().expect("close must be typed and succeed");
}

#[test]
fn core_audio_unknown_device_is_typed_error() {
    // Device-independent: must run even when only `default` is listed and the
    // rest of this test would otherwise skip.
    unknown_id_is_typed_error();

    let Some(_devices) = listed_devices() else {
        return;
    };
    if let Some(mut valid) = open_or_skip(DEFAULT_ID) {
        assert_eq!(valid.kind(), TrackKind::System);
        assert!(
            valid.sample_rate() > 0,
            "listed source must report a positive sample rate"
        );
        valid.close().expect("close must be typed and succeed");
    }
}
