# Collaboration control-plane architecture

## Purpose

This repository may be modified concurrently by multiple ChatGPT sessions. The control plane makes ownership, task priority, acceptance, liveness, and cross-project handoff durable through Git rather than depending on one conversation staying alive.

## State surfaces

### `handoff.md`
Fast machine-readable index. YAML front matter contains canonical issue numbers, role IDs, active tasks, paths, and heartbeat policy. It is intentionally not the full project log.

### `docs/coordination/tasks.json`
Machine-readable task database and scheduling source of truth. Tasks have stable sequence IDs, priority, state, owner, dependencies, acceptance criteria, and GitHub references. Priority beats age when selecting work.

### GitHub coordination Issues
- #39 task registry: human claim/completion event stream.
- #40 role declarations: every new/returning participant announces role and identity.
- #41 heartbeat: operational liveness while unfinished work is assigned.
- #42 visitor area: cross-project coordinator requests and result routing.
- #44 coordinator decisions: route/reassignment/acceptance changes.
- #45 audit queue: completed worker claims awaiting independent verification.

### `docs/manger.log`
Coordinator-owned durable chronology. Records role roster, assignments, acceptance criteria, task-plan changes, lost-worker reassignment, and route decisions. The intentionally requested filename is `manger.log`.

### `docs/worker-logs/<task-id>.md`
Per-task worker history. The acceptance criteria are repeated at the top. Handoffs append to the same task log; a material scope change gets a new task sequence/log.

### `docs/architecture/`
Architecture, threat model, planning, and route documents that let a new worker enter a task without reconstructing context from chat history.

## Roles

### Coordinator
Maintains architecture and task supply, assigns only a small number of active task IDs at once, keeps a larger prioritized backlog, records decisions, and performs final acceptance of auditor work. If no other participant exists, the coordinator self-assigns rather than freezing the project.

### Worker
Claims/receives a task ID, creates a branch, writes the worker log, produces multiple useful commits, and submits the branch/PR to audit. After accepted merge the worker becomes idle and claims the highest-priority eligible task.

### Auditor
Independently checks acceptance criteria using committed code, diffs, tests, workflow evidence, and documentation. A worker statement is not evidence. Failed tasks return with actionable findings. When no audit is pending, the auditor performs interruptible README accuracy and bug review.

One ChatGPT may temporarily hold C0/A0/W0, but implementation, audit, and final coordination decisions must be separated in time and evidence: commit -> re-fetch -> audit -> decision.

## Task lifecycle

`planned -> claimed -> in_progress -> reviewing -> done`

`blocked` can interrupt any execution/review state. A returned audit moves the task from `reviewing` back to `in_progress` with findings recorded in the audit issue and task log.

Workers can self-claim unassigned eligible tasks in #39 without approval. Dependencies must be satisfied and higher-priority eligible work should be preferred.

## Heartbeat and loss recovery

A participant with unfinished assigned work posts to #41 at least every 180 seconds. More than 300 seconds without a heartbeat permits the coordinator to consider that participant lost and reassign unfinished work. Reassignment must update #39, `tasks.json`, and `manger.log`. The returning participant re-declares in #40 before resuming.

A participant leaving an active conversation sends a final heartbeat only after checking for newly assigned work/messages. A speculative helper branch may be started while awaiting a request decision; it is abandoned if the request is rejected.

## Cross-project requests

External project coordinators use #42 and supply their repository/path, source task ID, exact requirement, acceptance criteria, priority, and return channel. C0 evaluates architecture impact and creates a local task if appropriate. When complete, C0 reports back to the caller's stated channel; the caller project's auditor performs final independent audit.

## Concurrency rules

- Prefer separate task branches.
- To assist a worker, branch from that worker branch, then merge back into the worker branch before the task PR enters audit.
- Avoid editing the same file concurrently unless the owning worker explicitly sub-allocates that surface.
- Durable decisions belong in GitHub/logs, not only chat.
- Do not merge a task branch solely because CI is green; acceptance criteria and audit evidence still apply.

## Bootstrap route

During bootstrap C0/A0/W0 activate only HLS-C001 and HLS-C002 while keeping HLS-C003..HLS-C009 planned. This limits work-in-progress while satisfying the requirement for a task pool larger than current staffing.