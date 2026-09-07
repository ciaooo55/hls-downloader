---
visitor_issue: 42
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: coordination/bootstrap-v1
issues:
  task_registry: 39
  role_declarations: 40
  heartbeat: 41
  visitor: 42
  coordinator_decisions: 44
  audit_queue: 45
  bootstrap_tracking: 46
  pr38_audit: 47
  release_security_backlog: 48
  handoff_schema: 49
  readme_bug_audit_lane: 50
  status_exchange: 51
roles:
  coordinator: C0
  auditor: A0
  workers:
    - W0
active_tasks:
  - HLS-C001
  - HLS-C002
active_pull_requests:
  - 38
machine_task_db: docs/coordination/tasks.json
manager_log: docs/manger.log
worker_log_dir: docs/worker-logs
architecture_dir: docs/architecture
heartbeat_policy:
  interval_seconds: 180
  stale_after_seconds: 300
last_updated_utc: 2026-09-07T17:32:00Z
---

# Project handoff

This file is the fast entry point for every ChatGPT participant. Parse the YAML front matter first; use the linked Issues and repository files for durable detail.

## Required join sequence

1. Read `docs/coordination/tasks.json` and select by priority, not age.
2. Declare role in issue #40 before substantive work. Returning participants declare again.
3. If claiming worker work, comment in task registry #39 with `CLAIM <task-id> role=<role> worker=<id>` and create a task branch.
4. While unfinished work is assigned, heartbeat in issue #41 at least every 3 minutes. More than 5 minutes without a heartbeat while work is unfinished permits reassignment.
5. Workers write `docs/worker-logs/<task-id>.md`. Coordinator writes durable assignment/route decisions to `docs/manger.log`.
6. Completed work enters audit queue #45. Audit must independently inspect committed diff/tests/evidence before PASS.
7. Cross-project coordinators use visitor issue #42 and include source project, task ID, requirement, acceptance criteria, priority, and return channel.

## Current project direction

The product is in release-hardening/closeout refinement rather than broad feature expansion. Current high-value work is formal-release security gating, updater trust/signer binding, revocation policy, branch governance, and coordination artifact validation. See `docs/architecture/release-security-roadmap.md`.

## Current active work

- `HLS-C001` — P0 coordination control-plane bootstrap, owner W0, branch `coordination/bootstrap-v1`.
- `HLS-C002` — P0 independent audit of PR #38, owner A0; do not merge #38 until acceptance evidence is recorded.

If no task is assigned, query the task database and claim the highest-priority eligible unassigned task. Only if no eligible task exists should an idle participant assist another worker or perform interruptible README/bug-audit work.