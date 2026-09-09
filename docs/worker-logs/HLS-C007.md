# HLS-C007 — Fix v7.0.2 MSI lifecycle version drift

## Acceptance criteria

1. Remove stale literal v7.0.1 assumptions from executable MSI lifecycle acceptance logic and bind candidate MSI version to the candidate manifest/product source of truth.
2. Keep the public v7.0.0 MSI baseline URL + SHA-256 pinned and preserve a real major-upgrade/product-code transition check.
3. Fail closed if candidate manifest `product_version` and MSI `ProductVersion` disagree; do not weaken `E:\h`, checkpoint, upgrade, rollback, registration, or process-recovery gates.
4. Update stale current-candidate v7.0.1 descriptions where they misdescribe the active candidate.
5. Extend repository-level validation so future version drift is caught without private signing material.
6. Use a dedicated PR and independent acceptance review before merge.

## Assignment history

- Priority: P0
- Finding source: interruptible HLS-C003 release-readiness pre-analysis while HLS-C002 waited on Candidate CI.
- Initially assigned to `worker-1`; no task ACK, branch, or heartbeat appeared for more than five minutes after assignment.
- Reassigned to `worker-0` under the heartbeat-loss rule.
- Branch: `fix/hls-c007-msi-lifecycle-version`
- Base refreshed to PR #38 merge commit `884f628eb264bb63ba7a093c67e700e3bf3d3024` before implementation.

## Reproduced defect

Canonical product metadata declares v7.0.2, while the formal MSI lifecycle verifier required both the candidate manifest version and candidate MSI `ProductVersion` to equal the literal `7.0.1`. The same script also required the post-upgrade installed product to be `7.0.1` and used `7.0.1` in the forced-rollback scratch MSI name.

That made the formal v7.0.2 lifecycle path deterministically fail even if signing, runner, browser, performance, and release-security gates were otherwise correct.

## Implementation

### Canonical version contract

Added `scripts/V7VersionContract.psm1` with two fail-closed functions:

- `Resolve-V7CandidateVersion` reads `artifacts/v7-productization/feature-parity.json`, validates schema/version syntax, requires the canonical product version to be newer than the fixed public baseline, and requires the candidate manifest version to match it.
- `Assert-V7CandidateMsiVersion` validates the MSI version syntax and requires exact equality with the resolved canonical version.

The public baseline remains `7.0.0`; no signing or lifecycle evidence requirement is weakened.

### MSI lifecycle gate

`verify-v7-msi-lifecycle.ps1` now:

- imports the shared version contract;
- resolves the expected candidate version from canonical product metadata + candidate manifest;
- compares MSI `ProductVersion` against that value;
- records an explicit `candidate-version-newer-than-baseline` step;
- requires the installed post-upgrade version to match the same value;
- uses the resolved version only for the temporary forced-rollback MSI filename;
- includes `candidate_version` in the report.

The fixed public v7.0.0 URL/SHA/version, UpgradeCode match, new ProductCode requirement, `E:\h`, real checkpoint creation, process shutdown/restart, resume verification, registration/shortcut checks, rollback failure injection, database preservation, uninstall, and cleanup logic remain intact.

### Existing repository validation

Rather than creating a second parallel test path, `verify-v7-version-contract.ps1` now imports the same production module and checks:

- the canonical product version resolves successfully against a matching synthetic manifest;
- manifest mismatch is rejected;
- MSI ProductVersion mismatch is rejected;
- a candidate not newer than its baseline is rejected.

`validate-powershell.ps1` already invokes `verify-v7-version-contract.ps1` in the default validation path, so this is exercised by the existing `Validate contracts` CI job under both Windows PowerShell 5.1 and PowerShell 7.

### Release evidence text

`invoke-v7-release-gates.ps1` now describes the installer input using the actual candidate manifest version rather than stale `v7.0.1` text.

## Commit ledger

- `02ad17a777b42f9bf4d06bc36c554dfff20181e2` — centralize v7 candidate version contract
- `0037c8dd735272ea2bdbc8a45f29c1854819c565` — initial standalone contract test (later consolidated)
- `85cfef45dd42bb83d8db57a3722fccc4ffb1c694` — bind MSI lifecycle gate to canonical version
- `a07428a05a712b7433c0c9258b32bb46c88cab48` — make release evidence use candidate version
- `eeb250c5e0cf67dd07026a6d30e0388cf0517a85` — extend existing canonical drift gate
- `68929c5dde756f071b6fe94495f28295bcd78b45` — remove redundant standalone test after consolidation

Net implementation diff before this log: four files, 6 commits, branch directly ahead of PR #38 merge base with no behind commits.

## Validation status

Static self-review of the actual lifecycle commit shows only the intended version-contract substitutions plus one report field; no lifecycle stage was removed. Repository CI/PowerShell validation is pending the dedicated PR. Independent auditor acceptance must use the actual PR diff and CI results rather than this worker log.
