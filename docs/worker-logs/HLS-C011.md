# HLS-C011 — Scope replay credential headers to their origin

## Acceptance criteria

1. Base replay `request_headers` do not cross from the task/source origin to an unscoped child origin.
2. Same-origin requests retain base replay headers.
3. An exact `request_contexts[target_origin]` may restore headers for that target origin after the base cross-origin filter runs.
4. Existing cross-origin Cookie / Authorization / Proxy-Authorization fail-closed behavior remains in force.
5. Top-level browser navigation context (`Referer`, `Origin`, `User-Agent`) keeps its existing fallback behavior unless an exact scoped context replaces/clears it.
6. Headers not injected by replay remain untouched by the new filter.
7. Windows/Rust CI and candidate packaging must pass on the exact final head before merge.

## Reproduced defect

`apply_replay_json_for()` merged the complete base replay `request_headers` map before checking whether a child request moved to another origin. On cross-origin requests it removed only Cookie, Authorization and Proxy-Authorization. Any other replay-provided credential header — for example `X-Api-Key`, `X-Playback`, or a site-specific bearer header — therefore remained attached to the child request.

An existing regression test explicitly demonstrated the gap by expecting base `X-Playback` to survive a manifest-origin -> CDN-origin transition.

## Implementation

Branch: `fix/hls-c011-replay-origin-headers`

- `merge_header_map()` now reports the canonical header names it actually injected.
- `apply_replay_json_for()` removes those base replay-injected names when `_task_url` and the current request resolve to different origins.
- Existing Cookie / Authorization / Proxy-Authorization stripping remains explicit.
- Top-level `Referer`, `Origin` and `User-Agent` are re-applied as navigation fallback after filtering; exact target-origin `request_contexts` still runs last and may replace/clear those fields and add target-scoped custom headers.
- Non-replay headers remain outside the removal set.

## Tests

The credential module now covers:

- same-origin base custom-header retention;
- unscoped cross-origin removal of arbitrary base replay headers (`X-Playback`, `X-Api-Key`) plus Cookie/Authorization;
- preservation of an unrelated pre-existing task header;
- preservation of top-level navigation fallback;
- exact target-origin scoped custom-header and Authorization restoration.

## Validation state

Implementation commits:

- `c454eff36755a06f9195a5d4abb39411989f09b5` — add origin filtering and regression coverage.
- `e61ab25e60a780c9447dd9543ce7253598a5ace4` — preserve existing navigation-header fallback and explicitly discard scoped merge bookkeeping.

No merge authorization is claimed until this branch is refreshed to the then-current `main`, exact-head CI/Candidate are green, and source acceptance is recorded separately from implementation claims.
