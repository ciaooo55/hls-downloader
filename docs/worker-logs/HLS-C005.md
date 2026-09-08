# HLS-C005 — Repository-wide bug and contract audit

## Acceptance criteria

1. Audit release/update trust, IPC boundaries, persistence, and browser-extension/native handoff contracts after the v7.0.2 changes.
2. Findings must be reproducible from current source, tests, CI, or exact runtime evidence; speculative concerns are not treated as defects.
3. Each confirmed defect receives its own prioritized task ID with narrow acceptance criteria before implementation.
4. Do not bundle unrelated fixes into this audit branch.

## Assignment

- Priority: P1
- Owner: `worker-0`
- Roles: coordinator + auditor + worker
- Branch: `audit/hls-c005-contract-audit`
- Depends on: HLS-C002, HLS-C007
- Concurrent work: `worker-1` owns HLS-C004 dependency triage.

## Audit order

1. updater/download/install trust chain and version/source binding;
2. Rust Core / Compose / Native Messaging IPC framing and authentication boundaries;
3. SQLite durability, checkpoint/restart and task-state handoff;
4. extension request-context replay, handoff ownership and retry/fallback semantics;
5. release/install identity continuity where it overlaps update trust.

## Rules

- Prefer existing tests and contract validators before proposing new code.
- Reproduce an invariant violation from current `main` before opening a fix task.
- If no defect is found in a boundary, record the evidence instead of inventing work.
- Any implementation fix must use a separate task branch/PR and independent review.

## Status

`in_progress`: audit branch initialized; source inspection starts with update trust and IPC boundaries.
