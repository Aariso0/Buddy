# M1 Audio Capture Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record system audio and microphone as two independent, synchronized tracks with chunked lossless writes, pause/resume, device-loss recovery and crash recovery — real on Linux (PipeWire) and Windows 10+, compiled but UNVERIFIED on macOS 14.2+.

**Architecture:** A new `buddy-capture` crate owns an `AudioSource` trait, an engine driven by a monotonic session clock, a bounded single-producer/single-consumer ring buffer on the capture hot path, and a writer thread that publishes one-second IEEE-float WAV chunks plus gap markers into the existing session directory. `buddy-core` gains the clock, a session finalizer, strict UUID validation and a data-dir bootstrap. Per-OS backends sit behind the trait; the same fake-source contract suite runs in CI on every OS. CI gains real-capture soak jobs on Arch Linux and Windows that gate `publish`.

**Tech Stack:** Rust stable; `thiserror`, `serde`, `chrono`, `uuid` (already in use); PipeWire monitor of the default sink (Linux), WASAPI loopback (Windows 10+), ScreenCaptureKit audio / CoreAudio taps (macOS 14.2+, UNVERIFIED); Tauri 2 commands for control. Exact backend crate versions are chosen by the license-verified spike at the start of each backend task, not guessed here.

**Spec:** `docs/specs/2026-11-22-mvp-branch-design.md` (approved 2026-11-22), `plan.md` §5, §7–§8, §13–§16, §18–§19, and `docs/specs/2026-11-22-m0-completion.md` §"Session identity and creation semantics".

**Scope:** M1 audio capture only. Screen/video capture (`ScreenSource`, slide detection, video encode) is M3 and out of scope. No ASR, OCR, AI, sync, accounts, telemetry, network calls or signing. Branch `M1/Audio_Capture`; the user owns the PR into `main` — do not push.

## Execution status (live — updated 2026-11-23)

| Task | State | Commits | Review |
|---|---|---|---|
| 1 — Clock, capture traits and fake source | complete | `09a8ca3` | clean; 3 Minor findings deferred |
| 2 — Engine, ring buffer, chunks, gap markers | complete | `d4747ca`, `f0ac818` | 4 Important findings fixed; re-review clean |
| 3 — Session integration, data dir, UUID rule, recovery | complete | `8d35d16` | clean; 2 Minor findings deferred |
| 4 — Linux PipeWire backend | complete | `7b86295`, `efdf0ea`, `b44b2eb` | 2 Important findings fixed; re-review clean |
| 5 — Windows WASAPI loopback backend (S2) | complete | `eb136ae`, `5e0500d` | 1 Important test-integrity finding fixed; re-review clean; Windows UNVERIFIED |
| 6 — macOS backend spike (S1), UNVERIFIED | complete | `7cdb496`, `65d09ef` | 3 Important findings fixed; re-review clean; macOS runtime UNVERIFIED |
| 7 — Desktop control surface and visible state | complete | `909dc7e`, `f7c7edd` | 1 Critical + 1 Important fixed; re-review clean; macOS desktop compile UNVERIFIED |
| 8 — Soak jobs, publish gating, legal recheck | complete | `7707035`, `d623566` | review Approved + hardening re-review clean; soaks not run locally |

All eight tasks are complete. Current branch head: `d623566`. Nothing has been pushed. The Windows endpoint decision was confirmed by the user: `soak-windows` targets the self-hosted `[self-hosted, windows, buddy-audio-soak]` runner with real endpoints; no virtual device is imposed on users. Windows compilation, contract tests and real loopback capture remain UNVERIFIED on this Linux host and are gated on the self-hosted soak. macOS linking, desktop compilation, test execution and real capture also remain UNVERIFIED; a clean Apple-target type-check is not a runtime pass. Soaks were correctly not run locally, so no soak pass is claimed. This section is updated after each task's review; the SDD ledger holds the per-finding detail.

### Execution rulings made so far

These are implementation rulings under the binding spec, not scope changes. Each was needed because the approved plan's literal text was impossible, ambiguous, or contradicted by a review finding.

