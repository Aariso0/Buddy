# CI layout

`ci.yml` is a thin orchestrator. It owns triggers, the release-version decision,
and the final `publish` gate; all platform work is delegated to reusable
workflows so one runner can change without touching the others.

| File | Purpose |
|---|---|
| `ci.yml` | Triggers, `prepare`, job wiring, `publish` gate |
| `ci-test.yml` | `cargo fmt`, `cargo test --workspace`, desktop build on one OS |
| `ci-package.yml` | Tauri bundles and artifact upload on one OS |
| `ci-soak-linux.yml` | PipeWire null-device real-capture soak |

The Windows WASAPI soak is disabled and no longer gates `publish`. A hosted
Windows runner cannot provide both a render endpoint for loopback and a capture
endpoint for the Mic track, and no self-hosted `buddy-audio-soak` runner is
registered. See `tests/soak/README.md` for the remaining local-only procedure.

Shared code lives outside the workflows:

- `.github/actions/setup-workspace/` — pnpm, Node, Rust, optional Linux packages
- `.github/scripts/decide-release.js` — semver comparison against `v*` tags
- `.github/scripts/stage-legal.sh` — Linux legal archive and MPL source locations

Reusable calls appear in the Checks tab as `<caller job> / <called job>` (for
example `test-linux / test (ubuntu-latest)`). Update branch-protection required
check names if they still pin the old single-workflow names.

Validate locally with:

```bash
go run github.com/rhysd/actionlint/cmd/actionlint@latest -no-color
python3 - <<'PY'
import yaml
from pathlib import Path
for path in [*Path(".github/workflows").glob("*.yml"), Path(".github/actions/setup-workspace/action.yml")]:
    yaml.safe_load(path.read_text())
PY
```
