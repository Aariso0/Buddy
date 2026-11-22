# Buddy MVP on the `application` branch — design

> Date: 2026-11-22. Status: draft for approval. This document covers the MVP only; later milestones are explicitly out of scope here.

## 1. Intent and why

Buddy is a local-first recorder for meetings and lectures. The repository currently contains a plan, README, license and brand assets — no source code. The goal of this branch is to deliver the MVP described in `plan.md` §5 and milestones M0–M6 as a working, distributable application, with a CI pipeline that produces release artifacts automatically for Windows, macOS and Linux.

The driving priority from `plan.md` §2 is that capture comes first: recording must never depend on an AI model, a network connection or an account. The MVP is the smallest release that is genuinely useful on its own — record a real lecture or meeting, get a transcript and slides, find things later, and export.

## 2. Goals

1. Record system audio and microphone as independent, synchronized tracks, with chunked writes, crash recovery and device-loss recovery.
2. Capture slide changes plus periodic safety snapshots, with optional low-fps screen video, per session.
3. Transcribe locally in English and Hindi, with a live preview and a final timestamped pass.
4. Run OCR on new slides and link text to screenshots and transcript time.
5. Browse, search (SQLite FTS5), replay and export sessions (Markdown and JSON), with library backup and restore.
6. Show visible recording state, and work with no account and no internet.
7. Build and test on a three-OS CI matrix, and publish unsigned release artifacts automatically.

### Non-goals (out of scope for this branch)

- AI providers and analysis (M7).
- Sync server, accounts, end-to-end encryption and the web viewer (M8–M9).
- Speaker diarization, media sync, web note editing, PDF export.
- Signing, notarization or an update mechanism. These are deferred until budget and credentials exist.
- Flatpak and Snap packages. Their sandboxing complicates the screen and audio portal permissions this MVP depends on; reconsider after capture is verified on real machines.

### Anti-goals (what would make this branch a failure even if it builds)

- A build that compiles on CI but whose capture backends have never been exercised on real hardware and whose limitations are undocumented.
- A release artifact users cannot run without unexplained extra steps.
- Any feature that blocks or slows audio capture.
- Shipping secrets, keys or private metadata in an export or release artifact.

## 3. Constraints

- **Branch:** all work lands on `application`. The Git remote is `https://github.com/Aariso0/Buddy.git`.
- **Plan is authoritative:** `plan.md` defines the MVP (§5), architecture (§7), platform matrix (§8) and exit criteria (§18). This design narrows and sequences it; it does not replace it.
- **License:** Apache-2.0, public repository. Dependency licenses must be verified before adoption; nothing in `plan.md` marked `(verify)` has been verified yet.
- **Runtime budget:** install size target under 60 MB without models; capture CPU under 5% of one core; idle memory under 250 MB; 60-minute soak with zero lost chunks. These are targets to measure, not yet results (`plan.md` §15).
- **Unsigned releases:** Windows, macOS and Linux artifacts are published unsigned in this branch. No code-signing credentials, secrets or signing steps exist in CI. The cost of signing (an Apple Developer Program membership at $99/yr, or a Windows signing service/certificate) is not available yet.
- **Verification limit:** hosted CI runners can prove compilation and unit/contract tests with fake sources, but cannot exercise real system-audio or screen capture. Real capture will be tested on Arch Linux and Windows; no macOS test machine is confirmed. A failed real-capture soak on Arch Linux or Windows blocks the release until fixed. macOS real-capture stays UNVERIFIED and must be documented as a known limit rather than blocking the MVP.
- **OS support floors:** macOS 14.2 or later; Linux with PipeWire only; Windows 10 or later, with backend limitations accepted rather than requiring users to install a virtual audio device. These replace the unverified guesses in `plan.md` §20 assumption 1.
- **Process:** this design must be approved before implementation begins. Work then follows an implementation plan with verifiable slices.

## 4. Approaches considered

| Approach | Shape | Trade-off |
|---|---|---|
| **A — Plan order, CI from day one** | M0 stands up the monorepo, Tauri shell, crate skeletons, session format and a GitHub Actions matrix that already emits every artifact. Then M1 audio → M2 ASR → M3 visual → M4 OCR → M5 UI → M6 search/export. Each milestone keeps its exit criteria and continuously produces installable builds. Platform-capture spikes are folded into the start of M0/M1. | Platform capture problems surface in M1 instead of at the end; there is always something installable. Cost: the first genuinely useful app takes the full MVP span. |
| **B — Spike-first de-risking** | Throwaway prototypes for per-OS audio/screen backends and whisper.cpp English/Hindi before real code. | Fewer wrong assumptions, but nothing buildable for a while, and it still cannot verify real capture without physical machines. |
| **C — Thin end-to-end spine, then deepen** | A crude record → transcript → search flow across all three OSes, then fill in depth. | Fast feedback, but the two hardest subsystems get redesigned later. |

