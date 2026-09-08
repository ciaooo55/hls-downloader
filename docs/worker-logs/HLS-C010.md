# HLS-C010 — Enforce loopback-only Core TCP debug transport

## Acceptance criteria

1. `HLS_V7_CORE_BIND` rejects every non-loopback IPv4/IPv6 address; wildcard, private/LAN and public addresses must not expose the unauthenticated Core protocol.
2. Default and explicitly configured loopback addresses (`127.0.0.1`, `::1`) continue to support the test/Linux TCP transport.
3. Windows named-pipe owner/SYSTEM DACL, frame-size limits, connection limits and read deadlines remain unchanged.
4. Automated tests cover IPv4 loopback, IPv6 loopback, IPv4/IPv6 wildcard and representative non-loopback addresses.
5. No new remote or unauthenticated transport is introduced.

## Assignment

- Priority: P1
- Owner: `worker-0`
- Roles: coordinator + auditor + worker
- Branch: `fix/hls-c010-loopback-bind`
- Source finding: HLS-C005

## Reproduced defect

`core_ipc.rs` describes TCP as the test/Linux loopback transport, but `default_core_bind()` previously accepted any syntactically valid `SocketAddr` from `HLS_V7_CORE_BIND`. On Windows, setting that environment variable also selects TCP. Because the TCP protocol has version framing but no client authentication and carries the full Core request surface, a value such as `0.0.0.0:18765` or a LAN interface contradicted the intended trust boundary.

## Implementation

The patch is deliberately confined to `native_shell/src/core_ipc.rs` plus this worker log:

- `parse_core_bind()` parses the optional explicit address and rejects every address whose `IpAddr::is_loopback()` is false.
- The normal default remains `127.0.0.1:18765`.
- `default_core_bind()` keeps its existing public return type so Core server call sites do not need a broad API refactor. An explicitly invalid/non-loopback environment configuration fails closed instead of silently becoming a remote bind.
- `CoreIpcClient::connect_addr()` independently rejects non-loopback targets, so callers cannot use the public client helper to connect the product Core protocol to a remote address.
- Named-pipe creation/DACL, framing, read deadlines and connection accounting are untouched.

Tests added in the same module cover:

- default IPv4 loopback;
- explicit IPv4 loopback including another `127/8` address;
- IPv6 `::1`;
- IPv4 and IPv6 wildcard;
- representative RFC1918/private and public/documentation addresses;
- invalid socket syntax.

## Commit ledger

- `0bb17a8f3de8e429369967d480cb5cd17354fd42` — initialize acceptance/reproduced-defect worker log before implementation.
- `b32a751a78a2e5cdff64af24b688218559f4a150` — introduce loopback validation/client guard/tests.
- `959dda13daf372b755a4caddd9f8c3661e1152cc` — preserve the existing Core bind API while retaining fail-closed validation, avoiding an unnecessary `core_server.rs` signature churn.

## Validation state

Static diff review shows no named-pipe code changes and no new transport. Repository CI is still required to compile the full Rust call graph and run the new tests. Independent acceptance must use the exact PR head and current-head CI rather than this worker log.

## Status

`ready_for_ci_review`: implementation is complete enough for a dedicated PR; merge remains unauthorized until independent diff review and applicable current-head CI pass.
