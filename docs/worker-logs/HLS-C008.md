# HLS-C008 — Reconcile current v7.0.2 release documentation drift

## Acceptance criteria

1. Current-facing release, branch and install guidance describes the active v7.0.2 source/candidate state consistently.
2. Historical v7.0.1 measurements, hashes and published candidate evidence remain historically accurate and are explicitly labeled historical rather than current guidance.
3. No documentation change sets or implies `release_ready=true` or authorizes formal publishing.
4. `.github/workflows/release-v7.yml`, executable release gates and product code are not weakened or changed by this documentation cleanup.

## Assignment and recovery

- Priority: `P2`
- Original planned owner after preaudit: `worker-1`
- Final owner: `worker-0`
- Branch: `docs/hls-c008-release-doc-drift`
- Base after HLS-C006 merge: `adf74c30d9936214b060d062f56813c879ca5ee2`

Worker-1 was assigned after the HLS-C008 read-only preaudit, but its exact last GitHub heartbeat was `2026-09-08T06:02:08Z`; a later worker-0 heartbeat at `07:08:24Z` proved the repository's five-minute lost-worker threshold had been exceeded. The C008 branch was still identical to its base with zero commits, so the branch was fast-forwarded to the HLS-C006 merge and reassigned without overwriting worker changes.

## Reproduced drift

Before HLS-C008, current-facing docs still mixed the active 7.0.2 source line with 7.0.1 release assumptions:

- `docs/v7-fast-release-path.md` called itself the v7.0.1 formal path and claimed current `release_ready=true`.
- `docs/v7-branch-state.md` called current product version 7.0.1.
- `docs/v7-verification.md` headed itself 7.0.1 and described a historical `28/28 / release_ready=true` snapshot as current, while later executable guidance hard-coded `manifest.version=7.0.1` and `product_version=7.0.1`.
- `docs/v7-local-upgrade.md` hard-coded current install/extension/provenance/smoke expectations to 7.0.1 even though `install-v7-local.ps1` resolves product version from canonical feature-parity metadata.
- `docs/v7-desktop-upgrade-note.md` presented 7.0.1 extension/start-menu paths as current.
- `docs/v7-release-runner.md` described a public v7.0.1 formal target even though `release-v7.yml` derives version/tag from canonical feature-parity.

Canonical source on the task base says `product_version=7.0.2`, `release_ready=false`, `audit_state=v7_0_2_iteration_in_progress`. GitHub Releases still exposes `v7.0.1-candidate.1` as the latest public test prerelease, so that historical download/evidence must remain labeled 7.0.1 rather than being renamed.

## Changes by commit

1. `v7-fast-release-path.md`: makes the active path 7.0.2/version-dynamic; restores the frozen-main exact-SHA four-workflow sequence; explicitly says current `release_ready=false` and that local candidate/package commands do not replace formal workflow trust boundaries.
2. `v7-branch-state.md`: records active 7.0.2/release_ready=false while preserving v7.0.0 stable baseline and historical v7.0.1 candidate.
3. `v7-verification.md`: separates 2026-08 historical measurements/hashes from current canonical state; changes executable examples to canonical-version placeholders and current formal gates instead of hard-coded 7.0.1.
4. `v7-local-upgrade.md`: documents current 7.0.2 install names but states the installer actually derives `$productVersion` from canonical metadata; historical 7.0.1 candidate remains historical.
5. `v7-desktop-upgrade-note.md`: updates current local paths/version and records C009/C010/C011 hardening without changing product behavior.
6. `v7-release-runner.md`: makes formal version/tag explicitly canonical-version-derived and keeps the dedicated hls-release runner, exact-SHA workflows, signing/timestamp, Draft digest and explicit publish boundaries.

## Historical evidence policy

HLS-C008 intentionally does **not** rewrite:

- the public `v7.0.1-candidate.1` tag or its asset names;
- historical performance/latency/test-count measurements;
- historical local EXE/MSI/Portable SHA-256 values;
- the immutable public `v7.0.0` MSI upgrade baseline.

Those values remain useful only when clearly scoped to their original tested commit/artifact line.

## Scope and validation

Changed files are documentation only. HLS-C008 does not modify:

- `artifacts/v7-productization/feature-parity.json`;
- `.github/workflows/release-v7.yml` or any other workflow;
- release gate PowerShell;
- runtime code or dependency manifests;
- GitHub tags/releases/assets.

Final acceptance must inspect the exact PR diff against current main and verify that every current-state statement matches source while every retained 7.0.1 value is clearly historical. Documentation-only path filters may produce no PR workflow; absence of a run is not represented as CI success.
