# Multi-agent coordination protocol

This document defines how ChatGPT participants cooperate in `ciaooo55/hls-downloader`. Product architecture remains documented in the existing v7 design files; this document only governs collaborative execution.

## Canonical channels

- Shared task registry: Issue #39
- Role declaration: Issue #40
- Heartbeat: Issue #41
- Cross-project visitor area: Issue #42
- Machine-readable state: `docs/coordination/tasks.json`
- Fast handoff entrypoint: `handoff.md`
- Coordinator log: `docs/manger.log`
- Worker logs: `docs/worker-logs/<task-id>.md`

## Roles

### Coordinator

The coordinator owns route selection, architecture-level planning, task decomposition, priorities, assignment, acceptance criteria, worker numbering, and durable progress logging. The coordinator keeps at least twice the active-worker count in unfinished planned work unless the project is genuinely closing out.

If no independent worker or auditor exists, the coordinator does not freeze. It explicitly assigns itself the worker task, implements on a task branch, then switches hats and checks the result against the predeclared acceptance criteria while clearly labeling the review as single-participant fallback rather than independent review.

### Ordinary worker

A worker claims or receives one narrow task ID, creates a task branch, and maintains `docs/worker-logs/<task-id>.md`. A worker should not accumulate many unrelated task IDs at once. If scope materially changes, the coordinator updates the registry and acceptance criteria; the worker records the changed scope in a new log file when the change becomes a new task.

After completion and accepted review, the branch is merged to `main` and the worker returns to idle. Idle workers immediately scan the task registry for the highest-priority compatible task. If none exists, they may help another worker through a child branch or do interruptible README/bug-audit work.

### Auditor

The auditor verifies acceptance criteria independently. A claim such as “tests pass” or “feature complete” is not evidence by itself. Review should inspect the actual diff, source contracts, workflow conclusions, and relevant artifacts. Failure produces an actionable review report and returns the task to active work.

When no review is waiting, the auditor performs low-priority repository bug review and refines `README.md` from verified project state. New audit work interrupts these background tasks.

## Task lifecycle

Allowed task states in `docs/coordination/tasks.json`:

- `planned`: available to claim when dependencies permit.
- `in_progress`: assigned work is being executed.
- `review`: implementation is complete and waiting on acceptance.
- `blocked`: cannot progress until a named dependency or external prerequisite changes.
- `done`: accepted and integrated.
- `cancelled`: intentionally abandoned with a logged reason.

Priority is semantic, not chronological: `P0 > P1 > P2 > P3`. An idle participant may self-claim an unassigned compatible task without approval and mirrors the event in Issue #39.

### Task-ID allocation

Task IDs are unique durable identities, not informal labels. Before allocating a new sequence number, refresh both Issue #39 and `docs/coordination/tasks.json` and choose a number not already claimed by either surface.

If concurrent sessions nevertheless allocate the same ID to different work:

1. stop both dependent routes before further merge activity;
2. preserve any already-merged task meaning under that ID rather than rewriting history;
3. if one duplicate meaning never began or has been completed by the already-merged task, mark it cancelled/superseded in durable evidence;
4. allocate a fresh sequence number for any remaining distinct work;
5. record the collision and resolution in Issue #39, `docs/manger.log`, machine state, handoff, and affected worker logs.

Never keep two live machine task objects with the same ID and never silently redefine an integrated task after merge.

## Branch and PR contract

1. Start from the current intended base, normally `main`.
2. Use a task-specific branch. A helper may branch from the worker branch and merge back to it.
3. Split logically independent changes into multiple commits where useful; commit messages should communicate state to other agents.
4. Worker completion references task ID, worker log, commits, PR, and acceptance evidence.
5. Auditor checks the actual PR diff and required checks.
6. Only accepted work merges to `main`; rejection returns the task to `in_progress` with findings recorded.
7. An exact-head PASS authorizes only that exact PR head. Any subsequent commit invalidates it and requires re-review.
8. Before merge, refresh live role/heartbeat state. A fallback self-review is invalid as an “independent” approval if a separate active auditor is already present.
9. A durable active FAIL/changes-required finding must be resolved or explicitly rebutted before merge even when shared GitHub identity prevents formal `REQUEST_CHANGES` semantics.

## Acceptance criteria contract

Every assigned task must have concrete acceptance criteria before substantial implementation. Criteria belong in the task registry and are copied to the top of the worker log so later scope drift is visible.

If the original task is no longer reasonable, the coordinator may change the route. The decision, reason, replacement task/criteria, and reassignment must be written to `docs/manger.log` and reflected in the registry.

## Heartbeat and disconnect handling

Participants with unfinished assigned work heartbeat in Issue #41 at least every 3 minutes. A participant exceeding 5 minutes without a heartbeat while expected to work may be considered lost only after the coordinator refreshes the live Issue #41 state and records the exact latest timestamp/comment used for the decision. The coordinator records a valid loss event, returns/reassigns unfinished work, and continues. A disconnected coordinator or auditor may be replaced by the user; a participant truly returning after a valid loss classification must redeclare in Issue #40.

Heartbeat reads and comment writes are concurrent distributed state. A refresh can race with a just-created heartbeat. If a later-discovered heartbeat has a creation timestamp **before** the loss/reassignment decision, the loss classification was stale when made and must be retracted. Append a correction to the manager log and machine/handoff state; restore the participant's prior active status and do not require redeclaration solely because of the erroneous timeout.

Before leaving, a participant posts a final heartbeat and checks both the registry and coordination messages for newly assigned work. A participant waiting on an approval may create a small disposable preparation branch, but abandons it if the request is rejected.

## Cross-project work

External coordinators use Issue #42. A request must identify the source project, source task ID, exact need, acceptance criteria, priority, and return channel. This project’s coordinator performs route analysis and creates a local task. On completion, the local coordinator reports the result to the requesting project; the requesting project’s auditor performs the final external audit.

## Communication precedence

Participants prioritize coordination messages over background cleanup. New audit assignments interrupt README polishing or broad bug scans. A rejected worker immediately commits or preserves its safe current state, reads the audit findings, and resumes the rejected task before unrelated work.

A coordinator route override that pauses a dependent task does not destroy the paused branch. The worker should stop at a clean commit, heartbeat the pause, and preserve the branch for later rebase/review unless the coordinator explicitly cancels it.

## Safety against stale state

`handoff.md` is the entrypoint, but `docs/coordination/tasks.json` plus live Issues/PRs are the coordination source of truth. Before claiming, reassigning, auditing, or merging, refresh relevant task/PR/role/heartbeat state so concurrent agents do not duplicate work or merge against stale assumptions.

When durable sources disagree, do not pick the most convenient one silently. Freeze dependent work, reconstruct the sequence from exact GitHub timestamps/commit SHAs, record the discrepancy, and repair all live coordination surfaces in a dedicated task before proceeding with irreversible governance/release decisions.
