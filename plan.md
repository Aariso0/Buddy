# Buddy: Project Plan (v2)

> Status: active development. M0 (Foundation) and M1 (Audio capture) are implemented and reviewed on branch `M1/Audio_Capture`; see [`docs/specs/2026-11-22-m0-completion.md`](docs/specs/2026-11-22-m0-completion.md) and [`docs/plans/2026-11-22-m1-audio-capture.md`](docs/plans/2026-11-22-m1-audio-capture.md). The remaining milestones below are plans, not shipped code. Refined from `Buddy_Project_Plan.md` after a requirements interview (2026-11-21).
> Items marked **(verify)** are beliefs that have not been checked against current docs or hardware. Each has a validation spike in section 19.

---

## 1. Overview

**Buddy** is a local-first recorder for meetings and lectures. It captures what was said (system audio and microphone, separately) and what was shown (slide screenshots and optional screen video), then produces a timestamped transcript, OCR text, and a searchable session library. AI summaries and Q&A are optional and pluggable. Notes can optionally sync to a multi-user server with end-to-end encryption (E2EE), so they can be read from a web app on any device.

**Core principle:** original evidence (audio, screenshots, video, raw transcript, OCR) is the source of truth. AI output is a separate, labelled layer. Recording must never depend on an AI model, a network connection, or an account.

### Who it is for
- Students recording lectures (slides, long sessions, revision notes).
- Professionals recording meetings (decisions, action items, follow-ups).
Both are first-class. A per-session **preset** (Lecture / Meeting) tunes slide sensitivity and default AI tasks. It never changes what is captured.

---

## 2. Principles

1. **Capture first.** Audio capture has top priority. Nothing else (transcription, OCR, UI, AI, sync) may block or slow it.
2. **Local by default.** With sync off, Buddy makes no network calls except user-initiated model or update downloads.
3. **Sync is invisible until wanted.** No sign-in wall, no prompts, no badges, no nag screens. The only sync UI lives in Settings → Sync.
4. **Evidence over generation.** AI results never overwrite transcripts. Every AI claim links back to timestamps or slides.
5. **Explicit consent for external data flow.** Before any transcript, OCR text or image leaves the machine (AI provider or sync), the user sees exactly what will be sent.
6. **Lightweight.** Small install, small idle footprint, small server. Prefer simple components, bounded queues, and on-demand processing.
7. **Honest limits.** Document what E2EE does and does not protect, what Hindi accuracy to expect, and what each platform can and cannot capture.

---

## 3. Decisions

| Area | Decision | Notes |
|---|---|---|
| Name | Buddy | Check for trademark/name conflicts before public launch |
| License | Apache-2.0 | Check dependency licenses (see section 8) |
| Desktop shell | Tauri 2 (Rust core + webview) | Shares TypeScript/React with the web app |
| UI | React + TypeScript (Vite), shared between desktop and web | Shared component package |
| Platforms | Windows, macOS, Linux (Wayland and X11), from the start | Platform traits from M0; per-OS capture backends |
| Local storage | SQLite + FTS5, files on disk | One folder per session |
| Transcription | whisper.cpp, local only; English + Hindi; live preview + final pass | Section 9 |
| Visual capture | Slides (change-detected screenshots), video, or both, per session | Section 10 |
| OCR | Tesseract (eng + hin), on new slides only | |
| AI | Provider abstraction: OpenAI-compatible API (covers LM Studio, Ollama, API keys), Ollama, OpenCode, Claude Code | Section 11 |
| Server | Node.js + TypeScript, REST, MongoDB | Stateless, small footprint |
| Accounts | Email + password, GitHub login, Google login | Identity only; does not unlock data |
| Sync privacy | End-to-end encrypted | Server cannot read or search synced data |
| Key model | WhatsApp-style: vault passphrase + recovery key + QR device pairing | Section 12 |
| Hosting | Self-hostable server plus a project-run instance | Docker image and docs for self-hosting |
| Sync scope (first release) | Notes, transcript, OCR text, AI artifacts, bookmarks, tags | Media sync is designed for, shipped later |

---

## 4. Goals and non-goals

