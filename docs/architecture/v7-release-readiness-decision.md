# v7.0.2 canonical readiness decision

## Purpose

HLS-C014 decides whether the repository may move canonical v7.0.2 readiness from iteration mode into the already-existing formal validation/release pipeline. This is a source/governance decision only. It does not tag, sign, dispatch, upload or publish a release.

## Inputs

- HLS-C013 / PR #75: frozen-source audit passed; canonical matrix 28/28 verified with zero partial/blocked entries; all four required exact-SHA main-push prerequisite workflows succeeded for the audited SHA; no new deterministic product/release-workflow blocker was reproduced.
- HLS-C015 / PR #78: live governance instructions were reconciled to v7.0.2 without changing product/runtime/workflow/build-script/feature-parity source. Root `AGENTS.md` now treats canonical feature-parity metadata as the version/readiness truth and explicitly separates candidate CI from publication authorization.
- Post-C015 baseline `main@d37e8cac666c3ebd7f3c8ffa326a15321ef76185`: v7 CI #527, v7 Candidate Package #175, Maintenance Security #70 and Rust Security #44 all completed successfully as `push/main/exact-SHA` workflows, and a final comparison showed main remained identical to that SHA before the decision transition.
- Executable formal gate: formal packaging still requires canonical completeness, `release_ready=true`, clean worktree and release evidence; the release workflow additionally requires exact-SHA prerequisite workflows, frozen current main, trusted Windows runner, real browsers, MSI/rollback/performance evidence, signing/trust, draft asset digest verification and explicit publish choice.

## Repository precedent

The v7.0.1 canonical ready state used `release_ready=true` with `audit_state=v7_0_1_release_specialties_verified`. The `build: start v7.0.2 iteration` commit intentionally reset those fields together to `false` and `v7_0_2_iteration_in_progress` while incrementing the product version.

The established meaning is therefore a repository lifecycle state: `release_ready=true` permits the formal package path to proceed **if and only if** the executable formal gates also pass. It is not evidence that external trusted-release prerequisites have already been satisfied.

## Decision rule and result

The v7.0.2 transition is authorized only because all repository-level decision conditions were revalidated:

1. the canonical matrix remains 28/28 verified, 0 partial, 0 blocked;
2. HLS-C013's source/CI conclusions remain applicable because later product-facing changes were governance/documentation only until the readiness metadata transition;
3. HLS-C015 removed the live version/readiness instruction contradiction;
4. all four post-C015 prerequisite workflows succeeded on exact `main@d37e8cac...`, and main remained unchanged through the decision checkpoint;
5. the readiness change modifies no executable release gate;
6. the final C014 PR head must still receive applicable pull-request checks and exact-head fallback review before merge;
7. the eventual C014 merge SHA must be separately frozen and receive four fresh successful `push/main/exact-SHA` prerequisite conclusions before any formal dispatch.

**Decision: AUTHORIZED_FOR_CANONICAL_READINESS_TRANSITION, PENDING FINAL PR-HEAD CHECKS/REVIEW.**

## Canonical transition

Commit `73cde20466d5218b55f1869ded79a08886bb6bc2` performs the narrow internally consistent lifecycle transition:

- `release_ready: false -> true`
- `audit_state: v7_0_2_iteration_in_progress -> v7_0_2_release_specialties_verified`

No feature entry, verification statement, product version, summary or generated-from reference changes. Product/runtime source, workflows and build scripts are also unchanged.

This metadata transition is not itself merge authorization. `feature-parity.json` is included in both v7 CI and v7 Candidate Package pull-request path filters, so final PR #79 must pass the applicable workflows on its exact final head. Any later head movement invalidates an earlier review/check set.

## External boundary remains unchanged

Even after the canonical transition, formal release remains fail-closed on the dedicated Windows x64 `hls-release` runner, fixed `E:\h` lifecycle environment, real Edge/Firefox, signing tool and trusted project certificate/private key, timestamp/network access, protected `v7-release` environment/approval, candidate-bound visual/performance/MSI/rollback evidence, draft asset digest verification and explicit operator publication choice.

C014 performs no tag, formal dispatch, signing, release creation or publication.

## Post-merge ownership

HLS-C016 owns the final exact-SHA readiness verification and external-gate handoff. It must reject use of predecessor-SHA successes as release evidence. Only the accepted C014 **merge SHA** may be treated as the prospective formal-release source after its own four prerequisite workflows succeed and remote `main` is still identical.

HLS-C016 evidence should remain in Issues and, if needed, an unmerged evidence branch while that SHA is intended for formal release. Merging post-freeze coordination evidence would create a different main SHA and invalidate the workflow set being handed off.
