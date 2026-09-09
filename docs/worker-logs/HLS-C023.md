# HLS-C023 — bind Native Messaging readiness evidence to the candidate host

## Acceptance criteria

1. Start from exact `main` `178bf4276d9f2c8c8c485286a0ad66e5d59dd468` / tree `4c3344becb42f635a4b9bf3049276bd467490ee3` and treat its previously completed C019 prerequisite runs as predecessor evidence once this fix merges.
2. The installed-browser media-push gate must reject a stale or unrelated Native Messaging registration even when the registry key points to an existing JSON file.
3. Edge and Firefox registration must resolve to valid UTF-8 JSON manifests with exact host name `com.ciaooo55.hls_downloader`, `type=stdio`, the browser-specific production allowlist, and a `path` that resolves exactly to the current candidate install's `E:\h\app\resources\HLSDownloaderNativeHost.exe`.
4. The candidate Native Host executable must exist; its SHA-256 and each manifest SHA-256 must be persisted in the gate summary.
5. The pre-readiness attestation must independently revalidate the same host path/hash and browser-specific registration contract before persisting `native_host_executable` and registration snapshots.
6. Keep canonical feature metadata unchanged at 27 verified / 1 partial / `release_ready=false`; this source-hardening task does not authorize feature promotion, readiness, tagging, signing, release creation, formal dispatch, or publication.
7. Require exact-head source validation from ordinary PR CI and Candidate Package. Any fallback source/diff audit must be explicitly labeled non-independent.
8. If merged, invalidate all C019 prerequisite/readiness evidence for predecessor SHAs and restart the four exact-main prerequisites plus external `v7 Media Push Readiness` against the new merge SHA.

## Finding

The C019 reverse audit found that `scripts/verify-v7-browser-media-push.ps1` previously proved only that Edge and Firefox each had one matching Native Messaging registry entry whose referenced manifest file existed. It did **not** parse that manifest and prove that the browser would launch the Native Host installed by the candidate MSI.

A dedicated runner with no MSI product installed can still contain stale HKCU Native Messaging keys or leftover manifest/host files from an earlier run. Under that state the old check could accept a registration snapshot without cryptographically or path-wise binding it to the candidate currently under test.

The registration implementation also legitimately supports two manifest locations: the Engine directory when writable, or `%LOCALAPPDATA%\HLSDownloader\v7-native-host` as a fallback. Therefore the safe invariant is not "manifest must live under the install directory"; it is "the registry-selected manifest must itself point to the exact candidate-installed Native Host, and that host identity must be hashed and attested."

## Implementation

### Commit `5bf5671fdb59102c33a2be2d88ac64623c97e4fd`

`verify-v7-browser-media-push.ps1` was hardened to:

- match only the exact host registry name `com.ciaooo55.hls_downloader`;
- parse the registry-selected manifest as UTF-8 JSON;
- require `name=com.ciaooo55.hls_downloader` and `type=stdio`;
- require the manifest-declared host path to equal the expected candidate-installed Native Host;
- require that host executable to exist and record its SHA-256;
- record each selected manifest SHA-256;
- require Edge `allowed_origins` to contain exactly `chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/`;
- require Firefox `allowed_extensions` to contain exactly `hls-downloader-store@ciaooo55.com`;
- require both browser registration snapshots to bind to the same candidate host digest;
- emit `native_host_executable = { path, sha256 }` in the gate summary.

### Commit `65b6c3f3393cc363c31364330e77d1a0d3ba29d0`

`.github/workflows/v7-media-push-readiness.yml` was hardened to independently revalidate the Native Host path/hash and browser-specific registration contracts before writing the readiness attestation.

### Reverse-audit correction before merge

A second source audit caught an incorrect packaging assumption in the first C023 implementation: it initially expected the Native Host directly at `E:\h\HLSDownloaderNativeHost.exe`.

Canonical MSI lifecycle source proves the installed Engine path is `E:\h\app\resources\HLSDownloaderEngine.exe`, and production native-host registration resolves `HLSDownloaderNativeHost.exe` as the Engine's sibling. Therefore the exact installed candidate Native Host contract is:

`E:\h\app\resources\HLSDownloaderNativeHost.exe`

This was corrected before merge:

- commit `1bf7818f70c2413a9271b7c04d65068019cd98c0` corrects the gate's expected packaged Native Host path;
- commit `a619c952ac2dcb6a5e24d4ed94f8df1ce757636c` corrects the readiness attestation's independent expected path;
- commit `261a5cc3e7515c4599839cea931a8f41e14e959c` records the package-layout invariant in the architecture document.

The earlier PR-head CI/Candidate runs are not accepted as final evidence after these commits; exact-head runs for the corrected PR head are required.

## Source audit notes

`native_shell/src/native_host_registration.rs` confirms the production registration contract used by the assertions:

- host name: `com.ciaooo55.hls_downloader`;
- executable: sibling `HLSDownloaderNativeHost.exe` of the Engine;
- Chromium allowlist: `chrome-extension://bbdfldcjnikaemnimalegbopgaknjhla/`;
- Firefox allowlist: `hls-downloader-store@ciaooo55.com`;
- preferred manifest location: Engine directory;
- fallback manifest location: `%LOCALAPPDATA%\HLSDownloader\v7-native-host`.

`scripts/verify-v7-msi-lifecycle.ps1` independently establishes that an installed candidate's Engine lives under `E:\h\app\resources\HLSDownloaderEngine.exe`. Combining those contracts gives the exact candidate host location used by C023 without constraining where the JSON manifest itself is stored.

## Validation state

PR #89 tracks this source-hardening task. The initial head `ba585cb80569dccd60dbf00d14dcec9948953d2a` reached successful PowerShell 5.1/7 contract parsing, but the subsequent package-layout correction moved the PR head. Those predecessor-head results are diagnostic only and cannot satisfy final acceptance.

Final acceptance requires fresh exact-head v7 CI and Candidate Package success plus a final source/diff audit. If single-participant fallback is used for that audit, it must be clearly labeled non-independent.

The old C019 frozen target `178bf4276d9f2c8c8c485286a0ad66e5d59dd468` remains untouched until this PR is accepted. If C023 merges, that SHA and its four prerequisite successes become predecessor-only evidence and cannot authorize the subsequent external run.

## Safety boundary

No real-LAN receiver evidence is fabricated here. No feature status or `release_ready` metadata is changed. No tag, signing, formal release dispatch, GitHub Release, or publication is authorized by C023.
