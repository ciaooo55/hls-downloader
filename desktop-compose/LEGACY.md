# Legacy Compose shell

This directory is **not** the shipping desktop shell.

Primary product shell: Tauri 2 + React (`frontend/` + `frontend/src-tauri/` or root packaging).

Do not reintroduce Compose as the default UI. Installer removes leftover `app/` and `runtime/` Compose folders on upgrade.
