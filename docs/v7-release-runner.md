# v7 formal release runner

`v7 Candidate Package` remains suitable for ordinary GitHub-hosted Windows runners. `.github/workflows/release-v7.yml` targets an on-demand ephemeral self-hosted Windows x64 runner carrying the custom label `hls-release`; readiness uses `hls-readiness`. Both workflows use the sole source directory `A:\Ubuntu\测试\hls-downloader` and do not run `actions/checkout`. The operator prepares that clean local `main` at the dispatched SHA before starting the runner; the workflow checks it against remote `main`.

On 2026-10-02 the operator explicitly requested unsigned formal Windows packages because no code-signing certificate is available. Signing tools, certificates and timestamp services are not prerequisites for this release. Unsigned packages require manual installation; the automatic updater retains its signature and signer-trust checks and must reject them.

The formal version is **not hard-coded in this document**. The workflow reads `artifacts/v7-productization/feature-parity.json.product_version` from the dispatched frozen `main` SHA and derives tag `v<product_version>`. The active contract currently says `7.0.2` and `release_ready=false`, so there is no current formal v7.0.2 publication authorization. The public `v7.0.2-candidate.1` remains a historical hosted candidate, not the formal target; `v7.0.1-candidate.1` is older history.

The public `v7.0.2-candidate.1` is the latest hosted candidate, but it targets remote commit `74f8473` and is not evidence for a newer `main` SHA. The MSI lifecycle contract verifies the immutable public `v7.0.0` MSI as the upgrade baseline and requires the real install root `E:\h`; browser evidence needs installed Edge and Firefox; installed-browser media-push evidence also needs a real interactive desktop and reachable LAN receiver.

## Runner contract

The `hls-release` runner must provide:

- Windows x64 and an `E:` volume. The lifecycle gate continues to reject any install root other than `E:\h`.
- An **interactive logged-in Windows desktop session** for browser/Compose evidence. Run the GitHub runner agent from the release user's desktop session, not as a Session-0 Windows service. The media-push readiness workflow fails before toolchain/bootstrap work when the runner process is in Session 0, `UserInteractive` is false, or the current session has no Explorer desktop. Keep the release desktop available/unlocked while the real browser + Java Access Bridge gate is running.
- Git, GitHub CLI (`gh`), Python with PyYAML, and Go on `PATH`.
- Edge and Firefox. Standard Program Files locations are auto-detected; non-standard installations can be passed as workflow-dispatch inputs or runner environment variables `HLS_V7_EDGE_BINARY` and `HLS_V7_FIREFOX_BINARY`. Optional pinned matching WebDriver paths can be supplied as `HLS_V7_EDGE_DRIVER` and `HLS_V7_FIREFOX_DRIVER`; otherwise Selenium Manager resolves the driver.
- A clean HLS Downloader install state before MSI-bound browser validation. The gate refuses to run when a related HLS Downloader product is already installed instead of silently reusing it.
- Network access to the pinned JDK, FFmpeg, libmpv and curl-impersonate downloads used by the build scripts, plus GitHub for release upload and digest verification.
- Layer-2/LAN reachability to the configured real TVBox receiver used by the installed-browser media-push gate. Discovery and receiver fetch are hard requirements, not optional smoke coverage.

The workflow itself installs the repository-pinned Rust, Node/pnpm, JDK and FFmpeg inputs. Those toolchains do not need to be permanently added to the runner.

## Release environment

Create a GitHub Actions environment named `v7-release` and protect it as appropriate for the repository. Configure these values there or at repository scope:

- Variable `HLS_V7_TVBOX_EXPECTED_HOST`: exact **private IPv4** address of the dedicated real TVBox receiver on the runner's LAN. The receiver must have its TVBox receive service enabled and must be able to fetch the deterministic media URL served by the runner. Missing, non-private, undiscoverable, wrong-host, or non-fetching receivers fail the readiness/formal media-push gate; there is no skip fallback.

Do not configure a signing identity for the current unsigned release. Runtime automatic-update authorization remains unchanged: the updater requires a trusted Authenticode signature and a compiled primary or reviewed rollover signer. Network release metadata cannot add a trusted signer or authorize an unsigned package.

## Pre-readiness media-push validation

While canonical `release_ready=false`, `.github/workflows/v7-media-push-readiness.yml` is the approved way to collect the missing installed-browser media-push evidence before the metadata readiness decision. It runs by `workflow_dispatch` on an on-demand ephemeral `hls-readiness` runner in the `v7-release` environment, using the same sole local source directory without checkout. It installs and uninstalls the candidate to collect evidence, but does not change readiness metadata or publish a release.

