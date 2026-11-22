# M0 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Buddy monorepo, session format v1 with recovery, a CLI harness, a Tauri desktop shell, and a three-OS CI matrix that emits unsigned release artifacts.

**Architecture:** A pnpm workspace for TypeScript plus a Cargo workspace for Rust live in one repo. `buddy-core` owns the session model, clock, settings and support matrix; thin binaries (`buddy-cli`, the Tauri shell) consume it. GitHub Actions builds and tests on Linux, Windows and macOS on PRs into `main` and on pushes to `main`, then packages and publishes unsigned artifacts only when the version in `apps/desktop/package.json` is newer than the latest `v*` tag (decided 2026-11-22, superseding tag-triggered releases on `application`).

**Tech Stack:** Rust (stable), Tauri 2, React + TypeScript + Vite, pnpm 10, SQLite/FTS5 (crate chosen in a later milestone), GitHub Actions.

**Spec:** `docs/specs/2026-11-22-mvp-branch-design.md` (approved 2026-11-22) and `plan.md` §7, §13, §16, §18.

**Scope:** M0 only. M1–M6 each get their own plan. No capture backends, ASR, OCR, UI screens, AI or sync code in this plan.

## Global Constraints

- Branch: `application`. Remote: `https://github.com/Aariso0/Buddy.git`.
- License: Apache-2.0, public repository. `NOTICE` at repo root is required; verify each dependency's license from its upstream before adopting it.
- OS floors (decided): macOS **14.2+**, Linux **PipeWire only**, Windows **10+ with backend limitations accepted**. No virtual loopback device requirement.
- Release artifacts (decided): Windows **NSIS `.exe`**, macOS **`.dmg`**, Linux **AppImage + `.deb` + `.rpm` + `.tar.gz`**. All **unsigned**; no signing secrets or signing steps anywhere in CI.
- Real-capture verification happens on **Arch Linux and Windows**; macOS capture is documented as UNVERIFIED. A failed real-capture soak on Arch or Windows blocks release — the soak jobs themselves arrive with M1 capture code; M0 gates releases on CI passing.
- Transcription will use GPU when available and fall back to CPU. GPU backend selection is implemented in M2; M0 only declares the support matrix.
- `session.json` is written atomically and carries state `recording` / `finalizing` / `complete`.
- Install-size target: under 60 MB without models; capture CPU under 5% of one core; idle memory under 250 MB; 60-minute soak with zero lost chunks. These are targets to measure, not claims.
- No telemetry, no account, no network calls for local use.
- No AI or sync crates are created in this branch.

## Review Focus

These are the inputs and conditions the spec implies but that a builder would likely overlook. Each has a test pinned to the task that owns the code.

1. Interrupted write leaves a truncated or half-written `session.json` — must be a typed error, never a panic or a partially opened session. Test: `corrupt_manifest_is_typed_error` in Task 2.
2. A crafted session id or path tries to escape the sessions root (`../`, absolute path) — must be rejected before any filesystem access. Test: `traversal_session_id_is_rejected` in Task 2.
3. A `session.json` from a future format version — must fail with a typed error naming the version, not be silently misread. Test: `unknown_version_is_typed_error` in Task 2.
4. OS floors drift from the approved values (10 / 14.2 / PipeWire-only) in code or docs. Test: `support_floors_match_spec` in Task 3.
5. Release notes omit the unsigned-build warnings, leaving users to discover Gatekeeper, SmartScreen or Smart App Control blocks alone. Test: required-phrase grep in Task 6.

---

### Task 1: Workspace scaffolding and license files

**Files:**
- Create: `Cargo.toml`, `package.json`, `pnpm-workspace.yaml`, `.gitignore`, `NOTICE`
- Create: `crates/buddy-core/Cargo.toml`, `crates/buddy-core/src/lib.rs` (empty)
- Create: `apps/cli/Cargo.toml`, `apps/cli/src/main.rs` (`fn main() {}`)
- Create: `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/build.rs`, `apps/desktop/src-tauri/src/main.rs`
- Create: `packages/schemas/package.json`, `packages/ui/package.json`, `packages/ui/src/index.ts` (empty)