- `AudioSource` gained object-safe `fn reconnect(&mut self) -> Result<(), CaptureError>`. The plan's `open(device_id) where Self: Sized` cannot be called on `Box<dyn AudioSource>`, so each backend now remembers its own id and returns a source ready to read or a typed error. Later backends must implement it.
- Gap markers carry a per-event timestamp through a bounded SPSC `GapQueue`, drained and globally sorted by the writer. The earlier single-slot design collapsed several gaps into one wrong time.
- A non-`DeviceLost` read error is recorded in a typed reader-error slot, appended as a `DeviceLoss` gap for the finished track, and returned from `stop()`. `GapReason { Overrun, DeviceLoss }` is unchanged; a failed stop is never reported as `Ok(())`.
- `CaptureEngine::elapsed_ms()` was added so the pause/resume test can assert a gap occurs at or after the real boundary; it will also serve Task 7's HUD.
- `finalize_session` receives the sessions root derived from `CaptureEngine.session_dir.parent()`, not the full session directory. A clean stop finalizes; a failed stop leaves the manifest recoverable and returns the original error. `SessionError` maps to `CaptureError::Backend("finalize session: …")`.
- The brief's uppercase UUID literal was all digits, so `.to_uppercase()` was a no-op. Only the test literal was corrected to a hex-letter v4 UUID; production validation is unchanged.
- Linux uses `pipewire = "=0.10.1"` (MIT), verified upstream and recorded in `NOTICE`. CI installs `libpipewire-0.3-dev` in both Linux apt lists. Headless tests accept a typed absence error; real capture is deferred to Task 8's Arch soak.
- Unknown PipeWire device ids fail at `open()`/`reconnect()` through the same registry used by `list_devices`. An empty-ring deadline returns a typed `Backend` error, never `Ok(0)` end-of-stream.
- Windows compilation and capture cannot be verified from this Linux host. Task 5 reports them as UNVERIFIED until a Windows target/CI soak runs; no local pass was claimed. The pinned `wasapi = "=0.27.0"` MIT binding sets `AUDCLNT_STREAMFLAGS_LOOPBACK` for render→capture in shared mode; the exclusive-mode limitation is documented in the design doc and `NOTICE`.
- A Windows contract test must never treat an arbitrary `Backend` error as "no device": device presence comes from `list_devices`, assertions are unconditional when a real endpoint exists, and genuine no-device is an explicit reason-labelled skip. `BUDDY_AUDIO_SOAK=1` makes absence a hard failure for Task 8's runner.
- macOS S1 resolved via CoreAudio process taps on 14.2+ (`coreaudio = "=0.6.2"`, `process-tap` feature), no virtual device; ScreenCaptureKit's binding needs macOS 15+ for mic and was rejected. macOS tap teardown is order-pinned (aggregate before tap), worker shutdown is idempotent/bounded, and a `DEVICE_IS_ALIVE` listener plus direct probe makes unplugs typed `DeviceLost` so reconnect works. Apple-target `cargo check --all-targets` passed but proves types only.
- The desktop shell carries both `NSAudioCaptureUsageDescription` (system taps) and `NSMicrophoneUsageDescription` (mic input) or macOS capture is TCC-denied into silence; it also calls `ensure_data_dir` before the first settings write or session creation. macOS desktop compilation remains UNVERIFIED because no Apple SDK is available for the Tauri crate.
- A typed stop failure must discard the dead engine and publish an error-annotated idle view: the engine clears `started` before its fallible returns, so restoring it wedges the app on REC. `create_session` takes an explicit `preset` with no hidden default so `start_recording("meeting")` really persists `Meeting` in `session.json`.
- The Windows soak endpoint decision was escalated to the user, who chose a self-hosted runner with real audio endpoints; `soak-windows` targets `[self-hosted, windows, buddy-audio-soak]`. No virtual-device requirement is imposed on users and no hosted-runner soak pass is faked. Both `soak-linux` and `soak-windows` are `publish.needs` dependencies, so a failed soak blocks release.
- The ignored `crates/buddy-capture/tests/soak_real.rs` hard-fails without `BUDDY_AUDIO_SOAK=1` and, after hardening, asserts per-track coverage of at least 95% of 3600 s (3420 s), per-chunk length of at least 96 000 interleaved f32 values with at most one partial final chunk, zero gaps and a non-silent System track. The soaks were not run locally; no pass is claimed.

### Deferred review findings for final review