**Chosen: A.** The user picked it on 2026-11-22. B's de-risking value is retained by starting the S1–S3 platform spikes inside M0/M1 rather than as a separate dead phase. C's rework risk is unjustified in the two hardest subsystems.

## 5. Decided design

### 5.1 Shape of the code

Follow the monorepo in `plan.md` §7: a pnpm workspace plus a Cargo workspace, with `apps/desktop` (Tauri 2, React + TypeScript + Vite), `apps/web` scaffolded but not filled in, `crates/` for the `buddy-*` Rust crates, `packages/` for shared UI/schemas/crypto types, `server/` only as a placeholder for later milestones, and `tests/` for integration, contract and soak tests.

The Rust core keeps the module boundaries from `plan.md` §7: `buddy-core` (session model, monotonic clock, events), `buddy-capture` (per-OS `AudioSource` and `ScreenSource` traits), `buddy-asr`, `buddy-vision`, `buddy-ocr`, `buddy-store`. The AI and sync crates are not built in this branch.

Capture stays real-time-safe: a monotonic session clock timestamps every event; audio threads do no allocation or locking on the hot path and write to a ring buffer; a writer thread flushes chunks to disk; all downstream work consumes bounded queues that degrade rather than throttling capture.

### 5.2 Data

Session folders and SQLite tables follow `plan.md` §13. `session.json` is written atomically with a state (`recording` / `finalizing` / `complete`) so interrupted sessions can be offered for repair on launch. Transcript segments carry a revision (`live` / `final` / `corrected`); corrections never overwrite the original.

### 5.3 Platform backends

Per-OS backends behind shared traits, as in `plan.md` §8: PipeWire monitor / WASAPI loopback / ScreenCaptureKit-audio-or-CoreAudio taps for system audio; PipeWire / WASAPI / CoreAudio for microphone; xdg-desktop-portal ScreenCast + X11 fallback / Windows Graphics Capture / ScreenCaptureKit for screen capture. Slide detection stays pure Rust (`image` + SSIM/dHash), not OpenCV. Every backend runs the same contract suite with a fake source driving CI.

Committed OS floors: **macOS 14.2+**, **Linux with PipeWire only** (no PulseAudio-only or plain-ALSA fallback), and **Windows 10+ with limitations accepted** (no virtual loopback device required). That makes the macOS 13 option from `plan.md` §20 obsolete. S1 is now resolved: on 14.2+ a native system-audio path exists without a virtual loopback device. Apple documents `SCStreamConfiguration.capturesAudio` (ScreenCaptureKit audio, macOS 13.0+) and CoreAudio process taps via `AudioHardwareCreateProcessTap` on macOS 14.2+; the process-tap sample explicitly requires macOS 14.2 or later, the `NSAudioCaptureUsageDescription` key and the user's System Audio Recording permission. Buddy adopts the MIT-licensed `coreaudio` 0.6.2 crate with its `process-tap` feature: `TrackKind::System` taps all CoreAudio processes into a private aggregate device, and `TrackKind::Mic` opens the default input device. ScreenCaptureKit is a confirmed alternative (`screencapturekit` 12.0.0, MIT OR Apache-2.0), but its microphone path needs macOS 15.0+, above the 14.2 floor, so it is not adopted. Licence and version evidence is in `NOTICE`. A cross-target `cargo check --target x86_64-apple-darwin --all-targets` passes for the backend and its contract tests, but no macOS linking, test execution or real capture was possible on this Linux host, so those stay UNVERIFIED. The desktop shell still has to add `NSAudioCaptureUsageDescription` before a real-capture attempt can obtain the permission prompt. Capture API choices and encoder availability remain open (S2–S3, S5) until the remaining spikes verify them.

Transcription uses a GPU when one is available and falls back to CPU otherwise. Each per-OS build must compile in the relevant backends (CUDA/Vulkan on Windows and Linux, Metal on macOS) and select at runtime. Bundling these may break the under-60 MB install target in `plan.md` §8; S4 must measure it and the target may need revision.

### 5.4 Release and CI

GitHub Actions, triggered on pull requests targeting `main` and on pushes to `main` (decided 2026-11-22, superseding tag-triggered releases on `application`):

