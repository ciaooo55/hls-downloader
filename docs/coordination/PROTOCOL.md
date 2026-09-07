# Multi-agent operating protocol

This document is the human-readable companion to `tasks.json`. `handoff.md` remains the fastest join index.

## Join

1. Read `handoff.md`, then `tasks.json`.
2. Declare in role issue #40.
3. Use the coordinator-assigned stable numeric identity.
4. Claim the highest-priority eligible task in #39 unless the coordinator has already assigned one.

## Work

- One task ID owns one primary worker branch.
- Worker log: `docs/worker-logs/<task-id>.md` with acceptance criteria at the top.
- Material scope changes use a new task ID/log; handoffs append to the existing task log.
- Helpers branch from the owning worker branch and merge back there before audit.
- Keep unfinished task state synchronized in `tasks.json` and GitHub coordination threads.
- While unfinished work is assigned, heartbeat in #41 every 3 minutes. More than 5 minutes stale permits reassignment.

## Review

- Worker submits branch/PR plus acceptance evidence to audit queue #45.
- Auditor re-fetches code/diff/tests/workflows independently.
- PASS requires every coordinator acceptance criterion. CI green is evidence, not automatic acceptance.
- FAIL includes findings and required fixes; worker immediately returns to that task before starting unrelated work.
- Coordinator independently accepts auditor work and records final route/merge decision in `docs/manger.log` / #44.

## Idle behavior

- Worker: claim highest-priority eligible unowned task; if none, help an active worker through a child branch.
- Auditor: interruptible README accuracy + bug audit (#50), but immediately switch to new audit requests.
- Coordinator: keep backlog larger than staffing (normally >=2x unfinished tasks vs active workers), refine architecture/acceptance criteria, and self-assign small tasks if no other participant exists.

## Cross-project work

Use visitor issue #42. Request must state source project/location, source task ID, requirement, acceptance criteria, priority, and return channel. Local coordinator evaluates and creates a local task. Result is returned to the caller; caller's auditor verifies it.

## Durable state rule

Do not leave a decision only in chat. Use at least one durable repository surface: commit, PR, Issue, `docs/architecture/`, manager/worker log, or machine-readable `handoff.md`/`tasks.json`.