**Interfaces:**
- Produces: three Cargo workspace members named `buddy-core`, `buddy-cli`, `buddy-desktop`; pnpm workspace packages under `apps/*` and `packages/*`.

- [ ] **Step 1: Create root manifests**

`Cargo.toml`: virtual workspace, `resolver = "2"`, `members = ["crates/buddy-core", "apps/cli", "apps/desktop/src-tauri"]`.
`pnpm-workspace.yaml`: `packages: ["apps/*", "packages/*"]`.
Root `package.json`: private, name `buddy`, scripts `build`, `test`, `clean`.
`.gitignore`: `target/`, `node_modules/`, `dist/`, `*.log`, `.DS_Store`.
`NOTICE`: project name, "Licensed under the Apache License, Version 2.0", and a line saying third-party attributions are appended as dependencies are adopted.

- [ ] **Step 2: Create minimal member manifests**

Each Cargo member gets `[package]` with `name` as above, `version = "0.1.0"`, `edition = "2021"`, `license = "Apache-2.0"` and no dependencies. `apps/desktop/src-tauri/build.rs` is `fn main() {}` for now; `main.rs` is `fn main() {}`. Each pnpm member gets a private `package.json` with the scoped name (`@buddy/desktop`, `@buddy/schemas`, `@buddy/ui`), `version: 0.1.0`, `type: module`. The desktop package declares a `build` script running `vite build`.

- [ ] **Step 3: Verify the workspace resolves**

Run: `cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(sorted(p["name"] for p in json.load(sys.stdin)["packages"]))'`
Expected: `['buddy-cli', 'buddy-core', 'buddy-desktop']`

Run: `npm install -g pnpm@10 && pnpm install`
Expected: `pnpm-lock.yaml` exists at repo root; exit 0.

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml package.json pnpm-workspace.yaml .gitignore NOTICE crates apps packages pnpm-lock.yaml
git commit -m "chore: scaffold monorepo and license files"
```

---

### Task 2: Session format v1, atomic write and recovery

**Files:**
- Modify: `crates/buddy-core/Cargo.toml` (add `uuid` v4 with `serde`,v4 features; `chrono` with `serde`; `serde` derive; `serde_json`; `thiserror`)
- Modify: `crates/buddy-core/src/lib.rs` (re-export `session`)
- Create: `crates/buddy-core/src/session.rs`
- Create: `crates/buddy-core/tests/session_format.rs`
- Create: `packages/schemas/src/session.ts`, `packages/schemas/src/index.ts`
- Create: `packages/schemas/fixtures/session-v1.json`

**Interfaces:**
- Consumes: nothing from earlier tasks beyond the workspace.
- Produces (all in `buddy_core::session`):
  - `pub const FORMAT_VERSION: u32 = 1;`
  - `pub enum SessionState { Recording, Finalizing, Complete }` — serde rename_all lowercase.
  - `pub enum Preset { Lecture, Meeting }` — serde rename_all lowercase.
  - `pub struct SessionManifest { pub id: String, pub format_version: u32, pub state: SessionState, pub started_at: DateTime<Utc>, pub ended_at: Option<DateTime<Utc>>, pub preset: Preset }` — derives `Debug, Clone, PartialEq, Serialize, Deserialize`.
  - `pub struct RecoveryOutcome { pub manifest: SessionManifest, pub needs_repair: bool }`
  - `pub enum SessionError { InvalidSessionId(String), UnsupportedVersion(u32), InvalidManifest(String), Io(std::io::Error) }` — typed via `thiserror`.
  - `pub fn create_session(root: &Path, id: &str, now: DateTime<Utc>) -> Result<SessionManifest, SessionError>`
  - `pub fn read_session(root: &Path, id: &str) -> Result<SessionManifest, SessionError>`
  - `pub fn recover_session(root: &Path, id: &str) -> Result<RecoveryOutcome, SessionError>`
  - Behavior: `recover_session` returns `needs_repair: true` for `Recording` or `Finalizing`, `false` for `Complete`. `create_session` writes `session.json` via a temporary file, fsync, atomic rename and parent-directory fsync.

- [ ] **Step 1: Write the failing tests**

In `crates/buddy-core/tests/session_format.rs`:

```rust
use buddy_core::session::*;
use chrono::{TimeZone, Utc};
use std::path::Path;
use uuid::Uuid;