### Goals
- Record system audio and microphone as independent tracks on one synchronized timeline.
- Capture slide changes (screenshots) and optionally screen video.
- Transcribe locally in English and Hindi, live (preview) and after recording (final).
- OCR slides and link text to screenshots and transcript time ranges.
- Browse, search, replay and export sessions locally.
- Optional AI analysis through a user-chosen provider, with evidence links.
- Optional multi-user, E2EE sync and a responsive web viewer.

### Non-goals
- Recording without a clear user action and a visible recording indicator.
- Replacing evidence with AI-generated text.
- Any account or cloud requirement for local use.
- Autonomous AI actions that can delete sessions or upload data without confirmation.
- Server-side AI over synced data (impossible under E2EE by design).
- Meeting-platform bots or joining calls on the user's behalf.

---

## 5. Release tiers

### What "MVP" means here
The **MVP** is the smallest release that is genuinely useful on its own: you can record a real lecture or meeting, get a transcript and slides, find things later, and export. Everything else layers on top.

### MVP (local-only, all three OSes)
1. Record system audio and microphone into separate tracks, simultaneously.
2. Survive a 60-minute recording without capture loss; recover after a crash.
3. Capture slide changes and periodic safety snapshots; optional screen video.
4. Live transcript preview, then a final timestamped transcript (English + Hindi).
5. OCR on slides, linked to screenshots and transcript time.
6. Click a transcript line: audio seeks and the matching slide shows.
7. Search transcript and OCR text; filter by date, tag, title.
8. Export a session to Markdown and JSON; backup and restore the library.
9. Visible recording state (window, tray, and OS indicator where available).
10. Works with no account and no internet.

### v1 (adds)
AI providers and analysis (M7).

### v1.x (adds)
Sync server, E2EE, web viewer (M8, M9). Media sync, note editing on web, and key rotation come after.

---

## 6. Features

Tags: **[MVP]**, **[v1]**, **[v1.x]**, **[later]**.

### A. Recording
- **[MVP]** Start, pause, resume, stop; global hotkeys (start/stop, bookmark); tray control.
- **[MVP]** System audio and microphone as separate tracks; device selection and refresh.
- **[MVP]** Incremental writes (chunked), crash recovery, device-disconnect recovery.
- **[MVP]** HUD: elapsed time, sources, levels, disk space remaining, recording indicator.
- **[MVP]** First-run notice that recording other people may require their consent. Buddy gives no legal advice.
- **[MVP]** Audio quality presets (lossless chunks while recording, compressed on finalize).

### B. Visual capture
- **[MVP]** Display or window/region selection; multi-monitor and display-change handling.
- **[MVP]** Slide mode: sampled frames, change detection, full-resolution screenshot on change, periodic safety snapshots.
- **[MVP, cuttable]** Video mode: low-fps screen video, estimated storage shown before start.
- **[MVP]** Per-session mode: Off / Slides / Video / Both.

### C. Transcription
- **[MVP]** Local whisper.cpp, English and Hindi; language setting Auto / English / Hindi.
- **[MVP]** Live preview (provisional text) and a final pass after stop. Raw live text is kept as its own revision.
- **[MVP]** "You" vs "Others" labels for free, from mic vs system track.
- **[MVP]** Model manager: download with checksum, size and speed guidance, delete.
- **[MVP]** Transcript corrections stored as a separate layer; original never overwritten.
- **[later]** True speaker diarization within the system-audio track.

### D. OCR and linking
- **[MVP]** Tesseract (eng + hin) on newly detected slides only; dedupe repeated text.
- **[MVP]** Link OCR to screenshot ID and capture time; link transcript segments to nearby slides.

### E. Timeline and playback
- **[MVP]** Unified timeline: audio, transcript, slides, OCR, bookmarks, video.
- **[MVP]** Click transcript to seek; highlight current segment; show nearest slide; track toggles; bookmarks.

### F. Library, search, export
- **[MVP]** Library with title, date, duration, tags, status; rename, tag, archive, delete (with confirmation).
- **[MVP]** FTS5 search over transcript and OCR; filters; jump to timestamp.
- **[MVP]** Export to Markdown and JSON; backup/restore of the library.
- **[later]** PDF export.

