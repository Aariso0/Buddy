# Real-capture soak tests

A soak is a **60-minute continuous capture against real audio endpoints**. It
is not a unit test: it needs live devices, an hour of wall time, and a generated
signal playing into the endpoint that the System track captures. Both soaks are
dependencies of the `publish` job in `.github/workflows/ci.yml`, so a failed
soak on either OS blocks a release.

## What a pass means

`crates/buddy-capture/tests/soak_real.rs` opens the real backend for both
tracks, captures for exactly 3600 seconds, stops cleanly, and then asserts:

- `read_gaps` returns **zero** gap markers — no ring overruns and no device-loss
  gaps. Any marker fails the soak.
- `read_chunk_index` returns **two** sequences (System and Mic) that are valid
  and gap-free: non-empty, `index == 0, 1, 2, …` with no missing index,
  `frames > 0`, 48 000 Hz, 2 channels.
- Each track covers at least **3 420 seconds = 95% of the 3 600 s target**.
  Coverage is `sum(chunk.frames) / (48 000 × 2)` interleaved f32 values per
  second, so an early stop cannot pass merely because the indices are
  contiguous and no gap marker was written.
- Every chunk except the single final one has at least **96 000 interleaved
  f32 values** (48 000 frames × 2 channels = one full second). The writer
  publishes only full chunks while capturing and flushes at most one short
  chunk on stop; a short chunk before the last one is lost audio. The final
  chunk may be partial but must still contain at least one value.
- The System track's first chunk contains the generated tone (peak amplitude
  above 0.01), proving the signal actually reached the captured endpoint
  rather than the soak recording silence.

A **zero-lost-chunk pass** therefore means the full hour was published as two
continuous chunk sequences with no overrun, no device re-arm, no fewer than
3 420 s of audio per track, no short chunk other than a possible final partial
chunk, and no missing chunk. The job prints the per-track chunk counts and
coverage in seconds (normally about 3600 chunks / 3600 s per track) and the
gap-marker list; both are exported as job outputs and shown in the job summary
even when the soak fails.

These soaks cannot be fully exercised on a development Linux host without the
matching audio graph/endpoint; CI is the place they run. Do not report a local
run as a soak pass unless it met every criterion above.

## Common requirement: `BUDDY_AUDIO_SOAK=1`

All real-capture tests honour `BUDDY_AUDIO_SOAK`. With `BUDDY_AUDIO_SOAK=1`, a
missing endpoint is a **hard failure** instead of a reason-labelled skip. Both
CI soak jobs set it, so a device-less runner cannot pass by skipping. The
ignored soak test also requires it explicitly:

```
BUDDY_AUDIO_SOAK=1 cargo test -p buddy-capture --test soak_real -- --ignored --nocapture
```

## Linux (Arch Linux, PipeWire)

CI job `soak-linux` runs in `archlinux:base` and installs PipeWire,
pipewire-pulse, WirePlumber, `pw-play`, `pactl` and FFmpeg. It creates a null
sink (`soak_null_sink`) and a null source (`soak_null_source`) with
`libpipewire-module-loopback`, makes them the defaults, generates a 3600-second
440 Hz stereo tone with FFmpeg, plays it into the default sink with `pw-play`,
and runs the soak. To reproduce locally:

```bash
# With PipeWire, WirePlumber and pipewire-pulse already running and your own
# null sink/source configured as the defaults:
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=3600' \
  -ac 2 -c:a pcm_s16le soak-tone.wav
pw-play soak-tone.wav &
BUDDY_AUDIO_SOAK=1 cargo test -p buddy-capture --test linux_pipewire -- --nocapture
BUDDY_AUDIO_SOAK=1 cargo test -p buddy-capture --test soak_real -- --ignored --nocapture
```

## Windows (WASAPI loopback, self-hosted)

CI job `soak-windows` runs on a **self-hosted** runner with labels
`self-hosted`, `windows` and `buddy-audio-soak`. GitHub-hosted Windows runners
have no audio endpoint, and the runner-images maintainers do not pre-install
virtual hardware, so this job must not be moved to `windows-latest`. The
runner must have:

- a real default **render** endpoint (WASAPI loopback captures its mix), and
- a real default **capture** endpoint for the Mic track.

No virtual audio device is installed or required by Buddy or by its users. The
job generates a looping 440 Hz WAV with PowerShell, plays it to the default
render endpoint with `System.Media.SoundPlayer`, sets `BUDDY_AUDIO_SOAK=1`, and
runs the two WASAPI integration tests (`windows_wasapi.rs`), `windows::tests::*`
and the ignored soak test. To reproduce locally on such a machine:

```powershell
$env:BUDDY_AUDIO_SOAK = '1'
cargo test -p buddy-capture --test windows_wasapi -- --nocapture
cargo test -p buddy-capture --lib windows::tests -- --nocapture
cargo test -p buddy-capture --test soak_real -- --ignored --nocapture
```

## macOS: UNVERIFIED

There is no macOS soak. The CoreAudio backend type-checks for Apple targets,
but real screen and audio capture has not been verified on macOS; no macOS
test machine is confirmed. The release notes keep that statement verbatim.
