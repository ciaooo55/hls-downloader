# v7.0.2 canonical readiness decision

## Purpose

HLS-C014 decides whether the repository may move canonical v7.0.2 readiness from iteration mode into the already-existing formal validation/release pipeline. This is a source/governance decision only. It does not tag, sign, dispatch, upload or publish a release.

## Inputs

- HLS-C013 / PR #75: frozen-source audit passed; canonical matrix 28/28 verified with zero partial/blocked entries; all four required exact-SHA main-push prerequisite workflows succeeded for the audited SHA; no new deterministic product/release-workflow blocker was reproduced.
- HLS-C015 / PR #78: live governance instructions were reconciled to v7.0.2 without changing product/runtime/workflow/build-script/feature-parity source. Root `AGENTS.md` now treats canonical feature-parity metadata as the version/readiness truth and explicitly separates candidate CI from publication authorization.
- Executable formal gate: formal packaging still requires canonical completeness, `release_ready=true`, clean worktree and release evidence; the release workflow additionally requires exact-SHA prerequisite workflows, frozen current main, trusted Windows runner, real browsers, MSI/rollback/performance evidence, signing/trust, draft asset digest verification and explicit publish choice.

## Repository precedent

The v7.0.1 canonical ready state used `release_ready=true` with `audit_state=v7_0_1_release_specialties_verified`. The `build: start v7.0.2 iteration` commit intentionally reset those fields together to `false` and `v7_0_2_iteration_in_progress` while incrementing the product version.

The established meaning is therefore a repository lifecycle state: `release_ready=true` permits the formal package path to proceed **if and only if** the executable formal gates also pass. It is not evidence that external trusted-release prerequisites have already been satisfied.

## Decision rule

The v7.0.2 transition may be authorized only when:

1. the canonical matrix remains 28/28 verified, 0 partial, 0 blocked;
2. HLS-C013's source/CI conclusions remain applicable because later changes are governance/documentation only;
3. HLS-C015 removed the live version/readiness instruction contradiction;
4. no post-C015 prerequisite workflow has failed and main has not unexpectedly moved before the decision change;
5. the readiness PR changes no executable release gate and receives an exact-head review;
6. the final merge SHA is separately frozen and receives four fresh successful push/main/exact-SHA prerequisite conclusions before any formal dispatch.

## Proposed canonical transition

When the rule above is satisfied, the narrow internally consistent transition is:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_specialties_verified`

No other feature entry, verification statement, product version or generated-from reference should change as part of the readiness decision.

## External boundary remains unchanged

Even after the canonical transition, formal release remains fail-closed on the dedicated Windows x64 `hls-release` runner, fixed `E:\h` lifecycle environment, real Edge/Firefox, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, candidate-bound visual/performance/MSI/rollback evidence, draft asset digest verification and explicit operator publication choice.

## Post-merge ownership

HLS-C016 owns the final exact-SHA readiness verification and external-gate handoff. It must reject use of predecessor-SHA successes as release evidence. Only the accepted C014 merge SHA may be treated as the prospective formal-release source after its own four prerequisite workflows succeed.