fn id() -> &'static str { "11111111-1111-4111-8111-111111111111" }
fn now() -> chrono::DateTime<Utc> { Utc.with_ymd_and_hms(2026, 11, 22, 10, 0, 0).unwrap() }

#[test]
fn create_read_recover_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let manifest = create_session(root.path(), id(), now()).unwrap();
    assert_eq!(manifest.format_version, 1);
    assert_eq!(manifest.id, id());
    assert!(matches!(manifest.state, SessionState::Recording));
    assert_eq!(read_session(root.path(), id()).unwrap(), manifest);
    let outcome = recover_session(root.path(), id()).unwrap();
    assert!(outcome.needs_repair);
    assert_eq!(outcome.manifest.state, SessionState::Recording);
}

#[test]
fn corrupt_manifest_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(id());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("session.json"), b"{\"format_version\":1,\"state\":").unwrap();
    assert!(matches!(read_session(root.path(), id()), Err(SessionError::InvalidManifest(_))));
}

#[test]
fn traversal_session_id_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    assert!(matches!(read_session(root.path(), "../etc/passwd"), Err(SessionError::InvalidSessionId(_))));
    assert!(matches!(read_session(root.path(), "/etc/passwd"), Err(SessionError::InvalidSessionId(_))));
}

#[test]
fn unknown_version_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(id());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("session.json"), b"{\"id\":\"11111111-1111-4111-8111-111111111111\",\"format_version\":2,\"state\":\"recording\",\"started_at\":\"2026-11-22T10:00:00Z\",\"ended_at\":null,\"preset\":\"lecture\"}").unwrap();
    assert!(matches!(read_session(root.path(), id()), Err(SessionError::UnsupportedVersion(2))));
}

#[test]
fn stale_tmp_file_does_not_affect_read() {
    let root = tempfile::tempdir().unwrap();
    create_session(root.path(), id(), now()).unwrap();
    std::fs::write(root.path().join(id()).join("session.json.tmp"), b"garbage").unwrap();
    assert_eq!(read_session(root.path(), id()).unwrap().state, SessionState::Recording);
}

