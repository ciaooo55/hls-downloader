# HLS-C011 — Scope replay credential headers to their origin

## Acceptance criteria

1. Base replay `request_headers` do not cross from the task/source origin to an unscoped child origin.
2. Same-origin requests retain base replay headers.
3. Cross-origin HTTP redirects cannot retain base replay custom headers.
4. An exact `request_contexts[target_origin]` may restore headers for that target origin after the base cross-origin filter runs.
5. Existing cross-origin Cookie / Authorization / Proxy-Authorization fail-closed behavior remains in force.
6. Top-level browser navigation context (`Referer`, `Origin`, `User-Agent`) keeps its existing fallback behavior unless an exact scoped context replaces/clears it.
7. Headers not injected by replay remain outside the new removal set.
8. Windows/Rust CI and candidate packaging must pass on the exact final head before merge.

## Reproduced defect

`apply_replay_json_for()` merged the complete base replay `request_headers` map before checking whether a child request moved to another origin. On cross-origin requests it removed only Cookie, Authorization and Proxy-Authorization. Any other replay-provided credential header — for example `X-Api-Key`, `X-Playback`, or a site-specific bearer header — therefore remained attached to the child request.

The HTTP engine also has a redirect handoff path: `drop_cross_origin_secrets()` strips the three legacy credential headers and then calls `apply_scoped_request_context()`. Before this fix that shared scoped boundary did not remove arbitrary base replay headers, so a 30x cross-origin redirect could preserve them as well.

An existing regression test explicitly demonstrated the child-request gap by expecting base `X-Playback` to survive a manifest-origin -> CDN-origin transition.

## Implementation

Branch: `fix/hls-c011-replay-origin-headers`

- `replay_request_header_names()` derives exactly the valid/canonical names present in base replay `request_headers` using the existing merge validation path.
- `apply_scoped_request_context()` now removes those base replay names whenever `_task_url` and the current request resolve to different origins, before looking up the target scoped context.
- Because the HTTP redirect handoff already calls this shared scoped boundary, redirects now receive the same custom-header filter without duplicating security logic in `http_engine.rs`.
- Existing Cookie / Authorization / Proxy-Authorization stripping remains explicit in the normal request and redirect paths.
- Top-level `Referer`, `Origin` and `User-Agent` are re-applied as navigation fallback after custom-header filtering; exact target-origin `request_contexts` still runs last and may replace/clear those fields and add target-scoped custom headers.
- Non-replay header names remain outside the removal set.

## Tests

The credential module now covers:

- same-origin base custom-header retention;
- unscoped cross-origin removal of arbitrary base replay headers (`X-Playback`, `X-Api-Key`) plus Cookie/Authorization;
- preservation of an unrelated pre-existing task header;
- preservation of top-level navigation fallback;
- direct scoped-boundary redirect handoff removing a source `X-Api-Key` and restoring only target `X-Cdn-Token`;
- exact target-origin scoped custom-header and Authorization restoration.

## Validation state

Implementation commits:

- `c454eff36755a06f9195a5d4abb39411989f09b5` — add initial origin filtering and regression coverage.
- `e61ab25e60a780c9447dd9543ce7253598a5ace4` — preserve navigation-header fallback and scoped merge bookkeeping semantics.
- `d24cc8701427e3613bef9728730c349091296433` — move arbitrary replay-header filtering into the shared scoped boundary so HTTP redirects are covered too.

No merge authorization is claimed until this branch is refreshed to the then-current `main`, exact-head CI/Candidate are green, and source acceptance is recorded separately from implementation claims.
