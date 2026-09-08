# HLS-C013 — Final post-hardening v7.0.2 release-readiness re-audit

## Audit boundary

- Role: auditor fallback (`worker-0`; no independent worker currently active)
- Frozen main SHA: `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`
- Audit branch: `audit/hls-c013-final-readiness`
- Audit is evidence-only. It does not change `release_ready`, product/runtime code, workflows, tags, releases, signing state or publication state.
- Any movement of `main` invalidates the frozen-SHA workflow conclusion and requires a fresh audit.

## Acceptance criteria

1. Re-read canonical version/readiness and the executable formal-release chain from the frozen SHA.
2. Verify the four canonical **push/main/exact-SHA** workflows for the frozen SHA before any source-ready classification.
3. Confirm whether C009/C010/C011 security fixes remain present and unmodified by later commits.
4. Separate deterministic repository/source blockers from external trusted-runner/signing/browser/environment/operator prerequisites.
5. Keep `release_ready=false`; perform no tag/release/publish action and weaken no formal gate.

## Source-state evidence

Canonical `artifacts/v7-productization/feature-parity.json` at the frozen SHA declares:

- `product_version = 7.0.2`
- `release_ready = false`
- `audit_state = v7_0_2_iteration_in_progress`
- feature summary = `28 verified / 0 partial / 0 blocked / 28 total` (`100.0%`).

An independent compare from the HLS-C011 merge `6b5f596325a4b00758ccd062b92eaeb80b0256ee` to frozen main `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea` contains only README, release/install documentation, worker logs and coordination files. It contains **no runtime, workflow, script or feature-parity change**. Therefore the already-validated C009/C010/C011 product-security implementation was not rewritten after its merged-main verification.

## Executable formal-release contract

Frozen `.github/workflows/release-v7.yml` still enforces:

1. dispatch from the current remote `main` and reject stale/non-main source;
2. canonical product version from feature-parity metadata and safe annotated-tag/draft retry semantics;
3. successful workflow matches bound by canonical workflow **name + path + event=push + head_branch=main + exact `GITHUB_SHA`** for `v7 CI`, `v7 Candidate Package`, `Maintenance Security` and `Rust Security`;
4. dedicated self-hosted Windows x64 `hls-release` runner, `E:` volume and required tools;
5. fresh candidate build from that frozen source;
6. browser, performance, MSI-upgrade and forced-rollback evidence from the same candidate manifest;
7. reconfirm remote `main` before formal package;
8. formal package build, then Authenticode signing/trust verification, source/SBOM/checksum staging, draft release, uploaded digest verification and optional explicit publish.

Frozen `scripts/build-v7.ps1` keeps candidate and formal package decisions separate:

- candidate: canonical matrix + no blocked features + clean worktree; no `release_ready` or canonical-complete requirement;
- formal package: `RequireCanonicalComplete + RequireReleaseReady + RequireCleanWorktree + ReleaseEvidence`.

Frozen `scripts/verify-v7-feature-parity.ps1` independently rejects a formal package when `release_ready` is not `true`, and separately requires the canonical 28/28 verified feature set. Thus `release_ready=false` is an intentional formal-package blocker. It is not a defect that this audit may bypass or flip.

## Security hardening carry-forward

- **HLS-C009 / PR #68**: automatic update acceptance is bound to project-owned release signer identity/explicit source-controlled rollover trust in addition to digest, WinVerifyTrust and MSI identity checks.
- **HLS-C010 / PR #67**: optional Core TCP config/client/actual pre-bound listener are loopback-only; Windows named-pipe protections remain unchanged.
- **HLS-C011 / PR #69**: replay-controlled custom headers are cleared on cross-origin child/redirect/multi-hop transitions before exact target-origin scoped restoration.

No product source changed after C011 before this frozen audit SHA.

## Frozen-SHA workflow evidence

For `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`, the final exact-SHA refresh found all four required **push/main** workflows completed successfully:

- `v7 CI` #525 — **SUCCESS**.
- `v7 Candidate Package` #173 — **SUCCESS**.
- `Maintenance Security` #68 — **SUCCESS**.
- `Rust Security` #42 — **SUCCESS**.

The initial audit capture observed Candidate #173 while it was still running; this final refresh supersedes that preliminary state. A separate compare of `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea...main` returned `identical` after Candidate #173 completed, proving the frozen audit SHA had not moved before classification.

## External trust boundary

Even after source/workflow acceptance, formal publication still depends on release infrastructure/governance that this repository audit must not manufacture or relax:

- self-hosted Windows x64 runner labeled `hls-release`;
- fixed `E:\h` lifecycle environment;
- real Edge and Firefox installations;
- Windows SDK `signtool.exe` (or explicit configured signing tool);
- trusted code-signing certificate and accessible private key in the selected certificate store;
- signing thumbprint/store/timestamp configuration and network access to pinned tools, GitHub and timestamp service;
- protected `v7-release` environment / operator approval controls;
- explicit operator choice to publish after draft asset digest verification.

## Non-blocking documentation observation

`docs/architecture/formal-release-readiness.md` still contains a final historical note saying some current-facing docs contain stale v7.0.1 wording. HLS-C008 has since reconciled those current-facing docs. This sentence is now stale documentation metadata, but it does not change the executable release workflow, package gate, product version or `release_ready` state. Do not move frozen main merely to edit it before this audit concludes; record it as later documentation cleanup if desired.

## Final classification

**PASS — SOURCE/CI CONTRACT CLEAN AT THE FROZEN AUDIT SHA; FORMAL PUBLICATION REMAINS DELIBERATELY BLOCKED.**

No new deterministic product, release-workflow, or prerequisite-CI source blocker was reproduced on the frozen SHA after C009-C011 hardening and C006/C008/C012 closeout. The canonical feature matrix is complete (28/28 verified), all four required exact-SHA main-push workflows succeeded, and `main` remained frozen through the final check.

This is **not** a publish authorization. `release_ready=false` still causes formal packaging to fail by design. The next project task should therefore be an explicit, separately reviewed readiness-decision/handoff task: decide whether project governance now authorizes changing canonical `release_ready`, and if so make that change in its own reviewable PR; after the final reviewed merge, freeze the resulting new `main` SHA and require the four exact-SHA prerequisite workflows again before any trusted-runner formal-release attempt. External runner/signing/browser/environment/operator gates remain mandatory.
