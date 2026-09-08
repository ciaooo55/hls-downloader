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
- `scripts/build-v7.ps1`
- `scripts/verify-v7-feature-parity.ps1`
- `scripts/invoke-v7-release-gates.ps1`
- `scripts/record-v7-release-gate.ps1`
- `.gitignore`
- HLS-C002 accepted exact-SHA security workflow contract
- HLS-C007 accepted canonical MSI candidate-version contract

## Classification

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

HLS-C002 fixed exact-SHA prerequisite workflow coverage. HLS-C007 fixed the deterministic v7.0.1/v7.0.2 MSI lifecycle version drift.

### Formal-package readiness decision

`build-v7.ps1 -Task package` deliberately invokes `verify-v7-feature-parity.ps1` with `-RequireCanonicalComplete -RequireReleaseReady -RequireCleanWorktree`. The verifier therefore rejects formal packaging until the canonical matrix is 28/28 verified, `release_ready=true`, the release evidence matches the current commit/tree, and the Git worktree is clean.

This is a policy gate rather than a defect. HLS-C003 does not flip `release_ready` because canonical `feature-parity.json` still says `release_ready=false` and `audit_state=v7_0_2_iteration_in_progress`.

The clean-worktree requirement does **not** deadlock with release evidence generation: `.gitignore` ignores `artifacts/v7-productization/*` except the canonical `feature-parity.json`. Browser/performance/installer/rollback reports, aggregate `release-evidence.json`, candidate output and formal package staging therefore remain local ignored artifacts. `record-v7-release-gate.ps1` binds each report and aggregate evidence file to the current source commit/tree and candidate manifest hash.

### Draft and publication protection

The formal workflow keeps the GitHub Release as a draft while validating uploaded asset count, byte size and GitHub-reported `sha256:` digest against local staged files. Only the explicit workflow-dispatch `publish` input permits the final `gh release edit --draft=false --latest` step.

No path inspected in HLS-C003 bypasses the current-main check, exact-SHA prerequisite check, release evidence binding, `release_ready` gate, signing verification or upload digest verification.

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

Canonical `artifacts/v7-productization/feature-parity.json` is `product_version=7.0.2`, `release_ready=false`, `audit_state=v7_0_2_iteration_in_progress`. HLS-C003 does not change that value.

The HLS-C007 merge SHA `c6779fd1017bb8f7378eec7d0ba5cd1e5f079dd1` did start all four required `main` push workflows, but subsequent coordination commits moved `main` and cancelled those obsolete-SHA runs. That is expected exact-SHA concurrency behavior, not a product-test success or failure. A formal release must target a final frozen `main` commit and allow all four exact-SHA prerequisite workflows for that final commit to finish successfully before dispatch.

A later coordination commit `f87e7a0b4225afd6348c3d9e9782ee543deb65d0` again started all four required `main` push workflows, demonstrating that the every-main-push contract remains active. Those runs are ordinary evidence for that commit, not authorization for release while `release_ready=false`.

### Documentation drift observed

`README.md` still mixes active product version 7.0.2 with v7.0.1 candidate/formal-release wording, and `docs/v7-fast-release-path.md` is explicitly a historical v7.0.1 path containing `release_ready=true`. These are documentation-quality risks, not executable formal-workflow blockers. They should be handled by the README/documentation refinement backlog without changing release gates or falsely promoting readiness.

## Route recommendation

1. Finish reviewed repository work and documentation before freezing a release SHA.
2. Do not invent another source fix: this audit found no third deterministic formal-release code blocker after HLS-C002 and HLS-C007.
3. Treat runner/signing/browser/`E:\h`/environment/operator requirements as external blockers, not reasons to weaken workflow gates.
4. Keep `release_ready=false` until the project's readiness policy has independently established the criteria for changing it and that change itself is reviewed.
5. When repository changes are finished, merge the final reviewed work, freeze that resulting `main` SHA, and wait for all four exact-SHA push workflows to succeed.
6. Only then may an authorized operator on the trusted release machine dispatch `v7 Formal Release`; publication remains a separate explicit action after draft digest verification.

## Independent acceptance result

**PASS for HLS-C003 scope.** The current formal-release chain and its external trust boundary are now enumerated from source. No additional deterministic source blocker was reproduced. `release_ready=false`, tag state and public release state were left unchanged.

This PASS authorizes merging only the HLS-C003 documentation/coordination PR after a final diff/base refresh. It does **not** authorize a formal v7.0.2 release.
