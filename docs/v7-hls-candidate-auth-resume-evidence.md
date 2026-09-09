# Candidate Portable Authenticated HLS Evidence

Date: 2026-08-27

This gate drives the existing candidate Portable ZIP rather than the Rust test
binary. It extracts `HLSDownloaderEngine.exe`, starts it with the v7 TCP Core
transport, and sends real length-prefixed v7 Core requests. A separate local
Python HTTP server requires `Authorization: Bearer hls-v7-candidate` on every
playlist and media request. The server returns `401` without that header and
records only path, authorization-present, and status fields.

## Commands

PowerShell 7:

```powershell
.\scripts\verify-hls-candidate-auth-resume.ps1 -Runs 1
```

Windows PowerShell 5.1:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-hls-candidate-auth-resume.ps1 -Runs 1
```

The script temporarily sets `HLS_V7_CORE_TCP=1`, selects a free loopback
`HLS_V7_CORE_BIND`, and uses an isolated `HLS_V7_DATA_DIR`. It removes the
temporary extracted image, server logs, and task data after each run. The
durable report is `artifacts/v7-productization/hls-candidate-auth-resume-evidence.txt`.

## Observed Results

Both commands exited `0` on 2026-08-27. The VOD and Live cases each reached
`paused` and then `completed`. Each case observed exactly one unauthenticated
playlist probe with HTTP `401`, one authenticated playlist success, one
authenticated request for each media segment, and no successful request without
Authorization.

```text
VOD: unauthorized=401, paused=paused, resumed=completed,
     output_bytes=30, first_segment_requests=1, second_segment_requests=1
Live: unauthorized=401, paused=paused, resumed=completed, checkpoint=present,
      output_bytes=32, first_segment_requests=1, second_segment_requests=1
```

The candidate ZIP used by this run is
`artifacts/v7-productization/candidate/HLSDownloader-7.0.0-Windows-x64-Portable-candidate.zip`
(SHA-256 `69fd778c4f67c4259abb3fb6923d270b99182a861c51c15769b6264e57f7c893`).
This is local authenticated fixture evidence; it does not claim validation
against an external provider endpoint.
