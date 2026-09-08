# HLS-C004 — Dependabot major-version triage

## Acceptance criteria

1. Group open dependency PRs by runtime/build/security risk.
2. Distinguish security-relevant updates from optional major-version churn.
3. Give every merge/defer recommendation project-specific compatibility evidence.
4. Never merge a broad dependency batch without repository-specific validation.

## Assignment

- Priority: `P1`
- Worker: `worker-1`
- Branch: `deps/hls-c004-dependabot-triage`
- Base at claim: `185973ca3bbd336762a3499739e922fb6ddf0c17`
- Canonical registry: Issue #39 / `docs/coordination/tasks.json`

`worker-1` first attempted to claim HLS-C003 from stale task state. After refreshing `main`, HLS-C003 was already owned by `worker-0`, so that claim was withdrawn and HLS-C004 was claimed instead. No HLS-C003 files are modified here.

## Current queue result

As of the audit, the repository has **no open Dependabot pull requests**. The only open pull request observed was the concurrent HLS-C003 documentation PR. HLS-C004 therefore does not manufacture dependency churn merely to satisfy an old task title; it audits the historical major-update wave and the versions actually present on `main`.

## Historical major/update wave

| PR | Area | Result | Risk class | HLS-C004 assessment |
| --- | --- | --- | --- | --- |
| #18 | `rusqlite` 0.37 -> 0.40 | merged | runtime + security | Good merge. 0.40.1 includes a tainted-SAVEPOINT SQL-injection fix; `main` now declares `rusqlite = 0.40`. |
| #19 | `windows-sys` 0.59 -> 0.61 | closed, not merged | Windows ABI/API compatibility | Defer. Dependabot could not rebase after third-party edits, and `main` still declares 0.59. No project-specific security requirement justifies reopening a broad Windows binding migration during release hardening. |
| #20 | `russh` 0.62.7 -> 0.63.2 | closed, not merged | runtime + security + breaking API | Superseded correctly by #36, which carried the security update and adapted the SFTP TOFU host-key handler to `PublicKeyOrCertificate`. |
| #21 | GitHub Actions major group | merged | CI/supply chain | Good merge. Workflows now use exact immutable commit SHAs for checkout/cache/setup-java/setup-node/upload-artifact and have been exercised by v7 CI/release work. |
| #22 | `@types/chrome` 0.0.326 -> 0.2.8 | merged | dev/type tooling | Good merge; current extension manifest is 0.2.8. |
| #23 | Compose/coroutines/serialization/Gradle group | merged | runtime + build | Good merge only because repository CI validates Compose tests/distribution. Current Compose is 1.12.0; coroutines/serialization are 1.11.0. |
| #24 | Vitest 3.2.6 -> 5.0.0 | merged | dev/test major | Accepted with project validation. Current extension uses Node 24.20.0 in CI and Vitest 5.0.0; extension tests have remained part of v7 CI. |
| #26 | TypeScript 5.8.3 -> 7.0.2 | merged | dev/compiler major | Accepted with project validation. Current extension manifest is TypeScript 7.0.2 and `pnpm test` includes `tsc --noEmit`. |

## Current-manifest evidence

`native_shell/Cargo.toml` currently declares:

- `rusqlite = 0.40`
- `russh = 0.63`
- `windows-sys = 0.59`

`extension/package.json` currently declares:

- `@types/chrome = 0.2.8`
- `typescript = 7.0.2`
- `vitest = 5.0.0`

`desktop_ui/build.gradle.kts` currently declares Compose `1.12.0`, coroutines Swing `1.11.0`, serialization JSON `1.11.0`, Kotlin `2.4.10`, and product version `7.0.2`.

## Policy conclusion

The historical queue demonstrates the desired policy:

- security-bearing runtime updates are high priority, but breaking APIs require a narrow project-owned adaptation PR (example: `russh` #20 -> #36);
- Windows binding majors are not upgraded solely to chase version numbers, especially while updater/release hardening depends heavily on Win32 APIs;
- CI Actions remain pinned by immutable commit SHA after update;
- compiler/test/build majors require the component's real build/test pipeline before acceptance;
- an empty current Dependabot queue is a valid result and is not a reason to create speculative dependency churn.

## Remaining work

- Add a machine-readable triage snapshot for future agents/auditors.
- Add the durable dependency-maintenance policy under `docs/architecture/`.
- Refresh against current `main` before review because worker-0 is concurrently working HLS-C003.
- Submit this documentation-only task for independent review by `worker-0`; do not self-merge without that review while the auditor is available.
