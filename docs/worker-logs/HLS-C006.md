# HLS-C006 — README accuracy audit

## Acceptance criteria

1. README describes the current architecture, build/release boundaries and release readiness accurately.
2. Current claims are verified against source, repository state and CI/release evidence rather than copied from stale planning text.
3. Quick-start material remains concise and usable on a clean Windows development machine.
4. README must not imply that v7.0.2 is formally published or that an unsigned/ungated release is authorized.

## Assignment

- Priority: `P2`
- Owner: `worker-0`
- Role: coordinator + auditor + worker
- Branch: `docs/hls-c006-readme-accuracy`
- Base: `6b5f596325a4b00758ccd062b92eaeb80b0256ee`
- Source task registry: Issue #39 / `docs/coordination/tasks.json`

## Independent fact audit

### Published build versus active source

GitHub Releases still exposes `v7.0.1-candidate.1` as the latest published public test build. That release is a prerelease and contains Windows candidate installers plus extension/manifest/provenance assets. It is therefore correct to keep the download link, but incorrect to present 7.0.1 as the version to which new active release evidence is bound.

The canonical `artifacts/v7-productization/feature-parity.json` on current `main` declares:

- `product_version = 7.0.2`
- `release_ready = false`
- `audit_state = v7_0_2_iteration_in_progress`

The README previously mixed these states by saying the product was 7.0.2 while also saying new release evidence/artifacts bind to v7.0.1. HLS-C006 separates the historical published candidate from the active 7.0.2 source/release contract.

### Candidate and formal package gates

Current `scripts/build-v7.ps1` verifies module versions against canonical `feature-parity.json` before packaging.

For `-Task candidate`, the script requires the canonical matrix, no blocked features and a clean worktree, but does not require `release_ready=true` or canonical completeness.

For `-Task package`, the script uses `-RequireCanonicalComplete`, `-RequireReleaseReady`, `-RequireCleanWorktree` and current release evidence. The README now describes that executable contract directly instead of keeping a brittle hard-coded feature count.

### Clean-machine onboarding

`build-v7.ps1` fails closed when the pinned JDK/toolchain is unavailable and explicitly directs the operator to `scripts/bootstrap-v7-toolchain.ps1`. README now shows that bootstrap command before component test commands so the first-run path is self-contained without changing any build behavior.

## Scope of the README change

- keep the real `v7.0.1-candidate.1` public test download link;
- mark that published artifact set as the earlier 7.0.1 test line;
- state that active source/current packaging contract is 7.0.2 and formal v7.0.2 publishing is not yet authorized;
- correct the stale architecture sentence that bound new evidence/artifacts to 7.0.1;
- add one clean-machine bootstrap command;
- describe candidate/formal gates from current executable source and remove the hard-coded feature count from prose.

No runtime code, workflow, feature-parity state, release tag, release asset or `release_ready` field is modified by HLS-C006.

## Validation and merge rule

The task is documentation-only, so absence of a path-triggered PR workflow is not evidence of failure and no CI success will be invented. Final acceptance requires an independent exact-head diff review that verifies every changed README claim against current source/release state and confirms there is no release authorization or executable-gate weakening.
