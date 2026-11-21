# M0 Foundation — completion record

Date: 2026-11-22. Branch: `application`. Merge base: `f994caf3509619b0de06622086a6f2e3a145d120`.
Final commit: `5ba00d8 docs: record main-merge version-gated release trigger`.

## Status

M0 is complete. All six tasks in `docs/plans/2026-11-22-m0-foundation.md` are implemented, reviewed and green. The branch has **not** been pushed or merged; the user is handling integration and the pull request.

Verified locally on the final tree:

- `cargo fmt --all -- --check` — clean.
- `cargo test --workspace` — 13 tests passing (CLI 1, core 10, desktop 2).
- `pnpm --filter @buddy/desktop build` — produces `apps/desktop/dist/index.html`.

A whole-branch review over `f994caf..5ba00d8` returned "ready to merge" with no Critical or Important findings. Two task reviews found defects that were fixed and re-reviewed before completion: a Windows post-rename directory-fsync false failure in session creation, and a Tauri configuration that declared no window.

## What shipped

| Area | Result |
|---|---|
| Workspace | Cargo workspace (`buddy-core`, `buddy-cli`, `buddy-desktop`) and pnpm workspace (`@buddy/desktop`); Apache-2.0 with root `NOTICE`. |
| Session format v1 | Typed errors, UUID ids with traversal rejection, atomic temp → fsync → rename write, corrupt/truncated manifest handling, read/recover with `needs_repair`. |
| Settings and support | `load_settings`/`save_settings` with typed errors and atomic writes; `SUPPORT_FLOORS` pinned to Windows 10, macOS 14.2 and PipeWire-only, with a regression test. |
| CLI harness | `buddy-cli session new|show|recover` emitting the manifest or `{"needs_repair":…,"manifest":…}`, with a real-binary integration test. |
| Desktop shell | Tauri 2 window titled `Buddy` (960×640), a `support_matrix()` command, generated icon set, `core:default`-only capability, and a compiled-config window test. The visible UI is deliberately the M0 placeholder; capture arrives in M1. |
| CI and releases | `.github/workflows/ci.yml` tests PRs into `main` and pushes to `main` on Linux, Windows and macOS; a push publishing only when `apps/desktop/package.json` is newer than the latest `v*` tag. Unsigned NSIS `.exe`, `.dmg`, AppImage, `.deb`, `.rpm` and `.tar.gz`; release notes carry all required OS and unsigned-build warnings. No signing steps or signing secrets. |

## Decisions carried forward

- Windows directory fsync after `rename` is `#[cfg(unix)]`-gated and best-effort; a committed write must never report failure.
- Dependencies are pinned in both lockfiles and attributed in `NOTICE`, including MPL-2.0 `option-ext` via `dirs`.
- `save_settings` assumes its parent directory exists; the first milestone that calls it must create the data directory.
- `apps/desktop/package.json` is the version source of truth. `v0.1.0` does not exist yet, so the first qualifying merge to `main` publishes it.
- Real-capture soaks on Arch Linux and Windows arrive with M1 capture code and must become dependencies of the `publish` job. Until then, the first CI release run is the real validation of bundle globs; failed uploads fail loudly via `if-no-files-found: error`.
- macOS real capture is UNVERIFIED and must stay documented that way in release notes.

## Deferred, non-blocking

- Test-file import suppression and untested `Finalizing`/`Complete` recovery mappings.
- A reused session id currently overwrites `session.json`; the embedded id is not compared with the requested id; non-canonical UUID spellings may produce an opaque Windows error. Decide create-vs-resume and canonicalisation semantics before M1 writes real chunks.
- The settings temp filename is fixed per path; a concurrent second writer would race. Safe with today's single writer; revisit when an M1 writer thread exists.
- TypeScript strict flags are present but not enforced; add `tsc --noEmit` when real UI code lands.
- The `sort -V` version comparison treats prerelease tags incorrectly; replace it with semver-aware comparison before publishing any `-rc` version.
- Root `clean` script uses `require()` under a `type: module` package; `node -e` is CommonJS today but a future Node default could change that.

The MPL-2.0 attribution needs its licence text and source-availability offer included in release artifacts; the root `NOTICE` records the obligation, but no bundler step copies it into artifacts yet.

## Next milestone

M1 adds real screen/audio capture. It needs its own plan before implementation. Start from `plan.md` §5/§7 and the approved design in `docs/specs/2026-11-22-mvp-branch-design.md`; keep the OS floors, unsigned-release rule, license verification, and offline/local-first constraints intact. Do not add network calls, accounts, telemetry, AI or sync code.