### G. AI analysis (all optional, never blocks capture)
- **[v1]** Overview and summary; topic outline with timestamps; detailed notes.
- **[v1]** Decisions, action items, questions, follow-ups (meeting preset).
- **[v1]** Key concepts, definitions, flashcards, quiz questions, revision sheets (lecture preset).
- **[v1]** Ask questions about a session, answers cite timestamps and slides.
- **[v1]** Summary-vs-source check that flags unsupported statements.
- **[v1]** Chunk long sessions, summarize chunks, consolidate. Regenerate without touching evidence.

### H. Sync and web (all optional)
- **[v1.x]** Create account, enable sync, per-session toggle, status and retry queue in Settings.
- **[v1.x]** Web viewer: session list, transcript, notes, slides (if synced), client-side search, Markdown export.
- **[later]** Web note editing, media sync, session sharing, key rotation.

---

## 7. Architecture

```text
┌──────────────────────────── Desktop app (Tauri 2) ────────────────────────────┐
│  React UI: library · HUD · viewer · search · settings                         │
│                         │  (Tauri commands / events)                          │
│  ┌──────────────────────┴───────────────────────┐                             │
│  │ Rust core                                    │                             │
│  │  capture  ── audio (per-OS)  ── screen (per-OS)                            │
│  │  asr      ── VAD → whisper.cpp (live + final)                              │
│  │  vision   ── downscale → SSIM/hash → slide events                          │
│  │  ocr      ── Tesseract on slide events                                     │
│  │  store    ── SQLite + FTS5, session folders, recovery                      │
│  │  ai       ── provider trait: HTTP APIs · agent CLIs                        │
│  │  sync     ── queue · crypto · REST client (off by default)                 │
│  └──────────────────────────────────────────────┘                             │
└──────────────────────────────┬────────────────────────────────────────────────┘
                               │ optional, E2EE ciphertext only
                               ▼
              ┌──────────── Server (Node + TypeScript) ────────────┐
              │ auth · vault key blobs · device pairing relay ·    │
              │ record store (ciphertext) · rate limiting          │
              │ MongoDB                                            │
              └─────────────────────────┬──────────────────────────┘
                                        ▼
                    Web app (React): decrypts and searches in the browser
```

### Capture pipeline rules
- One **session clock** (monotonic) timestamps every event: audio chunks, frames, screenshots, bookmarks.
- Audio threads are real-time-safe: no allocation or locks on the hot path, write to a ring buffer, a writer thread flushes chunks to disk.
- All downstream work (VAD, ASR, vision, OCR) consumes bounded queues. When a queue grows too long it degrades (live ASR pauses, smaller model, skipped frames) and records a gap marker. **Capture is never throttled.**
- Final processing runs after stop, resumable per chunk.

### Core modules (Rust crates)
`buddy-capture`, `buddy-asr`, `buddy-vision`, `buddy-ocr`, `buddy-store`, `buddy-ai`, `buddy-sync`, `buddy-crypto`, `buddy-core` (session model, clock, events).

### Repository layout (monorepo: pnpm workspace + Cargo workspace)
```text
buddy/
├── README.md  plan.md  LICENSE  NOTICE
├── assets/                  # logo set generated by logo.py
├── docs/                    # architecture, data-model, crypto-spec, privacy, providers
├── apps/
│   ├── desktop/             # Tauri app: src/ (React), src-tauri/ (Rust glue)
│   └── web/                 # React web viewer
├── crates/                  # buddy-* Rust crates
├── packages/
│   ├── ui/                  # shared React components
│   ├── schemas/             # shared record schemas + crypto test vectors
│   └── crypto/              # TypeScript E2EE implementation
├── server/                  # Node API
└── tests/                   # integration + soak tests, fixtures
```

---

## 8. Technology and platform matrix

| Concern | Choice | Linux | Windows | macOS |
|---|---|---|---|---|
| App shell | Tauri 2 | ✓ | ✓ | ✓ |
| System audio | per-OS backend behind `AudioSource` trait | PipeWire monitor of the output sink | WASAPI loopback | ScreenCaptureKit audio (macOS 13+) or CoreAudio process taps (macOS 14.2+) **(verify)** |
| Microphone | per-OS via `cpal` or native | PipeWire | WASAPI | CoreAudio |
| Screen capture | per-OS backend behind `ScreenSource` trait | xdg-desktop-portal ScreenCast + PipeWire (Wayland), X11 fallback | Windows Graphics Capture | ScreenCaptureKit (Screen Recording permission) |
| Video encode | OS-native or hardware encoder preferred | VA-API / NVENC **(verify)** | Media Foundation | VideoToolbox |
| ASR | whisper.cpp | CPU, CUDA, Vulkan | CPU, CUDA, Vulkan | CPU, Metal |
| Slide detection | pure Rust (`image` + custom SSIM/dHash) | ✓ | ✓ | ✓ |
| OCR | Tesseract (eng + hin) | ✓ | ✓ | ✓ |

