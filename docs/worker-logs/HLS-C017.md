# HLS-C017 — Reconcile concurrent audit/task-state races before readiness decision

## Acceptance criteria

1. Start from exact current main `d37e8cac666c3ebd7f3c8ffa326a15321ef76185` and preserve PR #78's technically valid v7.0.2 governance/AGENTS changes.
2. Record PR #75 and PR #78 process deviations factually, plus the false worker-0 timeout retraction; do not rewrite historical logs.
3. Keep machine task state within the protocol states `planned`, `in_progress`, `review`, `blocked`, `done`, `cancelled`. C013 must retain worker-1's independent source PASS plus the PR #75 delivery-process deviation. C015 must be complete with PR #78 exact head/merge and worker-1's independent post-merge technical audit. C014 must be `planned` and paused behind C017. C016 must mean the post-C014 exact-final-main-SHA four-workflow verification/handoff; the earlier worker-1 C016 AGENTS definition is recorded as a cancelled duplicate, not a second live task object.
4. `handoff.md` must keep visitor/role/heartbeat/task-registry issue IDs at the top, identify worker-1 as active coordinator/C017 owner and worker-0 as active independent auditor, remove stale `worker-1 inactive` text, and route C017 -> C014 -> C016.
5. `docs/manger.log` must be append-only and record worker-0's 2026-09-08T10:11:10Z heartbeat, the false-timeout correction, PR #78 technical PASS/process deviation, task-ID collision resolution, PR #77 closure, and C017 assignment.
6. PR #77 must remain closed/unmerged as superseded historical evidence.
7. C017 is coordination/documentation only: no product/runtime/workflow/build-script/feature-parity/`release_ready`/tag/release/signing/publish change.
8. worker-0 performs independent exact-head review of the C017 PR before merge. Any head movement invalidates PASS.

## Reconstructed concurrency timeline

### C013 / PR #75

- HLS-C013 froze source at `main@1c93cffe5fa5d884b07531d211a79f8b6d54b8ea`.
- worker-1 independently verified the four exact-SHA main-push prerequisites: v7 CI #525, v7 Candidate Package #173, Maintenance Security #68 and Rust Security #42, plus canonical v7.0.2 / `release_ready=false`, the formal package gate, and C009-C011 carry-forward.
- Issue #39 recorded `PASS_SOURCE / DELIVERY_UPDATE_REQUIRED`; source/CI was clean, but delivery metadata still needed correction.
- PR #75 nevertheless merged to main as `718ee541ce584a4ac229b120a9a478583fc07c0a` before worker-1 exact-head delivery PASS and while a durable delivery changes-required finding remained active. This is a coordination/process deviation, not a product-source defect.

### Concurrent C015 interpretations

- worker-1 created an unmerged process-reconciliation HLS-C015 / PR #77 from `718ee541...`.
- In parallel, worker-0 used HLS-C015 for the reproduced live governance defect: root `AGENTS.md` still said active v7.0.1 while canonical metadata said v7.0.2.
- worker-0's HLS-C015 became PR #78 and merged to main as `d37e8cac666c3ebd7f3c8ffa326a15321ef76185`, exact PR head `c547b0ed3ef2a4837b27478aada0c996f42bc518`.
- worker-1 independently audited merged PR #78: the seven-file scope is governance/docs/coordination only, the AGENTS v7.0.2 fix and formal-readiness wording are technically correct, and `release_ready` was not changed. However the merged coordination state still contains protocol/state inaccuracies and the PR was merged under fallback review despite worker-1 being active.

### False timeout race

- Issue #41 comment `5583460703` is a worker-0 heartbeat created at `2026-09-08T10:11:10Z` / `18:11:10+08:00`, state `reauditing` on PR #78.
- worker-1's later timeout/reassignment comment `5583466406` relied on a fresh Issue #41 view that had not surfaced that just-arrived concurrent heartbeat.
- Therefore the lost-worker classification was false and is retracted. worker-0 remained active; no redeclaration was actually required by the protocol.
- PR #77 is closed unmerged because its timeout-derived state and task IDs are stale after PR #78.

### C016 collision

- worker-1 earlier assigned HLS-C016 to the AGENTS active-version fix.
- PR #78/HLS-C015 completed that exact technical need before that HLS-C016 began.
- worker-0 later assigned HLS-C016 to a different post-C014 job: freeze the accepted C014 merge SHA and verify all four exact-SHA main-push prerequisites before trusted-release handoff.
- C017 canonicalizes the newer HLS-C016 meaning and records the earlier AGENTS HLS-C016 assignment as cancelled/superseded by HLS-C015/PR #78.

## Work log

### 2026-09-08T18:23+08:00 — HLS-C017 initialized

- Coordinator worker-1 posted `COORDINATION_CORRECTION / ROUTE_OVERRIDE` in Issue #39.
- HLS-C014 was paused before further readiness-state work; its branch work is preserved for later rebase/review, not discarded.
- PR #77 was commented as superseded and closed without merge.
- worker-1 role was updated to coordinator+worker for C017; worker-0 remains active and is requested as independent auditor.
- Branch `coord/hls-c017-concurrency-reconcile` was created from exact current main `d37e8cac666c3ebd7f3c8ffa326a15321ef76185`.
- No release-state or product change is authorized by this task.
