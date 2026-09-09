# Installed-browser media-push release gate

## Problem

Canonical v7.0.2 metadata declared `browser.media_push_device_selection` verified while also retaining an explicit final-package gap requiring real browser registration and a real LAN-device gate. The existing formal workflow could validate browser behavior, MSI registration and direct Core-to-TVBox behavior independently without proving the production chain joins those boundaries.

A release gate must not infer an end-to-end property from separately passing component checks when canonical metadata says that end-to-end property is still outstanding.

## Fail-closed invariant

Formal release has two independent protections:

1. canonical completeness: a feature with an outstanding release gate is `partial`, so `RequireCanonicalComplete` / `RequireReleaseReady` cannot pass;
2. residual-gap assertion: formal release rejects any non-empty feature `gap` before generating release evidence, even if a future edit accidentally labels that feature `verified`.

The residual-gap assertion is intentionally separate from the feature status summary. It turns the human-readable gap field into an executable release invariant rather than advisory prose.

## Candidate-bound end-to-end gate

The `browser_media_push` evidence must prove the production path on the dedicated Windows release runner:

1. Verify the candidate manifest belongs to the current source commit/tree.
2. Install the candidate MSI into the formal lifecycle location and verify the candidate's actual Native Messaging host registration is present.
3. Require configured real receiver identity (initial implementation target: TVBox expected host) from the protected release environment. Missing configuration or discovery is a hard failure, never a skip.
4. Start the installed Core/workbench and use the normal settings contract only to select the discovered expected receiver as preferred. Direct Core `share_media` is not allowed to satisfy this gate.
5. Launch real Edge and Firefox with the production extension path and invoke the actual browser-side TVBox/media-push action.
6. Observe the resulting `media_push_requested` request through Core/Compose. Use packaged accessibility semantics to activate `确认推送`, exercising `DevicePickerDialog` rather than a test-only control plane.
7. Require the browser-side operation to resolve successfully (`已发送` / done status).
8. Serve a deterministic media fixture on the runner's private LAN address and require the selected real receiver itself to fetch the media. Record receiver identity, request source, byte/range behavior, fixture SHA-256 and timing.
9. Record the evidence through the same candidate-manifest-bound release evidence mechanism as the existing browser/performance/installer/rollback gates.
10. Only after both browser families and the configured real receiver pass may the canonical gap be removed and the feature return to verified.

The Native Messaging portion is itself candidate-bound rather than merely presence-checked. For each browser the gate follows the HKCU registration to its selected JSON manifest, validates the production host name/type and browser-specific allowlist, and requires the manifest-declared executable to resolve exactly to the current candidate installation's `E:\h\HLSDownloaderNativeHost.exe`. The host executable and both selected manifests are SHA-256 bound into the gate evidence. A stale registry key or leftover manifest/host pair therefore cannot satisfy the candidate gate merely because the referenced file still exists.

The manifest file is not required to live under `E:\h`. Production registration prefers the Engine directory but may legitimately fall back to `%LOCALAPPDATA%\HLSDownloader\v7-native-host` when the preferred directory is not writable. The invariant is the manifest's declared host identity and digest, not its storage directory.

## Ordering

The current MSI lifecycle gate uninstalls its candidate during cleanup, so `browser_media_push` cannot simply be appended after `installer` and assume an installed candidate remains. Its implementation owns an isolated candidate install/cleanup lifecycle while preserving rollback isolation.

The formal gate order is:

1. browser (portable browser behavior)
2. performance
3. browser_media_push (candidate MSI install + production browser/native-host/device/LAN chain)
4. installer (upgrade lifecycle)
5. rollback

The aggregate release evidence therefore requires five passed gates rather than four.

## Pre-readiness execution and deadlock avoidance

The real-device evidence cannot be collected only inside the formal release workflow. Formal packaging requires `release_ready=true`, while the canonical status contract does not allow C014 to grant release readiness while this feature remains `partial`. Requiring the formal workflow to be the first place that closes the gap would create a circular dependency.

HLS-C018 therefore has a source-hardening boundary and HLS-C019 owns external evidence:

1. **HLS-C018** merges the fail-closed implementation while the feature remains truthfully `partial` and `release_ready=false`. It adds the production `browser_media_push` formal gate, residual-gap assertion, regression coverage, and the `v7 Media Push Readiness` workflow.
2. **HLS-C019** runs `v7 Media Push Readiness` from an exact, current `main` SHA on the protected `hls-release` Windows runner while `release_ready=false`. The workflow builds a candidate without formal-only readiness requirements, exercises real Edge and Firefox through the candidate MSI-installed Native Messaging registration to the configured LAN receiver, reconfirms main did not move, and uploads candidate-bound readiness evidence.
3. After a successful C019 run, a narrow reviewed metadata task may remove the canonical `gap` and promote `browser.media_push_device_selection` from `partial` to `verified`. That promotion must cite the workflow run, source SHA/tree, candidate manifest digest, browser report digests and receiver identity. It must not set `release_ready=true` in the same task.
4. **HLS-C014** restarts only after that promotion is accepted and main is again 28/28 verified. C014 obtains fresh exact-main prerequisite workflows before deciding `release_ready`.
5. The final formal release still reruns `browser_media_push` against the final readiness SHA. Pre-readiness evidence authorizes only the feature-status promotion; it never substitutes for final-SHA formal evidence.

The pre-readiness workflow refuses branch heads, stale main, `release_ready=true`, missing receiver configuration, and receiver/test failures. It has read-only repository permissions and performs no tag, signing, Release creation or publication.

## Evidence identity

Every report must include or derive from:

- candidate manifest path and digest;
- source commit and tree;
- product version;
- candidate MSI path/digest;
- browser executable identities;
- exact installed Native Messaging host identity: registry key -> selected manifest digest -> manifest-declared exact candidate host path -> `E:\h\HLSDownloaderNativeHost.exe` SHA-256, plus the browser-specific production allowlist;
- expected/discovered/selected receiver identity;
- media fixture SHA-256;
- receiver-originated fetch records;
- gate result and timestamps.

The pre-readiness attestation independently revalidates and persists the candidate Native Host path/hash and both registration snapshots, in addition to binding the exact workflow run/attempt and both browser report digests. Predecessor-SHA evidence, stale registration state, a direct Core push, or a receiver fetch not attributable to the selected receiver cannot authorize the gate.

## External dependency

A real LAN receiver is an external trusted-release prerequisite, like the signing key and dedicated runner. Repository CI can test the fail-closed contract and gate implementation, but it must not fabricate a receiver. If the protected release environment does not provide the configured receiver, HLS-C019 remains blocked, canonical readiness stays false, and formal release remains impossible by construction.
