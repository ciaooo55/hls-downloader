# HLS-C012 — Durable coordination state synchronization

## Acceptance criteria

1. `docs/coordination/tasks.json` records the actual PR/head/merge/review state for HLS-C009, HLS-C010 and HLS-C011 and the current HLS-C006/HLS-C008 ownership.
2. `handoff.md` names the actual active tasks/workers instead of routing new participants to already-completed security work.
3. `docs/manger.log` records the PR #64 active-FAIL process deviation, C009-C011 security closeout and the current route.
4. Machine-readable state stays valid and active/unfinished counts reflect the real coordination state.
5. This task changes coordination data only; it cannot modify product code, release workflows, release assets or `release_ready`.

## Assignment

- Priority: `P2`
- Owner: `worker-0`
- Role: coordinator + worker
- Branch: `coord/hls-c012-state-sync`
- Base: `main@6b5f596325a4b00758ccd062b92eaeb80b0256ee`
- Canonical registry: Issue #39

## Reconciled security state

### HLS-C009

- PR: #68
- Accepted exact head: `6623b534b3b298a237b4f8feea7ade286f9827fe`
- Merge: `acb6969bdeaaa1a7b96b30d5daa06772e0a35de9`
- Result: PASS
- Durable outcome: automatic update signer authorization is project-owned and source-controlled rather than accepting any Windows-trusted signer.

### HLS-C010

- PR: #67
- Accepted exact head: `047b7815a52de6008d9e281d895ea589aa5c0376`
- Merge: `125d146ad10971628019f02a688f0f27cf9468ac`
- Result: PASS
- Durable outcome: configured/client Core TCP addresses and the actual pre-bound server listener are all loopback-only; the named-pipe path remains unchanged.

### HLS-C011

- PR: #69
- Accepted exact head: `05ad6e52da0641cbdf73479dde3be8fe6e4015af`
- Merge: `6b5f596325a4b00758ccd062b92eaeb80b0256ee`
- Source result: PASS
- Exact-head v7 CI #521 and Candidate #169 passed before merge.
- Merged-main checkpoint: v7 CI #522, Maintenance Security #65 and Rust Security #39 passed; Candidate #170 remains in progress at this checkpoint. Therefore C011 is intentionally `post_merge_verification`, not `done`.

## Current documentation route

- HLS-C006: worker-0, PR #70, exact head `97e59b14d646e2dd57ca3017b166c507fabdc8f3`, frozen for independent worker-1 audit.
- HLS-C008: assigned to worker-1 on `docs/hls-c008-release-doc-drift`; current-facing v7.0.1 wording is being separated from historical 7.0.1 evidence and active 7.0.2 guidance.
- HLS-C013: planned final release-readiness re-audit after C006/C008/C012 closeout.

## Process deviation retained

PR #64/HLS-C003 was merged while worker-1 had a durable factual FAIL about a stale Dependabot queue statement. HLS-C004/PR #65 corrected the statement and the operating rule is now explicit: an active FAIL must be resolved or explicitly rebutted before merge even when GitHub cannot represent `REQUEST_CHANGES` because sessions share one account.

## Commit plan

1. synchronize machine-readable task state;
2. refresh fast-entry handoff;
3. add this evidence log;
4. append manager-log closeout/current-route entries after C006/C008/C011 settle and perform one final base/state refresh before opening or merging C012.

C012 should not merge while its recorded pending states are still changing. It is deliberately prepared early and finalized late so the durable state lands once rather than producing a series of stale coordination commits on `main`.