- Task 1: `NOTICE` first-party note still lists three crates, not four.
- Task 2: second `start()` after `stop()` can leak a writer thread; blocking backend `read()` can hang `stop()`; unchecked WAV size conversions; malformed gap JSON uses `InvalidChunk`; `ChunkInfo.frames` counts interleaved values, not block-aligned frames; `both_tracks_record_simultaneously` checks presence only; `GapMarker` has no track field.
- Task 3: `empty_reserved_dir_is_not_adopted` does not assert `recover_session` explicitly; failing-finalize error mapping has no dedicated test.
- Task 4: format/channel negotiation not checked; `pending_error` reports `DeviceLost` after close/failed reconnect; connect-timeout worker can linger briefly; poisoned `Mutex` in `list_devices` degrades silently; live-runtime test does not assert `count > 0`.
- Task 5: `shutdown_worker` swallows timeouts then unbounded `join()` can hang close/reconnect; error slots are consumed so repeat device-loss reads return `Backend("")`; `list_devices`/`spawn_worker` timeouts leave brief stray threads; public `open_for` permits inconsistent kind/id pairs; `reconnect` publishes LIVE before worker success and leaves a stale sample rate; full-ring pushes are dropped without an overflow signal.
- Task 6: consumed error slots make repeat reads `Backend("")` and allocate on fallback; 500 ms `yield_now` spin; `CHANNELS = 2` without channel normalization for mono/multichannel mics; contract test accepts silent taps (`count > 0` only) and `BUDDY_AUDIO_SOAK=1` cannot catch silence; `list_devices` `Backend` error skips outside soak mode.
- Task 7: system source not `close()`d on a failed start; discarded title/tray/emit results in the view sink; discarded hotkey/tray toggle errors; missing failed-stop/preset tests (the latter two now covered by the fix round's tests).
- Task 1 deferred item resolved: `NOTICE` first-party note now correctly lists four workspace crates including `buddy-capture`.
- Task 8: non-default endpoint assertion in `soak_real.rs` is stricter than the brief and could hard-fail a correctly configured real-endpoint-only runner; Linux/Windows toolchain drift (`pacman` Rust vs `dtolnay/rust-toolchain@stable`) could let compiler versions diverge.

## Global Constraints

- Branch `M1/Audio_Capture`; remote `https://github.com/Aariso0/Buddy.git`. Do not push.
- License Apache-2.0, public repository. Verify every new dependency's license from its upstream source before adoption; update `NOTICE` and, for MPL-2.0 dependencies, `LICENSE-MPL-2.0.txt` plus the exact-version source-locations generator in `.github/workflows/ci.yml` in the same task.
- OS floors (decided): macOS **14.2+** (capture UNVERIFIED), Linux **PipeWire only** (no PulseAudio-only or plain-ALSA fallback), Windows **10+ with backend limitations accepted**. No virtual loopback device requirement for users.
- Capture priority: audio threads do no allocation or locking on the hot path; a writer thread flushes chunks; downstream work consumes bounded queues that degrade rather than throttle capture.
- One monotonic session clock timestamps every event.
- Session identity (binding): `create_session` reserves the directory exclusively and never overwrites (`SessionError::AlreadyExists`); the manifest `id` must equal its directory id (`SessionError::IdMismatch`), never silently repaired. A crash between reservation and manifest rename leaves an empty directory — never adopt or delete it automatically in M1.
- Real-capture soaks on **Arch Linux and Windows** must gate the `publish` job. A failed soak blocks release. macOS real capture stays UNVERIFIED in docs and release notes.
- `save_settings` assumes its parent directory exists; M1 creates the data directory before the first call.
- Install target under 60 MB without models; capture CPU under 5% of one core; idle memory under 250 MB; 60-minute soak with zero lost chunks. Targets to measure, not claims.
- Local-first: no network calls, accounts, telemetry or sync code. All artifacts remain unsigned; no signing secrets or steps.
- Verification for every task: `cargo fmt --all -- --check`, `cargo test --workspace`, `pnpm --filter @buddy/desktop build`.

## Calls made in this plan (check these while reading)

1. **M1 is audio-only** (user-confirmed 2026-11-22). The manager mail said "screen/audio"; M3 owns screen capture. The M1 soak jobs gate `publish` on real **audio** capture; screen capture inherits the same gating when M3 lands.
2. **UUID canonicalisation:** reject any id that is not exactly lowercase hyphenated `Uuid::to_string()` form with `SessionError::InvalidSessionId`. No silent repair, no uppercase aliases. This closes the last open identity question from M0.
3. **Hot-path ring buffer** is implemented in-crate with `std::sync::atomic`, so capture needs no new dependency. A backend crate still needs upstream license verification; if no compatible crate exists, stop and ask — do not hand-roll a backend.
4. **Chunk format:** interleaved IEEE-float WAV (lossless), one second per chunk, named `audio/{system|mic}-{index:04}.wav`. Gap markers append as JSONL at `audio/gaps.jsonl` from the writer thread only.
5. **Windows soak needs an audio endpoint on the runner.** If hosted runners have none, that is a user decision (self-hosted runner versus a CI-only virtual device); the plan does not silently add a virtual-device requirement for users.

## Review Focus

These conditions are implied by the spec but easy to miss. Each is pinned to a test in the task that owns the code.

1. Ring-buffer overrun under load must insert a gap marker and keep capturing — never block capture, panic, or publish a short chunk as complete. Test: `overrun_records_gap_and_keeps_capturing` in Task 2.
2. Device disconnect mid-recording must be a typed error with a reconnect path and a gap marker; no partial chunk is published as complete. Test: `device_loss_records_gap_and_reconnects` in Task 2.
3. Pause/resume must keep the session clock monotonic and must not timestamp resumed frames before the pause boundary. Test: `pause_resume_keeps_monotonic_timeline` in Task 2.
4. A crash between directory reservation and manifest rename leaves an empty directory: creation returns `AlreadyExists`, read/recover return `Io(NotFound)`, and M1 never overwrites or auto-repairs it. Test: `empty_reserved_dir_is_not_adopted` in Task 3.
5. Non-canonical UUID spellings must fail as typed errors, not create a second directory that can never be read back. Test: `noncanonical_uuid_is_typed_error` in Task 3.
6. First run must create the data directory before `save_settings` writes into it. Test: `ensure_data_dir_precedes_save_settings` in Task 3.
7. `publish` must depend on the soak jobs; a YAML that merely comments about them is not a gate. Test: `publish_depends_on_both_soaks` in Task 8.
8. macOS must stay documented as UNVERIFIED even once its backend compiles. Test: `release_notes_keep_macos_unverified` in Task 8.

---

### Task 1: Clock, capture traits and fake source

**Files:**
- Modify: `Cargo.toml` (add `crates/buddy-capture` to `members`)
- Create: `crates/buddy-capture/Cargo.toml`, `crates/buddy-capture/src/lib.rs`, `crates/buddy-capture/src/fake.rs`
- Create: `crates/buddy-core/src/clock.rs`
- Test: `crates/buddy-core/tests/clock.rs`, `crates/buddy-capture/tests/contract.rs`

**Interfaces:**
- Consumes: nothing beyond the existing workspace.
- Produces:
  - `buddy_core::clock::SessionClock` with `new() -> Self`, `now(&self) -> std::time::Duration` (monotonic since construction, never decreases). Re-export from `crates/buddy-core/src/lib.rs`.
  - `buddy_capture::{TrackKind, DeviceInfo, CaptureError, AudioSource}` where `TrackKind` is `{ System, Mic }` with `#[serde(rename_all = "lowercase")]`, `DeviceInfo { id: String, name: String, kind: TrackKind }`, and `CaptureError` is a `thiserror` enum with at least `DeviceLost(String)`, `DeviceBusy(String)`, `UnsupportedPlatform(String)`, `Backend(String)`, `InvalidChunk(String)`, `Io(#[from] std::io::Error)`. `TrackKind` and `DeviceInfo` derive `Debug, Clone, PartialEq, Serialize, Deserialize` so Tauri commands can return them.
  - `pub trait AudioSource: Send` with `fn kind(&self) -> TrackKind`, `fn sample_rate(&self) -> u32`, `fn list_devices(kind: TrackKind) -> Result<Vec<DeviceInfo>, CaptureError> where Self: Sized`, `fn open(device_id: &str) -> Result<Self, CaptureError> where Self: Sized`, `fn read(&mut self, out: &mut [f32]) -> Result<usize, CaptureError>` (fills the caller's buffer, no allocation), `fn pause(&mut self) -> Result<(), CaptureError>`, `fn resume(&mut self) -> Result<(), CaptureError>`, `fn close(&mut self) -> Result<(), CaptureError>`.
  - `buddy_capture::fake::FakeAudioSource` with `from_samples(kind: TrackKind, sample_rate: u32, samples: Vec<f32>) -> Self`, `fail_next_read_with(error: CaptureError)`, and `frames_read(&self) -> usize`.

- [ ] **Step 1: Write the failing clock test**

```rust
use buddy_core::clock::SessionClock;

#[test]
fn clock_never_moves_backwards() {
    let clock = SessionClock::new();
    let mut last = clock.now();
    for _ in 0..1000 {
        let now = clock.now();
        assert!(now >= last);
        last = now;
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p buddy-core --test clock`
Expected: compile error — `buddy_core::clock` does not exist.

- [ ] **Step 3: Implement `SessionClock`**

Use `std::time::Instant` captured in `new()`; `now()` returns `self.origin.elapsed()`. No wall-clock time anywhere in capture.

- [ ] **Step 4: Write the failing fake-source contract test**

In `crates/buddy-capture/tests/contract.rs`:

```rust
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
    assert!(matches!(src.read(&mut [0.0; 16]), Err(CaptureError::DeviceLost(_))));
}
```

- [ ] **Step 5: Run it to verify it fails**

Run: `cargo test -p buddy-capture --test contract`
Expected: compile error — crate or fake module does not exist.

- [ ] **Step 6: Implement the traits and fake source**

Define the exact signatures above. `crates/buddy-capture/Cargo.toml` depends on `thiserror` and `serde` with `derive` (both already in the workspace lockfile); add `tempfile` as a dev-dependency in Task 2. `FakeAudioSource::read` copies from its script into the caller's slice and returns the count; an injected error is returned once, then reads resume. No timestamps in `AudioSource` — the engine applies `SessionClock`.

- [ ] **Step 7: Run all new tests and verify green**

Run: `cargo test -p buddy-core -p buddy-capture`
Expected: all clock and contract tests pass.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/buddy-core crates/buddy-capture
git commit -m "feat: add session clock and capture source traits with fake"
```

---

### Task 2: Engine, ring buffer, chunks and gap markers

**Files:**
- Create: `crates/buddy-capture/src/engine.rs`, `crates/buddy-capture/src/ring.rs`, `crates/buddy-capture/src/chunks.rs`
- Modify: `crates/buddy-capture/src/lib.rs` (re-export new modules)
- Test: `crates/buddy-capture/tests/contract.rs`

**Interfaces:**
- Consumes: `SessionClock`, `AudioSource`, `TrackKind`, `CaptureError` and `FakeAudioSource` from Task 1.
- Produces:
  - `buddy_capture::ring::RingBuffer` with `new(capacity_frames: usize, channels: usize) -> Self`, `push(&self, samples: &[f32]) -> Result<(), RingError>`, `pop(&self, out: &mut [f32]) -> Option<usize>`; `RingError::Full` is a plain enum, not a panic.
  - `buddy_capture::engine::CaptureConfig { chunk_frames: u32, ring_frames: usize, channels: u16, sample_rate: u32 }` with `Default` = 48_000 Hz, 2 channels, `chunk_frames = 48_000`, `ring_frames = 8192`.
  - `buddy_capture::engine::CaptureEngine::new(session_id: &str, session_dir: &Path, clock: SessionClock, system: Box<dyn AudioSource>, mic: Box<dyn AudioSource>, config: CaptureConfig) -> Self` (stores `session_id` as `String` for the finalizer), with `start`, `pause`, `resume`, `stop` (all `-> Result<(), CaptureError>`).
  - `buddy_capture::chunks::{ChunkInfo, write_chunk, read_chunk_index, GapMarker, GapReason, append_gap, read_gaps}` where `ChunkInfo { track: TrackKind, index: u32, path: PathBuf, frames: u32, sample_rate: u32, channels: u16 }`, `GapReason { Overrun, DeviceLoss }`, `GapMarker { start_ms: u64, end_ms: u64, reason: GapReason }`, `write_chunk(session_dir: &Path, track: TrackKind, index: u32, samples: &[f32], sample_rate: u32, channels: u16) -> Result<ChunkInfo, CaptureError>`, `read_chunk_index(session_dir: &Path) -> Result<Vec<ChunkInfo>, CaptureError>`, `append_gap(session_dir: &Path, marker: &GapMarker) -> Result<(), CaptureError>`, `read_gaps(session_dir: &Path) -> Result<Vec<GapMarker>, CaptureError>`.

- [ ] **Step 1: Write the failing chunk and gap tests**

```rust
use buddy_capture::chunks::*;
use buddy_capture::TrackKind;

#[test]
fn chunk_round_trip_preserves_float_samples() {
    let dir = tempfile::tempdir().unwrap();
    let samples: Vec<f32> = (0..96_000).map(|i| i as f32 / 96_000.0).collect();
    let info = write_chunk(dir.path(), TrackKind::System, 0, &samples, 48_000, 2).unwrap();
    assert_eq!(info.frames, 96_000);
    assert_eq!(read_chunk_index(dir.path()).unwrap().len(), 1);
    assert_eq!(read_chunk_samples(dir.path(), &info).unwrap(), samples); // helper in test module
}

#[test]
fn gap_markers_round_trip_as_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    append_gap(dir.path(), &GapMarker { start_ms: 1200, end_ms: 1350, reason: GapReason::DeviceLoss }).unwrap();
    let gaps = read_gaps(dir.path()).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].start_ms, 1200);
    assert!(matches!(gaps[0].reason, GapReason::DeviceLoss));
}
```

Add `tempfile` as a dev-dependency of `buddy-capture`. `read_chunk_samples` is a test helper that decodes the WAV data chunk back to `Vec<f32>`; it must not be part of the public API.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p buddy-capture --test contract`
Expected: compile error — modules do not exist.

