# HLS-C015 — Reconcile live v7.0.2 governance instructions

## Acceptance criteria

1. Root `AGENTS.md` identifies v7.0.2 as the active product version and points to canonical feature-parity metadata as the product-version/readiness source of truth.
2. The formal-package instruction preserves fully verified parity, an explicit reviewed `release_ready=true` decision, and visual/performance/installer/rollback gates. Green candidate CI alone must not imply publication authorization.
3. `docs/architecture/formal-release-readiness.md` no longer claims unresolved current-facing v7.0.1 wording remains after HLS-C008/C015; clearly historical v7.0.1 evidence remains allowed.
4. No product/runtime/workflow/build-script/feature-parity/tag/release/publish change.
5. Dedicated PR, exact-head fallback audit, and no invented CI success when documentation-only path filtering yields no PR checks.

## Assignment

- Priority: P0
- Owner: `worker-0`
- Role: coordinator + auditor + worker fallback
- Branch: `docs/hls-c015-live-governance-drift`
- Base: `main@718ee541ce584a4ac229b120a9a478583fc07c0a`
- Source task: HLS-C014 readiness-decision pre-audit

## Reproduced governance drift

Root `AGENTS.md` on the post-C013 main baseline says the only active product version is `7.0.1`, while canonical `artifacts/v7-productization/feature-parity.json` declares `product_version = 7.0.2`. The canonical matrix is 28/28 verified with zero partial/blocked entries and still deliberately carries `release_ready=false`.

The same root instruction is used by the v7 refinement plan as a repository-level packaging constraint. This makes the stale active-version declaration a live governance contradiction rather than harmless historical documentation. C014 therefore paused before making any readiness-state change.

`docs/architecture/formal-release-readiness.md` also still says some current-facing v7.0.1 wording remains. HLS-C008 already reconciled the release/install/branch documentation; after correcting root `AGENTS.md`, that observation should be updated rather than preserved as a false open problem.

## Implementation boundary

C015 only aligns live instructions/documentation with the already-canonical v7.0.2 state. It does not change the canonical feature matrix, `release_ready`, runtime code, build scripts, workflows, tags, releases, signing state or publication state.

The strengthened package instruction continues to fail closed conceptually: formal packaging requires fully verified canonical parity, a separately reviewed `release_ready=true` decision, and the visual/performance/installer/rollback gates. Candidate CI success is explicitly insufficient to authorize publication.

## First review — rejected head

PR #78 was first reviewed at `05f84711e23eb8299600b5fdc751055b3583bf19`. The intended six-file boundary was otherwise narrow and the manager-log patch was append-only, but the review found a factual coordination defect outside that changed-file set: `docs/architecture/project-plan.md` still called HLS-C013 active and HLS-C014 next even though the task registry and handoff had already routed HLS-C015 active with HLS-C014 waiting.

That exact head is **rejected**. This is not waived as harmless documentation because the project plan is explicitly the live coordinator route and the user requires replanning to be reflected there. The finding was recorded in Issue #39.

The fix adds the live project plan to the same task branch, marks C013 complete, describes C015 as active and C014 as waiting, and records that this stale-plan finding itself invalidated the earlier head. Because the head moved, any eventual PASS must re-review the complete new diff from scratch.

## Review checklist

- Diff contains only governance/docs/coordination files.
- `AGENTS.md` says active v7.0.2 and names canonical feature parity as source of truth.
- Formal-package wording does not bypass any executable gate or authorize publication.
- Formal-readiness architecture doc treats historical v7.0.1 evidence as historical rather than an unresolved current-facing defect.
- Project plan matches tasks.json/handoff: C015 active, C014 waiting/resumes next.
- Manager log changes are append-only.
- Task registry keeps one active worker and at least two unfinished tasks (C015 active, C014 waiting/resumes next).
- Any PR head movement invalidates the fallback PASS and requires a fresh review.
