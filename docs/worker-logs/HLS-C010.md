# HLS-C010 — Enforce loopback-only Core TCP debug transport

## Acceptance criteria

1. `HLS_V7_CORE_BIND` rejects every non-loopback IPv4/IPv6 address; wildcard, private/LAN and public addresses must not expose the unauthenticated Core protocol.
2. Default and explicitly configured loopback addresses (`127.0.0.1`, `::1`) continue to support the test/Linux TCP transport.
3. A caller cannot bypass the policy by passing a `TcpListener` already bound to a wildcard, LAN or public address; the shared server boundary validates the listener's actual local address before accepting clients.
4. Windows named-pipe owner/SYSTEM DACL, frame-size limits, connection limits and read deadlines remain unchanged.
5. Automated tests cover IPv4/IPv6 loopback, wildcard/private/public configured addresses, and a real pre-bound `0.0.0.0:0` listener rejection.
6. No new remote or unauthenticated transport is introduced.

## Assignment

- Priority: P1
- Current owner: `worker-1`
- Original owner: `worker-0`
- Roles: coordinator + auditor-fallback + worker
- Branch: `fix/hls-c010-loopback-bind`
- PR: `#67`
- Source finding: HLS-C005

## Recovery takeover

`worker-0` stopped heartbeating HLS-C010 after 2026-09-08T04:03:40Z, exceeding the repository's lost-worker threshold. `worker-1` formally reassigned and declared takeover in the coordination issues. The existing branch and implementation history were retained rather than restarted.

During independent review of the original PR head, `worker-1` found that configured bind addresses and client targets were loopback-only but the public `serve_tcp_listener(listener, ...)` boundary still accepted a caller-supplied listener already bound to `0.0.0.0` or another non-loopback interface. That head was explicitly blocked from merge.

## Implementation

The patch remains deliberately confined to `native_shell/src/core_ipc.rs` plus this worker log:

- `parse_core_bind()` parses the optional explicit address and rejects every address whose `IpAddr::is_loopback()` is false.
- The normal default remains `127.0.0.1:18765`.
- `default_core_bind()` keeps its existing public return type; an explicitly invalid/non-loopback environment configuration fails closed.
- `CoreIpcClient::connect_addr()` independently rejects non-loopback targets.
- `serve_tcp_listener()` independently reads `listener.local_addr()` and rejects any actual non-loopback binding before setting nonblocking mode or entering the accept loop.
- Named-pipe creation/DACL, framing, read deadlines and connection accounting are untouched.

Tests in the same module cover:

- default IPv4 loopback;
- explicit IPv4 loopback including another `127/8` address;
- IPv6 `::1`;
- IPv4 and IPv6 wildcard;
- representative RFC1918/private and public/documentation addresses;
- invalid socket syntax;
- a listener really bound to `0.0.0.0:0`, which the shared server boundary must reject.

## Commit ledger

- `0bb17a8f3de8e429369967d480cb5cd17354fd42` — initialize acceptance/reproduced-defect worker log before implementation.
- `b32a751a78a2e5cdff64af24b688218559f4a150` — introduce loopback validation/client guard/tests.
- `959dda13daf372b755a4caddd9f8c3661e1152cc` — preserve the existing Core bind API while retaining fail-closed validation.
- `420dc50fb448304ff113539c756a26eb3c7a830b` — close the audited pre-bound non-loopback listener bypass and add the real-listener regression test.
- Refresh after HLS-C009 merge: merge current main into this branch and rerun exact-head CI/Candidate before final authorization.

## Validation state

Before the main refresh, PR head `420dc50fb448304ff113539c756a26eb3c7a830b` passed full `v7 CI #509`. Windows Rust Core executed the pre-bound listener regression test successfully. A separate source audit confirmed no named-pipe security changes.

Because HLS-C009 / PR #68 changed main after those results, the refreshed HLS-C010 head must obtain fresh CI and Candidate evidence before merge. Earlier green results are supporting evidence only, not final authorization.

## Status

`refreshing_after_c009`: source fix is complete; final merge remains unauthorized until the post-#68 exact head passes fresh CI/Candidate and final audit.