#[test]
fn manifest_matches_shared_schema_fixture() {
    let root = tempfile::tempdir().unwrap();
    let manifest = create_session(root.path(), id(), now()).unwrap();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../packages/schemas/fixtures/session-v1.json")).unwrap();
    assert_eq!(serde_json::to_value(&manifest).unwrap(), fixture);
}
```

Also add `tempfile` as a dev-dependency to `crates/buddy-core/Cargo.toml`.

Fixture `packages/schemas/fixtures/session-v1.json`:

```json
{
  "id": "11111111-1111-4111-8111-111111111111",
  "format_version": 1,
  "state": "recording",
  "started_at": "2026-11-22T10:00:00Z",
  "ended_at": null,
  "preset": "lecture"
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p buddy-core --test session_format`
Expected: compile error or failures because `buddy_core::session` does not exist.

- [ ] **Step 3: Implement the session module**

Write `crates/buddy-core/src/session.rs` with the exact types and signatures in Interfaces. Validate `id` by parsing it as a UUID and rejecting any string containing a path separator; join it under `root` and reject absolute or traversing paths before touching the filesystem. Serialize with `serde_json::to_vec_pretty`, write to `session.json.tmp`, `sync_all`, `rename` to `session.json`, then open the parent directory and `sync_all`. On read, check `format_version` before deserializing the rest so an unsupported version yields `UnsupportedVersion(v)`.

- [ ] **Step 4: Add the TypeScript mirror**

`packages/schemas/src/session.ts` exports `SESSION_FORMAT_VERSION = 1`, `type SessionState = "recording" | "finalizing" | "complete"`, `type Preset = "lecture" | "meeting"`, and `interface SessionManifest { id: string; format_version: number; state: SessionState; started_at: string; ended_at: string | null; preset: Preset }`. `packages/schemas/src/index.ts` re-exports it.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p buddy-core`
Expected: all 6 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/buddy-core packages/schemas
git commit -m "feat: add session format v1 with atomic write and recovery"
```

---

### Task 3: Settings persistence and support matrix

**Files:**
- Modify: `crates/buddy-core/Cargo.toml` (add `dirs`)
- Modify: `crates/buddy-core/src/lib.rs` (re-export `settings`, `support`)
- Create: `crates/buddy-core/src/settings.rs`, `crates/buddy-core/src/support.rs`
- Create: `crates/buddy-core/tests/settings.rs`

**Interfaces:**
- Consumes: `buddy_core::session::Preset` is not needed here.
- Produces:
  - `buddy_core::support::{SupportFloors, SUPPORT_FLOORS}` where `SupportFloors` derives `Debug, Clone, PartialEq, Serialize` with fields `pub windows_min: &'static str`, `pub macos_min: &'static str`, `pub linux_requires_pipewire: bool`; `SUPPORT_FLOORS` is `SupportFloors { windows_min: "10", macos_min: "14.2", linux_requires_pipewire: true }`.
  - `buddy_core::settings::{Settings, SettingsError, load_settings, save_settings}` where `Settings { data_dir: PathBuf, log_level: String }` derives `Debug, Clone, PartialEq, Serialize, Deserialize`; `impl Default` uses `dirs::data_local_dir().join("Buddy")` and `"info"`.
  - `pub fn load_settings(path: &Path) -> Result<Settings, SettingsError>` returns `Settings::default()` when the file does not exist, and a typed error when it exists but cannot be parsed.
  - `pub fn save_settings(path: &Path, settings: &Settings) -> Result<(), SettingsError>` writes atomically (same temp-and-rename pattern as Task 2).

- [ ] **Step 1: Write the failing tests**

In `crates/buddy-core/tests/settings.rs`:

```rust
use buddy_core::settings::*;
use buddy_core::support::SUPPORT_FLOORS;

#[test]
fn support_floors_match_spec() {
    assert_eq!(SUPPORT_FLOORS.windows_min, "10");
    assert_eq!(SUPPORT_FLOORS.macos_min, "14.2");
    assert!(SUPPORT_FLOORS.linux_requires_pipewire);
}

#[test]
fn missing_settings_file_returns_defaults() {
    let root = tempfile::tempdir().unwrap();
    let settings = load_settings(&root.path().join("settings.json")).unwrap();
    assert_eq!(settings.log_level, "info");
    assert_eq!(settings.data_dir, buddy_core::settings::Settings::default().data_dir);
}

#[test]
fn settings_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    let settings = Settings { data_dir: root.path().join("data"), log_level: "debug".into() };
    save_settings(&path, &settings).unwrap();
    assert_eq!(load_settings(&path).unwrap(), settings);
}

