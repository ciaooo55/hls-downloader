# HLS Downloader 7.0.0 workbench architecture

## Decision

v7 keeps the Rust download engine, SQLite ownership and browser protocol, while
replacing the shipping main workbench with Compose Desktop. The workbench uses
Compose Foundation/UI plus the local `WorkbenchComponents` control layer; it no
longer directly depends on Material3 default controls. Skia/Skiko remains the
renderer and the Rust Core remains independent of the JVM process.

```text
WXT extension -- Native Messaging -- Rust Core + SQLite -- named pipe -- Compose UI
                                        |                    |
                              transfer / media / BT     snapshots + events
                                        |
                              warm native presenter (target)
```

`CoreServer` remains the only SQLite owner. UI clients never read a database,
never carry browser credentials, and exchange only length-prefixed JSON on
`\\.\pipe\HLSDownloader.v7`. The Core accepts a v6 hello only for an
explicit frozen-client compatibility path; the v7 UI and extension never
select it.

## Runtime roles

| Role | Owner | Requirement |
| --- | --- | --- |
| Resource recognition, download scheduling, recovery, persistence | Rust Core | Always resident while downloads or browser takeover are enabled. |
| Browser Native Messaging | Rust host | Connects to the same Core and sends bounded resource offers. |
| Hot confirmation, transfer and completion feedback | Slim native presenter | Pre-created before the browser reports an offer. This is the latency-critical route and is not the main workbench. |
| Main workbench, settings, task inspection and bulk workflows | Compose Desktop | Uses the same snapshot, event and command contract. |
| Local playback window | `hls-downloader-engine.exe --player-process` | A player crash must not terminate Core or any active transfer. |

Compose may render a handoff confirmation when it is the sole UI client, but
it must not race a resident native presenter. The v7 runtime elects the
pre-created native presenter as the primary confirmation surface and keeps
Compose only as the explicit fallback.

## Current status

`desktop_ui/` is a functional development workbench, not the shipping
installer. It consumes snapshots, sequenced events, browser status, handoff
offers, casting/player sessions and structured Core export results. Its custom
component layer owns text, icons, buttons, fields, menus, dialogs, selection,
progress, sliders and status controls. The verified minimum window is
1024x600; the default is 1400x820.

`presenter_ui` builds `hls-downloader-presenter.exe`, a pre-warmed temporary
surface with a separate presenter lock and no SQLite access. Historical main
workbenches are available through Git tags rather than active source roots. The Rust engine
now launches `--player-process` for player commands; only that child loads
libmpv, while Core keeps a bounded JSON control channel. Real Windows libmpv
track switching and process-kill recovery remain promotion gates.

`scripts/build-v7.ps1 -Task candidate` creates a machine-validation package in
`artifacts/v7-productization/candidate` with the Rust engine, versioned Native
Messaging host, warm presenter, bundled FFmpeg tools and pinned `libmpv-2.dll`.
It requires the canonical matrix, no blocked features and a clean Git worktree.
Partial features remain eligible because the candidate is the evidence vehicle
for closing those gaps; `release_ready` remains available for the external
validation decision.
`scripts/build-v7.ps1 -Task package` creates the formal package in the existing
`artifacts/v7-productization/package` directory and adds the `release_ready=true`
gate after all 28 features are verified. Both package tiers carry the v7 Native
Messaging manifests and atomic upgrade/rollback script. v6 remains the rollback
path until the independent MSI install, upgrade, uninstall and registration gate
is completed.

## Promotion gates

v7 can replace v6 only after all of these are true:

1. The installer ships and registers a versioned Native Messaging host that
   can reach a resident Core after a reboot, without requiring the workbench
   to have been opened manually.
2. A single presenter is elected for every browser offer; its `present_handoff`
   acknowledgement, accept, reject and restart recovery are covered by an
   end-to-end test.
3. The presenter is warm before an offer. The P95 time from Core offer to a
   visible confirmation stays below the v6 baseline; a cold JVM launch is not
   an acceptable substitute.
4. Compose parity covers every v3/v5 workbench action that has an existing
   Core command, including task details/logs, media selection/playback,
   casting, import/export, duplicate resolution, queue ordering and settings.
5. Windows CI runs the Compose protocol tests, native Core tests, package
   smoke test and browser extension takeover test against the same bundle.
6. `feature-parity.json` reaches 100%, the 1000-task and IPC/startup P95 gates
   pass, and player/Core crash isolation is demonstrated by process-kill tests.

## Development verification

```powershell
$env:JAVA_HOME = 'E:\HLSDownloaderBuildCache\jdk-21'
$env:GRADLE_USER_HOME = 'E:\HLSDownloaderBuildCache\gradle'
E:\HLSDownloaderBuildCache\gradle-9.7.0\bin\gradle.bat -p desktop_ui test --no-daemon

& .\scripts\adversarial-v7.ps1 -Scope @('native', 'browser', 'transfer')
```

The layout uses a 34dp title bar, fixed toolbar, 190dp queue rail, responsive
task columns, virtualized 52dp rows and a 28dp status bar. At the 1024x600
minimum, the table switches to compact columns and preserves the complete
operation menu. Dialogs cap their scrolling content so the action row remains
visible at 100%, 125% and 150% DPI.

## Media cast and browser push

Browser `media_push` requests are durable Core handoffs, not an in-memory UI
shortcut. The Native Host creates a `MediaPushRequest`, the Core persists it
and emits `media_push_requested`, and Compose opens the same source-aware device
picker used by task playback. The picker separates DLNA/Chromecast from TVBox,
shows the title and source URL/path, supports LAN publishing when a receiver
cannot be controlled directly, and keeps the action row visible while devices
are discovered or a connection is pending.

The selected device or LAN publisher resolves the request through the Core with
`media_push_resolved`. Compose converts `done`, `failed` and `canceled` into a
visible completion Toast; the browser polls the same persisted row, so a browser
restart or a closed workbench cannot lose the result. TVBox push uses the Core's
LAN-only POST/GET fallback and reports receiver errors instead of treating an
HTTP response as success.

Device discovery runs inside the Core. The command carries an explicit `cast`
or `tvbox` mode, so DLNA/Chromecast discovery and TVBox receiver probing do not
delay or overwrite one another. DLNA SSDP and Chromecast mDNS queries are sent
from every eligible LAN adapter; TVBox scans the common 9976-9979 ports across
those adapters. Windows enumeration uses `GetAdaptersAddresses`, ignores
loopback, tunnel, VPN and virtual adapters, respects each prefix and caps the
TVBox scan at 512 hosts with 64 concurrent probes. Compose exposes two distinct
actions and a segmented Settings > 投屏与推送 page.

## Browser request identity

The extension hands the Core the top-level page URL plus captured request
identity for each resource origin. Referer, Origin and User-Agent from the page
form the fallback identity; an exact `request_contexts[origin]` entry overrides
them for an iframe, CDN manifest or segment request. Cookie and Authorization
never live in a public task snapshot: the Native Host seals the replay context
through DPAPI and the Core materializes it only for the outgoing request. A
scoped CDN context with no cookie explicitly removes the page cookie and
Authorization so credentials cannot cross origins. Settings defaults apply only
to tasks that have no browser replay context.
