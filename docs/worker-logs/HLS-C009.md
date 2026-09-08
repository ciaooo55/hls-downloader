# HLS-C009 — Bind runtime automatic updates to the project release signer identity

## Acceptance criteria

1. Automatic update rejects an MSI whose Authenticode chain is valid but whose signer is not an explicitly trusted HLS Downloader release signer.
2. Expected signer identity is local/versioned, not learned from GitHub release metadata or another network-controlled field.
3. Certificate rotation is deliberate and supported without trusting arbitrary Windows-trusted code-signing certificates.
4. Existing SHA-256, size, exact product version, ProductName, UpgradeCode, per-user MSI and WinVerifyTrust checks remain fail-closed.
5. Windows CI covers accepted signer and wrong-signer authorization before merge.

## Assignment

- Priority: `P1`
- Worker: `worker-1`
- Original branch: `fix/hls-c009-updater-signer`
- Rebased implementation branch: `fix/hls-c009-updater-signer-v2`
- Source task: HLS-C005

## Resume / route correction

The original branch was created before ten later main commits and contained only the trust JSON, architecture design and this work log. On session recovery, `worker-1` found it diverged by 3 ahead / 10 behind. Rather than implement security code against stale runtime sources, the worker created `fix/hls-c009-updater-signer-v2` from current main `a2ec36c526ee19f5242280b34226eb0f6863abb2` and migrated the reviewed trust design.

## Implementation

### Local trust contract

Commit `96fd34f` adds `artifacts/v7-productization/update-signer-trust.json`. The empty rollover list means no extra certificate is trusted by default.

### Runtime gate

Commit `9ddf57e` changes only the independent `HLSDownloaderUpdater.exe` entrypoint rather than the large shared updater module. This is the narrowest installation boundary: the helper is the process that ultimately invokes `msiexec`.

Before calling existing `hls_native_shell::run_update_helper`, the helper now:

1. extracts the existing `--msi` argument;
2. validates Authenticode using `WinVerifyTrust`;
3. obtains the signer certificate from that verified WinTrust state;
4. reads its SHA-1 certificate thumbprint;
5. requires it to equal compile-time `HLS_V7_SIGN_CERT_THUMBPRINT` or a reviewed rollover thumbprint compiled from the local trust JSON.

A missing formal signer, malformed trust contract, missing verified signer state, or wrong signer exits before existing installation logic.

The original library helper still runs afterwards, so its second WinVerifyTrust pass and MSI ProductName/ProductVersion/UpgradeCode/per-user checks remain intact.

### Tests

The updater-helper binary contains tests for:

- formal primary signer accepted;
- explicit rollover signer accepted;
- unrelated signer rejected;
- missing formal signer rejected;
- malformed/duplicate/unexplained rollover entries rejected;
- helper `--msi` extraction;
- on Windows, an unsigned MSI-shaped payload rejected by the actual WinTrust signer-extraction path.

## Pending evidence

The Windows CI must compile the `windows-sys 0.59` WinTrust/Cryptography calls and run the binary tests. No PASS is claimed until current-head CI succeeds and an independent auditor reviews the exact PR head.
