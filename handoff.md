---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: audit/hls-c003-release-readiness
coordinator: worker-0
auditor: worker-0
workers:
  - worker-0
active_task: HLS-C003
status: formal-release-readiness-triage
last_updated: 2026-09-08T11:20:00+08:00
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
- HLS-C002 independently accepted product PR #38 at exact head `8b5161eaf5bddeab061f714e5e8a88fa1e42f8ec`; PR #38 merged as `884f628eb264bb63ba7a093c67e700e3bf3d3024`.
- HLS-C007 fixed stale v7.0.1 assumptions in the executable MSI lifecycle gate. PR #63 passed current-head v7 CI run 481 and Candidate Package run 129, was independently accepted at head `e3761362625ef017c1e842293ca36ae07d4b3842`, and merged as `c6779fd1017bb8f7378eec7d0ba5cd1e5f079dd1`.
- Formal release is still **not** authorized: canonical `feature-parity.json` remains `release_ready=false`; trusted signing/runner/browser/performance/MSI/rollback gates remain mandatory.
- HLS-C003 is now active on `audit/hls-c003-release-readiness`. Its goal is to enumerate every remaining formal-release prerequisite from source, split repository-code defects from external infrastructure/operator blockers, and recommend the shortest safe route without weakening gates.
- HLS-C004/HLS-C005/HLS-C006 remain planned, keeping the unfinished backlog above the 2x-active-worker minimum.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues work. A returning participant must declare its role again in Issue #40 before resuming.