#[test]
fn corrupt_settings_file_is_typed_error() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("settings.json");
    std::fs::write(&path, b"{not json").unwrap();
    assert!(matches!(load_settings(&path), Err(SettingsError::InvalidSettings(_))));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p buddy-core --test settings`
Expected: compile error because ` buddy_core::settings` and `buddy_core::support` do not exist.

- [ ] **Step 3: Implement settings and support**

Write both modules with the exact signatures in Interfaces. `SettingsError` is a `thiserror` enum with at least `InvalidSettings(String)` and `Io(std::io::Error)` variants. Reuse the temp-file-and-rename write pattern from Task 2 rather than adding a dependency.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p buddy-core`
Expected: all 10 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/buddy-core
git commit -m "feat: add settings persistence and support matrix"
```

---

### Task 4: CLI harness for session tests

**Files:**
- Modify: `apps/cli/Cargo.toml` (add `buddy-core` path dependency, `clap` v4 with derive, `serde_json`, `chrono`; dev-dependency `tempfile`)
- Modify: `apps/cli/src/main.rs`
- Create: `apps/cli/tests/cli.rs`

**Interfaces:**
- Consumes: `buddy_core::session::{create_session, read_session, recover_session}` from Task 2.
- Produces: binary `buddy-cli` with commands:
  - `buddy-cli session new --root <dir> --id <uuid>` — writes a session, prints the manifest as JSON.
  - `buddy-cli session show --root <dir> --id <uuid>` — prints the manifest as JSON.
  - `buddy-cli session recover --root <dir> --id <uuid>` — prints `{"needs_repair": bool, "manifest": {...}}`.
  All commands use the current UTC time when a time is required and exit non-zero with the error message on stderr for any `SessionError`.

- [ ] **Step 1: Write the failing test**

In `apps/cli/tests/cli.rs`:

```rust
use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_buddy-cli")).args(args).output().unwrap();
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn cli_create_show_recover_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    let dir = root.path().to_str().unwrap();
    let created: serde_json::Value = serde_json::from_str(&run(&["session", "new", "--root", dir, "--id", id])).unwrap();
    assert_eq!(created["format_version"], 1);
    let shown: serde_json::Value = serde_json::from_str(&run(&["session", "show", "--root", dir, "--id", id])).unwrap();
    assert_eq!(shown, created);
    let recovered: serde_json::Value = serde_json::from_str(&run(&["session", "recover", "--root", dir, "--id", id])).unwrap();
    assert_eq!(recovered["needs_repair"], true);
    assert_eq!(recovered["manifest"], created);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p buddy-cli --test cli`
Expected: compile error or failure because the CLI commands do not exist.

- [ ] **Step 3: Implement the CLI**

Use `clap` derive types for `Cli`, `Commands::Session`, and `SessionCommands::{New, Show, Recover}` with `--root` and `--id` options. Call the Task 2 functions and serialize results with `serde_json::to_string`. Map errors to a non-zero exit with the message on stderr.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p buddy-cli`
Expected: 1 test passes.

- [ ] **Step 5: Commit**

```bash
git add apps/cli
git commit -m "feat: add CLI harness for session tests"
```

---

### Task 5: Tauri desktop shell

**Files:**
- Modify: `apps/desktop/package.json` (add `react`, `react-dom`, `vite`, `@vitejs/plugin-react`, `typescript`, dev types, and `@tauri-apps/cli` v2 as a devDependency)
- Modify: `apps/desktop/src-tauri/Cargo.toml` (add `tauri` v2, `serde`, `buddy-core` path dependency; `[build-dependencies] tauri-build` v2; `[[bin]] name = "buddy"`)
- Modify: `apps/desktop/src-tauri/build.rs` → `fn main() { tauri_build::build() }`
- Modify: `apps/desktop/src-tauri/src/main.rs` → `fn main() { buddy_desktop::run() }` (rename the lib target accordingly)
- Create: `apps/desktop/src-tauri/src/lib.rs`, `apps/desktop/src-tauri/tauri.conf.json`, `apps/desktop/src-tauri/capabilities/default.json`
- Create: `apps/desktop/index.html`, `apps/desktop/vite.config.ts`, `apps/desktop/tsconfig.json`
- Create: `apps/desktop/src/main.tsx`, `apps/desktop/src/App.tsx`
- Generate: `apps/desktop/src-tauri/icons/*` from `assets/icon-1024.png` via `pnpm exec tauri icon`

