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

`core_ipc.rs` describes TCP as the test/Linux loopback transport, but `default_core_bind()` currently accepts any syntactically valid `SocketAddr` from `HLS_V7_CORE_BIND`. On Windows, setting that environment variable also selects TCP. Because the TCP protocol has version framing but no client authentication and carries the full Core request surface, a value such as `0.0.0.0:18765` or a LAN interface contradicts the intended trust boundary.

## Implementation rule

Keep the patch narrow: validate the configured `SocketAddr` after parsing and reject it unless `addr.ip().is_loopback()` is true. Preserve the existing default `127.0.0.1:18765` and all named-pipe code.

## Status

`in_progress`: acceptance recorded before implementation.