Notes
- **Slide detection is pure Rust, not OpenCV.** This is a change from the original plan: OpenCV adds a heavy native dependency to every platform build. Reconsider only if quality demands it.
- **FFmpeg is not assumed.** Bundling x264/FFmpeg has license implications for an Apache-2.0 project **(verify)**; prefer OS encoders or a permissively licensed encoder.
- Crate choices (`cpal`, `pipewire`, `whisper-rs`, `xcap`/`scap`, `keyring`, `rusqlite`) must be checked for maintenance and platform coverage before adoption **(verify)**.
- Install size target: under 60 MB without models. Models are downloaded on demand.

---

## 9. Transcription (English + Hindi, live + final)

### Approach
- **Models:** multilingual ggml models (not `.en`) are required for Hindi. Offer Tiny, Base, Small, Medium; Large-class optional **(verify availability and size)**.
- **Defaults:** Small as the default, Medium recommended when hardware allows. Hindi accuracy on small models is expected to be noticeably weaker. **Unknown until measured (S4).**
- **Language:** Auto / English / Hindi. In Auto, detection runs per utterance and is restricted to English vs Hindi where whisper.cpp allows it **(verify)**. Mixed Hindi-English speech is a known weak spot; Hindi output is in Devanagari, and Roman-script Hinglish is not produced natively.
- **Two tracks, two labels:** mic track is "You", system track is "Others". Each track is transcribed separately in the final pass.

### Live preview pipeline
1. Resample to 16 kHz mono ring buffer per track (or one mixed stream for live preview to halve compute; final pass is always per-track).
2. VAD segmenter (Silero-class VAD **(verify)**) cuts utterances (max ~15 s).
3. Worker thread decodes each utterance; results are emitted as `provisional` segments.
4. **Backpressure:** if the decode lag exceeds a threshold, switch to a smaller model or pause live preview and insert a "live preview paused" marker. The final pass covers the gap.
5. After stop: final pass with the user's chosen model, chunked and resumable. Display switches to final text; the live text stays stored as its own revision.

### Storage
Segments: start/end (session clock), text, track, speaker label, confidence, language, revision (`live` / `final` / `corrected`).

---

## 10. Visual capture

### Modes (per session)
Off · **Slides** (default) · Video · Both.

### Slide mode
- Sample every 1-2 s (configurable), downscale to ~320 px grayscale.
- Compare against the last saved slide using SSIM and a perceptual hash; save a full-resolution PNG/WebP when the change exceeds the threshold.
- **Settle logic:** wait for N consecutive stable frames before saving, to skip mid-transition frames and animations.
- Ignore small regions (cursor, clock, notification badges) via a minimum changed-area rule.
- Safety snapshot every few minutes even if nothing changed.
- Presets: Lecture (more sensitive, slide-like content) and Meeting (less sensitive, screen share noise).

### Video mode
- Default 5 fps, max 15 fps, hardware-accelerated encode where available.
- Fragmented MP4 so an interrupted recording is still playable.
- Video start offset stored against the session clock; slide screenshots can be extracted later from video.
- Storage estimate (MB per hour) shown before start; low-disk warning during recording.

### Platform capture notes
- Wayland: the portal shows its own picker; restore tokens avoid re-prompting where the compositor supports them **(verify, including Hyprland and GNOME/KDE)**.
- macOS: Screen Recording permission flow must be guided in onboarding.
- Windows: Windows Graphics Capture border/indicator behavior must be checked **(verify)**.

---

## 11. AI providers

### Interface
A Rust trait `AiProvider`: `capabilities()`, `check_available()`, `run(task, input, cancel) -> stream of events`. Providers live in `buddy-ai` and are independent of capture and storage.

