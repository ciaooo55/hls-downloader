# Native supervisor (5.0.0)

`HLSNativeShell.exe` is packaged next to `HLSDownloader.exe`. The desktop
shell starts it after the Python core is listening. On Windows it creates
hidden confirm / progress / complete / main HWNDs first, then
`POST /desktop/native-shell/boot`. The existing desktop tray stays the
Open/Quit icon so the user does not get two tray icons.

```
HLSNativeShell.exe --core-url http://127.0.0.1:8765/api --token <token>
```

Linux tests use `--headless`. Do not start WebView2 from this process.
HTTP GET jobs arrive as `kind: "http_job"` on the same event pipe and run on
a worker thread. `--job` remains a fallback when this process is not polling.
