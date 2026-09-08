# HLS-C003 — Formal v7 release readiness triage

## Acceptance criteria

1. Enumerate the current formal release workflow prerequisites from repository source.
2. Separate repository-code defects from external-only trusted-runner/signing/operator prerequisites.
3. Do not set `release_ready=true`, create a formal tag/release, or weaken any formal gate.
4. Record a shortest-safe-route recommendation for coordinator planning.

## Assignment

- Priority: P1
- Owner: `worker-0`
- Roles: coordinator + auditor + worker (single-participant fallback)
- Branch: `audit/hls-c003-release-readiness`
- Depends on: HLS-C002, HLS-C007
- Claim mirrored in Issue #39; active heartbeat mirrored in Issue #41.

## Source evidence inspected

- `.github/workflows/release-v7.yml`
- `docs/v7-release-runner.md`
- `artifacts/v7-productization/feature-parity.json`
- HLS-C002 accepted exact-SHA security workflow contract
- HLS-C007 accepted canonical MSI candidate-version contract

## Initial classification

### Repository contract already present

The formal workflow is deliberately fail-closed. It requires:

- manual dispatch from current `main`;
- four successful push workflows for the exact same `main` SHA (`v7 CI`, `v7 Candidate Package`, `Maintenance Security`, `Rust Security`), matching both canonical workflow name and path;
- dedicated self-hosted Windows x64 runner label `hls-release`;
- an `E:` volume because MSI lifecycle verification remains fixed to `E:\h`;
- Git, `gh`, Python+PyYAML, Go and the workflow-pinned Rust/Node/pnpm/JDK/FFmpeg toolchains;
- browser/performance/installer-upgrade/failure-rollback evidence against one frozen candidate manifest;
- Authenticode signing and timestamp verification;
- frozen-main rechecks before packaging/tagging;
- staged assets, checksums, SBOM/evidence, draft-release upload, and post-upload byte-size/SHA-256 verification;
- explicit `publish=true` before a verified draft becomes Latest.

HLS-C002 fixed exact-SHA prerequisite workflow coverage. HLS-C007 fixed the deterministic v7.0.1/v7.0.2 MSI lifecycle version drift. No remaining source defect is established by those two previously identified blockers.

### External / operator prerequisites

These cannot be satisfied by ordinary repository code alone and must remain external gates:

- an online self-hosted Windows x64 runner carrying `hls-release`;
- real `E:` volume and permission to install/uninstall at `E:\h`;
- installed Edge and Firefox, or valid explicit binary paths;
- Windows SDK `signtool.exe` or `HLS_V7_SIGNTOOL`;
- a trusted code-signing certificate with accessible private key, referenced only by `HLS_V7_SIGN_CERT_THUMBPRINT` and certificate-store selection;
- network access to pinned tool/media downloads, timestamp service and GitHub;
- a configured/protected `v7-release` environment and operator authorization;
- final operator decision whether to dispatch with `publish=true` after draft digest verification.

### Current readiness state

Canonical `artifacts/v7-productization/feature-parity.json` is `product_version=7.0.2`, `release_ready=false`, `audit_state=v7_0_2_iteration_in_progress`. HLS-C003 will not change that value.

The HLS-C007 merge SHA `c6779fd1017bb8f7378eec7d0ba5cd1e5f079dd1` did start all four required `main` push workflows, but subsequent coordination commits moved `main` and cancelled those runs. This is expected concurrency behavior, not product-test success or failure. Therefore a formal release must target a final frozen `main` commit and allow all four exact-SHA prerequisite workflows for that final commit to finish successfully before dispatch.

## Route recommendation

1. Finish coordination/release-readiness documentation on this task branch; avoid unnecessary direct `main` writes while collecting exact-SHA evidence.
2. Audit current `main` for any additional reproducible release-code defect. If none is found, do not invent a source fix.
3. Treat runner/signing/browser/`E:\h`/environment/operator requirements as external blockers, not reasons to weaken workflow gates.
4. When repository changes are finished, merge one final reviewed documentation/triage PR, freeze that resulting `main` SHA, and wait for all four exact-SHA push workflows to succeed.
5. Only then may an operator with the trusted release machine dispatch `v7 Formal Release`; `release_ready=false` remains unchanged until the project's existing release-readiness policy explicitly authorizes changing it.

## Status

`in_progress`: initial prerequisite classification complete; final coordinator/auditor pass still requires checking the current final-main workflow evidence and confirming no additional source blocker from the formal workflow path.
