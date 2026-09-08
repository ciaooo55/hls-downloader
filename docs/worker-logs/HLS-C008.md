# HLS-C008 — Reconcile current v7.0.2 release documentation drift

## Acceptance criteria

1. Current-facing README/release guidance distinguishes the active `7.0.2` source/candidate line from historical `v7.0.1` evidence.
2. Historical v7.0.1 material remains historically accurate but cannot be mistaken for current release authorization.
3. Documentation never sets or implies `release_ready=true` for current `main` while canonical feature parity says false.
4. No workflow, signing rule, version contract, package gate, or executable release script is weakened.
5. Final diff must be documentation/coordination-only and receive an exact-head source-state audit.

## Reproduced drift

Current `main` has canonical product version `7.0.2` and `release_ready=false`, while several user-facing documents still used v7.0.1 literals as current state:

- README correctly titled the product 7.0.2 but said new release evidence/artifacts bind to v7.0.1.
- `docs/v7-branch-state.md` still declared product version 7.0.1.
- `docs/v7-release-runner.md` described a public v7.0.1 release rather than the canonical current version.
- `docs/v7-local-upgrade.md` and `docs/v7-desktop-upgrade-note.md` hard-coded v7.0.1 package/install identities.
- `docs/v7-fast-release-path.md` preserved a real historical v7.0.1 snapshot containing `release_ready=true` but did not clearly prohibit treating that snapshot as current guidance.

GitHub Releases was also checked: `v7.0.1-candidate.1` remains the latest published prerelease, so it must not be silently renamed to 7.0.2. Instead it is labeled as the latest published **historical** test package while current source/candidate state remains 7.0.2.

## Implementation

Branch: `docs/hls-c008-v702-doc-reconcile`

- Current entry docs now resolve release/install identity from canonical feature-parity and candidate manifest state.
- README distinguishes published v7.0.1 prerelease assets from current v7.0.2 source.
- The formal runner guide is version-current and retains all exact-SHA, signing, runner and publication gates.
- Local/desktop upgrade docs stop presenting v7.0.1 file names as the current installed identity.
- `docs/v7-fast-release-path.md` is explicitly labeled a historical snapshot and points current readers to formal readiness/runner guidance.
- Machine task registry records HLS-C008 as in progress.

## Non-changes

- `artifacts/v7-productization/feature-parity.json` is not modified.
- `.github/workflows/**` is not modified.
- `scripts/**` is not modified.
- No release, tag, draft, publish action, readiness transition, or signing configuration is performed.

## Validation plan

- Compare the branch against its exact main base and require only documentation/coordination files.
- Inspect all changed text for `release_ready` semantics and v7.0.1/v7.0.2 role separation.
- Confirm current GitHub release state and canonical feature-parity state independently.
- Record final audit only after the PR head is frozen.