### Providers
1. **OpenAI-compatible HTTP**: base URL + model + optional key. This one adapter covers **LM Studio**, **Ollama's** OpenAI-compatible endpoint, and regular API keys from compatible vendors.
2. **Ollama (native)**: model listing and health check; optional.
3. **Vendor-native adapters** (Anthropic, Gemini): added only where the compatible endpoint is insufficient.
4. **Agent CLIs, opt-in and labelled experimental:** **OpenCode** and **Claude Code**. They are coding agents, not summarization APIs. Both are believed to offer a non-interactive mode **(verify exact flags and behavior before building the adapter)**.
   - Run in a temporary working directory containing only an exported evidence bundle (Markdown/JSON), with read-only permissions.
   - Bounded input and output, timeout, cancel, no shell access beyond what the user approved.

### Rules
- **Consent screen** before any external call: provider, model, and exactly which data (transcript ranges, OCR text, screenshots) will be sent. Localhost endpoints still show what is sent, but are labelled "stays on this machine".
- Secrets live in the OS keychain (`keyring`). Never in session folders, exports, or sync records.
- Output is requested as structured JSON, validated against a schema, with timestamps checked to exist in the session.
- Each result is stored as a separate `ai_artifact` with provider, model, date, task and prompt version, and evidence links. Re-running with another provider keeps the old result.
- Long sessions: chunk by topic/time, summarize chunks, consolidate. Never send a whole long session in one prompt unless the user chooses full-session analysis.
- Rate limits, timeouts, cancellation and retries are handled without affecting recording or stored data.
- **AI runs on the desktop only.** The server holds ciphertext and cannot run AI.

---

## 12. Accounts, sync and E2EE

### Optional by design
- Sync is off until the user enables it in Settings → Sync.
- With sync off there are zero Buddy network calls. This is enforced by an automated test (network blocked, full recording flow passes).
- No sign-in prompts, banners, or badges anywhere else in the app.

### Accounts (identity only)
- Email + password (Argon2id on the server), GitHub login, Google login.
- Email verification and password reset need a mail provider (open decision, section 20).
- Authentication proves who you are to the server. It never grants access to readable data.

### Vault keys (WhatsApp-style)
Modelled on WhatsApp's encrypted backups (user password or recovery key) and linked devices (QR).

1. **Account Master Key (AMK):** 256-bit random, generated on the first device when sync is enabled. Never leaves a device in plaintext.
2. **Vault passphrase:** user-chosen. `Argon2id(passphrase, salt)` produces a key-encrypting key that wraps the AMK. The wrapped blob is stored on the server.
3. **Recovery key:** random 256-bit, shown once at setup (grouped base32), also wraps the AMK. The wrapped blob is stored on the server.
4. **Device pairing:** a new device shows a QR code (ephemeral X25519 public key + short code). An existing device scans it, derives a shared secret, and sends the wrapped AMK through the server relay. Pairing is single-use and expires in minutes. Both screens show a short verification string to defeat a malicious relay.
5. **Per-session data keys (DEK):** random per session, wrapped by the AMK. This keeps future sharing and rotation possible.
6. **Records:** encrypted with XChaCha20-Poly1305, with associated data = user ID, record ID, version. One wire format, specified in `docs/crypto-spec.md`, with shared test vectors so the Rust and TypeScript implementations are proven compatible. Use audited primitives (libsodium or well-reviewed Rust crates). No custom crypto.

### What the server sees
User ID, record IDs and types, versions, ciphertext sizes, coarse timestamps, device list. It does not see titles, text, tags, or notes.

### Search and AI under E2EE
- Desktop: SQLite FTS5 over local plaintext.
- Web: decrypts in the browser, builds a client-side index in memory (optional encrypted persistence), searches locally **(verify Devanagari tokenization)**.
- No server-side search, no server-side AI. This is the cost of E2EE and is accepted.

### Honest limits (to be stated in the docs)
- If the user loses both the passphrase and the recovery key, synced data is unrecoverable. Password reset on the account does not recover the vault.
- The web app's code is delivered by the server, so a compromised server could ship malicious JavaScript. The desktop app is the stronger trust anchor.
- Metadata (sizes, timing, record counts) is visible to the server.
- Key rotation and multi-user sharing are not in the first release.

