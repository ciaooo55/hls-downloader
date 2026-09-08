# Formal v7 release readiness boundaries

This document is the durable architecture-level output of HLS-C003. It distinguishes repository-enforced release contracts from prerequisites that deliberately live outside the repository.

## Source-enforced release chain

The formal v7 release path is one frozen-source pipeline:

1. Dispatch only from current `main`.
2. Resolve the canonical product version from `artifacts/v7-productization/feature-parity.json`.
3. Require successful exact-SHA **push** conclusions for the canonical `v7 CI`, `v7 Candidate Package`, `Maintenance Security`, and `Rust Security` workflow paths.
4. Run on the dedicated Windows x64 `hls-release` runner and verify the fixed `E:\h` lifecycle environment.
5. Build a candidate from that same source SHA.
6. Produce browser, performance, MSI upgrade, and forced-rollback evidence from that candidate manifest.
7. Reconfirm that remote `main` has not moved.
8. Build the formal package from the same evidence.
9. Authenticode-sign and timestamp the public Windows artifacts, then verify trust.
10. Stage the source bundle, SBOM/evidence, release assets and checksums.
11. Create or resume only an annotated tag/draft release proven to belong to the same frozen commit.
12. Verify uploaded asset byte sizes and SHA-256 digests before publication.
13. Publish Latest only when the dispatch explicitly requests publication.

HLS-C002 and HLS-C007 close the two currently reproduced source-level defects in this chain: exact-SHA prerequisite workflow starvation and stale v7.0.1 MSI lifecycle candidate assumptions.

## Formal package decision gate

The candidate path and formal package path intentionally have different requirements. Candidate packaging may run while the iteration is still closing evidence, but formal packaging calls the canonical feature verifier with complete-feature, `release_ready`, release-evidence and clean-worktree requirements.

Therefore `release_ready=false` is a real formal-package blocker by design, not an implementation defect to bypass. Changing it is a reviewed readiness decision outside HLS-C003.

Release evidence generation does not violate the clean-worktree gate. Repository `.gitignore` ignores generated content under `artifacts/v7-productization/*` while explicitly keeping only the canonical `feature-parity.json` tracked. The aggregate and per-gate evidence, candidate artifacts and formal staging files can therefore be created without modifying tracked source. The evidence recorder additionally binds reports to the current source commit/tree and candidate manifest digest.

## External trust boundary

The following inputs must **not** be manufactured or relaxed by repository code merely to make a release pass:

- self-hosted Windows x64 runner with the `hls-release` label;
- physical/logical `E:` volume supporting the fixed `E:\h` lifecycle test;
- Edge and Firefox installations for real browser evidence;
- Windows SDK `signtool.exe` or an explicitly supplied signing tool;
- trusted code-signing certificate and private key in the selected Windows certificate store;
- signing thumbprint/environment configuration without storing private key material in Git;
- network connectivity to pinned tool downloads, GitHub and the timestamp endpoint;
- protected `v7-release` environment and operator approval controls;
- final operator choice to publish after draft asset verification.

These are release infrastructure and governance prerequisites. A missing prerequisite should produce a blocked formal release, not a code change that bypasses the check.

## Freeze semantics

The exact-SHA model means the releaseable unit is the **final frozen main SHA**, not "the product code before some later docs commits". Any commit that moves `main` creates a new prospective release SHA and requires fresh exact-SHA prerequisite conclusions. Concurrency cancellation of an older SHA after a newer push is therefore expected and safe.

Operational consequence: once the project is preparing a formal release, coordination-only changes should be accumulated on branches and merged deliberately. After the final reviewed merge, stop moving `main` until all four required push workflows finish for that SHA and the formal release dispatch either completes or is abandoned.

## Draft and publication boundary

The workflow creates or resumes only a draft release associated with the exact frozen commit. It then compares every uploaded asset's byte size and GitHub-reported SHA-256 digest with the local staging files. The release must still be a draft during this verification. Conversion to Latest occurs only when the workflow-dispatch `publish` input is explicitly true.

This separates "formal artifacts have been built and uploaded" from "the public release is authorized" and avoids treating an interrupted draft upload as an implicit publication decision.

## Readiness truth source

`feature-parity.json` currently declares v7.0.2 with `release_ready=false`. HLS-C003 treats that state as authoritative and does not change it. Candidate CI success, repository source completeness, or availability of a draft package is insufficient by itself to authorize a formal public release.

## Documentation boundary

Some current-facing documentation still contains v7.0.1 candidate/formal-release wording while the active product version is v7.0.2. Those stale descriptions should be corrected as documentation work. Historical v7.0.1 evidence may remain when clearly labeled historical. Documentation cleanup must not alter the executable release gate or imply `release_ready=true` before the canonical readiness decision is actually made.
