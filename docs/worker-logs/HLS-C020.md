# HLS-C020 — Drive explicit DevicePicker selection in media-push readiness smoke

## Acceptance criteria

1. Do not change product DevicePicker behavior or add a test-only product bypass.
2. Do not rely on writing `preferred_cast_device_id` after Compose has already loaded settings.
3. Discover exactly one receiver at the configured private IPv4 address, then select that receiver through the production desktop UI before invoking `确认推送`.
4. Preserve real Edge + Firefox, candidate MSI Native Messaging registration, browser-originated media-push request, Core/Compose path, and real receiver GET evidence.
5. Keep `release_ready=false` and `browser.media_push_device_selection=partial`; this task only repairs the pre-readiness evidence path.
6. Require exact-head source CI/Candidate success and independent review before merge.
7. After merge, invalidate the old C019 frozen SHA and restart C019 on the new exact main SHA.

## Preflight failure that created this task

HLS-C019 initially froze main `a50f5dffedf45c8575464b47ebb8fe7f23039d96` after HLS-C018/#82 merged.

Source preflight showed the readiness smoke called Core `store_settings` to write `preferred_cast_device_id` only after the installed Compose application was already running. Compose keeps its own `settings` state and reloads Core settings inside `LaunchedEffect(refreshKey)`. The out-of-band Core write does not update that in-memory state.

`DevicePickerDialog` initializes its local selection from `settings.preferredCastDeviceId`, and its `确认推送` primary action is enabled only when `selected != null`. Therefore the old smoke could discover the correct receiver while still leaving confirmation disabled on a clean runner.

No external readiness run was dispatched for that SHA, so the defect was caught before consuming trusted-runner time or producing misleading evidence.

## Implementation

Branch: `fix/hls-c020-media-push-device-select`

Commit `bcc54771404d520aae0a2aa7155d62ba93bb0865` changes only the readiness smoke behavior:

- `discover_and_preselect` becomes `discover_expected_device` and no longer mutates settings;
- browser still starts the production `TVBox` action;
- Java Access Bridge inventories the real Compose DevicePicker;
- the smoke identifies the unique visible node containing the configured receiver host (falling back to the exact discovered label), brings the HLS Downloader window forward, and performs a real desktop click on that device row;
- a second Access Bridge operation requires and invokes the now-enabled `确认推送` action;
- the final report records `device_selection` evidence in addition to the existing browser identity, selected receiver, receiver HTTP requests and confirmation accessibility evidence.

The receiver must still fetch `/stream.mp4`; direct Core `share_media`/cast commands remain forbidden in this browser-path smoke.

## Evidence status

- Source preflight: fix implemented; review pending.
- Hosted CI: pending PR creation/exact-head runs.
- Trusted-runner readiness: intentionally not run in HLS-C020. C019 resumes only after this source fix merges.
