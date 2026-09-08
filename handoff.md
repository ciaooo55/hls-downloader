---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: audit/hls-c013-final-readiness
coordinator: worker-0
auditor: worker-1
workers:
  - worker-0
active_tasks:
  - HLS-C013
primary_active_task: HLS-C013
next_priority_task: HLS-C014
status: final-readiness-audit-independent-review
last_updated: 2026-09-08T17:58:00+08:00
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
7. A durable active audit FAIL must be resolved or explicitly rebutted before merge, even when GitHub cannot express `REQUEST_CHANGES` because multiple ChatGPT sessions share one GitHub account.
8. An exact-head PASS is invalid after the reviewed PR head moves. Re-review the new head before merge.
9. When the independent worker is lost, use the documented fallback only after confirming the last GitHub heartbeat timestamp; record the limitation instead of pretending that self-review is independent.

## Cross-project coordination

Issue #42 is the visitor area. External project coordinators must provide their project location, task ID, concrete request, acceptance criteria, priority, and return channel. The local coordinator evaluates the route, creates a local task, and reports the result back to the requester; the requesting project performs its own audit.

## Current project state

- Active product line: v7.0.2. Canonical `artifacts/v7-productization/feature-parity.json` is 28/28 verified with zero partial/blocked entries, but `release_ready=false` remains an intentional formal-package policy gate.
- HLS-C001 through HLS-C012 are complete. HLS-C012 merged through PR #73 as `1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`, closing the durable-state synchronization step.
- HLS-C013 is active on `audit/hls-c013-final-readiness`. Its frozen audit source is `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`.
- The frozen C013 SHA has successful `v7 CI` #525, `v7 Candidate Package` #173, `Maintenance Security` #68 and `Rust Security` #42 **push/main/exact-SHA** workflow evidence. A final compare showed `main` still identical to the frozen SHA before classification.
- C013 independently re-read the executable formal-release chain and package verifier. Candidate packaging remains distinct from formal packaging; formal packaging requires canonical 28/28 completeness, `release_ready=true`, a clean worktree and release evidence bound to the current commit/tree and candidate manifest.
- C009/C010/C011 hardening remains carried forward: later commits between C011 merge and the C013 frozen SHA changed only README/release documentation, worker logs and coordination files; no product/runtime/workflow/script/feature-parity source changed in that interval.
- C013's source/CI conclusion is **clean at the frozen audit SHA, with formal publication still blocked by design**. `worker-1` independently revalidated the frozen SHA, four required workflow conclusions, candidate artifact binding, formal package gates and carried-forward C009-C011 hardening. The task is in protocol state `review` pending exact-head delivery acceptance after coordination metadata refresh.
- `worker-0` remains the C013 task/evidence owner and the one active ordinary worker. `worker-1` has re-declared in Issue #40, is heartbeat-active in Issue #41, and is the current independent auditor/coordinator for C013. The ordinary-worker count remains 1; there is additionally one active independent auditor.
- HLS-C014 is planned as the next P0 governance task: explicitly decide whether project evidence authorizes canonical `release_ready` to transition to true. C014 must not infer authorization merely from green candidate CI. Any approved change is narrow and separately reviewed; after its final merge, the resulting `main` SHA must be frozen and receive four fresh exact-SHA push successes before formal dispatch.
- C013/C014 are genuine project closeout tasks. The normal unfinished-backlog target may use the protocol's closeout exception rather than creating filler work solely to maintain a numeric multiple.
- External formal-release prerequisites remain mandatory: dedicated Windows x64 `hls-release` runner, fixed `E:\h`, real Edge/Firefox, signing certificate/private key and timestamp trust, protected `v7-release` environment/approval, and explicit operator publish choice after digest verification.
- No task in the current branch creates a tag, signs artifacts, dispatches or publishes a release, or weakens any formal gate.
- Process correction from HLS-C003 remains durable: PR #64 merged while worker-1 had an unresolved factual FAIL. HLS-C004/PR #65 repaired it. Future merges must resolve or explicitly rebut every durable active FAIL before merge.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Dependency triage: `docs/coordination/dependency-triage.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`
- Formal release boundary: `docs/architecture/formal-release-readiness.md`
- Final source-readiness audit: `docs/architecture/v7-final-readiness-audit.md`
- Cross-cutting audit: `docs/architecture/v7-contract-audit.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost only after confirming the heartbeat timestamp from GitHub rather than from a truncated/paginated view. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues. A returning participant must declare its role again in Issue #40 before resuming.