### Sync protocol
- Records: session, transcript segment set, OCR set, notes, AI artifact, bookmark, tag. Stable UUIDs, versioned schemas.
- Transcript and OCR are append-only; title, tags, notes, and bookmarks are mutable.
- Server assigns a per-user monotonic `seq`. Clients pull `since=seq` and push with `baseVersion`.
- Version mismatch returns 409 with the current ciphertext; the client keeps both sides as a **conflict copy surfaced to the user**. No silent overwrite, and no reliance on wall-clock time.
- Explicit tombstones for deletes; idempotency keys on writes.
- Small request bodies (target under ~256 KB), resumable, exponential backoff with jitter, tolerant of slow or cold server responses.
- Per-session toggle and per-data-type selection. Default: notes, transcript, OCR text, AI artifacts. Audio, screenshots and video are not synced in the first release; the manifest format reserves space for them.

### Server (Node + TypeScript)
- Fastify, REST, MongoDB. Stateless; horizontal scaling not required initially.
- Collections: `users`, `identities`, `vault_keys`, `devices`, `records`, `pairing_sessions` (TTL), `refresh_tokens`.
- Short-lived access tokens with rotating refresh tokens, CSRF protection, rate limiting, input validation (schemas shared with clients), account export and account deletion.
- Docker image and self-hosting guide.

### Web app
React SPA, responsive, read-only first: session list, transcript, notes, slides (when synced), client-side search, Markdown export.

---

## 13. Data model

### Local (SQLite)
| Table | Key fields |
|---|---|
| `sessions` | id (UUID), title, preset, started/ended, duration, state, tags, sync flags |
| `audio_tracks` | session, kind (system/mic), path, format, duration, integrity |
| `video_tracks` | session, path, fps, start offset, integrity |
| `transcript_segments` | session, track, start/end, text, speaker, language, confidence, revision |
| `screenshots` | session, time, path, display, hash, change score, kind (change/safety) |
| `ocr_results` | screenshot, text, blocks, language, confidence |
| `bookmarks` | session, time, label, note |
| `ai_artifacts` | session, type, content, provider, model, prompt version, evidence links |
| `sync_records` | record id, local/remote version, state, last error |
| `segments_fts`, `ocr_fts` | FTS5 virtual tables |

### Session folder
```text
sessions/<uuid>/
├── session.json            # manifest: format version, clock origin, tracks, state
├── audio/  system-0001.wav … mic-0001.wav …
├── video/  screen-0001.mp4 …
├── slides/ 000123.webp …
└── exports/
```

### Recovery rules
`session.json` is written atomically and carries a state (`recording` / `finalizing` / `complete`). On launch, any `recording` or `finalizing` session is offered for repair: chunks are validated, tracks are stitched, and processing resumes.

### Server (MongoDB)
Documents hold ciphertext plus the minimal plaintext fields listed in section 12. All fields that identify content are inside the encrypted payload.

---

## 14. Privacy, security, reliability

- Visible recording state in the window and tray, plus the OS indicator where one exists; recording starts only on user action.
- Local-first: capture, transcription, OCR, search and export work offline.
- AI and sync are separate code paths from capture, and can be compiled out for audits.
- No telemetry. Crash reports, if ever added, are opt-in and contain no content.
- Secrets in the OS keychain; session exports contain no keys.
- Disk-space warnings, configurable storage limits, backup and restore tested.
- Privacy tests: sync-off means no network; notes-only sync uploads no media; exported files contain no secrets.
- Dependency and license audit (`NOTICE` file), signed releases, reproducible build instructions.
- A third-party review of the crypto design before the sync server accepts real users.

---

## 15. Performance budgets (targets, to be measured)

| Item | Target |
|---|---|
| Capture CPU while recording audio + slides | under 5% of one core on a mid-range laptop |
| Idle app memory | under 250 MB (webview included) |
| Audio capture loss in 60-minute test | 0 dropped chunks |
| Live preview delay (Small, GPU) | under ~5 s; CPU-only may be higher **(unknown, S4)** |
| Slide detection | under 15 ms per sampled frame |
| Search latency over 100 sessions | under 200 ms |
| Reference hardware | Linux/Windows with 16 GB RAM and a 6 GB GPU; CPU-only must still work |

---

## 16. Testing

