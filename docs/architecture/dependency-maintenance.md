# Dependency maintenance policy

This document records the project-specific policy derived from HLS-C004. It is not a blanket instruction to keep every dependency at the newest major version. The release line values security fixes and reproducibility while requiring compatibility evidence for major changes.

## Risk classes

### 1. Runtime security dependencies — highest priority

Examples: SSH/SFTP libraries, database/runtime libraries, cryptography or transport components.

A security-bearing update should be handled promptly. If the update changes a public API, review and adapt it on local `main` while preserving the affected trust/behavior contract. Do not create a dependency-update branch or PR; remote synchronization requires explicit operator authorization.

Repository precedent: Dependabot PR #20 proposed `russh` 0.63.2, which included security fixes and a breaking host-key callback API. The project closed the raw update and merged PR #36, which adapted the SFTP TOFU handler to `PublicKeyOrCertificate` while continuing to fingerprint the underlying public key.

Acceptance evidence should include the component's tests, compile/lint checks, and a review of any changed trust boundary.

### 2. Windows binding/API majors — compatibility first

`windows-sys` is a low-level binding used across updater trust verification, WinHTTP/network code, shell/registry integration, IPC, power handling, and other Windows-specific paths. A major binding bump can require signature/type/constant changes without itself providing a product security fix.

Current policy: retain the working 0.59 line unless one of these triggers exists:

- a relevant security advisory requires a newer binding/runtime;
- a required Win32 API is unavailable or incorrect in the current version;
- a Rust/toolchain compatibility issue forces migration;
- a dedicated compatibility task can validate all affected Windows build/test paths.

Historical PR #19 (`windows-sys` 0.59 -> 0.61.2) closed without merge after its Dependabot branch became non-rebasable. HLS-C004 found no evidence that this update is a release-blocking security requirement, so it must not be recreated casually during v7.0.2 release hardening.

### 3. GitHub Actions — supply-chain pinning plus CI validation

Action updates may carry security fixes and runner-runtime changes. After selecting an updated action version, workflows must continue to reference immutable commit SHAs rather than mutable major tags.

The repository's merged Actions update (#21) follows this model. Future updates must verify hosted workflow compatibility and, where the formal release workflow is affected, the self-hosted runner requirements as well.

### 4. Compiler, test, and development-tool majors

Examples: TypeScript, Vitest, type definitions.

These changes do not normally justify destabilizing the release line solely for version freshness. Accept them when the actual project checks prove compatibility:

- extension: `tsc --noEmit`, Vitest, Chrome/Firefox extension build;
- Rust: locked build/test/clippy/fmt and Rust security audit where relevant;
- Compose: Gradle test plus distributable/package validation.

Historical TypeScript 7 and Vitest 5 updates are acceptable precedents because those checks remained active after their merges.

### 5. UI/runtime build-stack majors

Compose, Kotlin, serialization, coroutines and Gradle affect both build compatibility and shipped desktop behavior. A grouped update should be accepted only when the grouping is coherent and the Compose workbench/distribution pipeline passes. Avoid unrelated broad batches that make regressions hard to attribute.

## Triage outcomes

Every dependency update should end in one of four explicit states:

- `land`: compatible and validated;
- `adapt`: update is valuable but requires project-owned code changes before landing;
- `defer`: no current security/functional requirement justifies the migration risk;
- `reject`: update conflicts with an intentional project constraint or cannot meet validation requirements.

`defer` is not permanent. Record the trigger that should reopen the decision.

## Empty queue behavior

An empty Dependabot queue is a valid healthy state. Do not create speculative major-version PRs simply to keep a maintenance task busy. Instead:

1. verify that prior security-bearing updates actually reached current manifests;
2. record intentionally deferred majors and their reopen triggers;
3. keep the security audit workflows active while Dependabot version-update PRs remain paused;
4. let a future advisory, required feature, or new bot PR reopen the affected dependency decision.

## Automation boundary

The current Dependabot configuration sets `open-pull-requests-limit: 0` for supported ecosystems. Review dependency updates on local `main`; retain the security audit workflows. Major updates require the compatibility review described above, and GitHub Actions must retain exact-SHA action pins after acceptance.

This policy deliberately separates dependency hygiene from release pressure: dependency work may improve the release, but version freshness alone is never evidence that the release is safer.
