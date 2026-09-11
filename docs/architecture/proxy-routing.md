# Proxy routing identity

The v7 core preserves routing intent instead of collapsing routes into an empty proxy string.

- `direct` -> `hls-downloader://direct-proxy`
- `system` -> `hls-downloader://system-proxy`
- `manual` -> the configured proxy URL
- bypass matches -> forced `direct`

Site rules are resolved before the global route. Explicit site `direct`, `system`, or `manual` routes override the global mode, while bypass remains the final safety override.

At the HTTP transport boundary, direct routes disable proxying, system routes use WinHTTP automatic proxy selection on Windows, and manual routes use the named proxy URL. `curl-impersonate` is skipped for system routing so it cannot silently bypass the OS proxy decision; direct curl requests explicitly disable environment proxying with `--noproxy *`.
