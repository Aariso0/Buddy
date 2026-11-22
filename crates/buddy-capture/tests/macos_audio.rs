//! macOS CoreAudio backend contract. These tests only run on macOS. A Linux
//! host can cross-check their compilation with
//! `cargo check --target x86_64-apple-darwin --all-targets`, but cannot execute
//! them; real capture remains UNVERIFIED here.
//!
//! `BUDDY_AUDIO_SOAK=1` turns a missing endpoint or permission into a hard
//! failure, so a device-less runner cannot pass by skipping both tests.

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

/// Device presence comes only from `list_devices`. A `Backend` error from
/// `open` is never treated as absence: once a real CoreAudio process is listed,
/// the caller must assert the full contract and let any `open` failure fail.
fn system_devices_or_skip() -> Option<Vec<DeviceInfo>> {
    let devices = match CoreAudioSource::list_devices(TrackKind::System) {
        Ok(devices) => devices,
        Err(error) => {
            skip_or_fail(&format!(
                "CoreAudio device enumeration to succeed (got {error})"
            ));
            return None;
        }
    };
    // `default` is a pseudo-device that list_devices always injects, so a
    // listed process id is the evidence that a real tap source exists.
    if !devices
        .iter()
        .any(|device| device.id.starts_with(PROCESS_PREFIX))
    {
        skip_or_fail("a real CoreAudio process to be listed for system audio");
        return None;
    }
    Some(devices)
}

#[test]
fn core_audio_source_implements_contract() {
    let Some(devices) = system_devices_or_skip() else {
        return;
    };
    assert!(
        devices.iter().any(|device| device.id == DEFAULT_ID),
        "system devices must include the default process-tap source: {devices:?}"
    );
    assert!(devices
        .iter()
        .all(|device| device.kind == TrackKind::System));

    // A listed System source makes every assertion below mandatory. A Backend
    // error here is a broken backend, not a no-device skip.
    let mut source = CoreAudioSource::open(DEFAULT_ID)
        .unwrap_or_else(|error| panic!("listed System source but open(default) failed: {error}"));
    assert_eq!(source.kind(), TrackKind::System);
    assert!(source.sample_rate() > 0, "sample rate must be positive");

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
    let Some(_devices) = system_devices_or_skip() else {
        return;
    };

    // Prove the backend can open a listed id before testing an unknown one;
    // otherwise a global Backend failure could satisfy both assertions.
    let mut valid = CoreAudioSource::open(DEFAULT_ID)
        .unwrap_or_else(|error| panic!("listed default System source failed to open: {error}"));
    assert_eq!(valid.kind(), TrackKind::System);
    assert!(
        valid.sample_rate() > 0,
        "listed source must report a positive sample rate"
    );
    valid.close().expect("close must be typed and succeed");

    match CoreAudioSource::open("process:definitely-not-a-coreaudio-process") {
        Ok(_) => panic!("unknown device must not open successfully"),
        Err(CaptureError::DeviceLost(_)) | Err(CaptureError::Backend(_)) => {}
        Err(error) => panic!("unexpected error for unknown id: {error}"),
    }
}