- **Unit:** clock, session format, recovery, change detection (golden frame sets), crypto vectors.
- **Contract tests:** every platform backend runs against the same `AudioSource` / `ScreenSource` contract suite; a fake source drives the engine in CI.
- **Fixtures:** recorded English and Hindi samples with reference transcripts for WER tracking; slide-deck recordings for detection tuning.
- **Soak:** 60-minute (and later 3-hour) recordings per OS before each release.
- **Fault injection:** kill the process mid-recording, unplug the device, fill the disk, drop the network during sync.
- **Security:** sync-off network test, E2EE round-trip across Rust and TypeScript, server authorization tests.
- **CI:** GitHub Actions matrix (Linux, Windows, macOS) for build, lint, unit and contract tests. Real capture on each OS requires physical machines (open question S9).
- **Release trigger (decided 2026-11-22):** tests run on pull requests targeting `main` and on pushes to `main`. A push to `main` publishes a release only when `apps/desktop/package.json` is newer than the latest `v*` tag; the release is tagged `v<version>`. An unchanged version tests without publishing. Development continues on `application`.
- A test is never reported as passing unless it was run.

---

## 17. Working agreements

- Monorepo, trunk-based, small focused PRs, conventional commits.
- Each milestone has exit criteria with measurable checks.
- Platform abstraction and fakes come first; real backends are added per OS inside each milestone.
- Docs are code: `docs/crypto-spec.md` and `docs/data-model.md` are written before the matching implementation.

---

## 18. Milestones

Estimates assume about 20-25 focused hours per week and are **guesses**. The original plan's 3-6 week desktop estimate did not include three operating systems, live transcription, Hindi validation, video, or E2EE. Treat the ranges below as the planning baseline and re-estimate after M1.

| # | Milestone | Weeks |
|---|---|---|
| M0 | Foundation | 1-2 |
| M1 | Audio capture | 3-4 |
| M2 | Transcription (final + live, EN + HI) | 3-4 |
| M3 | Visual capture (slides, then video) | 4-5 |
| M4 | OCR and timeline linking | 2 |
| M5 | Desktop UI and session viewer | 4-5 |
| M6 | Search, export, backup | 1-2 |
| | **MVP total** | **18-24** |
| M7 | AI providers and analysis | 3-5 |
| M8 | Sync server, accounts, E2EE | 5-7 |
| M9 | Web app | 3-4 |
| M10 | Hardening, packaging, signing | 3-5 |
| | **Everything** | **32-45** |

### M0: Foundation (1-2 weeks)
Monorepo, Tauri shell, crate skeletons, CI matrix, logging, settings, session format v1 and recovery rules, schema package, license/NOTICE, minimal CLI harness for capture tests.
**Exit:** clean build on three OSes in CI; session create/read/recover tests pass.

### M1: Audio capture (3-4 weeks)
`AudioSource` trait and engine; Linux (PipeWire), Windows (WASAPI loopback), macOS backends; two tracks; start/pause/resume/stop; chunked writes; device loss and reconnect; HUD, tray, hotkeys, recording indicator.
**Exit:** both tracks record simultaneously on each OS; pause/resume correct; 60-minute soak with zero lost chunks; crash recovery verified.

### M2: Transcription (3-4 weeks)
whisper.cpp integration, model manager with checksums, GPU backends, chunked final pass with retry, VAD, live preview with backpressure, English and Hindi fixtures with measured WER.
**Exit:** final transcript for a test recording with timestamp navigation; live preview never affects capture; measured WER and real-time factor recorded per model and hardware.

### M3: Visual capture (4-5 weeks)
`ScreenSource` trait; Wayland portal, X11, Windows, macOS backends; sampling, change detection, settle logic, safety snapshots, multi-monitor; then **video mode** (M3b, 1-2 weeks, the first item to cut if the schedule slips).
**Exit:** reliable slide capture on a test presentation per OS; safety snapshots work; display changes handled; video recording playable after a forced kill.

### M4: OCR and linking (2 weeks)
Tesseract (eng + hin), OCR on slide events, dedupe, links between OCR, screenshots, and transcript time.
**Exit:** click a transcript line, see the matching slide and OCR text.

### M5: Desktop UI (4-5 weeks)
Library, recording HUD, viewer (transcript, slide pane, playback, timeline), settings, onboarding (permissions, consent notice, model choice).
**Exit:** create, stop, reopen, browse, and play back a session entirely in the UI on each OS.

