# Windows Desktop Lifecycle Design

## Scope

The Windows desktop package must include working FFmpeg binaries, behave like a
normal download manager in the notification area, and uninstall without leaving
application-owned state behind.

## Runtime behavior

- Closing the desktop window hides it to the notification area and keeps active
  downloads running.
- The notification icon supports double-click to restore the window and exposes
  `Open HLS Downloader` and `Exit` menu items.
- Exit has one idempotent path: stop accepting UI events, shut down active work
  through the FastAPI lifespan, wait for the server, stop the tray icon, and
  destroy the window.
- A second program launch activates the existing window. A shutdown command is
  available to the uninstaller.

## Runtime data

- Source checkouts continue to use project-relative state.
- Portable builds continue to use files next to the executable when the
  `portable` marker is present.
- Installed builds store configuration, task history, and WebView state under
  `%LOCALAPPDATA%\HLS Downloader`.
- The default installed download directory is
  `%USERPROFILE%\Downloads\HLS Downloader`, outside program and application-data
  directories.
- On first upgraded launch, legacy configuration and task history are copied to
  the new data directory. Existing absolute download paths are preserved.

## Uninstall behavior

- Uninstall is available from Windows Installed Apps, the Start menu, and the
  application's Settings dialog.
- The uninstaller first asks the running app to exit, waits, and only then uses a
  forced process termination as a fallback.
- It recursively removes the install directory, application-data directory,
  shortcuts, and registry keys.
- It asks whether downloaded videos should also be removed. The safe default is
  to preserve them; choosing yes removes the app-owned default download folder.

## Packaging and verification

- `pystray` and its image dependency are frozen into the desktop executable.
- The build selects the actual Chocolatey FFmpeg binaries and executes
  `ffmpeg -version` and `ffprobe -version` before packaging.
- Automated tests cover path selection, migration, tray close/restore/exit,
  uninstall entry points, and package contents.
- Release acceptance installs the setup package, validates FFmpeg and health,
  verifies close/exit behavior, uninstalls it, and checks owned paths and the
  local port are gone.