**Interfaces:**
- Consumes: `buddy_core::support::SUPPORT_FLOORS` from Task 3; `buddy_core::settings::{load_settings, save_settings}` for a later milestone's settings screen (not called yet).
- Produces: Tauri app binary `buddy`, command `support_matrix() -> SupportFloors`, window titled `Buddy`. `tauri.conf.json` sets `productName: "Buddy"`, `identifier: "com.buddy.app"`, `frontendDist: "../dist"`, `beforeBuildCommand: "pnpm build"`, `build.devUrl: "http://localhost:5173"`, `bundle.active: true`, `bundle.macOS.minimumSystemVersion: "14.2"`, and `bundle.icon` listing the generated `icons/32x32.png`, `icons/128x128.png`, `icons/128x128@2x.png`, `icons/icon.icns` and `icons/icon.ico` (NSIS and DMG bundling need the `.ico` and `.icns` files).

- [ ] **Step 1: Write the failing test**

In `apps/desktop/src-tauri/src/lib.rs`, add a unit test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn support_matrix_command_matches_spec() {
        let m = support_matrix();
        assert_eq!(m.windows_min, "10");
        assert_eq!(m.macos_min, "14.2");
        assert!(m.linux_requires_pipewire);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p buddy-desktop`
Expected: compile error because `support_matrix` does not exist.

- [ ] **Step 3: Implement the shell**

From `apps/desktop`, run `pnpm exec tauri icon ../../assets/icon-1024.png --output src-tauri/icons` to generate the full icon set. Write `lib.rs` with `#[tauri::command] fn support_matrix() -> SupportFloors { SUPPORT_FLOORS.clone() }` and a `pub fn run()` that calls `tauri::Builder::default().invoke_handler(tauri::generate_handler![support_matrix]).run(tauri::generate_context!())`. Write the Vite/React files with a single `App` showing the title `Buddy` and the text "Foundation build — recording arrives in M1". Add the Tauri capability allowing only the `core:default` permission set. Do not add any plugin that opens the network.

- [ ] **Step 4: Run tests and build to verify**

Run: `cargo test -p buddy-desktop`
Expected: 1 test passes.

Run: `pnpm --filter @buddy/desktop build`
Expected: `apps/desktop/dist/index.html` exists and contains `<title>Buddy</title>`.

Run: `cargo check -p buddy-desktop`
Expected: exit 0.

- [ ] **Step 5: Commit**

```bash
git add apps/desktop assets/icon-1024.png
git commit -m "feat: add Tauri desktop shell"
```

---

### Task 6: Three-OS CI matrix and unsigned release artifacts

**Files:**
- Create: `.github/workflows/ci.yml`
- Create: `.github/release-notes-template.md`

**Interfaces:**
- Consumes: all workspace tests from Tasks 2–5 and the Tauri bundles from Task 5.
- Produces: on PRs into `main` and pushes to `main`, three test jobs; on a push to `main` whose `apps/desktop/package.json` version is newer than the latest `v*` tag, three packaging jobs plus a publish job that uploads **unsigned** artifacts via `gh release create` under tag `v<version>`. An unchanged version tests but publishes nothing.

- [ ] **Step 1: Write the release notes template**

`.github/release-notes-template.md` must contain, verbatim, these statements:
- `These builds are unsigned.`
- `macOS 14.2 or later is required; you may need to right-click the app and choose Open, or run xattr -d com.apple.quarantine /Applications/Buddy.app.`
- `Windows 10 or later is required; SmartScreen may warn, and Windows 11 with Smart App Control can block unsigned apps.`
- `Linux requires PipeWire; use the AppImage, .deb, .rpm or .tar.gz artifact for your distribution.`
- `Real screen and audio capture has not been verified on macOS in this release.`

- [ ] **Step 2: Write the CI workflow**

`.github/workflows/ci.yml`: name `CI`; triggers `pull_request` with `branches: [main]`, `push` with `branches: [main]`, and `workflow_dispatch`. No `application` triggers. Jobs:
- `prepare` (`ubuntu-latest`): `if: github.event_name == 'push'`, `permissions: contents: read`, checkout with `fetch-depth: 0`, fetch tags, read the version from `apps/desktop/package.json` with Node, get the newest `v*` tag, compare with `sort -V` (handling the no-tag case), and write `version` and `should_release` to `$GITHUB_OUTPUT`.
- `test-linux` (`ubuntu-latest`), `test-windows` (`windows-latest`), `test-macos` (`macos-latest`): checkout, `pnpm/action-setup@v4` with `version: 10`, `actions/setup-node@v4` with `node-version: 24` and pnpm cache, `dtolnay/rust-toolchain@stable` with `components: rustfmt`, then `pnpm install --frozen-lockfile`, `cargo fmt --all -- --check`, `cargo test --workspace`, `pnpm --filter @buddy/desktop build`. The Linux job first installs `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf` via apt.
- `package-linux`, `package-windows`, `package-macos`: same setup, each with `needs: [test-linux, test-windows, test-macos, prepare]` and `if: github.event_name == 'push' && needs.prepare.outputs.should_release == 'true'`. Linux also installs `rpm`. Before `tauri build`, rewrite the `version` field in `apps/desktop/src-tauri/tauri.conf.json` to `needs.prepare.outputs.version` with Node without committing. Package commands: Linux `pnpm exec tauri build --bundles appimage,deb,rpm` then stage `NOTICE`, `LICENSE` and an exact-version MPL source-locations file in a `legal/` directory and `tar czf buddy-linux-x86_64.tar.gz -C target/release buddy -C "$stage" legal`; Windows `pnpm exec tauri build --bundles nsis`; macOS `pnpm exec tauri build --bundles dmg`. Upload each with `actions/upload-artifact@v4` under names `artifact-linux`, `artifact-windows`, `artifact-macos`, using wildcards for the bundle directories plus the tar.gz.
- `publish`: `needs: [prepare, package-linux, package-windows, package-macos]`, `if: github.event_name == 'push' && needs.prepare.outputs.should_release == 'true'`, `permissions: contents: write`; downloads all three artifacts into `dist/`, then creates the release with tag and title `v<version>` (from `needs.prepare.outputs.version`) and `--notes-file .github/release-notes-template.md`, attaching all files found under `dist/` via `find dist -type f` in a bash array. `GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}` is the only secret; no signing steps or credentials anywhere. Artifact subdirectory layout is left to the implementer; `find` avoids depending on shell glob depth.

Add a comment in the workflow: real-capture soak jobs are added in M1; a soak failure must block the publish job at that point.

- [ ] **Step 3: Verify the workflow parses and the warnings exist**

Run: `python3 -c 'import yaml; yaml.safe_load(open(".github/workflows/ci.yml")); print("ok")'`
Expected: `ok`.

Run: `for s in "These builds are unsigned." "macOS 14.2 or later is required" "Windows 10 or later is required" "Linux requires PipeWire" "Real screen and audio capture has not been verified on macOS"; do grep -Fq "$s" .github/release-notes-template.md || exit 1; done; echo "ok"`
Expected: `ok`.

- [ ] **Step 4: Verify the local equivalent of the CI checks**

Run: `cargo fmt --all -- --check && cargo test --workspace && pnpm --filter @buddy/desktop build`
Expected: all pass with no formatting diff.

- [ ] **Step 5: Commit**

```bash
git add .github
git commit -m "ci: add three-OS build matrix and unsigned release artifacts"
```

---

## Exit Criteria for M0

- `cargo test --workspace` passes locally.
- `cargo fmt --all -- --check` is clean.
- `pnpm --filter @buddy/desktop build` produces `dist/index.html`.
- CI is green on Linux, Windows and macOS.
- Merging to `main` with `apps/desktop/package.json` at a version newer than the latest `v*` tag produces one GitHub release tagged `v<version>` containing NSIS `.exe`, `.dmg`, AppImage, `.deb`, `.rpm` and `.tar.gz`, with the unsigned-build warnings in the notes, and no signing step anywhere; a merge with an unchanged version tests but publishes nothing.
- Session create/read/recover and the corrupt, traversal and unknown-version cases are covered by passing tests.
