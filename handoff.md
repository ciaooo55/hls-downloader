---
repository: ciaooo55/hls-downloader
default_branch: main
product_version: 7.0.2
release_ready: false
last_updated: 2026-09-11
---

# Project handoff

## Current baseline

- The canonical product state is `artifacts/v7-productization/feature-parity.json`: 27 verified, 1 partial, 0 blocked, `release_ready=false`.
- The remaining partial feature is `browser.media_push_device_selection`; its source path is implemented, but final acceptance still requires the installed-package real-browser and real-LAN-device gate.
- The active implementation is only `desktop_ui/`, `native_shell/`, `presenter_ui/`, and `extension/`.
- `native_shell` is the sole resident Core and SQLite owner. Compose and Presenter communicate through `hls-downloader-v7-core` on `\\.\pipe\HLSDownloader.v7`.
- `main` remains the integration baseline and must not be modified or merged from this workstream without explicit operator authorization.

## `网页版gpt` workstream

- This workstream is branch-only. All current hardening stays isolated on `网页版gpt` until explicit integration authorization.
- Recent hardening makes cURL long options with inline values (`--option=value`) follow the same validation path as split arguments, including proxy, cookie-file, user and data-urlencode rejection rules.
- The reviewed Metalink XML entity fix is integrated on this branch: predefined and numeric XML entities are decoded before remote-URL safety checks, preserving signed query strings such as `&amp;`.
- Proxy route identity hardening is complete in `e38bba5e7adb05404e36e3fa7f9cf75021354c6e`: direct/system/manual identities survive task/site/global policy resolution; site-rule `system` overrides global routes; bypass remains forced-direct; system routing uses WinHTTP automatic proxy selection; curl-impersonate does not silently bypass system routing; direct curl fallback disables environment proxying.
- HTTPS/custom POST body forwarding is complete in `9b6dae814d59d0b3a128084cc5dac18949c72b2d`: WinHTTP sends the persisted request body and curl fallback preserves POST/HEAD method/body semantics without inventing Content-Type.
- Duplicate reuse request identity hardening is complete in `c1d60cce6c7534130fd7021fff352a1a879e4963`: URL-based reuse is limited to safe comparable requests and requires matching method plus normalized non-sensitive headers; credential-backed, sensitive-header and POST/body requests do not reuse stale tasks.
- FTP/FTPS stalled data-channel task control hardening is complete in `e3d843d6039111af257cb172ba093396a7eeafde`: blocking data reads no longer prevent prompt pause/cancel, including Schannel-backed FTPS.
- Native Host replay-credential rollback is complete in `73af496086029d77ae9a5c85b7a719d995e2104f`: resident Core IPC exposes credential deletion and failed/no-task browser CreateTask rolls back only the replay credential created by that request.
- Selected external HLS AUDIO result integrity is complete in `8d549b4c396d9a95d81357ef5446feb768e8264e`: VOD propagates selected-audio download failure, Live propagates worker errors and join panic, while subtitle best-effort behavior remains unchanged. Focused Windows validation plus the full HLS test group passed. The same product source then passed full v7 CI #738 and Maintenance Security #233 on connector-authored head `8e2a2a0344caf80c87096a9eea01e3a21acbab92`.
- Legacy 5.x settings migration atomicity is complete in `450046133d52289f9ee0ae8c1a684b65e5488b5f`: legacy ordinary settings are collected first and committed through the existing transactional `CoreStore::set_settings()` boundary; default-cookie decode/protect failure happens before settings writes; when a cookie update precedes a settings transaction failure, the previous credential is restored or the newly-created credential is deleted. Windows format/check, the failure-atomicity regression, normal cookie/settings import regression, and the full migrate test group passed.
- FTP URL percent decoding is fail-closed in `bf3d5a2bf2403a78621ba58ae5e0cf4be62b112b`: incomplete percent escapes, invalid hex and invalid decoded UTF-8 are rejected instead of being passed literally or replaced lossily. Existing UTF-8 user/password/path decoding and stalled-transfer pause/cancel coverage remain green in the full FTP test group.
- Media URL static recognition is tightened in `038f8b3a62c0d2414bb14a60733d0e4a194544f9`: `.m3u8` and `.mpd` classification requires a real path suffix rather than a substring such as `.m3u8.txt`, and `live=false/no/off` is case-insensitive. Content-based HTTP probing remains unchanged and can still identify opaque HLS/DASH endpoints. Format/check and all recognition tests passed.
- HLS live-name recognition false positives are closed in `b1a8fbc251fd373b1a221c04d39c8c9ea1c5c0db`: static/content-sniffed HLS classification now tests `live` only as a token in the actual URL path, so hostnames such as `live.example.com` and unrelated path components such as `deliver`/`olive` no longer force `ResourceKind::Live`; `/live/`, `live.m3u8`, `my-live-stream.m3u8`, and explicit `live=` query semantics remain covered by regressions.
- Opaque HTTP content probing now treats the `http`/`https` scheme case-insensitively in `0b8448c478833fb592b787f2ddb9052a3a3e45be`, matching the transport/safety layer. Uppercase forms such as `HTTPS://host/opaque` therefore retain HLS/DASH/page content sniffing instead of being skipped. The exact source head passed v7 CI #748 Rust Core full test/build/lint, Native presenter, Browser extensions, and contract validation before branch-only validation cleanup; the remaining Compose/package jobs are independent of these recognition changes.
- The temporary recognition validation workflow was removed in `d1a31603afaae8b1326005a64418668d48f6a1bd`; focused validation helpers are not product surface.
- Temporary repair workflows/scripts used for focused Windows validation are removed after tested source commits land; they are not product surface.
- Candidate Package #383 on pre-HLS source head `d51a1262d1f1d9ef9db1decd801c4f3d11039685` completed successfully with artifact `10188409926`, size `809272400`, digest `sha256:b365c7083475739610c75f469b9cb6a6989cda1244228ccb7e9c1325f395b0a0`.
- Candidate #387 on the HLS-validated connector head was intentionally superseded by the subsequent migration/FTP/recognition source-hardening commits; its cancellation is not a product failure. Subsequent recognition hardening remains isolated to this branch and does not change the release-readiness boundary.
- No further reviewed source defect is queued for automatic modification. Task-export replay fidelity changes require an explicit product decision because they affect whether headers/body/credential context should be portable or intentionally omitted from exported task files.
- The remaining product release gate is `browser.media_push_device_selection`. Existing source verification covers Native Host request IDs, Core persistence/resolution, Compose requested/resolved handling, completion feedback, extension polling, and a strict real-device smoke path. Do not mark it verified until the installed candidate is exercised through real browser registration, the Compose device picker, and a real LAN/TVBox receiver.

## Repository guidance

- Treat `main` plus the canonical feature matrix as the release source of truth; branch-specific hardening remains isolated until explicitly authorized for integration.
- Files under `docs/coordination/`, `docs/worker-logs/`, and `docs/manger.log` are retained historical execution evidence, not live task-assignment instructions.
- Historical Python/FastAPI, React/Tauri, WebView2, and v6 implementations remain available through Git history and tags; do not restore them as active directories.
- Candidate CI success does not authorize formal publication. Formal packaging still requires complete canonical parity, a separately reviewed `release_ready=true`, exact-main evidence, visual/performance/installer/rollback gates, signing, and explicit operator authorization.

## Validation

Repository validation commands remain defined by `AGENTS.md`. Keep `release_ready=false` until the real installed-browser/LAN media-push gate is accepted and canonical metadata is separately reviewed.
