//! Portable, allocation-free linear resampling for interleaved f32 audio.
//!
//! The state machine consumes whole interleaved frames and emits frames at the
//! target rate through a caller-supplied closure. It owns only fixed-size
//! arrays and integers, so it can run directly inside an audio callback.

/// Largest channel count this stack-only resampler accepts.
pub const MAX_CHANNELS: usize = 8;

/// Streaming linear resampler from `input_rate` to `output_rate` frames/s.
pub struct LinearResampler {
    input_rate: u32,
    output_rate: u32,
    channels: usize,
    /// Output frame index of the next frame to emit.
    next_output: u64,
    /// Complete input frames consumed so far. The current frame's index is
    /// `frames_processed` before it is incremented.
    frames_processed: u64,
    started: bool,
    /// Last fully consumed frame (one value per channel).
    previous: [f32; MAX_CHANNELS],
    /// Up to one incomplete frame carried across `push` calls.
    partial: [f32; MAX_CHANNELS],
    partial_len: usize,
}

impl LinearResampler {
    pub fn new(input_rate: u32, output_rate: u32, channels: usize) -> Self {
        assert!(input_rate > 0, "input rate must be positive");
        assert!(output_rate > 0, "output rate must be positive");
        assert!(
            (1..=MAX_CHANNELS).contains(&channels),
            "channel count must be between 1 and {MAX_CHANNELS}"
        );
        Self {
            input_rate,
            output_rate,
            channels,
            next_output: 0,
            frames_processed: 0,
            started: false,
            previous: [0.0; MAX_CHANNELS],
            partial: [0.0; MAX_CHANNELS],
            partial_len: 0,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Feed a flat interleaved stream, emitting one output frame per call to
    /// `emit`. Trailing samples short of a full frame are carried to the next
    /// call; no heap allocation is performed.
    pub fn push<F>(&mut self, input: &[f32], mut emit: F)
    where
        F: FnMut(&[f32]),
    {
        let channels = self.channels;
        let mut rest = input;

        if self.partial_len > 0 {
            let needed = channels - self.partial_len;
            let take = needed.min(rest.len());
            self.partial[self.partial_len..self.partial_len + take].copy_from_slice(&rest[..take]);
            self.partial_len += take;
            rest = &rest[take..];
            if self.partial_len == channels {
                let frame = self.partial;
                self.push_frame(&frame[..channels], &mut emit);
                self.partial_len = 0;
            }
        }

        let mut frames = rest.chunks_exact(channels);
        for frame in &mut frames {
            self.push_frame(frame, &mut emit);
        }
        let leftover = frames.remainder();
        if !leftover.is_empty() {
            self.partial[..leftover.len()].copy_from_slice(leftover);
            self.partial_len = leftover.len();
        }
    }

    /// Emit the tail of a finite stream. Streaming callers do not need this:
    /// the next input frame supplies the missing right endpoint. It holds the
    /// last frame for outputs whose interpolation point already lies beyond
    /// the final input frame.
    pub fn flush<F>(&mut self, mut emit: F)
    where
        F: FnMut(&[f32]),
    {
        if !self.started {
            return;
        }
        // One past the final frame index, so a 1 s 44.1 kHz clip produces
        // exactly 48 000 frames at 48 kHz.
        let limit = u128::from(self.frames_processed) * u128::from(self.output_rate);
        loop {
            let due = u128::from(self.next_output) * u128::from(self.input_rate);
            if due >= limit {
                break;
            }
            emit(&self.previous[..self.channels]);
            self.next_output += 1;
        }
    }

    fn push_frame<F>(&mut self, current: &[f32], emit: &mut F)
    where
        F: FnMut(&[f32]),
    {
        let channels = self.channels;
        if !self.started {
            self.previous[..channels].copy_from_slice(current);
            self.started = true;
            self.frames_processed = 1;
            // Output frame 0 lands exactly on input frame 0.
            emit(&self.previous[..channels]);
            self.next_output = 1;
            return;
        }

        let current_index = self.frames_processed;
        let mut frame = [0.0f32; MAX_CHANNELS];
        loop {
            let due = u128::from(self.next_output) * u128::from(self.input_rate);
            let now = u128::from(current_index) * u128::from(self.output_rate);
            if due > now {
                break;
            }
            // Interpolate between the previous frame (index n-1) and the
            // current one (index n). Integer comparisons decide scheduling;
            // only the fraction uses floating point.
            let left = u128::from(current_index - 1) * u128::from(self.output_rate);
            let offset = due as i128 - left as i128;
            let fraction = (offset as f64 / f64::from(self.output_rate)).clamp(0.0, 1.0);
            for channel in 0..channels {
                let from = self.previous[channel];
                frame[channel] = from + (current[channel] - from) * fraction as f32;
            }
            emit(&frame[..channels]);
            self.next_output += 1;
        }
        self.previous[..channels].copy_from_slice(current);
        self.frames_processed += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(channels: usize, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|frame| (0..channels).map(move |channel| frame as f32 + channel as f32 * 0.5))
            .collect()
    }

    #[test]
    fn equal_rates_are_identity() {
        let mut resampler = LinearResampler::new(48_000, 48_000, 2);
        let input = ramp(2, 128);
        let mut output = Vec::new();
        resampler.push(&input, |frame| output.extend_from_slice(frame));
        resampler.flush(|frame| output.extend_from_slice(frame));
        assert_eq!(output.len(), input.len());
        for (got, want) in output.iter().zip(input.iter()) {
            assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
        }
    }

    #[test]
    fn upsample_44100_to_48000_emits_48000_frames_per_second() {
        let mut resampler = LinearResampler::new(44_100, 48_000, 2);
        let input = ramp(2, 44_100);
        let mut output = Vec::new();
        resampler.push(&input, |frame| output.extend_from_slice(frame));
        resampler.flush(|frame| output.extend_from_slice(frame));
        assert_eq!(output.len(), 48_000 * 2);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn downsample_48000_to_44100_emits_44100_frames_per_second() {
        let mut resampler = LinearResampler::new(48_000, 44_100, 2);
        let input = ramp(2, 48_000);
        let mut output = Vec::new();
        resampler.push(&input, |frame| output.extend_from_slice(frame));
        resampler.flush(|frame| output.extend_from_slice(frame));
        assert_eq!(output.len(), 44_100 * 2);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn chunked_push_matches_one_shot_and_keeps_partial_frames() {
        let input = ramp(2, 1_000);
        let mut one_shot = LinearResampler::new(44_100, 48_000, 2);
        let mut expected = Vec::new();
        one_shot.push(&input, |frame| expected.extend_from_slice(frame));
        one_shot.flush(|frame| expected.extend_from_slice(frame));

        let mut chunked = LinearResampler::new(44_100, 48_000, 2);
        let mut actual = Vec::new();
        for chunk in input.chunks(37) {
            chunked.push(chunk, |frame| actual.extend_from_slice(frame));
        }
        chunked.flush(|frame| actual.extend_from_slice(frame));

        assert_eq!(actual.len(), expected.len());
        for (got, want) in actual.iter().zip(expected.iter()) {
            assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
        }
    }
}
