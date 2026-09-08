---
visitor_issue: 42
role_declaration_issue: 40
heartbeat_issue: 41
task_registry_issue: 39
schema_version: 1
repository: ciaooo55/hls-downloader
default_branch: main
coordination_branch: coord/hls-c012-state-sync
coordinator: worker-0
auditor: worker-0
workers:
  - worker-0
  - worker-1
active_tasks:
  - HLS-C006
  - HLS-C008
  - HLS-C011
  - HLS-C012
primary_active_task: HLS-C006
next_priority_task: HLS-C013
status: post-hardening-documentation-and-coordination-closeout
last_updated: 2026-09-08T15:12:00+08:00
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

## Cross-project coordination

Issue #42 is the visitor area. External project coordinators must provide their project location, task ID, concrete request, acceptance criteria, priority, and return channel. The local coordinator evaluates the route, creates a local task, and reports the result back to the requester; the requesting project performs its own audit.

## Current project state

- Active product line: v7.0.2 iteration. Canonical `artifacts/v7-productization/feature-parity.json` remains `release_ready=false`; no current coordination task authorizes formal publishing.
- HLS-C001 coordination bootstrap, HLS-C002 exact-SHA release workflow contract, HLS-C007 MSI lifecycle version binding, HLS-C003 formal-release triage, HLS-C004 dependency triage and HLS-C005 cross-cutting contract audit are complete.
- Process correction from HLS-C003 remains durable: PR #64 merged while worker-1 had an unresolved factual FAIL about the Dependabot queue. HLS-C004/PR #65 repaired that stale statement. Future merges must resolve or explicitly rebut every durable active FAIL before merge.
- HLS-C009 closed the updater signer-ownership gap in PR #68. Runtime automatic installation now requires the formal project signer or a source-controlled rollover identity in addition to SHA-256, WinVerifyTrust and MSI identity checks. PR #68 merged as `acb6969bdeaaa1a7b96b30d5daa06772e0a35de9`; merged-main required workflows later passed.
- HLS-C010 closed the optional Core TCP exposure in PR #67. Configured/client addresses and the actual pre-bound server listener must be loopback; the Windows named-pipe security path was not weakened. PR #67 merged as `125d146ad10971628019f02a688f0f27cf9468ac`; merged-main v7 CI #515, Candidate #163, Maintenance Security and Rust Security passed.
- HLS-C011 closed cross-origin custom replay-header leakage in PR #69. The exact accepted head `05ad6e52da0641cbdf73479dde3be8fe6e4015af` passed v7 CI #521 and Candidate #169 and merged as `6b5f596325a4b00758ccd062b92eaeb80b0256ee`. On that merged-main SHA, v7 CI #522, Maintenance Security #65 and Rust Security #39 passed; Candidate #170 is still the only outstanding post-merge verification at this checkpoint. Do not mark C011 done until it succeeds.
- `worker-0` owns HLS-C006 on `docs/hls-c006-readme-accuracy`; PR #70 is frozen at `97e59b14d646e2dd57ca3017b166c507fabdc8f3` for independent worker-1 audit. It distinguishes the real published `v7.0.1-candidate.1` test build from the active 7.0.2 source/formal-package contract and leaves `release_ready=false` intact.
- `worker-1` owns HLS-C008 on `docs/hls-c008-release-doc-drift`. Its scope is current-facing v7 release/branch/install documentation: preserve historical 7.0.1 measurements as history while correcting stale current guidance to the active 7.0.2 contract. It must not edit release workflows or imply release authorization.
- `worker-0` owns HLS-C012 on `coord/hls-c012-state-sync`. This branch synchronizes `tasks.json`, this handoff and `docs/manger.log` with the already-merged security work. It is coordination-only and should receive a final refresh after C006/C008/C011 settle before merge.
- HLS-C013 is the planned final post-hardening release-readiness re-audit. It runs only after C006/C008/C012 closeout, freezes the resulting `main` SHA, requires all four exact-SHA push workflows, keeps `release_ready=false`, and recommends either external trusted-release gates or an actionable source fix. It never publishes.

## Durable coordination files

- Manager log: `docs/manger.log`
- Machine-readable tasks: `docs/coordination/tasks.json`
- Dependency triage: `docs/coordination/dependency-triage.json`
- Worker logs: `docs/worker-logs/`
- Coordination architecture: `docs/architecture/coordination-protocol.md`
- Current project plan: `docs/architecture/project-plan.md`
- Formal release boundary: `docs/architecture/formal-release-readiness.md`
- Cross-cutting audit: `docs/architecture/v7-contract-audit.md`

## Recovery rule

If a participant with unfinished assigned work has no heartbeat for more than 5 minutes, treat it as lost only after confirming the heartbeat timestamp from GitHub rather than from a truncated/paginated view. The coordinator records the event in `docs/manger.log`, returns or reassigns the task in the registry, and continues work. A returning participant must declare its role again in Issue #40 before resuming.
