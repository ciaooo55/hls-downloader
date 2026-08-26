# HLS Authenticated VOD/Live Pause-Resume Evidence

Date: 2026-08-26

This evidence uses a loopback HTTP server inside the Rust test process. The
server requires `Authorization: Bearer hls-v7-test` for the playlist and every
media request, returns `401` to an unauthenticated probe, and records request
counts. No GUI, external endpoint, credential, cookie, or network service is
used.

## Verification

Reusable command:

```powershell
.\scripts\verify-hls-auth-resume.ps1
```

The default command runs three repetitions per test. The same command
completed under PowerShell 7.6 and Windows PowerShell 5.1, with exit status
`0` in both hosts. The complete captured output is kept in
`artifacts/v7-productization/hls-auth-resume-evidence.txt`.

The script runs these exact focused tests:

```text
cargo test --manifest-path native_shell/Cargo.toml --lib media::hls::tests::authenticated_vod_pause_resume_reuses_completed_segments -- --exact --nocapture
cargo test --manifest-path native_shell/Cargo.toml --lib media::hls::tests::authenticated_live_pause_resume_restores_atomic_timeline -- --exact --nocapture
```

Observed result on 2026-08-26 (three repetitions per test):

```text
authenticated_vod_pause_resume_reuses_completed_segments: 3/3 passed, 0 failed, exit 0
authenticated_live_pause_resume_restores_atomic_timeline: 3/3 passed, 0 failed, exit 0
```

Baseline command and result:

```text
cargo test --manifest-path native_shell/Cargo.toml --lib media::hls::tests::vod_playlist_concatenates_ts_segments -- --exact --nocapture
test result: ok. 1 passed; 0 failed; exit 0
```

## Covered Behavior

- Missing authorization is rejected with HTTP `401`.
- Authenticated VOD playlist and segments download successfully.
- VOD pause returns `paused`; resume produces `AAABBB` and requests the first
  segment once and the second segment once.
- Authenticated Live pause returns `paused` after the first segment is stored.
- Pause persists `live_state.json` with version `2` and one timeline entry.
- Live resume avoids refetching segment zero, fetches segment one once, and
  produces `LIVE0LIVE1` in media-sequence order.
