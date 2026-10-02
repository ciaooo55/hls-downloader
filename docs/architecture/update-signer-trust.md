# Automatic-update signer trust

HLS-C009 closes the gap between formal release signing and runtime automatic-update acceptance.

On 2026-10-02 the operator requested unsigned formal Windows packages because no certificate is available, with no code-signed variant. Code-signing configuration is therefore not a prerequisite for the current publication workflow. This does not relax automatic-update acceptance: unsigned packages must be installed manually, and the updater continues to reject unsigned or untrusted payloads.

## Threat boundary

Before HLS-C009, automatic update already fails closed on GitHub release-asset ownership, reported size, SHA-256 digest, Windows `WinVerifyTrust`, MSI `ProductName`, exact `ProductVersion`, stable `UpgradeCode`, and per-user installation context. Those checks still allow an unrelated Windows-trusted code-signing certificate to satisfy the Authenticode layer if network-side release metadata is also compromised.

For signed builds, `HLS_V7_SIGN_CERT_THUMBPRINT` selects the primary signer identity embedded in the installed updater helper. HLS-C009 restricts automatic installation to that project signer or a deliberately versioned rollover identity. The current unsigned publication workflow does not supply a signing identity or claim automatic-upgrade support.

## Local identity sources

The updater helper uses two local sources only:

1. **Primary compiled signer.** `HLS_V7_SIGN_CERT_THUMBPRINT` is read with Rust `option_env!` and embedded when `HLSDownloaderUpdater.exe` is compiled. The current unsigned formal release does not require this secret. A build without a primary signer rejects automatic installation, including payloads signed by a rollover identity.
2. **Versioned rollover set.** `artifacts/v7-productization/update-signer-trust.json` is compiled into the helper. Network release JSON, asset metadata, release notes, or URLs cannot add a signer.

The identity is the normalized 40-hex SHA-1 thumbprint of the already-verified Authenticode leaf certificate. SHA-1 is used here only as the Windows certificate identity for signed automatic-update payloads; file integrity remains SHA-256 and Authenticode chain validation remains `WinVerifyTrust`.

## Runtime order

The main process still performs its existing downloaded-file and MSI checks before launching the updater helper. The helper then adds a second fail-closed authorization boundary before `msiexec`:

1. parse the already-existing `--msi` helper argument;
2. run `WinVerifyTrust(WINTRUST_ACTION_GENERIC_VERIFY_V2)` using the existing cache-only/revocation policy;
3. obtain provider data from the verified WinTrust state;
4. obtain the primary signer and its leaf certificate;
5. read `CERT_SHA1_HASH_PROP_ID` from that certificate context;
6. require the normalized thumbprint to equal the compile-time formal signer or one reviewed rollover signer;
7. only then call the existing `run_update_helper`, which repeats Authenticode verification and enforces MSI product/version/upgrade/per-user identity before installation.

Missing formal signer identity, malformed local trust data, missing WinTrust provider/signer/certificate state, or signer mismatch prevents installation.

## Deliberate certificate rotation

If signed automatic-update releases are authorized in the future, certificate rotation remains a two-phase source-controlled operation. It is not required to publish the current unsigned packages.

### Phase A — bridge release

Before changing the signing certificate:

1. obtain and review the future certificate;
2. add its thumbprint to `rollover_signers` with a non-empty reason;
3. publish a normal formal release still signed by the old certificate;
4. keep the bridge release available long enough for installed clients to acquire the future signer authorization.

### Phase B — cutover

After the overlap window:

1. change the protected formal-release `HLS_V7_SIGN_CERT_THUMBPRINT` to the new certificate;
2. publish the next formal release signed by the new certificate;
3. bridge clients accept it through the compiled rollover identity;
4. the new binary embeds the new certificate as its primary signer.

Old rollover entries should be removed when compatibility no longer requires them. Clients that never install a bridge release do not learn an unannounced future signer; that is intentional fail-closed behavior.

## Trust contract

```json
{
  "schema": 1,
  "identity": "authenticode_leaf_certificate_sha1",
  "policy": "formal-build-signer-or-explicit-rollover",
  "rollover_signers": []
}
```

A rollover entry is an object with `thumbprint` and non-empty `reason`. The parser rejects unknown fields, unknown schema/identity/policy values, malformed thumbprints, duplicate thumbprints, and empty reasons.

## Validation

Windows CI must compile and run the updater-helper tests. Pure authorization tests cover primary acceptance, rollover acceptance, unrelated-signer rejection, missing formal signer, malformed/duplicate rollover data and MSI argument extraction. A Windows-only test additionally proves that the real WinTrust signer-extraction path rejects an unsigned payload.

The existing SHA-256, MSI identity, WinVerifyTrust and compiled signer-ownership checks remain in force for automatic updates. Current formal publication stages unsigned assets without a signing or timestamp gate; publishing them does not authorize the updater to install them automatically.