- [ ] **Step 3: Implement chunks and gaps**

Write IEEE-float WAV (`WAVE_FORMAT_IEEE_FLOAT`) with a temp-file → fsync → rename publish, matching `buddy_core::session::write_manifest`. `read_chunk_index` validates WAV headers and returns `CaptureError::InvalidChunk` for truncated files. Append gap markers with one `write_all` plus `sync_all` per line; writer thread only.

- [ ] **Step 4: Write the failing engine contract tests**

```rust
use buddy_capture::chunks::{read_chunk_index, read_gaps, GapReason};
use buddy_capture::engine::*;
use buddy_capture::fake::FakeAudioSource;
use buddy_capture::{CaptureError, TrackKind};

fn engine_with(system: Vec<f32>, mic: Vec<f32>, dir: &std::path::Path) -> CaptureEngine {
    CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir,
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(TrackKind::System, 48_000, system)),
        Box::new(FakeAudioSource::from_samples(TrackKind::Mic, 48_000, mic)),
        CaptureConfig::default(),
    )
}

#[test]
fn both_tracks_record_simultaneously() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = engine_with(vec![0.5; 96_000], vec![0.25; 96_000], dir.path());
    engine.start().unwrap();
    engine.stop().unwrap();
    let kinds: Vec<_> = read_chunk_index(dir.path()).unwrap().into_iter().map(|c| c.track).collect();
    assert!(kinds.contains(&TrackKind::System));
    assert!(kinds.contains(&TrackKind::Mic));
}

#[test]
fn pause_resume_keeps_monotonic_timeline() {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = engine_with(vec![0.5; 144_000], vec![0.25; 144_000], dir.path());
    engine.start().unwrap();
    engine.pause().unwrap();
    engine.resume().unwrap();
    engine.stop().unwrap();
    let starts: Vec<u64> = read_gaps(dir.path()).unwrap().iter().map(|g| g.start_ms).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    assert!(read_chunk_index(dir.path()).unwrap().iter().all(|c| c.frames > 0));
}

#[test]
fn device_loss_records_gap_and_reconnects() {
    let dir = tempfile::tempdir().unwrap();
    let mut mic = FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.25; 96_000]);
    mic.fail_next_read_with(CaptureError::DeviceLost("mic-1".into()));
    let mut engine = CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir.path(),
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(TrackKind::System, 48_000, vec![0.5; 96_000])),
        Box::new(mic),
        CaptureConfig::default(),
    );
    engine.start().unwrap();
    engine.stop().unwrap();
    let gaps = read_gaps(dir.path()).unwrap();
    assert_eq!(gaps.iter().filter(|g| matches!(g.reason, GapReason::DeviceLoss)).count(), 1);
    assert!(read_chunk_index(dir.path()).unwrap().iter().any(|c| c.track == TrackKind::Mic));
}

#[test]
fn overrun_records_gap_and_keeps_capturing() {
    let dir = tempfile::tempdir().unwrap();
    let config = CaptureConfig { ring_frames: 8, ..CaptureConfig::default() };
    let mut engine = CaptureEngine::new(
        "11111111-1111-4111-8111-111111111111",
        dir.path(),
        buddy_core::clock::SessionClock::new(),
        Box::new(FakeAudioSource::from_samples(TrackKind::System, 48_000, vec![0.5; 96_000])),
        Box::new(FakeAudioSource::from_samples(TrackKind::Mic, 48_000, vec![0.25; 96_000])),
        config,
    );
    engine.start().unwrap();
    engine.stop().unwrap();
    assert!(read_gaps(dir.path()).unwrap().iter().any(|g| matches!(g.reason, GapReason::Overrun)));
    assert!(read_chunk_index(dir.path()).unwrap().iter().all(|c| c.frames > 0));
}
```

