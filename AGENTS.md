# HLS Downloader repository instructions

HLS Downloader is a Windows-first desktop download manager. The only active product version is `7.0.2`.

Canonical product version and release-readiness state come from `artifacts/v7-productization/feature-parity.json`. Live instructions and current-facing documentation must follow that tracked metadata rather than overriding it with a hard-coded historical version or an inferred readiness state.

## Active architecture

- `desktop_ui/`: Compose Desktop main workbench. It never opens SQLite.
- `native_shell/`: single resident Rust Core, transfer engines, database owner and Native Messaging host.
- `presenter_ui/`: native hot presenter only; it is not a second main workbench.
- `extension/`: WXT Manifest V3 Chromium/Firefox extension.

The default protocol is `hls-downloader-v7-core` on `\\.\pipe\HLSDownloader.v7`. Legacy protocol handling exists only for explicit migration compatibility and must not become a default launch path.

Python/FastAPI, React/Tauri, WebView2 and the v6 Win32 supervisor are historical implementations. Their source is available through Git tags (`v3.0.39`, `v5.0.13`, `v6.0.1`) and must not be restored as active directories.

## Branch ownership and repository hygiene

- Keep exactly one local branch, `main`, and one remote branch, `origin/main`.
- Develop, review and validate on local `main`. Preserve useful changes from retired branches and dependency updates before removing their refs.
- Remote synchronization remains operator-controlled: push local `main` only when explicitly requested. Never force-push or rewrite published history.
- Do not create pull requests, additional branches or additional worktrees. Dependency version-update PRs are paused; retain the security audit workflows and review dependency updates on `main`.
- This operator-approved single-main workflow (2026-09-15) supersedes the older two-branch / Draft-PR instructions in `handoff.md` and historical coordination logs. It does not grant formal release authorization or bypass protected-branch/server controls.
- Keep source in the existing product directories. Put test evidence under `artifacts/v7-productization/` and user-facing deliverables under `outputs/`; do not scatter temporary reports or debug scripts in the repository root. Local state and outputs must not be committed.
- `.workbuddy-ai/` holds project state and memory, not disposable cache. Preserve it. Before removing obsolete worktrees or personal files, list the exact targets, preserve unmerged changes, back up as required and obtain confirmation.

## Validation

```powershell
cargo test --manifest-path native_shell/Cargo.toml --lib
cargo test --manifest-path presenter_ui/Cargo.toml
cd desktop_ui; .\gradlew.bat test --no-daemon
cd ..\extension; pnpm test; pnpm run build
```

PowerShell scripts intended for users must parse under Windows PowerShell 5.1 and PowerShell 7. Text JSON/manifests must be written as UTF-8 without BOM unless the target format requires otherwise.

Do not generate formal packages unless the canonical feature-parity matrix is fully verified, `release_ready=true` has been explicitly reviewed and authorized, and the visual, performance, installer and rollback gates pass for the formal-release attempt. Candidate CI success alone is not publication authorization, and no repository instruction may bypass the trusted release workflow or its operator controls.