### M6: Search, export, backup (1-2 weeks)
FTS5 over transcript and OCR (Devanagari check), filters, timestamp jump, Markdown/JSON export, backup/restore.
**Exit:** search finds and jumps to evidence in English and Hindi; export is usable; restore reproduces the library. **MVP release.**

### M7: AI providers (3-5 weeks)
Provider trait and settings; OpenAI-compatible adapter (LM Studio, Ollama, API keys); consent screen; structured output validation; chunk-and-consolidate; tasks for both presets; Q&A with citations; OpenCode and Claude Code adapters as experiments after flag verification.
**Exit:** useful structured summary of a long session; provider and model shown; transcript untouched; failures recoverable.

### M8: Sync server, accounts, E2EE (5-7 weeks)
Crypto spec and vectors; Rust and TypeScript implementations; server (auth, vault blobs, pairing relay, records); sync engine with conflict copies; Settings → Sync UI; self-hosting image and guide.
**Exit:** notes sync from desktop and decrypt on a second device after QR pairing; sync-off network test passes; media never uploaded; interrupted sync recovers; recovery-key flow verified; crypto review completed.

### M9: Web app (3-4 weeks)
Responsive React viewer, client-side decryption and search, Markdown export, session view.
**Exit:** read and search synced sessions from a phone and a desktop browser.

### M10: Hardening and packaging (3-5 weeks)
Signed installers (Windows, macOS notarized, Linux packages), update strategy, privacy audit, long soak, docs.
**Exit:** reproducible install on three OSes; recovery, soak, and privacy tests pass.

---

## 19. Risks and validation spikes

Do these early, before committing to the dependent design.

| # | Spike | Why it matters |
|---|---|---|
| S1 | macOS system-audio capture: ScreenCaptureKit audio vs CoreAudio taps, minimum OS, permissions | Determines the macOS support floor; may need a documented virtual-device fallback |
| S2 | Windows WASAPI loopback through the chosen crate; behavior with exclusive-mode apps | Core capability |
| S3 | Wayland portal capture and restore tokens on GNOME, KDE, Hyprland | Re-prompting on every recording would hurt usability |
| S4 | Whisper EN + Hindi: WER by model, real-time factor on CPU/GPU, Hinglish behavior | Decides default model and whether live Hindi preview is feasible |
| S5 | Video encoding: hardware encoders per OS and license-safe options | Decides whether video can ship in MVP |
| S6 | FTS5 tokenization of Devanagari | Search quality for Hindi |
| S7 | Tesseract `hin` quality on real slides | May require a different OCR engine later |
| S8 | Libsodium/crypto in the Tauri webview and web build; Rust/TS vector parity | E2EE correctness |
| S9 | Test devices: access to Windows and macOS machines for real capture tests | CI cannot exercise capture |
| S10 | Agent-CLI behavior (OpenCode, Claude Code): non-interactive flags, output format, permissions | Decides whether adapters are viable |

### Other risks
- **Scope:** three OSes plus live ASR plus video plus E2EE is a large surface. Mitigation: platform traits and fakes first, cut video (M3b) before cutting reliability.
- **Legal:** recording consent laws vary by place. Mitigation: onboarding notice, visible indicator, no legal claims.
- **Account support load:** password resets and email delivery. Mitigation: keep account features minimal; document self-hosting.
- **E2EE usability:** lost keys mean lost data. Mitigation: recovery-key confirmation step, clear warnings.

---

## 20. Assumptions made (review these)

These were not explicitly decided in the interview; change any that are wrong.

1. Minimum OS targets (decided 2026-11-22): Windows 10 or later with backend limitations accepted, macOS 14.2 or later, Linux with PipeWire only. Previously assumed macOS 13+; superseded because system-audio capture is expected to need 14.2+. **(verify capture APIs against S1-S3)**
2. UI language is English at first, with i18n scaffolding in place from M5.
3. Global hotkeys: start/stop and add-bookmark, user-configurable.
4. Audio is written as lossless chunks during recording and compressed on finalize.
5. Mail provider for verification and password reset is undecided; the server will keep it behind a small interface.
6. No telemetry or analytics of any kind.
7. Auto-update is decided during M10.
8. Account deletion and data export are included in the first server release.
9. Public repository on GitHub under Apache-2.0; the repository URL is a placeholder in the README.
10. The server image is built for self-hosting from day one.
