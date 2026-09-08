# HLS-C011 — Scope replay credential headers to their origin

## Acceptance criteria

1. Base replay `request_headers` do not cross from the task/source origin to an unscoped child origin.
2. Same-origin requests retain base replay headers.
3. Cross-origin HTTP redirects cannot retain base replay custom headers.
4. A custom header injected by one scoped replay origin cannot survive a later redirect to a different origin.
5. An exact `request_contexts[target_origin]` may restore headers for that target origin after the cross-origin filter runs.
6. Existing cross-origin Cookie / Authorization / Proxy-Authorization fail-closed behavior remains in force.
7. Top-level browser navigation context (`Referer`, `Origin`, `User-Agent`) keeps its existing fallback behavior unless an exact scoped context replaces/clears it.
8. Header names absent from the replay contract remain outside the new removal set.
9. Windows/Rust CI and candidate packaging must pass on the exact final head before merge.

## Reproduced defect

`apply_replay_json_for()` merged the complete base replay `request_headers` map before checking whether a child request moved to another origin. On cross-origin requests it removed only Cookie, Authorization and Proxy-Authorization. Any other replay-provided credential header — for example `X-Api-Key`, `X-Playback`, or a site-specific bearer header — therefore remained attached to the child request.

The HTTP engine also has a redirect handoff path: `drop_cross_origin_secrets()` strips the three legacy credential headers and then calls `apply_scoped_request_context()`. Before this fix that shared scoped boundary did not remove arbitrary replay custom headers, so a 30x cross-origin redirect could preserve them as well.

A second audit found the multi-hop form of the same defect: after A -> B, an exact B scoped context could inject `X-B-Token`; on B -> C, filtering only base replay names would leave that B-only custom credential attached to the C request.

An existing regression test explicitly demonstrated the first child-request gap by expecting base `X-Playback` to survive a manifest-origin -> CDN-origin transition.

## Implementation

Branch: `fix/hls-c011-replay-origin-headers`

- `replay_request_header_names()` derives the valid/canonical custom-header names from both base replay `request_headers` and every scoped `request_contexts[*].request_headers` using the existing merge validation path.
- `apply_scoped_request_context()` removes those replay-controlled names whenever `_task_url` and the current request resolve to different origins, before looking up the target scoped context.
- Because the HTTP redirect handoff already calls this shared scoped boundary, redirects receive the same custom-header filter without duplicating security logic in `http_engine.rs`.
- On a multi-hop redirect, a previous scoped origin's custom headers are therefore cleared before the next target origin's scoped context is applied.
- Existing Cookie / Authorization / Proxy-Authorization stripping remains explicit in the normal request and redirect paths.
- Top-level `Referer`, `Origin` and `User-Agent` are re-applied as navigation fallback after custom-header filtering; exact target-origin `request_contexts` still runs last and may replace/clear those fields and add target-scoped custom headers.
- Header names absent from the replay contract remain outside the removal set. A colliding header name that is replay-controlled is intentionally treated fail-closed.

## Tests

The credential module now covers:

- same-origin base custom-header retention;
- unscoped cross-origin removal of arbitrary base replay headers (`X-Playback`, `X-Api-Key`) plus Cookie/Authorization;
- preservation of an unrelated pre-existing task header;
- preservation of top-level navigation fallback;
- direct scoped-boundary redirect handoff removing a source `X-Api-Key` and restoring only target `X-Cdn-Token`;
- multi-hop scoped redirect A -> B -> C, proving B-only `X-Cdn-A-Token` is removed before C-only `X-Cdn-B-Token` is restored;
- exact target-origin scoped custom-header and Authorization restoration.

## Validation state

Implementation commits include:

- `c454eff36755a06f9195a5d4abb39411989f09b5` — add initial origin filtering and regression coverage.
- `e61ab25e60a780c9447dd9543ce7253598a5ace4` — preserve navigation-header fallback and scoped merge bookkeeping semantics.
- `d24cc8701427e3613bef9728730c349091296433` — move arbitrary replay-header filtering into the shared scoped boundary so HTTP redirects are covered too.
- `907eb407db0eff38ca026ddb3afdf5408c5bc686` — apply rustfmt-required regression-test formatting after the first refreshed CI run.
- `99e96aa2b0b02acb9cb62ea1d4d2c798af8ebf45` — extend the filter to all scoped replay custom-header names and add multi-hop redirect coverage.

No merge authorization is claimed until this branch is refreshed/current with `main`, exact-head CI/Candidate are green, and source acceptance is recorded separately from implementation claims.
