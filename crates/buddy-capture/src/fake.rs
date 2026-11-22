//! Scripted in-memory `AudioSource` for CI contract tests.

use crate::{AudioSource, CaptureError, DeviceInfo, TrackKind};

const FAKE_SAMPLE_RATE: u32 = 48_000;
const SYSTEM_DEVICE_ID: &str = "fake-system";
const MIC_DEVICE_ID: &str = "fake-mic";

/// Plays back a fixed sample script. `read` only copies into the caller's
/// buffer, mirroring the no-allocation contract of real backends.
pub struct FakeAudioSource {
    kind: TrackKind,
    sample_rate: u32,
    samples: Vec<f32>,
    position: usize,
    frames_read: usize,
    pending_error: Option<CaptureError>,
}

impl FakeAudioSource {
    /// Create a source that yields `samples` from beginning to end.
    pub fn from_samples(kind: TrackKind, sample_rate: u32, samples: Vec<f32>) -> Self {
        Self {
            kind,
            sample_rate,
            samples,
            position: 0,
            frames_read: 0,
            pending_error: None,
        }
    }

    /// Make the next `read` return this typed error once; reads after it
    /// resume from the script so the engine can exercise device-loss recovery.
    pub fn fail_next_read_with(&mut self, error: CaptureError) {
        self.pending_error = Some(error);
    }

    /// Total f32 values copied into caller buffers so far. Failed reads add
    /// nothing, so a resumed script continues from where it stopped.
    pub fn frames_read(&self) -> usize {
        self.frames_read
    }
}

impl AudioSource for FakeAudioSource {
    fn kind(&self) -> TrackKind {
        self.kind.clone()
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn list_devices(kind: TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> {
        let (id, name) = match &kind {
            TrackKind::System => (SYSTEM_DEVICE_ID, "Fake system audio"),
            TrackKind::Mic => (MIC_DEVICE_ID, "Fake microphone"),
        };
        Ok(vec![DeviceInfo {
            id: id.to_owned(),
            name: name.to_owned(),
            kind,
        }])
    }

    /// The fake has no real device: a reopened source starts with an empty
    /// script. Tests build scripted sources with [`Self::from_samples`].
    fn open(device_id: &str) -> Result<Self, CaptureError> {
        let kind = if device_id == MIC_DEVICE_ID {
            TrackKind::Mic
        } else {
            TrackKind::System
        };
        Ok(Self::from_samples(kind, FAKE_SAMPLE_RATE, Vec::new()))
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize, CaptureError> {
        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }

        let count = (self.samples.len() - self.position).min(out.len());
        // Clear the tail so a reused caller buffer cannot expose stale samples.
        out[count..].fill(0.0);
        out[..count].copy_from_slice(&self.samples[self.position..self.position + count]);
        self.position += count;
        self.frames_read += count;
        Ok(count)
    }

    // Pause and close are engine concerns; the fake has no device state to
    // change, and the script stays available for the contract tests.
    fn pause(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }

    fn resume(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }

    fn close(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }
}
