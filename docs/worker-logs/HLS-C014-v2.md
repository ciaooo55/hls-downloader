# HLS-C014 v2 — canonical v7.0.2 readiness decision

## Acceptance criteria

1. Base all decision work on exact post-C017 `main@426fc8eed97339885be16bf0ef5fff386a38bd31`; do not reuse the stale-base PR #79 as merge input.
2. Before any readiness metadata change, require v7 CI, v7 Candidate Package, Maintenance Security and Rust Security SUCCESS for `push/main/exact-SHA=426fc8eed97339885be16bf0ef5fff386a38bd31`.
3. Revalidate canonical v7.0.2 remains 28/28 verified with zero partial/blocked entries and confirm executable formal package/release gates remain fail-closed.
4. If authorized, change only `release_ready` and matching `audit_state`; do not change feature entries, product version, runtime/workflow/build scripts, tags, releases, signing, dispatch or publication.
5. Dedicated PR must be reviewed on its exact final head; any head movement invalidates PASS.
6. After an accepted C014 merge, HLS-C016 freezes that merge SHA and requires four fresh exact-SHA main-push prerequisite successes before any trusted formal-release attempt.
7. Never treat the readiness transition itself as external runner/signing/browser/operator evidence or publication authorization.

## 2026-09-08T21:49+08:00 — clean restart after C017

- C017 / PR #80 merged as `426fc8eed97339885be16bf0ef5fff386a38bd31` after explicitly labeled single-participant fallback review.
- Old C014 PR #79 was closed unmerged as superseded. Its head `7daf403e...` had 15 commits and an unmerged `release_ready=true` proposal mixed with stale coordination snapshots.
- New branch `coord/hls-c014-release-readiness-decision-v2` was created directly from post-C017 main rather than merging/rebasing the stale coordination history.
- No readiness state was changed during restart.

## 2026-09-08T22:06+08:00 — post-C017 baseline complete

Exact `main@426fc8eed97339885be16bf0ef5fff386a38bd31` prerequisite results:

- v7 CI #534 — SUCCESS
- v7 Candidate Package #182 — SUCCESS
- Maintenance Security #71 — SUCCESS
- Rust Security #45 — SUCCESS

A live main refresh still resolved to the same SHA. The C013 frozen-source to post-C017 main comparison contains only governance/documentation changes; runtime, workflows, build scripts and canonical feature-parity source did not change in that interval.

## Audit-state correction

The superseded #79 readiness patch changed `audit_state` to `v7_0_2_release_specialties_verified`. That value is not accepted for v2.

Evidence:

1. Git history shows the v7.0.1 `release_specialties_verified` label entered with commit `d7ebd90b8dcaa8b9426690415081caaf49a49a63`, which also promoted remaining partial features using actual focused/browser release-specialty evidence.
2. Current `scripts/bump-v7-version.ps1` defines `-ReleaseReady` lifecycle state as `v<version>_release_ready`; for 7.0.2 this is `v7_0_2_release_ready`.
3. Formal release still regenerates candidate-bound browser/performance/MSI/rollback evidence on the trusted runner before formal packaging, signing, draft creation or publication.

Using `v7_0_2_release_specialties_verified` before those v7.0.2 trusted-runner gates run would overstate evidence. The authorized narrow transition is therefore:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_ready`

No feature entry, verification text, product version, source, workflow, build script, tag, release, signing, dispatch or publication state is authorized to change.

## Final-head rule

The readiness metadata commit must be the final content commit so that the resulting PR head triggers the applicable v7 CI and Candidate Package pull-request workflows. Once that head exists it is frozen; any subsequent commit invalidates exact-head review/check evidence.

After merge, HLS-C016 owns the exact merge SHA and must wait for four fresh `push/main/exact-SHA` successes before any trusted formal-release attempt.

## Boundary

This task does not tag, sign, dispatch, create a Release or publish. External trusted-release prerequisites remain mandatory even after canonical readiness is authorized.
