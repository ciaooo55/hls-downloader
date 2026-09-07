# v7 formal release runner

`v7 Candidate Package` remains suitable for ordinary GitHub-hosted Windows runners. A public `v7.0.1` release is intentionally stricter: `.github/workflows/release-v7.yml` only targets a dedicated self-hosted Windows x64 runner carrying the custom label `hls-release`.

The distinction is deliberate. The MSI lifecycle contract verifies the public `v7.0.0` MSI and requires the real install root `E:\h`; browser evidence needs installed Edge and Firefox; public Windows artifacts also require a code-signing private key that must never be stored in the repository or copied into an ordinary hosted runner.

## Runner contract

The `hls-release` runner must provide:

- Windows x64 and an `E:` volume. The lifecycle gate continues to reject any install root other than `E:\h`.
- Git, GitHub CLI (`gh`), Python with PyYAML, and Go on `PATH`.
- Edge and Firefox. Standard Program Files locations are auto-detected; non-standard installations can be passed as workflow-dispatch inputs or runner environment variables `HLS_V7_EDGE_BINARY` and `HLS_V7_FIREFOX_BINARY`.
- A Windows SDK containing `signtool.exe`, or an explicit `HLS_V7_SIGNTOOL` runner environment variable.
- A trusted code-signing certificate with an accessible private key in the selected `My` certificate store.
- Network access to the pinned JDK, FFmpeg, libmpv, curl-impersonate and timestamp endpoints already used by the build scripts, plus GitHub for release upload and digest verification.

The workflow itself installs the repository-pinned Rust, Node/pnpm, JDK and FFmpeg inputs. Those toolchains do not need to be permanently added to the runner.

## Release environment

Create a GitHub Actions environment named `v7-release` and protect it as appropriate for the repository. Configure these values there or at repository scope:

- Secret `HLS_V7_SIGN_CERT_THUMBPRINT`: SHA-1 certificate thumbprint from the runner certificate store. The private key itself stays on the release machine.
- Variable `HLS_V7_SIGN_CERT_STORE`: `CurrentUser` or `LocalMachine`; defaults to `CurrentUser` when omitted.
- Variable `HLS_V7_TIMESTAMP_URL`: RFC 3161 timestamp server; defaults to `http://timestamp.digicert.com` when omitted.

For a hardware-backed or externally managed signing identity, keep the private key in that provider and expose only a Windows certificate-store identity that `signtool.exe` can use. Do not add PFX files, passwords, tokens or private keys to Git.

## What the formal workflow proves

A dispatch from `main` refuses to continue unless the dispatched commit is still the current remote `main`, the matching `v7 CI` and `v7 Candidate Package` push runs succeeded, and the target version tag does not already exist. It then:

1. builds a fresh candidate from that exact commit;
2. records browser, performance, installer-upgrade and failure-rollback evidence against the same candidate manifest;
3. rechecks that `main` has not moved;
4. builds the formal package using that evidence;
5. Authenticode-signs and timestamps the top-level Windows EXE/MSI and the first-party executables carried by the Portable ZIP, then verifies signer identity and trust;
6. creates the Firefox store source bundle, CycloneDX SBOM, release evidence bundle and `SHA256SUMS.txt`;
7. uploads the staged files as a retained Actions artifact;
8. creates an annotated version tag and a **Draft** GitHub Release;
9. compares every uploaded GitHub asset's reported byte size and `sha256:` digest with the local staged file;
10. publishes the verified draft as Latest only when the dispatch `publish` switch is enabled.

A missing browser, failed threshold, MSI lifecycle regression, missing signing certificate, invalid/timestamp-free signature, changed `main`, mismatched upload digest, or any missing release asset stops the workflow. No fallback turns those failures into a public release.

## Candidate versus formal release

Use the hosted candidate workflow for ordinary development validation and downloadable test packages. Use the formal workflow only on the controlled `hls-release` machine. This preserves the existing hard release evidence instead of weakening it to fit a generic hosted runner.