The workflow refuses non-`main` or stale source, requires `release_ready=false`, requires `HLS_V7_TVBOX_EXPECTED_HOST`, verifies the interactive desktop contract before expensive build work, builds a fresh candidate from that exact commit, installs the candidate MSI, validates the installed Edge/Firefox Native Messaging registrations, drives the production browser -> Native Host -> Core -> Compose DevicePicker path in both browsers, and requires the configured receiver to fetch the deterministic media fixture. It then reconfirms remote `main` did not move and uploads a source/candidate/browser/receiver-bound readiness attestation.

This pre-readiness attestation is evidence for the later narrow feature-metadata promotion only. It does **not** set `release_ready=true`, create a tag, sign artifacts, create a GitHub Release, or substitute for the final-SHA formal media-push gate.

## What the formal workflow proves

A dispatch from `main` refuses to continue unless the dispatched commit is still the current remote `main` and four exact workflow identities have successful runs for that same `main` SHA:

- `v7 CI` at `.github/workflows/ci.yml`;
- `v7 Candidate Package` at `.github/workflows/package-v7-candidate.yml` (explicit `workflow_dispatch` from `main`);
- `Maintenance Security` at `.github/workflows/maintenance-security.yml`;
- `Rust Security` at `.github/workflows/rust-security.yml`.

Candidate packaging runs only on explicit manual dispatch, not on every merge. The other three prerequisites require `event=push`. Both readiness and formal release use `scripts/assert-v7-exact-main-workflows.ps1`, which binds each workflow's display name, canonical path and expected event, plus `head_branch=main` and the exact `GITHUB_SHA`. A renamed, duplicate-name, wrong-event or stale workflow cannot satisfy the gate. After freezing `main`, explicitly run the candidate workflow once and ensure the three push prerequisites succeeded before dispatching readiness.

A fresh release requires no existing canonical-version tag; a retry may reuse only an annotated tag that resolves to that exact same frozen commit and, when a release already exists, only while that release is still a draft. It then:

1. checks that the dispatch is the current `main`, reads/validates the canonical product version, and validates existing tag/draft retry state; this early step does **not** change or bypass `release_ready`;
2. validates all four exact-SHA prerequisite workflow identities before building release inputs;
3. verifies the dedicated Windows release runner and bootstraps the pinned toolchains;
4. builds a fresh candidate from that exact commit;
5. records five candidate-bound release gates against that same candidate manifest: packaged-browser behavior, installed-browser-to-real-LAN media push, performance, installer upgrade, and failure rollback;
6. rechecks that `main` has not moved;
7. builds the formal package using that evidence; `build-v7.ps1 -Task package` is where canonical completeness, current release evidence, clean-worktree and `release_ready=true` are enforced;
8. retains unsigned Windows EXE/MSI and Portable ZIP artifacts according to the operator's 2026-10-02 instruction;
9. creates the Firefox store source bundle, CycloneDX SBOM, release evidence bundle and `SHA256SUMS.txt`;
10. uploads the staged files as a retained Actions artifact;
11. creates or safely resumes the canonical annotated version tag and **Draft** GitHub Release;
12. compares every uploaded GitHub asset's reported byte size and `sha256:` digest with the local staged file;
13. publishes the verified draft as Latest only when the dispatch `publish` switch is enabled.

A missing, renamed, path-mismatched, wrong-event, wrong-branch, wrong-SHA, or failed required prerequisite workflow stops the release before candidate build. `release_ready=false`, a residual canonical feature gap, a missing browser, non-interactive desktop, missing/wrong LAN receiver, failed receiver fetch, failed performance threshold, MSI lifecycle regression, changed `main`, mismatched upload digest, or any missing release asset also stops the workflow. No fallback turns those failures into a public release.

## Safe retry semantics

The formal workflow is resumable only for release state that can be proven to belong to the same frozen `main` commit and canonical product version:

- no tag: create a fresh annotated `v<product_version>` tag and draft release;
- annotated canonical tag on the exact current commit, but no release: reuse the validated tag and create the draft;
- annotated canonical tag on the exact current commit with an existing draft release: reject unexpected assets, refresh title and notes, and replace only the expected staged assets before digest verification.

A lightweight tag, a tag pointing at any other commit, a mismatched-version tag, or an already-published release is always rejected. This lets an interrupted tag push or draft upload be retried without weakening the source, release-gate, or digest checks.

## Candidate versus formal release

Use the hosted candidate workflow for ordinary development validation and downloadable test packages. Use the formal workflow only on the controlled `hls-release` machine after the canonical source state is actually release-ready. Use the pre-readiness media-push workflow only to collect the candidate-bound real-browser/LAN evidence needed to resolve the remaining media-push feature gap while readiness is still false. This preserves the existing hard release evidence instead of weakening it to fit a generic hosted runner.

For the active 7.0.2 iteration, `release_ready=false` means hosted candidate validation may continue while formal publication remains intentionally blocked.