- **Linux** (`ubuntu-latest`): build, lint, unit and contract tests; package as AppImage, `.deb`, `.rpm` and `.tar.gz`.
- **Windows** (`windows-latest`): build and test; package as an NSIS `.exe` installer.
- **macOS** (`macos-latest`): build and test; package as a `.dmg`.

`apps/desktop/package.json` is the single source of truth for the version. A push to `main` runs the full test matrix; it publishes a release only when that version is newer than the latest `v*` tag, using tag `v<version>`. A merge with an unchanged version tests but publishes nothing. Artifacts are uploaded from successful package jobs and attached to that release. A release is published only after real-capture tests pass on Arch Linux and Windows; a failure there blocks the release until fixed. macOS real capture is untested and ships as a documented UNVERIFIED limit. No signing or notarization step exists. Release notes must state plainly that builds are unsigned and list the OS floors: on macOS 14.2+, users may need to whitelist the app in Privacy & Security or remove the quarantine attribute; on Windows 10+, SmartScreen may warn, and Windows 11 with Smart App Control can block unsigned apps entirely. Adding signing later must be a matter of inserting a signing step and secrets, not restructuring the pipeline.

All dependencies are pinned in lockfiles and their licenses verified before adoption. A `NOTICE` file is generated or maintained for attribution.

## 6. Risks carried into implementation

- S1–S10 in `plan.md` §19 are all still open; S1's capability question is resolved as recorded in §5.3, but its real-capture verification is not. Real capture will be exercised on Arch Linux and Windows; no macOS machine is confirmed, so macOS capture ships UNVERIFIED despite the plan's three-OS exit criteria. A failed soak on Arch Linux or Windows blocks release until fixed. The macOS process tap needs the user's System Audio Recording grant and an `NSAudioCaptureUsageDescription` Info.plist entry; without the grant the tap is created but delivers silence, so this is a documented UNVERIFIED limit rather than a silent success.
- Windows 10+ is supported with the WASAPI backend's limitation characterised and accepted. Buddy captures system audio with a shared-mode WASAPI loopback on the default render endpoint (`wasapi` 0.27.0, MIT). Microsoft documents loopback as shared-mode only — exclusive-mode streams cannot operate in loopback mode — and event-driven loopback clients are supported on Windows 10 version 1703 and later. A shared-mode loopback stream captures what the audio engine mixes for the endpoint; when an application takes the endpoint exclusively it seizes the device and silences other applications, so that application's audio is not available to Buddy's loopback and the stream may report device loss. No virtual loopback device is required. Release notes must state this plainly.
- Linux support is PipeWire-only; S3 must cover the Wayland portal behaviour on Arch.
- Hindi accuracy, live-preview latency and Devanagari FTS5 tokenization are unmeasured; defaults and quality claims must wait for S4 and S6.
- Unsigned artifacts will produce user-visible OS warnings; the install steps above are part of the deliverable, not an afterthought.
- The repository license is Apache-2.0, but dependency licensing — including media encoders and any server-side components kept as placeholders — is not yet verified.

## 7. What is left to the builder

This document fixes scope, sequencing, platform coverage, data shape and the release bar. It deliberately leaves these to implementation planning, where a wrong guess is cheaper to correct:

1. Exact crate versions and library choices for each backend, with license verification against upstream sources, plus the `NOTICE` generation approach.
2. The concrete session-format schema, FTS5 tokenization strategy and recovery algorithm.
3. The CI workflow structure, cache strategy, package-building commands and artifact naming.
4. How each platform contract test drives a fake source, and which tests run on which runner.
5. The spike order and exit evidence for S1–S7 and S10 within M0/M1.
6. Wording of the unsigned-build notice and the release notes.
7. Whether `apps/web` and `server/` are empty placeholders or minimal scaffolds in M0.

Minimum OS versions are now a user decision, not an open assumption: macOS 14.2+, PipeWire-only Linux, Windows 10+ with limitations accepted. The other items in `plan.md` §20 (English-first UI, configurable hotkeys, lossless-then-compressed audio, no telemetry, auto-update deferred) remain planning defaults; confirm any that a milestone depends on before building on it.

## 8. Evidence

- `README.md` — product description, principles, platform and tech stack.
- `plan.md` — goals (§4), MVP (§5), architecture (§7), platform matrix (§8), testing (§16), milestones and exit criteria (§18), risks (§19), assumptions (§20).
- Tauri 2 signing guides and Microsoft Artifact Signing documentation, verified 2026-11-22, for the signing constraints and unsigned-artifact consequences.