- [ ] **Step 5: Run them to verify they fail**

Run: `cargo test -p buddy-capture --test contract`
Expected: compile error — engine/ring modules do not exist.

- [ ] **Step 6: Implement ring buffer and engine**

`RingBuffer` is a bounded SPSC queue of `f32` samples using atomics for head/tail; `push` fails with `RingError::Full` instead of allocating or blocking. The engine runs one reader thread per source, copies into the ring, counts overruns in an atomic, and a writer thread drains rings into chunks via `write_chunk`. On `DeviceLost`, the engine records a `DeviceLoss` gap from the writer thread and reopens the same device id; on overrun it records an `Overrun` gap. `stop` drains, publishes final chunks, and returns only after the writer thread has flushed and synced.

- [ ] **Step 7: Run all tests and verify green**

Run: `cargo test -p buddy-capture`
Expected: all contract tests pass on the host OS.

- [ ] **Step 8: Commit**

```bash
git add crates/buddy-capture
git commit -m "feat: add capture engine with chunked writes and gap recovery"
```

---

### Task 3: Session integration, data dir, UUID rule and recovery

**Files:**
- Modify: `crates/buddy-core/src/session.rs` (`session_dir` validation, add `finalize_session`)
- Modify: `crates/buddy-core/src/settings.rs` (add `ensure_data_dir`)
- Modify: `crates/buddy-core/src/lib.rs` (re-exports if needed)
- Modify: `crates/buddy-capture/src/engine.rs` (call `finalize_session` on clean stop)
- Test: `crates/buddy-core/tests/session_format.rs`, `crates/buddy-core/tests/settings.rs`

