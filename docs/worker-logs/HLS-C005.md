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
- Concurrent work: `worker-1` completed HLS-C004 and is now implementing HLS-C009.

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

## Persistence and restart boundary

**Result: PASS; no separate defect task created.**

Evidence from current source:

- `Store::open` enables foreign keys, WAL mode and `synchronous=NORMAL`, and refuses unsupported schema versions instead of silently migrating unknown layouts.
- Task snapshots, task events, task specs, settings and event checkpoints use transactions where the state must move atomically; task deletion also removes its spec/log state.
- Credential data is stored as protected blobs, not copied into public task snapshots.
- Core service persistence rolls in-memory runtime state back when the SQLite write fails instead of reporting an event that was never committed.
- Startup recovery only converts genuinely interrupted `downloading` / `recording` / `merging` / `checking` tasks into `paused`, or into `queued` when the explicit `resume_interrupted_on_startup` policy is enabled.
- Update preparation pauses active tasks and waits for worker termination before installer handoff; timeout aborts the update rather than proceeding with live workers.

No reproducible lost-update, accidental auto-resume, or public-secret persistence violation was found in this audit scope.

## Finding 3 — arbitrary custom replay headers cross origins

**Result: confirmed; split to HLS-C011 (P1).**

The browser extension captures arbitrary request headers in `RequestChainStore`. `replayableRequestHeaders()` removes cookies, proxy authentication and hop-by-hop/transport-owned fields, but intentionally permits ordinary custom headers and `Authorization` so an exact origin context can replay authenticated resources.

The Core credential replay layer binds a replay JSON object to `_task_url`. When a child request moves to a different origin, `apply_replay_json_for()` applies the base request headers first and then removes only `Cookie`, `Authorization` and `Proxy-Authorization` before applying an exact `request_contexts[target_origin]` entry.

An existing unit test explicitly verifies that a base `X-Playback: ok` header survives an unscoped cross-origin child while Cookie and Authorization are removed. That behavior is unsafe as a generic rule because arbitrary custom headers may themselves be credentials (`X-Api-Key`, `X-Auth-Token`, vendor session headers, signed API headers, etc.). The extension does not know which arbitrary names are secrets at capture time.

Consequence: a task whose source-origin replay context contains a custom credential header can send that header to an unscoped cross-origin HLS/DASH child even though the contract claims task secrets are not forwarded cross-origin. Exact origin-scoped `request_contexts` already provide the correct mechanism for deliberately replaying required headers to a CDN origin.

Action: created HLS-C011. The fix must prefer an allowlist or explicit origin ownership model rather than adding an inevitably incomplete list of secret-looking header names. Same-origin custom replay and exact matching scoped contexts must remain supported.

## Native Messaging / handoff positive controls

**Result: PASS apart from HLS-C011 replay leakage.**

- Native Messaging registration constrains Chromium and Firefox to the intended extension identities.
- Credential-bearing browser handoff state is stored separately from public handoff presentation; Windows uses DPAPI-backed protected blobs and non-Windows paths reject credential persistence rather than pretending it is protected.
- Extension replay metadata exposed for diagnostics is filtered for keys that look like cookies, authorization, tokens, passwords, secrets, credentials or request-header payloads.
- Origin-scoped request contexts override the browser-page fallback for their exact origin, including clearing invented Cookie/Referer/Origin values when the browser did not send them.

No additional handoff ownership or credential-at-rest defect was reproduced beyond HLS-C011.

## Audit conclusion

HLS-C005 covered all declared high-risk boundaries and produced three narrow implementation tasks:

- **HLS-C009 (P1):** bind runtime automatic updates to a locally versioned project signer identity.
- **HLS-C010 (P1):** enforce loopback-only optional Core TCP binding.
- **HLS-C011 (P1):** prevent arbitrary custom credential headers from crossing replay origins.

The SQLite/restart and Native Messaging registration/credential-at-rest boundaries passed the source audit. HLS-C005 itself contains documentation/evidence only; product fixes belong to C009/C010/C011 branches and require their own validation and review.

## Status

`ready_for_review`: all acceptance boundaries have been inspected, confirmed defects are split into independent tasks, and no speculative implementation is bundled into this branch. Final acceptance must refresh the branch against current `main`, inspect the exact diff, and confirm task-registry/handoff state remains consistent.
