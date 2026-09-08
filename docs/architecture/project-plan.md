# Current execution plan — v7.0.2

This plan complements the existing `docs/v7-refinement-plan.md` and `docs/v7-iteration-log.md`. It is intentionally narrow: it describes the next route from the repository state observed during coordination bootstrap, not a rewrite of the completed v7 architecture work.

## Current baseline

- Product iteration: v7.0.2.
- Existing v7 architecture/refinement documents already describe the major module boundaries and previous hardening work.
- Formal release readiness remains gated; candidate availability alone is not sufficient to publish a formal release.
- HLS-C002 accepted and merged the exact-SHA four-workflow formal-release security contract through PR #38.
- HLS-C007 accepted and merged the canonical v7.0.2 MSI lifecycle version contract through PR #63.
- Canonical `feature-parity.json` still declares `release_ready=false`; no coordination task is authorized to change that merely because CI or a candidate build succeeds.
- Multiple Dependabot major-version PRs remain open and should be treated as compatibility work, not merged as a single bulk update by default.

## Route principles

1. Preserve the current v7 architectural boundaries unless a verified defect requires change.
2. Close release-integrity gaps before optional dependency churn or cosmetic work.
3. Separate external release infrastructure blockers from source-code blockers.
4. Prefer small, independently reviewable tasks and multiple meaningful commits.
5. Keep `release_ready=false` semantics until formal gates are actually satisfied and the existing release-readiness policy explicitly authorizes changing it.
6. Use the coordination registry as the operational source of truth; use existing v7 docs as historical/product context.
7. During formal-release preparation, treat the final frozen `main` SHA as the releasable unit: after the final reviewed merge, avoid unrelated `main` movement until all exact-SHA prerequisite workflows finish.

## Phase 0 — Collaboration bootstrap (P0, complete)

Task: `HLS-C001`.

The shared task database, role/heartbeat/visitor channels, manager log, worker-log convention, handoff entrypoint, and project plan were accepted and merged through PR #60.

## Phase 1 — Release security gate audit (P0, complete)

Task: `HLS-C002`.

PR #38 was independently audited against the four-workflow exact-SHA contract. The accepted implementation requires canonical workflow name + path, push event, main branch and exact SHA, and ensures every `main` commit receives prerequisite workflow conclusions. It merged as `884f628eb264bb63ba7a093c67e700e3bf3d3024`.

## Phase 1.5 — MSI lifecycle version drift (P0, complete)

Task: `HLS-C007`.

The active v7.0.2 line was still checked against executable literal v7.0.1 assumptions. PR #63 centralized the candidate version contract on canonical feature-parity metadata, preserved the public v7.0.0 upgrade baseline and all lifecycle safety gates, extended non-secret drift validation, passed current-head v7 CI and Candidate Package checks, and merged as `c6779fd1017bb8f7378eec7d0ba5cd1e5f079dd1`.

## Phase 2 — Formal release readiness triage (P1, active)

Task: `HLS-C003`.

Classify every remaining prerequisite as one of:

- repository source defect,
- CI/release-gate defect,
- trusted-runner/signing/browser/environment prerequisite,
- operator/manual approval prerequisite.

Current route evidence shows the formal workflow already requires a dedicated self-hosted Windows x64 `hls-release` runner, fixed `E:\h` lifecycle environment, real Edge/Firefox evidence, Authenticode signing/timestamp trust, four successful exact-SHA main-push workflows, frozen-main rechecks, staged asset checksums/SBOM/evidence, draft upload digest verification, and explicit publish authorization.

The HLS-C002 and HLS-C007 source defects are closed. External runner/signing/browser/environment requirements are intentionally not solvable by weakening repository code. HLS-C003 must confirm no additional reproducible source blocker, document the trust boundary, and leave `release_ready=false` unchanged.

Shortest safe route after HLS-C003: finish reviewed repository work, merge the final required PR, freeze the resulting `main` SHA, allow all four exact-SHA prerequisite push workflows for that SHA to succeed, then let an authorized operator use the trusted release machine to dispatch the formal workflow. Any main movement invalidates that prospective release SHA and requires fresh prerequisite conclusions.

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

- A failed exact-SHA prerequisite on the final frozen main SHA is investigated before formal release, but a cancelled obsolete SHA after main moves is not misclassified as a product regression.
- A release runner/signing blocker that cannot be solved in repository code becomes an external blocker rather than consuming worker time or weakening gates.
- A critical reproducible updater or persistence bug discovered during audit becomes a new P0/P1 task and is inserted ahead of README work.
- If more workers join, the coordinator expands the backlog so unfinished planned tasks remain at least twice active worker count, except near genuine closeout.

Every route change is recorded in `docs/manger.log` and reflected in `docs/coordination/tasks.json`.
