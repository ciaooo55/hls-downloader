# Automatic-update signer trust

HLS-C009 closes the gap between formal release signing and runtime automatic-update acceptance.

On 2026-10-02 the operator requested unsigned formal Windows packages because no certificate is available, with no code-signed variant. Code-signing configuration is therefore not a prerequisite for the current publication workflow. This does not relax automatic-update acceptance: unsigned packages must be installed manually, and the updater continues to reject unsigned or untrusted payloads.

## Threat boundary

Before HLS-C009, automatic update already fails closed on GitHub release-asset ownership, reported size, SHA-256 digest, Windows `WinVerifyTrust`, MSI `ProductName`, exact `ProductVersion`, stable `UpgradeCode`, and per-user installation context. Those checks still allow an unrelated Windows-trusted code-signing certificate to satisfy the Authenticode layer if network-side release metadata is also compromised.

For signed builds, `HLS_V7_SIGN_CERT_THUMBPRINT` selects the primary signer identity embedded in the installed updater helper. HLS-C009 restricts automatic installation to that project signer or a deliberately versioned rollover identity. The current unsigned publication workflow does not supply a signing identity or claim automatic-upgrade support.

## Local identity sources

The updater helper uses two local sources only:

1. **Primary compiled signer.** `HLS_V7_SIGN_CERT_THUMBPRINT` is read with Rust `option_env!` and embedded when `HLSDownloaderUpdater.exe` is compiled. The current unsigned formal release does not require this secret. A build without a primary signer rejects automatic installation, including payloads signed by a rollover identity.
2. **Versioned rollover set.** `config/release/update-signer-trust.json` is compiled into the helper. Network release JSON, asset metadata, release notes, or URLs cannot add a signer.

The identity is the normalized 40-hex SHA-1 thumbprint of the already-verified Authenticode leaf certificate. SHA-1 is used here only as the Windows certificate identity for signed automatic-update payloads; file integrity remains SHA-256 and Authenticode chain validation remains `WinVerifyTrust`.

## Runtime order

The main process checks the downloaded asset, Authenticode trust and MSI identity before launching the helper. The helper executable passes its arguments directly to `hls_client::update_install::run_update_helper`. The client library owns the embedded signer policy and performs one Authenticode verification inside the helper.

1. Parse the helper arguments and open the MSI for reading with `FILE_SHARE_READ`. Windows denies write and delete access while this handle is open.
2. Pass that handle to `WinVerifyTrust(WINTRUST_ACTION_GENERIC_VERIFY_V2)` with cache-only retrieval and no revocation checks. From the verified provider state, read the primary signer's leaf certificate thumbprint using `CERT_SHA1_HASH_PROP_ID`.
3. Validate the embedded primary identity and the complete rollover policy, then require the verified thumbprint to match an authorized signer. Close the WinTrust state after this check.
4. Check the MSI product name, version, upgrade code and per-user context. Keep the same file handle open through product shutdown and until `msiexec` returns, including any installer-busy retries.

Both helper entry points require these checks on Windows. `run_update_helper_with_verifier` can additionally reject the locked package before the mandatory checks. A lock, trust, policy or identity failure prevents installation.

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

Windows CI must run the client library and updater binary tests. The library tests exercise the primary and rollover policy, argument parsing, Windows sharing restrictions and unsigned payload rejection. The production updater entry point must record a WinTrust failure before installation when given an unsigned package.

The existing SHA-256, MSI identity, WinVerifyTrust and compiled signer-ownership checks remain in force for automatic updates. Current formal publication stages unsigned assets without a signing or timestamp gate; publishing them does not authorize the updater to install them automatically.