**Interfaces:**
- Consumes: `create_session`, `read_session`, `recover_session`, `SessionError` from M0; `CaptureEngine` from Task 2.
- Produces:
  - `buddy_core::session::finalize_session(root: &Path, id: &str, now: chrono::DateTime<chrono::Utc>) -> Result<SessionManifest, SessionError>` — sets `state = Complete` and `ended_at = Some(now)` through the existing atomic manifest write.
  - `buddy_core::settings::ensure_data_dir(settings: &Settings) -> Result<PathBuf, SettingsError>` — creates `settings.data_dir` and `settings.data_dir.join("sessions")`, returns the sessions root; idempotent.
  - `session_dir` now rejects any id where `Uuid::parse_str(id)` succeeds but `parsed.to_string() != id` with `SessionError::InvalidSessionId(id.to_owned())`.

- [ ] **Step 1: Write the failing identity and data-dir tests**

```rust
#[test]
fn noncanonical_uuid_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    for id in ["11111111-1111-4111-8111-111111111111".to_uppercase().as_str(),
               "11111111111141118111111111111111"] {
        assert!(matches!(
            create_session(root.path(), id, now()),
            Err(SessionError::InvalidSessionId(_))
        ));
    }
}

#[test]
fn empty_reserved_dir_is_not_adopted() {
    let root = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    std::fs::create_dir_all(root.path().join(id)).unwrap(); // crash left this behind
    assert!(matches!(create_session(root.path(), id, now()), Err(SessionError::AlreadyExists(_))));
    assert!(matches!(read_session(root.path(), id), Err(SessionError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound));
}

#[test]
fn ensure_data_dir_precedes_save_settings() {
    let root = tempfile::tempdir().unwrap();
    let settings = Settings { data_dir: root.path().join("data").join("nested"), log_level: "info".into() };
    let sessions_root = ensure_data_dir(&settings).unwrap();
    assert!(sessions_root.is_dir());
    save_settings(&settings.data_dir.join("settings.json"), &settings).unwrap();
    assert_eq!(load_settings(&settings.data_dir.join("settings.json")).unwrap(), settings);
}

#[test]
fn finalize_session_marks_complete() {
    let root = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    create_session(root.path(), id, now()).unwrap();
    let done = finalize_session(root.path(), id, now()).unwrap();
    assert!(matches!(done.state, SessionState::Complete));
    assert!(!recover_session(root.path(), id).unwrap().needs_repair);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p buddy-core`
Expected: compile errors for the new functions; the identity test fails if `create_session` still accepts uppercase ids.

- [ ] **Step 3: Implement the session and settings changes**

Add the canonical check inside `session_dir` before `Uuid::parse_str` returns success. Add `finalize_session` using the existing `write_manifest`. Add `ensure_data_dir` with `fs::create_dir_all`. Update `CaptureEngine::stop` to call `finalize_session(&self.session_dir, &self.session_id, Utc::now())` after the writer thread flushes. `session_id` is already stored by `CaptureEngine::new` in Task 2; do not change the constructor signature here.

- [ ] **Step 4: Run the whole workspace and verify green**

Run: `cargo fmt --all -- --check && cargo test --workspace && pnpm --filter @buddy/desktop build`
Expected: all green; existing M0 tests still pass unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/buddy-core crates/buddy-capture
git commit -m "feat: integrate capture with sessions and canonical ids"
```

---

### Task 4: Linux PipeWire backend

**Files:**
- Create: `crates/buddy-capture/src/linux.rs`
- Modify: `crates/buddy-capture/src/lib.rs` (cfg-gated re-export), `NOTICE`, `.github/workflows/ci.yml` (legal staging only if a new MPL dependency is adopted)
- Test: `crates/buddy-capture/tests/linux_pipewire.rs`

**Interfaces:**
- Consumes: `AudioSource`, `DeviceInfo`, `CaptureError`, `TrackKind` from Task 1.
- Produces: `buddy_capture::linux::PipeWireSource` implementing `AudioSource`, compiled only under `#[cfg(target_os = "linux")]`. `PipeWireSource::open("default")` opens the default sink monitor for `TrackKind::System` and the default source for `TrackKind::Mic`.

- [ ] **Step 1: Spike and record the crate choice**

Before writing code, verify one PipeWire Rust binding against its upstream: exact version, license text, maintained status, and that it exposes sink-monitor capture without a PulseAudio fallback. Write the finding into `NOTICE` and, if MPL-2.0 applies, the source-locations generator. If no compatible crate exists, stop this task and ask the user — do not hand-roll a PipeWire client or add a fallback backend.

- [ ] **Step 2: Write the failing backend test**

```rust
#[cfg(target_os = "linux")]
#[test]
fn pipewire_source_implements_contract() {
    use buddy_capture::linux::PipeWireSource;
    // list_devices(System) contains a default sink monitor; open("default")
    // yields kind System and a positive sample_rate. read() into a caller-
    // owned buffer never allocates on the hot path.
}

#[cfg(target_os = "linux")]
#[test]
fn pipewire_reports_missing_pipewire_as_typed_error() {
    // With no PipeWire runtime, open("default") returns CaptureError::Backend(_)
    // or UnsupportedPlatform(_), never a panic or an empty Ok.
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p buddy-capture --test linux_pipewire`
Expected: compile error — module does not exist.

- [ ] **Step 4: Implement the backend**

Implement the exact `AudioSource` signatures. Map native errors to `CaptureError` variants; device removal maps to `DeviceLost`. No network calls, no PulseAudio or ALSA fallback, no allocation in `read`.

