# Release-security hardening roadmap

## Current posture

The main productization/refinement work is substantially mature. The highest-value remaining route is not broad feature expansion but closing release/update supply-chain gaps and making those guarantees auditable.

Known current facts from repository inspection:

- Formal release checks `v7 CI` and `v7 Candidate Package` on the frozen `main` SHA.
- PR #38 proposes making `Maintenance Security` and `Rust Security` produce a result for every `main` SHA and requiring exact-SHA successful push runs before formal release gates proceed.
- The updater verifies downloaded MSI size and SHA-256, MSI product/version/UpgradeCode/per-user identity, and Windows Authenticode trust with `WinVerifyTrust`.
- The updater's current Authenticode check establishes trusted-signature validity but does not yet bind the package to an explicit HLS Downloader signer/publisher identity.
- Current updater trust invocation uses cache-only URL retrieval and `WTD_REVOKE_NONE`; the intended offline/online revocation policy needs an explicit threat/availability decision.
- `main` was observed without branch protection/required status checks at bootstrap time; this must be re-checked before HLS-C005 because repository governance can change externally.

## Ordered work

### HLS-C002 — P0 audit PR #38

Do not merge based only on author intent. Verify exact-SHA event selection, push-vs-PR filtering, workflow completion, and whether any stale/rerun/wrong-event result could satisfy the formal-release lookup. Required outcome: independent PASS/FAIL evidence.

### HLS-C003 — P0 updater signer identity binding

Threat: any package with a valid Windows-trusted Authenticode signature could pass the generic trust check if the attacker can also satisfy the other package metadata/hash distribution constraints. Existing SHA-256/release metadata substantially reduces exposure, but defense-in-depth should bind execution to the project's own publishing identity.

Design requirements:
- never commit private keys, PFX material, or secrets;
- identify the signer using stable certificate/public-key/publisher material suitable for runtime verification;
- include a controlled certificate/key rotation strategy rather than a single forever fingerprint;
- define deterministic rejection behavior and tests for a valid but unexpected signer;
- keep formal release signing and updater verification consistent.

### HLS-C004 — P1 revocation/network policy

Current availability bias (`WTD_REVOKE_NONE`, cache-only URL retrieval) needs a documented policy. Evaluate:
- online chain/revocation check where network exists;
- offline update behavior;
- timestamped signatures and certificate expiry;
- whether revocation uncertainty blocks install or degrades with explicit user-facing diagnostics;
- avoiding accidental release/update outages due to network dependency.

### HLS-C005 — P1 branch governance

Re-check current branch/ruleset state. Define checks that are guaranteed to emit for every protected SHA; path-filtered required checks can deadlock merges. Prefer PR review/required CI where repository capability permits, while preserving emergency recovery ownership.

### HLS-C006 — P1 coordination validation

Add a small deterministic validator for YAML front matter and tasks JSON so concurrent agents cannot silently corrupt shared coordination state. This is coordination reliability, not product runtime scope.

### HLS-C008 — P1 post-merge security-gate validation

After HLS-C002 passes and PR #38 merges, verify the exact new `main` SHA receives all expected push workflow conclusions. Validate lookup behavior without dispatching a public formal release.

### HLS-C009 — P2 security exception inventory

The repository currently has explicit exceptions such as a RustSec ignore for `RUSTSEC-2023-0071`. Exceptions must have rationale, removal condition, and review trigger so suppressions do not become invisible permanent policy.

## Stop conditions

Release-security closeout can be considered reached when:

1. every formal release source SHA has deterministic required workflow conclusions;
2. updater integrity + product identity + publisher identity are all checked;
3. revocation/offline behavior is explicit and tested/documented;
4. `main` governance is either enforced or has an exact owner-action blocker documented;
5. security exceptions are explicit and reviewable;
6. formal release evidence remains reproducible and no security hardening weakens the existing browser/performance/MSI rollback gates.
