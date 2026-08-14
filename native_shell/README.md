# Native supervisor (5.0.0-alpha)

`hls-native-shell` is the resident process: tray + pre-created confirm /
progress / complete / main windows. Python stays the download core.

```
hls-native-shell --headless --core-url http://127.0.0.1:8765/api --token <token>
```

On Windows without `--headless` it creates real HWNDs (`SW_HIDE` at boot,
`ShowWindow` on offer). Closing a window hides it; the tray stays.

## Hot path

1. Supervisor boots, creates hidden overlays, `POST /desktop/native-shell/boot`.
2. Long-poll `GET /desktop/native-shell/events` (wakes on offer).
3. Confirmation paints from the event snapshot. No `GET /browser/handoffs/{id}`.
4. `POST /browser/handoffs` from the extension already returns `native-shell`.

Do not start WebView2 from this process. Do not bump `APP_VERSION` to 5.0.0
until a Windows build of this binary is the installer entrypoint.
