# v7 formal release runner

`v7 Candidate Package` remains suitable for ordinary GitHub-hosted Windows runners. Formal public release is intentionally stricter: `.github/workflows/release-v7.yml` only targets a dedicated self-hosted Windows x64 runner carrying the custom label `hls-release`.

The formal version is **not hard-coded in this document**. The workflow reads `artifacts/v7-productization/feature-parity.json.product_version` from the dispatched frozen `main` SHA and derives tag `v<product_version>`. The active contract currently says `7.0.2` and `release_ready=false`, so there is no current formal v7.0.2 publication authorization. The public `v7.0.1-candidate.1` remains a historical hosted candidate, not the formal target.

The runner distinction is deliberate. The MSI lifecycle contract verifies the immutable public `v7.0.0` MSI as the upgrade baseline and requires the real install root `E:\h`; browser evidence needs installed Edge and Firefox; public Windows artifacts also require a code-signing private key that must never be stored in the repository or copied into an ordinary hosted runner.

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

The same formal signer identity is also part of runtime automatic-update authorization: formal builds embed the configured primary signer in the updater helper, while source-controlled `update-signer-trust.json` can define deliberately reviewed rollover identities. Network release metadata cannot add a trusted signer.

## What the formal workflow proves

A dispatch from `main` refuses to continue unless the dispatched commit is still the current remote `main` and four exact workflow identities have successful **push** runs for that same `main` SHA:

- `v7 CI` at `.github/workflows/ci.yml`;
- `v7 Candidate Package` at `.github/workflows/package-v7-candidate.yml`;
- `Maintenance Security` at `.github/workflows/maintenance-security.yml`;
- `Rust Security` at `.github/workflows/rust-security.yml`.

All four required workflows emit a push result for every `main` commit so an exact-SHA formal release can never be stranded by a docs-only or otherwise path-filtered merge. Pull-request execution remains path-filtered to avoid needlessly running heavyweight validation for unrelated PR changes. The formal workflow binds both workflow display name and canonical workflow path, requires `event=push`, `head_branch=main`, and the exact `GITHUB_SHA`, so a renamed or duplicate-name workflow cannot silently satisfy the gate.

A fresh release requires no existing canonical-version tag; a retry may reuse only an annotated tag that resolves to that exact same frozen commit and, when a release already exists, only while that release is still a draft. It then:

1. reads/validates the canonical product version and requires the formal package contract, including `release_ready=true`;
2. validates all four exact-SHA workflow identities before building release inputs;
3. builds a fresh candidate from that exact commit;
4. records browser, performance, installer-upgrade and failure-rollback evidence against the same candidate manifest;
5. rechecks that `main` has not moved;
6. builds the formal package using that evidence;
7. Authenticode-signs and timestamps the top-level Windows EXE/MSI and the first-party executables carried by the Portable ZIP, then verifies signer identity and trust;
8. creates the Firefox store source bundle, CycloneDX SBOM, release evidence bundle and `SHA256SUMS.txt`;
9. uploads the staged files as a retained Actions artifact;
10. creates or safely resumes the canonical annotated version tag and **Draft** GitHub Release;
11. compares every uploaded GitHub asset's reported byte size and `sha256:` digest with the local staged file;
12. publishes the verified draft as Latest only when the dispatch `publish` switch is enabled.

A missing, renamed, path-mismatched, wrong-event, wrong-branch, wrong-SHA, or failed required workflow stops the release before candidate build. `release_ready=false`, a missing browser, failed threshold, MSI lifecycle regression, missing signing certificate, invalid/timestamp-free signature, changed `main`, mismatched upload digest, or any missing release asset also stops the workflow. No fallback turns those failures into a public release.

## Safe retry semantics

The formal workflow is resumable only for release state that can be proven to belong to the same frozen `main` commit and canonical product version:

- no tag: create a fresh annotated `v<product_version>` tag and draft release;
- annotated canonical tag on the exact current commit, but no release: reuse the validated tag and create the draft;
- annotated canonical tag on the exact current commit with an existing draft release: reject unexpected assets, refresh title and notes, and replace only the expected staged assets before digest verification.

A lightweight tag, a tag pointing at any other commit, a mismatched-version tag, or an already-published release is always rejected. This lets an interrupted tag push or draft upload be retried without weakening the source, signing, release-gate, or digest checks.

## Candidate versus formal release

Use the hosted candidate workflow for ordinary development validation and downloadable test packages. Use the formal workflow only on the controlled `hls-release` machine after the canonical source state is actually release-ready. This preserves the existing hard release evidence instead of weakening it to fit a generic hosted runner.

For the active 7.0.2 iteration, `release_ready=false` means hosted candidate validation may continue while formal publication remains intentionally blocked.
