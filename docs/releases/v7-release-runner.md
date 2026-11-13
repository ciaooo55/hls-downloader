# v7 formal release on hosted Windows

Candidate packaging, browser acceptance, native desktop acceptance and formal release run on disposable `windows-latest` machines. The formal workflow checks out the dispatched source and rejects non-main, stale or dirty source. It does not use the operator’s computer or a fixed local directory.

Windows EXE/MSI and Portable releases remain unsigned under the operator’s existing instruction. Manual installation is supported. The automatic updater still requires a trusted signature and cannot install these unsigned packages. Firefox is delivered as an unsigned ZIP and source bundle pending Mozilla signing; permanent installation in standard Firefox has not been accepted.

## Frozen-source sequence

1. Complete implementation and record the actual verification and remaining scope in the canonical feature matrix. Do not set readiness to bypass a failed gate.
2. Freeze `main`. Require successful exact-SHA push runs of `ci.yml`, `rust-security.yml` and `maintenance-security.yml`.
3. Dispatch `package-v7-candidate.yml`. It builds native installers/extensions and checks packaged CLI/Core/Host behavior, Host job lifetime, transfers, portable upgrade/rollback, actual MSI upgrade/failure rollback, EXE installation and visible shortcut startup.
4. Dispatch `verify-v7-candidate-browsers.yml` with that candidate run ID. It checks real Edge/Firefox media recognition, credentials, response ownership/restart recovery and actual Host/Core handoff using isolated test profiles.
5. Dispatch `verify-v7-native-candidate.yml` with that same run ID, `extended=true` and `frames=true`. It checks actual native business flows, file pickers, keyboard/queue dragging, window sizes, libmpv embedding, native redraw intervals with 1000 persisted tasks, Slint scales 1/1.25/1.5/2, public HTTPS, tail-splitting recovery and 30 wall-clock minutes of populated downloads. Software scale tests record actual OS DPI separately and do not assert physical multi-monitor acceptance.
6. After all required evidence is successful and canonical non-excluded feature gaps are closed, dispatch `release-v7.yml` from that exact frozen `main`. Keep `publish=false` for a draft, or use the already-authorized `publish=true` publication path.

A test-script overlay from another SHA remains useful for development diagnostics. It cannot satisfy formal release: both validation and candidate source identities, candidate manifest hash, component hashes and candidate workflow run ID must match.

## Release inputs and gates

The hosted runner supplies Git, GitHub CLI and Python. The workflow installs pinned Rust, Node/pnpm and media tools, plus ordinary Python validation dependencies. Native release does not require the old Compose/JDK/Java Access Bridge environment. A GitHub environment named `v7-release` is retained for repository release controls.

`receive-v7-candidate.ps1` finds successful candidate, browser and native acceptance runs for the frozen SHA. It verifies authenticated GitHub artifact ZIP digests before unpacking, then checks report identities and results. A successful short native run without the frame/layout and sustained-download reports cannot satisfy release acceptance.

The recorder retains five candidate-bound gates:

- `browser`: authenticated same-source hosted Edge/Firefox media, credentials, recovery and real Host/Core reports;
- `performance`: native software-renderer evidence, populated sustained downloads and tail recovery, plus actual local Host startup/IPC/transfer thresholds;
- `installer`: candidate-bound hosted MSI upgrade evidence;
- `rollback`: candidate-bound hosted MSI failure rollback evidence;
- `browser_media_push`: the canonical user-approved real casting/TVBox exclusion, recorded as excluded rather than passed. If the exclusion is removed, real receiver acceptance is required again.

The existing user-approved cloud subtitle quality exclusion also remains explicit. Neither exclusion removes the feature implementation or claims unexecuted tests passed. The legacy self-hosted media-push readiness workflow is outside this release path and must not be dispatched on the operator’s machine.

`build-v7.ps1 -Task package` still requires complete canonical metadata, readiness, a clean source tree and exact current release evidence. The formal workflow then refuses executable components whose hashes differ from the tested candidate. It stages installers, extensions and Firefox sources, provenance, SBOM, evidence and SHA256 sums. Uploaded GitHub asset sizes and digests must match before publication.

## Retry behavior

A release retry accepts only the same frozen version and source:

- no version tag: create an annotated tag and draft;
- matching annotated tag with no release: reuse the tag and create the draft;
- matching annotated tag and existing draft: reject unexpected assets and replace only expected staging assets before digest verification.

Lightweight tags, tags on other commits, stale source, an already-published canonical release, failed/missing evidence and upload mismatches stop publication. A retry does not rewrite published history or turn failed gates into success.

Hosted CLI/Host validation uses a nested job that allows explicit Core breakaway while retaining the outer runner limits. A separate test terminates a real Host’s named kill-on-close job and verifies that the same Core survives and accepts reconnection. This proves process lifetime behavior; the separate browser workflow supplies browser acceptance.