- [ ] **Step 5: Verify on the host and in CI shape**

Run: `cargo test -p buddy-capture && cargo check --workspace`
Expected: tests pass on Linux; `cargo check` passes on other hosts with the module cfg-gated out.

- [ ] **Step 6: Commit**

```bash
git add crates/buddy-capture NOTICE .github/workflows/ci.yml Cargo.lock
git commit -m "feat: add PipeWire audio backend for Linux"
```

---

### Task 5: Windows WASAPI loopback backend (S2)

**Files:**
- Create: `crates/buddy-capture/src/windows.rs`
- Modify: `crates/buddy-capture/src/lib.rs` (cfg-gated re-export), `NOTICE`, `.github/workflows/ci.yml` (legal staging if needed)
- Test: `crates/buddy-capture/tests/windows_wasapi.rs`

**Interfaces:**
- Consumes: `AudioSource`, `DeviceInfo`, `CaptureError`, `TrackKind` from Task 1.
- Produces: `buddy_capture::windows::WasapiSource` implementing `AudioSource`, compiled only under `#[cfg(target_os = "windows")]`. `WasapiSource::open("default")` opens WASAPI loopback for `TrackKind::System` and the default capture endpoint for `TrackKind::Mic`.

- [ ] **Step 1: Spike and record the crate choice (S2)**

Verify one WASAPI binding against upstream: exact version, license, loopback support on Windows 10+, and observed behaviour with exclusive-mode apps. Record in `NOTICE` (and source locations if MPL). Document the characterisation in `docs/specs/2026-11-22-mvp-branch-design.md` §"Risks carried into implementation" so release notes can state the limitation honestly. If loopback is not viable on Windows 10, stop and ask the user — no virtual loopback device requirement may be introduced.

- [ ] **Step 2: Write the failing backend test**

```rust
#[cfg(target_os = "windows")]
#[test]
fn wasapi_source_implements_contract() {
    // list_devices(System) contains a default loopback endpoint; open("default")
    // yields kind System and a positive sample_rate; read() fills the caller's
    // buffer without allocating.
}

#[cfg(target_os = "windows")]
#[test]
fn wasapi_missing_endpoint_is_typed_error() {
    // An invalid device id returns CaptureError::DeviceLost(_) or Backend(_).
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test -p buddy-capture --test windows_wasapi`
Expected: compile error on Windows — module does not exist.

- [ ] **Step 4: Implement the backend**

Implement the exact `AudioSource` signatures. Map endpoint removal to `DeviceLost`; no allocation in `read`; no virtual-device installation step in setup or docs.

- [ ] **Step 5: Verify**

Run: `cargo test -p buddy-capture && cargo check --workspace`
Expected: green on Windows; other hosts compile with the module gated out.

- [ ] **Step 6: Commit**

```bash
git add crates/buddy-capture docs/specs/2026-11-22-mvp-branch-design.md NOTICE Cargo.lock
git commit -m "feat: add WASAPI loopback audio backend for Windows"
```

---

### Task 6: macOS backend spike (S1) — UNVERIFIED

**Files:**
- Create: `crates/buddy-capture/src/macos.rs`
- Modify: `crates/buddy-capture/src/lib.rs` (cfg-gated re-export), `NOTICE`
- Test: `crates/buddy-capture/tests/macos_audio.rs`
- Modify: `.github/release-notes-template.md` (keep the UNVERIFIED statement; no change expected)

**Interfaces:**
- Consumes: `AudioSource`, `DeviceInfo`, `CaptureError`, `TrackKind` from Task 1.
- Produces: `buddy_capture::macos::CoreAudioSource` implementing `AudioSource`, compiled only under `#[cfg(target_os = "macos")]`, or no backend at all if S1 finds no viable system-audio path. macOS stays UNVERIFIED either way.

- [ ] **Step 1: Resolve S1 before writing a backend**

Check ScreenCaptureKit audio and CoreAudio process taps on macOS 14.2+ against upstream docs, exact version and license; record the finding in `NOTICE` and `docs/specs/2026-11-22-mvp-branch-design.md`. If no viable system-audio path exists, do not implement a backend: leave the task here, keep the UNVERIFIED statement, and ask the user how to proceed. No machine is confirmed for real capture, so compilation is the only verification available.

- [ ] **Step 2: Write or skip the failing test, then implement if viable**

```rust
#[cfg(target_os = "macos")]
#[test]
fn core_audio_source_implements_contract() {
    // list_devices(System) is non-empty or returns a typed Backend error;
    // open("default") yields the expected TrackKind; read() fills the caller's
    // buffer without allocating. This proves compilation and shape, not real
    // system-audio capture.
}
```

- [ ] **Step 3: Verify and keep the limit visible**

Run: `cargo test -p buddy-capture && cargo check --workspace`
Run: `grep -Fq "Real screen and audio capture has not been verified on macOS" .github/release-notes-template.md`
Expected: green; the UNVERIFIED statement is still present verbatim.

- [ ] **Step 4: Commit (only if a backend was implemented)**

```bash
git add crates/buddy-capture docs/specs/2026-11-22-mvp-branch-design.md NOTICE
git commit -m "feat: add macOS audio backend (unverified on hardware)"
```

---

### Task 7: Desktop control surface and visible recording state

**Files:**
- Modify: `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src/App.tsx`, `apps/desktop/src/main.tsx`
- Modify: `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/package.json` (only license-verified Tauri plugins), `NOTICE`
- Test: `apps/desktop/src-tauri/src/lib.rs` unit tests

