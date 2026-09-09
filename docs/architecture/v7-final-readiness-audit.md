# v7.0.2 final source-readiness audit

## Decision scope

HLS-C013 audits the post-hardening repository state after C009-C011 security fixes and C006/C008/C012 documentation/coordination closeout. The audited frozen source is `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`.

This document classifies source and CI readiness only. It does not authorize changing `artifacts/v7-productization/feature-parity.json`, creating a tag or release, dispatching a formal release, signing artifacts, or publishing Latest.

## Frozen-source findings

At the audited frozen source `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`, the canonical product metadata reported v7.0.2, `release_ready=false`, and a complete 28/28 verified feature matrix with zero partial or blocked entries. This is historical evidence for that SHA, not the current matrix; later media-push gate work truthfully returned the active matrix to 27 verified and 1 partial.

The formal release workflow still requires exact identity for every prerequisite result: canonical workflow name and path, `push` event, `main` branch, and exact release SHA. For the frozen audit SHA all four required workflows completed successfully:

- `v7 CI` #525;
- `v7 Candidate Package` #173;
- `Maintenance Security` #68;
- `Rust Security` #42.

A final `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea...main` comparison was identical after the candidate workflow completed. The workflow evidence therefore belongs to the same source that was audited.

A separate compare from the HLS-C011 merge to the frozen SHA contains only documentation and coordination files. No runtime, release workflow, build script, or canonical feature-parity source changed after the accepted C009-C011 security hardening.

## Formal-package boundary

Candidate and formal packaging intentionally use different gates. Candidate packaging requires the canonical matrix, no blocked features, and a clean worktree. Formal packaging additionally requires canonical completeness, `release_ready=true`, clean worktree, and release evidence bound to the current commit/tree and candidate manifest.

`release_ready=false` is therefore the remaining repository-level *policy decision gate*, not a reproduced implementation defect. HLS-C013 must not turn it on merely because candidate CI is green.

## External trust boundary

The following remain external prerequisites rather than source defects:

- self-hosted Windows x64 runner labeled `hls-release`;
- fixed `E:\h` lifecycle environment;
- real Edge and Firefox installations;
- signing tool, trusted project certificate/private key, signer configuration and timestamp/network access;
- protected `v7-release` environment and its approval policy;
- explicit operator decision to publish only after draft asset digest verification.

Repository code must fail closed when these prerequisites are absent rather than weakening or simulating them.

## C013 decision

**PASS: the frozen source/CI contract is clean. Formal publication remains blocked by design.**

No new deterministic source or prerequisite-workflow defect was reproduced. The next safe route is a separate reviewed readiness-decision task. If governance authorizes canonical `release_ready=true`, that change must be made explicitly in its own reviewable change. The resulting final `main` SHA must then be frozen and all four exact-SHA push workflows must succeed again before an authorized trusted-runner formal-release attempt.

Any later `main` movement invalidates the old workflow set for release purposes, even if the movement is documentation-only.
