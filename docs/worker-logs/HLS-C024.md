# HLS-C024 — make candidate Native Host provenance survive build cleanup

## Acceptance criteria

1. Start from exact `main` `82aae2204d9d3718f68b2cc9b5f96e0d33e48517` / tree `4c181d0e459129dbe3418234045a98ed046b84b8`.
2. Fix the C023 runtime dead path without weakening its zero-residual registration/host checks or its installed Host SHA binding.
3. The provenance source consumed by the media-push verifier must survive `scripts/build-v7.ps1 -Task candidate` cleanup and be cryptographically bound by `ARTIFACT-MANIFEST.json`.
4. The same repaired entry point must cover both pre-readiness `v7 Media Push Readiness` and final formal `browser_media_push` invocation.
5. Temporary provenance staging must be removed on both success and failure.
6. Require exact-head PR v7 CI and Candidate Package success plus final source/diff audit before merge.
7. If merged, all C019 prerequisite/readiness evidence for predecessor SHAs becomes stale; restart four exact-main prerequisites before external real-device readiness.
8. Keep canonical feature metadata at 27 verified / 1 partial with `release_ready=false`; no tagging, signing, formal release, GitHub Release, or publication is authorized.

## Finding

After C023 merged, reverse audit of the real candidate build lifecycle found that `build-v7.ps1` deliberately removes `desktop_ui\resources\common` in its outer `finally`. C023's final Host-provenance check attempted to read `desktop_ui\resources\common\HLSDownloaderNativeHost.exe` only after the candidate build had returned. Therefore a genuine readiness run would fail before MSI installation even when the candidate itself was correct.

This is a release-gate wiring defect, not a product feature defect. Consuming a real receiver run while the failure is deterministic would waste external validation and must be avoided.

## Implementation

Commit `6e4053f7c1f955e10f748f68bd232da9de55f0d0` preserves the C023 verifier byte-for-byte as `scripts/verify-v7-browser-media-push-core.ps1` and replaces the original entry point with a narrow provenance wrapper.

The wrapper:

- requires the candidate manifest to match the current commit/tree and `package_tier=candidate`;
- resolves the manifest's `artifacts.portable` entry inside the candidate root and verifies the ZIP SHA-256 from the manifest;
- expands that manifest-bound Portable ZIP into an isolated runner temp directory;
- requires `HLSDownloader\app\resources\HLSDownloaderNativeHost.exe` inside the Portable package;
- temporarily stages exactly that Host at `desktop_ui\resources\common\HLSDownloaderNativeHost.exe`, the path C023 core already hashes;
- calls the unchanged C023 core verifier with the original arguments;
- removes both the temporary extraction directory and generated staging directory in `finally`.

The core then requires the MSI-installed `E:\h\app\resources\HLSDownloaderNativeHost.exe` SHA-256 to equal the Host SHA restored from the manifest-bound Portable ZIP. This yields a durable chain:

`exact source commit/tree -> ARTIFACT-MANIFEST -> portable ZIP SHA-256 -> packaged Native Host SHA-256 -> MSI-installed Native Host SHA-256 -> Edge/Firefox registry-selected manifest -> real browser path`.

No repository build staging directory is treated as durable evidence anymore.

## Formal-release coverage

`invoke-v7-release-gates.ps1` already invokes `scripts/verify-v7-browser-media-push.ps1` with the exact current candidate manifest, and `v7-media-push-readiness.yml` invokes the same entry point. Replacing that entry point therefore repairs both the pre-readiness and final formal media-push gates without duplicating provenance logic.

The wrapper cleans generated staging before returning, so later installer/rollback gates do not inherit its temporary resource state.

## Validation state

Source hardening is on `fix/hls-c024-durable-native-host-provenance`. Exact-head CI/Candidate validation and final audit are required before merge. The predecessor main prerequisite set for `82aae220...` cannot authorize C019 if C024 is merged.

## Safety boundary

C024 does not fabricate real-LAN evidence and does not change canonical feature status or release readiness. No tag, signing, formal release dispatch, GitHub Release creation, or publication is authorized.