**Interfaces:**
- Consumes: `buddy_capture::engine::{CaptureEngine, CaptureConfig}`, `buddy_capture::AudioSource`, `buddy_core::settings::ensure_data_dir`, `buddy_core::session::create_session`.
- Produces: Tauri commands `list_audio_devices(kind: TrackKind) -> Result<Vec<DeviceInfo>, String>`, `start_recording(preset: Preset) -> Result<String, String>` (returns the session id), `pause_recording(id: String) -> Result<(), String>`, `resume_recording(id: String) -> Result<(), String>`, `stop_recording(id: String) -> Result<(), String>`. A single process-wide engine guarded by a mutex; visible recording state shown in the window title and a HUD line.

- [ ] **Step 1: Write the failing command test**

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn recording_state_is_visible_and_hotkey_toggles_it() {
        let app = test_app_with_fake_engine(); // helper in this test module
        app.call("start_recording", "lecture").unwrap();
        assert!(app.window_title().contains("REC"));
        assert!(app.hud_text().contains("system"));
        assert!(app.hud_text().contains("mic"));
        app.trigger_hotkey("start_stop");
        assert!(!app.window_title().contains("REC"));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p buddy-desktop`
Expected: compile error — commands do not exist.

- [ ] **Step 3: Implement commands and visible state**

Call `ensure_data_dir` before the first settings write or session creation. Map every typed error to a non-zero command result with the error text; never panic or return an empty success on error. The tray/hotkey plugins must each have their license verified from upstream and be recorded in `NOTICE` before adoption; if a plugin is not compatible, stop and ask rather than adding an unverified dependency.

- [ ] **Step 4: Verify the full local gate**

Run: `cargo fmt --all -- --check && cargo test --workspace && pnpm --filter @buddy/desktop build`
Expected: all green; `apps/desktop/dist/index.html` exists.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop NOTICE Cargo.lock pnpm-lock.yaml
git commit -m "feat: expose recording controls and visible recording state"
```

---

### Task 8: Soak jobs, publish gating, legal recheck and exit verification

**Files:**
- Modify: `.github/workflows/ci.yml` (add `soak-linux`, `soak-windows`; add both to `publish.needs`), `.github/release-notes-template.md`, `NOTICE`
- Create: `tests/soak/README.md` (how to run each soak and what a pass means)

**Interfaces:**
- Consumes: real backends from Tasks 4–6, engine from Task 2, existing `prepare`/`package-*`/`publish` jobs.
- Produces: `soak-linux` and `soak-windows` jobs on the push-to-`main` path; `publish.needs` includes both; a documented 60-minute zero-lost-chunk pass criterion.

- [ ] **Step 1: Write the failing gate test**

```python
import yaml
wf = yaml.safe_load(open(".github/workflows/ci.yml"))
publish = wf["jobs"]["publish"]
assert "soak-linux" in publish["needs"]
assert "soak-windows" in publish["needs"]
```

Also assert the release-notes file still contains the exact UNVERIFIED and unsigned statements from the M0 plan.

- [ ] **Step 2: Run it to verify it fails**

Run: the Python check above.
Expected: `AssertionError` — the soak jobs are not yet dependencies of `publish`.

- [ ] **Step 3: Add the soak jobs and gate publish**

`soak-linux`: `archlinux:base` container with PipeWire, pipewire-pulse and WirePlumber; create a null sink and a null source; run a generated 60-minute signal through the default sink; assert zero gap markers from overrun/device loss and that `read_chunk_index` returns two valid, gap-free track sequences. `soak-windows`: a Windows runner with an audio endpoint; run the same generated signal through WASAPI loopback; assert the same criteria. Both report chunk counts and gap markers as job output on failure. If the hosted Windows runner has no endpoint, stop and ask the user (self-hosted runner versus CI-only virtual device) — do not silently add a virtual-device requirement for users.

- [ ] **Step 4: Recheck legal attribution**

Re-run the legal staging dry run. Confirm `NOTICE`, `LICENSE`, `LICENSE-MPL-2.0.txt` and the exact-version source-locations file still stage into `legal/` before upload, and that every dependency adopted in Tasks 4–7 is attributed. Confirm no signing step was added.

- [ ] **Step 5: Verify the specified checks**

```bash
python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/ci.yml")); print("ok")'
cargo fmt --all -- --check && cargo test --workspace && pnpm --filter @buddy/desktop build
```

Expected: YAML parses; all three commands pass. Soak jobs themselves cannot be fully exercised locally — report that plainly rather than claiming they passed.

- [ ] **Step 6: Commit**

```bash
git add .github tests/soak NOTICE
git commit -m "ci: gate publish on real-capture soaks"
```

---

## Exit Criteria for M1

- `cargo fmt --all -- --check`, `cargo test --workspace` and `pnpm --filter @buddy/desktop build` pass locally.
- Both tracks record simultaneously with a shared monotonic clock; pause/resume keeps the timeline ordered; two independent chunk sequences are written per session.
- A 60-minute soak on Arch Linux and on Windows records zero lost chunks; both soak jobs are dependencies of `publish`.
- A simulated crash leaves a session that `recover_session` flags with `needs_repair` and whose published chunks validate; `finalize_session` marks a clean stop `Complete`.
- Device loss and ring overrun produce typed errors and gap markers and never throttle capture or publish a short chunk as complete.
- `create_session` never overwrites an existing session; non-canonical UUIDs are rejected with a typed error; manifest id always equals directory id.
- Data directory exists before the first `save_settings`.
- Linux PipeWire and Windows WASAPI backends compile on their targets and pass the fake-source contract suite; macOS backend, if implemented, compiles only and remains documented UNVERIFIED.
- Every adopted dependency is license-verified and attributed; no signing, network, account, telemetry, AI or sync code was added; nothing was pushed.
