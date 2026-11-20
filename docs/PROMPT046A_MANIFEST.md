# P046A — Live quota source qualification preflight

## Status

P046A adds an explicit `quota --live --diagnose` command. Local engineering
validation is complete; final-head cross-platform CI and release-candidate
validation run after push. Production live-source semantic qualification remains
**UNVERIFIED**.

## Diagnostic contract

The diagnostic calls the existing initialized Codex App Server client once, reads
`account/rateLimits/read` once, and passes the existing P045A normalized
`AccountRateLimitsSnapshot` to the pure `evaluate_live_quota_source` evaluator.
The evaluator does not parse raw RPC JSON or change P045A bucket selection,
window classification, percentage validation, reset normalization, or warning
semantics.

JSON schema `1.0.0` reports the fixed source identifier, normalized acquisition
time, connection and RPC outcomes, response and bucket structure, independent
five-hour and weekly evidence, per-meter reset metadata, typed warnings,
structural eligibility, bounded runtime version metadata, and explicit trust
limitations. It contains no raw response, account details, arbitrary bucket
labels, paths, credentials, task delta, token capacity, or git metadata.

Structural source eligibility describes recognized, internally consistent
account-level evidence in this response. Each meter is reported independently;
missing, unsupported, ambiguous, or unavailable evidence is not converted to a
zero. A valid observed 0% or 100% remains a percentage observation. A reset
timestamp is metadata only and a single read cannot establish window stability.

The initialized App Server user-agent is retained only if it exactly matches a
bounded `codex_cli_rs/major.minor.patch` form. The exact pinned audit version,
`0.163.0`, may be marked as a metadata match. This does not prove that the
runtime is compatible or that the reported values are accurate. A missing or
ambiguous user-agent remains unavailable.

## Explicit opt-in and privacy

`--diagnose` requires `--live` and rejects `--save`, `--history`, `--source`,
`--after`, `--limit`, and `--database`. The command performs no database open,
snapshot persistence, polling, login, model turn, reset-credit consumption, or
background work. Existing commands without `--diagnose`, including P045B's
explicit `quota --live --save`, retain their existing behavior.

Connection and RPC errors remain sanitized typed CLI errors. The diagnostic
does not expose task attribution, create P044 credentials or events, assemble a
P013 Observation, modify estimator datasets, or perform timestamp-proximity
joins. Account-level percentage evidence is not token count or task-level
consumption evidence.

## Evidence limits and deferred gates

One successful read can demonstrate a connection, supported RPC response,
recognized schema, identified Codex bucket, and whichever per-meter percentages
and reset metadata are present. It cannot establish official plan allowance,
exhaustive token coverage, task causality, exclusive task use, before/after
stability, external account-use isolation, accuracy over time, compatibility
with all installations, or capacity-estimator readiness.

Fake-server and fixture tests establish deterministic classification and
privacy behavior only. A real authenticated runtime must be independently
verified for source accuracy. Controlled before/after measurements, overlap
uncertainty, and task isolation remain deferred. P038 G4/G5 hardware gates
remain deferred. Production semantic qualification is **UNVERIFIED**.

## Pinned compatibility evidence

P045A's pinned audit remains Codex CLI `0.163.0`, upstream revision
`83086188ae0614bd882818ff1aa23f3194962012`, `account/rateLimits/read`,
optional `supportsLunaReserve: false`, and
`excludeResetCreditDetails: true`. P046A does not widen an execution allowlist
or infer compatibility from a user-agent match.

## Validation record

Local final-source validation:

- Rust formatting and workspace Clippy with all targets and features: passed.
- Workspace Rust tests: default 650 passed; all-features 641 passed.
- P046A diagnostic evaluator/CLI/fake-server tests: passed, including the
  single-read guard, one-meter-only and reversed-slot responses, reset
  availability, unsupported/missing bucket structure, redaction, process exit,
  and timeout categories.
- P045A App Server rate-limit adapter regressions: 9 passed.
- P045B live snapshot storage regressions: 16 passed.
- P044 sampling coordinator regressions: 45 passed.
- Python pytest: 678 passed; Ruff lint and format: passed.
- Contract fixtures: 8 schemas, 19 positive fixtures, and 7 negative fixtures
  passed.
- Installed-worker smoke and Linux PTY: passed.

Windows ConPTY, macOS Intel and ARM64, four-platform release candidate, and
aggregate checksum/provenance are validated by final-head GitHub Actions after
the branch is pushed; their run URLs and conclusions are recorded in the PR.
