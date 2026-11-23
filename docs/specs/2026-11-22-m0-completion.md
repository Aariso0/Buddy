# M0 Foundation — completion record

Date: 2026-11-22. Branch: `application`. Merge base: `f994caf3509619b0de06622086a6f2e3a145d120`.
M0 final commit: `5ba00d8`. Later review-fix commits are listed in the Post-M0 review fixes section and in `git log`.

## Status

M0 is complete. All six tasks in `docs/plans/2026-11-22-m0-foundation.md` are implemented, reviewed and green. The branch has **not** been merged into `main`; the user is handling integration and the pull request.

Verified locally on the final tree:

- `cargo fmt --all -- --check` — clean.
- `cargo test --workspace` — 13 tests passing (CLI 1, core 10, desktop 2).
- `pnpm --filter @buddy/desktop build` — produces `apps/desktop/dist/index.html`.

A whole-branch review over `f994caf..5ba00d8` returned "ready to merge" with no Critical or Important findings. Two task reviews found defects that were fixed and re-reviewed before completion: a Windows post-rename directory-fsync false failure in session creation, and a Tauri configuration that declared no window.

## Post-M0 review fixes

A later review pass found ten still-valid findings, all fixed on `application` before integration:

- Linux `.tar.gz` stages `NOTICE`, `LICENSE`, the MPL-2.0 text and exact-version source locations for both Rust crates and npm packages before upload; `gh release create` pins the tag with `--target "$GITHUB_SHA"`. A follow-up review corrected the license-file wording and added the npm source locations.
- Tauri declares `beforeDevCommand` so `tauri dev` starts Vite on port 5173 with `strictPort`.
- Session creation reserves its directory exclusively; reads and recovery reject a manifest id that differs from its directory id; manifest and settings writes use unique, exclusively created staging files with failure cleanup.
- The foundation plan's icon command and `publish` dependency list, and this document's table escaping, were corrected.
- macOS: `generate_context!()` is called only once, inside `app_context()`, so the macOS-only Info.plist embedding does not define `_EMBED_INFO_PLIST` twice in the desktop test build.
- A final review replaced `sort -V` with semver precedence for tag selection and version comparison, added best-effort directory fsync for a new session's ancestors and parent before the manifest rename, and made settings fsync reuse the normalized parent so bare filenames sync `.`.

Verification after the fixes: `cargo fmt --all -- --check`, `cargo test --workspace` (18 tests passing), `pnpm --filter @buddy/desktop build`, workflow YAML/config JSON parse, a dry run of the legal staging script, and a live Vite start on port 5173 — all green.

## What shipped

| Area | Result |
|---|---|
| Workspace | Cargo workspace (`buddy-core`, `buddy-cli`, `buddy-desktop`) and pnpm workspace (`@buddy/desktop`); Apache-2.0 with root `NOTICE`. |
| Session format v1 | Typed errors, UUID ids with traversal rejection, atomic temp → fsync → rename write, corrupt/truncated manifest handling, read/recover with `needs_repair`. |
| Settings and support | `load_settings`/`save_settings` with typed errors and atomic writes; `SUPPORT_FLOORS` pinned to Windows 10, macOS 14.2 and PipeWire-only, with a regression test. |
| CLI harness | `buddy-cli session new\|show\|recover` emitting the manifest or `{"needs_repair":…,"manifest":…}`, with a real-binary integration test. |
| Desktop shell | Tauri 2 window titled `Buddy` (960×640), a `support_matrix()` command, generated icon set, `core:default`-only capability, and a compiled-config window test. The visible UI was deliberately the M0 placeholder; M1 added the capture control surface and visible state. |
| CI and releases | `.github/workflows/ci.yml` orchestrates PRs into `main` and pushes to `main` through reusable `ci-test.yml`, `ci-package.yml`, `ci-soak-linux.yml` and `ci-soak-windows.yml` jobs on Linux, Windows and macOS; shared setup lives in `.github/actions/setup-workspace` and the release/legal logic in `.github/scripts/`. A push publishes only when `apps/desktop/package.json` is newer than the latest `v*` tag. Unsigned NSIS `.exe`, `.dmg`, AppImage, `.deb`, `.rpm` and `.tar.gz`; release notes carry all required OS and unsigned-build warnings. No signing steps or signing secrets. |

