# Desktop HLS Downloader Design

## Goal

Turn the existing Windows HLS downloader into a conventional desktop download manager. The installed application must open an independent native window, preserve every existing download feature, support direct and browser-assisted URL recognition, export the userscript to a chosen folder, and pass automated and installed-app acceptance tests before delivery.

## Product Decisions

- Target platform: Windows 10 and Windows 11.
- Window shell: pywebview using the Edge Chromium (WebView2) renderer.
- Layout: classic download-manager table with a left status sidebar and bottom aggregate status bar.
- Theme: system theme by default; users may select dark or light and the override is remembered.
- Closing behavior: show a confirmation dialog; confirmation stops active work, shuts down the local server, and exits completely. There is no system tray mode.
- History: preserve all task history until the user deletes it.
- Unsupported media remains unchanged: live playlists, SAMPLE-AES/DRM, independent audio tracks, and subtitle muxing are outside this phase.

## Desktop Architecture

`HLSDownloader.exe` owns one desktop window and one FastAPI service in the same application process. The GUI loop runs on the main thread and the existing server runs on a managed background thread at `127.0.0.1:8765`. The window loads `/ui`; it does not expose an address bar and does not launch an external browser.

The application uses a Windows single-instance guard. A second launch calls an authenticated local activation endpoint, restores and focuses the existing window, and then exits. If another unrelated program owns the configured port, startup displays a diagnostic error instead of opening a blank window.

The desktop bridge exposes only narrow native operations needed by the UI: choose an export directory, export the userscript, show the native close confirmation, and report desktop runtime information. Download control continues through the existing authenticated HTTP API so browser and desktop behavior stay consistent.

The installer checks for the WebView2 runtime. If it is absent, the installer runs a bundled Evergreen bootstrapper and reports installation failure clearly. Python and pywebview dependencies remain frozen into the executable; ffmpeg and ffprobe remain installed under `bin/`.

## Window And Navigation

The default window is approximately 1180 by 760 pixels with a minimum size of 900 by 600. Standard Windows minimize, maximize, and close controls remain visible.

The top toolbar contains icon-led actions for new download, paste and recognize, batch add, start, pause, resume, cancel, retry, open output, delete, userscript tools, refresh, and settings. Destructive actions require confirmation. Buttons that do not apply to the current selection are disabled.

The left sidebar filters all, active, paused, completed, failed, and canceled tasks. It also shows userscript state as recently detected, previously detected, or not detected this run.

The main table supports selection, multi-selection, sorting, keyboard navigation, double-click details, and a context menu. Columns cover file name, total size, downloaded size, status, progress, speed, remaining time, segment count, and updated time. Columns have stable minimum widths and horizontal overflow so content never overlaps.

The bottom status bar shows active task count, queued count, aggregate speed, completed size, and available disk space. Task details and logs open in focused dialogs rather than a permanent right sidebar.

## Preserved Download Features

The desktop UI must expose every existing capability:

- Create one direct HLS task or a validated batch.
- Pause, resume, cancel, retry, and delete tasks using TaskManager state rules.
- Display segment, merge, and remux progress without freezing the event loop.
- Display speed, ETA, worker activity, connection state, errors, output path, and logs.
- Open the completed file or containing directory.
- Edit download directory, concurrency, maximum active tasks, request headers, temporary-file retention, ffmpeg path, host restrictions, and token.
- Preserve restart recovery and all task history.
- Support existing VOD HLS capabilities: recursive master playlists, AES-128, BYTERANGE, init-map changes, and discontinuities.

## URL Recognition

The new-download dialog accepts a URL from typing, drag/drop text, or clipboard paste and classifies it before task creation.

For direct HLS input, classification uses the URL, response content type, redirects, and the `#EXTM3U` signature rather than relying only on a `.m3u8` suffix. A confirmed playlist follows the existing parser and downloader flow.

For an ordinary webpage URL, the backend performs a bounded static-page scan. It follows safe HTTP redirects, uses configured request headers, parses URL-bearing HTML attributes and embedded page data, resolves relative candidates, and returns zero, one, or multiple HLS candidates. One candidate may be started immediately; multiple candidates are presented for selection.

