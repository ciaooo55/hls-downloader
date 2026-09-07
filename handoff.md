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
active_task: HLS-C002
status: reaudit-pr-38-checks-in-progress
last_updated: 2026-09-08T01:42:00+08:00
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
- HLS-C002 is re-auditing product PR #38 after its head changed during the mandatory pre-merge refresh.
- Superseded PR #38 head: `a99582c76e582210264ebc4e2f2761b6a41714f0`; its PASS is not valid for merge authorization.
- Current PR #38 head: `556722c4397e905dfd352dd60bfa03956d453ebd`.
- Current-head source review is positive and includes the new `.github/workflows/release-v7.yml` exact identity gate; however the four current-head checks are still in progress, so PR #38 is not authorized to merge yet.
- Formal release remains gated; do not mark a formal release ready merely because candidate artifacts exist.
- The task registry is authoritative for coordination status; historical product details remain in the existing v7 architecture/refinement/iteration documents.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues work. A returning participant must declare its role again in Issue #40 before resuming.