# HLS-C012 — Durable coordination state synchronization

## Acceptance criteria

1. `docs/coordination/tasks.json` records actual C009/C010/C011 and C006/C008 completion evidence.
2. `handoff.md` routes new participants to actual active work rather than completed tasks.
3. `docs/manger.log` retains the PR #64 active-FAIL process deviation and records hardening/documentation closeout, worker timeout and the C012→C013 route.
4. Machine-readable state reflects one active worker and exactly two unfinished tasks (C012/C013), satisfying the 2x-active-worker target.
5. Coordination-only change; no product code, release workflow/gate, asset, tag/release or `release_ready` modification.

## Assignment

- Priority: `P2`
- Owner: `worker-0`
- Role: coordinator + worker + auditor fallback
- Branch: `coord/hls-c012-state-sync`
- Original branch base: `6b5f596325a4b00758ccd062b92eaeb80b0256ee`
- Current PR target at finalization: `main@7e40cd509a5cc49bc887d1814199b9f5a810326c`
- Canonical registry: Issue #39

The branch was intentionally prepared early and finalized late. C006/C008 changed different files on main, so C012 preserves its own commit history and is reviewed as a PR against current main instead of force-resetting away its coordination commits.

## Reconciled security state

### HLS-C009

- PR #68
- Accepted head `6623b534b3b298a237b4f8feea7ade286f9827fe`
- Merge `acb6969bdeaaa1a7b96b30d5daa06772e0a35de9`
- PASS; merged-main four required workflows passed.
- Durable outcome: automatic update signer authorization is project-owned/source-controlled rather than accepting any Windows-trusted signer.

### HLS-C010

- PR #67
- Accepted head `047b7815a52de6008d9e281d895ea589aa5c0376`
- Merge `125d146ad10971628019f02a688f0f27cf9468ac`
- PASS; merged-main v7 CI #515, Candidate #163, Maintenance Security and Rust Security passed.
- Durable outcome: configured/client Core TCP addresses and the actual pre-bound server listener are all loopback-only; named-pipe protections remain unchanged.

### HLS-C011

- PR #69
- Accepted head `05ad6e52da0641cbdf73479dde3be8fe6e4015af`
- Merge `6b5f596325a4b00758ccd062b92eaeb80b0256ee`
- Exact-head v7 CI #521 and Candidate #169 passed.
- Merged-main v7 CI #522, Candidate #170, Maintenance Security #65 and Rust Security #39 all passed.
- Durable outcome: replay-owned custom headers are removed on cross-origin transitions before an exact target-origin `request_context` may restore its own identity; redirect and multi-hop isolation are covered.

## Documentation closeout

### HLS-C006

- PR #70
- Accepted head `97e59b14d646e2dd57ca3017b166c507fabdc8f3`
- Merge `adf74c30d9936214b060d062f56813c879ca5ee2`
- PASS under documented no-independent-auditor fallback after worker-1 exceeded the heartbeat threshold.
- README now distinguishes the real public historical `v7.0.1-candidate.1` from active 7.0.2/release_ready=false and matches executable candidate/formal build semantics.

### HLS-C008

- PR #72
- Final accepted head `060a51d1e605381de24a017e8f1c4370b06d22d4`
- Merge `7e40cd509a5cc49bc887d1814199b9f5a810326c`
- PASS under the same fallback. Audit was restarted after head movement from `9923173...`; the final extra commit accurately corrected formal release workflow gate ordering.
- Current-facing release/branch/install docs follow canonical 7.0.2 while historical v7.0.1 measurements, hashes and public candidate evidence remain explicitly historical.

## Worker-state reconciliation

Worker-1's last exact heartbeat was `2026-09-08T06:02:08Z`; worker-0 had a later heartbeat at `07:08:24Z`, proving a >66 minute gap and therefore a lost-worker condition under the 5-minute protocol threshold. C008 had zero worker-1 commits when reassigned. Worker-1 remains inactive until it redeclares/heartbeats.

## Process deviation retained

PR #64/HLS-C003 merged while worker-1 had a durable factual FAIL about a stale Dependabot queue statement. HLS-C004/PR #65 corrected the statement. `handoff.md`, tasks state and manager log preserve the operating rule: every active FAIL must be resolved or explicitly rebutted before merge, even when shared-account GitHub limitations prevent a formal independent REQUEST_CHANGES state.

## Final route

At C012 finalization current main is `7e40cd509a5cc49bc887d1814199b9f5a810326c` and has started its own four main-push workflows. Those results are integration evidence only. C012 itself is coordination-only; after its reviewed merge, HLS-C013 must freeze the **new C012 merge SHA** and require that SHA's own v7 CI, v7 Candidate Package, Maintenance Security and Rust Security successes before final source-readiness classification.

Canonical feature parity remains `product_version=7.0.2`, `release_ready=false`. C012 never changes or authorizes a tag, release, publish action, signing bypass or release gate.

## Review state

Ready for exact-head fallback audit. Required checks:

- compare PR diff against current main and confirm exactly four coordination files;
- confirm `docs/coordination/tasks.json` is valid JSON and reports active_workers=1 / unfinished C012+C013=2;
- confirm no current product/release file appears in the diff;
- confirm manager log only appends later closeout/route entries after the existing C010 checkpoint;
- confirm handoff routes directly to C012 then C013;
- do not invent CI success if coordination docs have no PR workflow runs.
