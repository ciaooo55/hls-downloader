# v7.0.2 cross-cutting contract audit

HLS-C005 audited the post-v7.0.2 trust and durability boundaries without bundling implementation fixes into the audit branch. Confirmed defects are isolated into independent tasks so each fix can be tested and reviewed against a narrow contract.

## Audited boundaries

### Automatic update trust

Existing positive controls remain substantial: fixed repository release discovery, asset size and SHA-256 verification, exact MSI ProductVersion, ProductName, fixed UpgradeCode, per-user installation context, and Windows WinVerifyTrust.

The remaining gap is signer **identity** rather than signature validity. Formal release signing binds artifacts to `HLS_V7_SIGN_CERT_THUMBPRINT`; the runtime updater currently accepts any signer that WinVerifyTrust considers valid. HLS-C009 owns the correction and certificate-rotation contract.

### Core IPC

Normal Windows product IPC uses the named-pipe path with an owner/SYSTEM DACL, bounded frames, connection limits and read deadlines. Native Messaging manifests restrict browser identities.

The optional TCP transport is intended for loopback test/Linux use but `HLS_V7_CORE_BIND` currently accepts non-loopback addresses. Because that transport carries the full unauthenticated Core request surface, HLS-C010 must reject non-loopback binds rather than adding remote unauthenticated behavior.

### Persistence and restart

No separate defect was reproduced. SQLite uses schema rejection, foreign keys, WAL and transactional state movement for task/event/spec/settings/checkpoint boundaries. Core persistence rolls runtime state back if durable writes fail. Interrupted active states become paused on startup unless explicit resume policy queues them. Update preparation waits for active workers to stop before installer handoff.

### Browser handoff and replay

Browser secrets are stored outside public handoff presentation and Windows uses DPAPI-backed protected blobs. Exact request contexts are origin-scoped and can clear page-level fallback identity.

The remaining gap is cross-origin inheritance of arbitrary custom request headers. The current replay path removes Cookie/Authorization/Proxy-Authorization when the target origin differs but preserves other custom headers. Because captured custom headers may themselves be credentials, HLS-C011 must make unscoped cross-origin replay allowlist/ownership based while retaining same-origin and exact scoped replay.

## Fix backlog produced by this audit

| Task | Priority | Boundary | Required invariant |
| --- | --- | --- | --- |
| HLS-C009 | P1 | updater trust | A valid Windows signature is insufficient unless its signer is in the locally versioned HLS release-signer trust contract. |
| HLS-C010 | P1 | Core IPC | Optional TCP IPC may bind only loopback IPv4/IPv6 addresses. |
| HLS-C011 | P1 | browser replay | Unscoped cross-origin requests must not inherit arbitrary source-origin custom headers; exact origin-scoped contexts are the opt-in path. |

## Non-findings

This audit did **not** reproduce a SQLite durability/restart defect, a Windows named-pipe DACL bypass, a Native Messaging manifest origin bypass, or a credential-at-rest plaintext regression. Those boundaries should remain covered by their existing tests rather than receiving speculative refactors.

## Review rule

HLS-C005 is evidence-only. None of C009/C010/C011 may be considered fixed because this audit document exists. Each task requires its own implementation branch, targeted tests, full applicable CI, independent diff review and task-state update before merge.
