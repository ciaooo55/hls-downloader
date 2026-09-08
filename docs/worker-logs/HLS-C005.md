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

## Finding 1 — runtime updater signer identity is not pinned

**Result: confirmed; split to HLS-C009 (P1).**

Evidence from current `main`:

- Release discovery is fixed to the repository's GitHub `releases/latest` API.
- Automatic MSI selection requires a non-zero GitHub asset size and a `sha256:` digest; the downloaded bytes are rechecked against both before atomic publish.
- Runtime MSI acceptance also requires exact expected `ProductVersion`, `ProductName=HLSDownloader`, the fixed project `UpgradeCode`, per-user installation context and a successful Windows `WinVerifyTrust` result.
- The formal release path is stronger: `sign-v7-authenticode.ps1` verifies that every signed first-party artifact's signer certificate thumbprint equals the configured `HLS_V7_SIGN_CERT_THUMBPRINT`, and `verify-v7-authenticode.ps1` binds its signature report to that signer and the exact source commit/tree.
- `native_shell/src/updater.rs::verify_installer_authenticode`, however, stops after generic `WinVerifyTrust` success. It does not compare the signer certificate/public-key identity against a locally trusted HLS Downloader signer contract.

Consequence: a release-channel compromise that supplies a matching version/name/UpgradeCode MSI, updates the network-controlled digest/size metadata, and signs the MSI with a different but Windows-trusted code-signing certificate is not explicitly rejected by the existing runtime signer check. The formal pipeline would reject such a signer, but an already-installed client's automatic updater does not enforce that same identity boundary.

Action: created HLS-C009 with narrow acceptance criteria. HLS-C005 remains audit-only and does not modify updater implementation.

## Finding 2 — optional Core TCP transport does not enforce loopback

**Result: confirmed; split to HLS-C010 (P1).**

Positive controls observed first:

- Windows product IPC uses a named pipe by default.
- The named-pipe server builds a protected DACL granting Generic All only to the object Owner Rights SID and SYSTEM, and fails startup if that owner DACL cannot be created.
- Core frames are capped at 4 MiB, connections are capped, and named-pipe/TCP body reads have absolute deadlines.
- Native Messaging registration restricts Chromium to the fixed extension origin and Firefox to the fixed extension ID.

The defect is limited to the optional TCP path. Source comments call it the test/Linux **loopback** transport, but `default_core_bind()` accepts any syntactically valid `SocketAddr` from `HLS_V7_CORE_BIND` without checking `ip().is_loopback()`. On Windows, merely setting that variable also opts the product into TCP. A wildcard or LAN address can therefore bind the complete Core protocol off-host.

The TCP transport has protocol/version hello but no client authentication/session secret. The request surface includes arbitrary Core commands plus settings, credential storage/load, default-cookie changes, handoff persistence and shutdown-capable operations. Consequently the configuration `HLS_V7_CORE_BIND=0.0.0.0:18765` contradicts the loopback trust model and can expose privileged same-user Core operations to a network peer.

Action: created HLS-C010. Its fix scope is to reject non-loopback IPv4/IPv6 bind addresses while preserving loopback test/Linux behavior and the existing Windows named-pipe security model.

## Status

`in_progress`: update trust produced HLS-C009; IPC audit produced HLS-C010. Next boundary is SQLite durability/checkpoint/restart and task-state handoff.
