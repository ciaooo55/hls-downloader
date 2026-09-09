# HLS-C024 — close media-push release wiring gaps before real-device validation

## Acceptance criteria

1. Start from exact `main` `82aae2204d9d3718f68b2cc9b5f96e0d33e48517` / tree `4c181d0e459129dbe3418234045a98ed046b84b8`.
2. Fix the C023 post-build Native Host provenance dead path without weakening its zero-residual registration/host checks or installed Host SHA binding.
3. The provenance source consumed by the media-push verifier must survive `scripts/build-v7.ps1 -Task candidate` cleanup and be cryptographically bound by `ARTIFACT-MANIFEST.json`.
4. Make the fifth formal `browser_media_push` gate accepted by `record-v7-release-gate.ps1` rather than rejected during parameter binding.
5. Ensure the default CI PowerShell validation automatically parses every `scripts/*.ps1` under Windows PowerShell 5.1 and PowerShell 7 and checks caller/recorder GateId compatibility.
6. Fix any real parse defect exposed by that expanded validation without changing media-push semantics.
7. Temporary provenance staging must be removed on success and failure.
8. Require exact-head PR v7 CI and Candidate Package success plus final source/diff audit before merge.
9. If merged, all C019 prerequisite/readiness evidence for predecessor SHAs becomes stale; restart four exact-main prerequisites before external real-device readiness.
10. Keep canonical feature metadata at 27 verified / 1 partial with `release_ready=false`; no tagging, signing, formal release, GitHub Release, or publication is authorized.

## Findings

### P0-1 — post-build provenance path was already deleted

`build-v7.ps1` deliberately removes `desktop_ui\resources\common` in its outer `finally`. C023 attempted to hash `desktop_ui\resources\common\HLSDownloaderNativeHost.exe` only after candidate build completion. A genuine readiness run would therefore fail before MSI installation even when the candidate itself was correct.

### P0-2 — final formal recorder rejected the fifth gate

`invoke-v7-release-gates.ps1` invokes `Invoke-Gate 'browser_media_push'`, but `record-v7-release-gate.ps1` still accepted only `browser`, `performance`, `installer`, and `rollback`. Final formal release would fail in parameter binding before the media-push gate executed.

### P0-3 — CI parser coverage was stale

`scripts/validate-powershell.ps1` used a hard-coded legacy list. Newer release scripts, including the media-push verifier, were absent. Therefore earlier green `Validate PowerShell 5.1 and 7` jobs did not prove those scripts parsed.

### P0-4 — expanded CI exposed an existing core parse error

Once default validation was changed to scan every `scripts/*.ps1`, Windows PowerShell 5.1 immediately rejected the C023 core string `... $AllowlistField: ...` because `:` directly after a variable name is parsed as scoped-variable syntax. C024 changes only that interpolation to `${AllowlistField}:`; the gate behavior is otherwise unchanged.

These are release-gate wiring/validation defects, not evidence that the product feature passed. Consuming a real receiver run while deterministic local failures remain would waste external validation and is prohibited.

## Implementation

### Durable Native Host provenance wrapper

The C023 verifier was moved to `scripts/verify-v7-browser-media-push-core.ps1`; after the parser finding it differs from the C023 blob only by the required `${AllowlistField}:` variable delimiter fix.

The public entry point `scripts/verify-v7-browser-media-push.ps1` now:

- requires the candidate manifest to match current source commit/tree and `package_tier=candidate`;
- resolves `artifacts.portable` inside the candidate root and verifies its SHA-256 from `ARTIFACT-MANIFEST.json`;
- expands that manifest-bound Portable ZIP into isolated runner temp;
- requires `HLSDownloader\app\resources\HLSDownloaderNativeHost.exe` inside the Portable package;
- temporarily stages exactly that Host at `desktop_ui\resources\common\HLSDownloaderNativeHost.exe`, the path the C023 core hashes;
- calls the core with the original arguments;
- requires the core result's installed Host SHA to equal the Portable Host SHA;
- emits `native_host_provenance` with candidate Portable path/SHA-256 and Portable Host SHA-256 in the final gate JSON;
- removes both extraction and temporary staging in `finally`.

The readiness workflow already persists the complete gate JSON as `GATE-SUMMARY.json` and binds that file by `gate_summary_sha256` into `READINESS-ATTESTATION.json`, so these provenance values become durable evidence without another workflow-specific copy of the logic.

The resulting chain is:

`exact source commit/tree -> ARTIFACT-MANIFEST -> Portable ZIP SHA-256 -> packaged Native Host SHA-256 -> MSI-installed Native Host SHA-256 -> Edge/Firefox registry-selected manifest -> real browser -> real receiver`.

### Formal recorder contract

`record-v7-release-gate.ps1` now accepts `browser_media_push` in `GateId`'s `ValidateSet`, matching the five gates already invoked and aggregated by `invoke-v7-release-gates.ps1`.

### CI release-script contract

`validate-powershell.ps1` now discovers all top-level `scripts/*.ps1` by default rather than maintaining a manual list. The same default run also:

- keeps the product-version drift check;
- extracts the unique literal `Invoke-Gate` IDs from `invoke-v7-release-gates.ps1`;
- requires exactly five unique gate IDs;
- extracts the recorder's `GateId` `ValidateSet`;
- fails if any invoked gate is rejected by the recorder.

This converts both the parser coverage and fifth-gate compatibility from human assumptions into executable CI invariants.

## Formal/pre-readiness coverage

Both `v7 Media Push Readiness` and final formal release call `scripts/verify-v7-browser-media-push.ps1` with a current-source candidate manifest, so the durable provenance repair covers both paths. The wrapper cleanup occurs before control returns, preventing later installer/rollback gates from inheriting its ignored staging directory.

## Validation state

The branch is `fix/hls-c024-durable-native-host-provenance`. Earlier PR-head CI runs are diagnostic only because the head moved while defects were being exposed. The final head must pass the expanded PowerShell 5.1/7 validator, the semantic GateId contract, complete v7 CI, and Candidate Package before merge.

The predecessor main prerequisite set for `82aae220...` cannot authorize C019 if C024 merges.

## Safety boundary

C024 does not fabricate real-LAN evidence and does not change canonical feature status or release readiness. No tag, signing, formal release dispatch, GitHub Release creation, or publication is authorized.