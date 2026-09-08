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
- No readiness state has been changed on v2 yet.
- Post-C017 main triggered exactly four prerequisite push workflows. At restart, v7 CI #534 and Candidate #182 were still running; remaining security workflows were also part of the same exact-SHA set. Transition is withheld until all four are successful.

## Boundary

This task does not tag, sign, dispatch, create a Release or publish. External trusted-release prerequisites remain mandatory even if canonical readiness is eventually authorized.
