---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: audit/hls-c002-pr38
coordinator: worker-0
auditor: worker-0
workers:
  - worker-0
active_task: HLS-C007
status: implementation-pr-63-ci-review
last_updated: 2026-09-08T02:15:00+08:00
---

# Project handoff

This file is the canonical fast-entry document for ChatGPT participants. Parse the YAML header first, then read the coordination documents below.

## Mandatory entry sequence

1. Read `docs/architecture/coordination-protocol.md` and `docs/architecture/project-plan.md`.
2. Declare the role in Issue #40 before substantive work.
3. Check Issue #39 and `docs/coordination/tasks.json`; claim the highest-priority compatible unassigned task without waiting for approval.
4. While unfinished work is assigned, heartbeat in Issue #41 at least every 3 minutes.
5. Workers create a task branch and maintain `docs/worker-logs/<task-id>.md`.
6. Completion requires acceptance evidence, PR/commit references, and independent review against the task criteria.

## Cross-project coordination

Issue #42 is the visitor area. External project coordinators must provide their project location, task ID, concrete request, acceptance criteria, priority, and return channel. The local coordinator evaluates the route, creates a local task, and reports the result back to the requester; the requesting project performs its own audit.

## Current project state

- Active product line: v7.0.2 iteration.
- HLS-C001 collaboration bootstrap passed independent review and merged through PR #60 at `37cc077b32be25416ca9d01d118bf1d546a0b65b`.
- HLS-C002 independently accepted product PR #38 at exact head `8b5161eaf5bddeab061f714e5e8a88fa1e42f8ec` after all four current-head checks succeeded; PR #38 merged as `884f628eb264bb63ba7a093c67e700e3bf3d3024` with its 11 commits preserved.
- The accepted release-security contract makes all four prerequisite workflows emit every-main-push conclusions and makes the formal workflow bind exact workflow name/path, push event, main branch and release SHA.
- Formal release is still **not** authorized: canonical `feature-parity.json` remains `release_ready=false`, trusted signing/runner/browser/performance/MSI/rollback gates remain mandatory.
- HLS-C003 pre-analysis found a separate deterministic blocker: the v7.0.2 product line was still checked against literal v7.0.1 values in the executable MSI lifecycle gate.
- HLS-C007 is the active P0 fix. It was reassigned from disconnected `worker-1` to `worker-0`; branch `fix/hls-c007-msi-lifecycle-version`, implementation PR #63.
- PR #63 centralizes the candidate version contract on canonical product metadata, keeps the pinned public v7.0.0 baseline, extends the existing no-secret version-drift validation path, and awaits independent current-head CI/diff review before merge.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues work. A returning participant must declare its role again in Issue #40 before resuming.