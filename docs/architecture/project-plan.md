# Current execution plan — v7.0.2

This plan complements the existing `docs/v7-refinement-plan.md` and `docs/v7-iteration-log.md`. It is intentionally narrow: it describes the next route from the repository state observed during coordination bootstrap, not a rewrite of the completed v7 architecture work.

## Current baseline

- Product iteration: v7.0.2.
- Existing v7 architecture/refinement documents already describe the major module boundaries and previous hardening work.
- Formal release readiness remains gated; candidate availability alone is not sufficient to publish a formal release.
- PR #38 is the active first-party product PR at bootstrap and attempts to require both Maintenance Security and Rust Security exact-SHA results in the formal release path.
- Multiple Dependabot major-version PRs remain open and should be treated as compatibility work, not merged as a single bulk update by default.

## Route principles

1. Preserve the current v7 architectural boundaries unless a verified defect requires change.
2. Close release-integrity gaps before optional dependency churn or cosmetic work.
3. Separate external release infrastructure blockers from source-code blockers.
4. Prefer small, independently reviewable tasks and multiple meaningful commits.
5. Keep `release_ready=false` semantics until formal gates are actually satisfied.
6. Use the coordination registry as the operational source of truth; use existing v7 docs as historical/product context.

## Phase 0 — Collaboration bootstrap (P0)

Task: `HLS-C001`.

Deliver the shared task database, role/heartbeat/visitor channels, manager log, worker-log convention, handoff entrypoint, and this plan. Acceptance is based on the actual branch diff and issue state.

Exit condition: bootstrap PR accepted and merged, task state marked done, then `worker-0` immediately self-claims the highest-priority compatible task.

## Phase 1 — Release security gate audit (P0)

Task: `HLS-C002`.

Independently audit PR #38:

- Verify Maintenance Security and Rust Security produce usable conclusions for every relevant `main` SHA while preserving PR path filtering where intended.
- Verify formal release lookup is bound to the candidate/source commit and cannot accidentally accept a successful run from another SHA.
- Verify `GITHUB_SHA` mismatch fails closed in GitHub Actions while local/manual behavior remains intentionally scoped.
- Inspect workflow run conclusions for PR #38 head.
- Merge only if source inspection and checks satisfy the declared contract; otherwise issue an actionable rejection report.

## Phase 2 — Formal release readiness triage (P1)

Task: `HLS-C003`.

After Phase 1, re-read the formal release workflow and runner documentation and classify every remaining prerequisite as one of:

- source defect,
- CI/release-gate defect,
- trusted-runner/signing infrastructure prerequisite,
- operator/manual approval prerequisite.

The goal is a precise shortest path to a formal v7.0.2 release without weakening signing, installer lifecycle, digest, browser, performance, rollback, or security gates.

## Phase 3 — Dependency maintenance triage (P1)

Task: `HLS-C004`.

Review open Dependabot PRs individually. Major upgrades for Actions, TypeScript, Vitest, Compose/Kotlin ecosystem, rusqlite, windows-sys, and similar components can change runtime/toolchain contracts. Rank by security value and compatibility risk; merge only after project-specific evidence is available.

Security fixes that are already superseded by first-party integrated work should be closed or documented rather than duplicated.

## Phase 4 — Cross-cutting bug audit (P1)

Task: `HLS-C005`.

Audit the highest-risk boundaries after v7.0.2 changes:

- updater and Authenticode trust,
- release/source SHA binding,
- IPC framing and lifecycle,
- SQLite durability and handoff persistence,
- browser/native bridge ownership and retry semantics,
- installer/update/rollback identity continuity.

Only reproducible findings become fix tasks. Each new defect receives a narrow task ID and acceptance criteria before implementation.

## Phase 5 — README refinement (P2)

Task: `HLS-C006`.

The auditor maintains a concise README that teaches:

- what the product is,
- module architecture and data ownership,
- how to build/test at a high level,
- candidate vs formal release distinction,
- current limitations/readiness without overstating release status.

README claims are verified against source and CI state, not copied blindly from old planning text.

## Dynamic replanning

The coordinator may reorder or split tasks when evidence changes. Examples:

- A failed PR #38 security check stays P0 and preempts dependency work.
- A release runner/signing blocker that cannot be solved in repository code becomes `blocked` rather than consuming worker time.
- A critical reproducible updater or persistence bug discovered during audit becomes a new P0/P1 task and is inserted ahead of README work.
- If more workers join, the coordinator expands the backlog so unfinished planned tasks remain at least twice active worker count, except near genuine closeout.

Every route change is recorded in `docs/manger.log` and reflected in `docs/coordination/tasks.json`.