Dynamic media requests, authenticated browser state, JavaScript-only players, and blob URLs are handled by the userscript. The userscript reports discovered m3u8 URLs and browser request context to the same local API. A webpage scan that finds nothing must explain that browser sniffing is required; it must not remain in an indefinite parsing state.

Ordinary webpage recognition is best effort and does not claim universal site support. DRM and unsupported encryption are reported explicitly.

## Userscript Workflow

The Tools area provides two first-class actions:

1. Install userscript opens the local `.user.js` installation URL using the default browser. The served script is generated with the current host, port, token, and version.
2. Export userscript opens a native directory picker and atomically writes `m3u8-sniffer.user.js` to the selected directory. Existing files require overwrite confirmation.

The exported and served variants come from one template to prevent configuration drift. The userscript reports a heartbeat after loading and periodically while a matching HTTPS page is open. Detection means recently observed execution, not direct access to Tampermonkey's installed-script list.

The desktop application remains fully usable without the userscript for direct HLS URLs and statically discoverable page URLs.

## State And Data Flow

UI commands call the existing authenticated API. TaskManager remains the only owner of task lifecycle transitions. SSE pushes complete mergeable task snapshots to the desktop UI; a low-frequency reconciliation request repairs missed events.

Desktop-only actions call the restricted pywebview bridge. Script export obtains current settings from the backend, renders the userscript template, writes a temporary file in the chosen directory, and atomically replaces the destination after validation.

On confirmed close, the shell prevents new task creation, requests TaskManager shutdown, cancels and awaits active download and ffmpeg processes, forces final database persistence, stops uvicorn, and only then destroys the window. If graceful shutdown exceeds a bounded timeout, the confirmation dialog reports the problem and offers a second explicit force-exit action.

## Error Handling And Diagnostics

Before showing the main window, startup checks the database, UI build assets, ffmpeg, ffprobe, configured download directory, local port, WebView2 renderer, and server health endpoint. A failure opens a readable diagnostic view with the failing component, path, and corrective action.

The UI defines loading, empty, reconnecting, unavailable, validation-error, conflict, and terminal task states. Commands are disabled while in flight and display API conflict messages instead of silently retrying illegal actions. Long titles and paths use truncation with full-value tooltips. Empty and large task lists must preserve the table layout.

Static webpage scanning has response-size and time limits and rejects non-HTTP(S) URLs. Candidate URLs are deduplicated and validated before display. Script export rejects inaccessible destinations and leaves no partial output.

## Testing And Acceptance

Python tests cover the desktop startup controller, single-instance activation, close/shutdown ordering, startup diagnostics, direct HLS classification, static webpage candidate extraction, userscript rendering/export, and all existing backend behavior.

Vitest covers table filtering and selection, command enablement, SSE state merging, direct/page recognition states, userscript status, theme persistence, dialogs, and close-flow UI state. TypeScript production compilation must pass.

Desktop visual checks use WebView2 remote debugging or a packaged-window automation fallback. Screenshots at the default and minimum window sizes cover dark and light themes, empty state, active downloads, merge progress, failures, long file names, dialogs, and large history. Checks reject blank rendering, clipped controls, overlapping text, and missing assets.

The installer acceptance run must:

- Install to a non-default temporary directory.
- Verify program, frontend, ffmpeg, ffprobe, and userscript files.
- Launch one visible desktop window without an external browser window.
- Verify duplicate launch activates the same instance.
- Exercise script export to a selected test directory and validate generated metadata and token.
- Download a local direct HLS sample, verify ongoing progress, complete merge/remux, and validate output with ffprobe.
- Exercise pause/resume/cancel and close confirmation while a task is active.
- Verify theme switching and persistence after restart.
- Uninstall and confirm no downloader or ffmpeg process remains.

Delivery is blocked until Python tests, frontend tests, TypeScript build, desktop screenshot checks, packaged executable smoke tests, and installed end-to-end acceptance all pass.