## Decisions carried forward

- Windows directory fsync after `rename` is `#[cfg(unix)]`-gated and best-effort; a committed write must never report failure.
- Dependencies are pinned in both lockfiles and attributed in `NOTICE`, including MPL-2.0 `option-ext` via `dirs`.
- `save_settings` assumes its parent directory exists; the first milestone that calls it must create the data directory. **Resolved in M1:** the desktop shell calls `ensure_data_dir` before the first settings write or session creation.
- `apps/desktop/package.json` is the version source of truth. No `v*` tag exists yet; the version is now `0.1.1` (bumped with M1), so the first qualifying merge to `main` publishes `v0.1.1`.
- Real-capture soaks on Arch Linux and Windows arrive with M1 capture code and must become dependencies of the `publish` job. Until then, the first CI release run is the real validation of bundle globs; failed uploads fail loudly via `if-no-files-found: error`. **Delivered in M1:** `soak-linux` and `soak-windows` are now `publish.needs` dependencies; see `tests/soak/README.md`. They have not been run locally, so no pass is claimed.
- macOS real capture is UNVERIFIED and must stay documented that way in release notes. **Still true after M1:** the release notes keep the statement "Real screen and audio capture has not been verified on macOS" verbatim.

## Session identity and creation semantics (binding for M1)

- **Create versus resume:** `create_session` reserves the session directory exclusively. An id that already exists returns `SessionError::AlreadyExists` and leaves the existing manifest and all other state untouched. Resuming an interrupted session is a separate path through `read_session`/`recover_session`; creation never silently adopts or overwrites an existing session.
- **Session identity rule:** the `id` stored in `session.json` must equal the id of the directory holding it. `read_session` and `recover_session` both return `SessionError::IdMismatch` when they differ; a mismatch is never silently repaired. Any future auto-repair or rename behaviour must be specified and approved before M1 writes real chunks.
- Non-canonical UUID spellings may still produce an opaque Windows error; decide canonicalisation semantics before M1 writes real chunks. **Resolved in M1:** non-canonical ids are rejected with the typed `SessionError::InvalidSessionId`; no silent repair.
- A crash between directory reservation and manifest rename leaves an empty session directory: creation then returns `AlreadyExists` while read/recover return `Io(NotFound)`. Delete that directory manually; no committed manifest is ever lost. **Confirmed in M1:** M1 deliberately does not adopt or auto-repair it (`empty_reserved_dir_is_not_adopted`).

## Deferred, non-blocking

- Test-file import suppression and untested `Finalizing`/`Complete` recovery mappings.
- TypeScript strict flags are present but not enforced; add `tsc --noEmit` when real UI code lands.
- Root `clean` script uses `require()` under a `type: module` package; `node -e` is CommonJS today but a future Node default could change that.

Resolved during review fixes: session directories are now reserved exclusively; manifest ids are checked against their directory id; both session and settings writes use unique, exclusively created staging files. The Linux `.tar.gz` now stages `NOTICE`, `LICENSE` and an exact-version MPL source-locations file before upload.

## Next milestone

M1 added real **audio** capture (screen/video capture is M3, out of scope for M1). Its approved plan and completion record are in [`docs/plans/2026-11-22-m1-audio-capture.md`](../plans/2026-11-22-m1-audio-capture.md). The next milestones are transcription (M2) and visual capture (M3); start from `plan.md` §5/§7 and the approved design in `docs/specs/2026-11-22-mvp-branch-design.md`; keep the OS floors, unsigned-release rule, license verification, and offline/local-first constraints intact. Do not add network calls, accounts, telemetry, AI or sync code before their milestones.
