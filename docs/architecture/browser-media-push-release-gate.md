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

The eventual `browser_media_push` evidence must prove the production path on the dedicated Windows release runner:

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

## Ordering

The current MSI lifecycle gate uninstalls its candidate during cleanup, so `browser_media_push` cannot simply be appended after `installer` and assume an installed candidate remains. Its implementation must own an isolated candidate install/cleanup lifecycle or refactor a shared lifecycle helper while preserving rollback isolation.

A safe target order is:

1. browser (portable browser behavior)
2. performance
3. browser_media_push (candidate MSI install + production browser/native-host/device/LAN chain)
4. installer (upgrade lifecycle)
5. rollback

The aggregate release evidence then requires five passed gates rather than four.

## Evidence identity

Every report must include or derive from:

- candidate manifest path and digest;
- source commit and tree;
- product version;
- candidate MSI path/digest;
- browser executable identities;
- installed Native Messaging registration state;
- expected/discovered/selected receiver identity;
- media fixture SHA-256;
- receiver-originated fetch records;
- gate result and timestamps.

Predecessor-SHA evidence, a direct Core push, or a receiver fetch not attributable to the selected receiver cannot authorize the gate.

## External dependency

A real LAN receiver is an external trusted-release prerequisite, like the signing key and dedicated runner. Repository CI can test the fail-closed contract and gate implementation, but it must not fabricate a receiver. If the protected release environment does not provide the configured receiver, formal release remains blocked and canonical readiness stays false.