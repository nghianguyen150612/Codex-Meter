# PROMPT032 implementation manifest

## Status and baseline

- Status: **PARTIAL** until the reference Arch Linux live-device gate is
  completed and reviewed.
- Verified starting `origin/main`: `d375328ff0c92a186be8b621e7f2bdd6cef087f8`,
  the two-parent P031 merge commit.
- Feature branch: `feat/p032-usage-history-cli`.
- P031's v0.1.0 tag `171b54ee4f80c007ab6e0b4af3cd1bb22613e755` was preserved.
- Pull request and final commit SHAs: recorded after the feature branch is
  pushed and the PR is opened.
- The reference Arch device is not attached to this environment. Live Codex
  version compatibility and real-device quota accuracy remain unverified.

P031's implementation remains verified only for the audited Codex CLI 0.157.1
rollout format. The P031 merge did not clear its outstanding device gate. The
current project validation policy says the live-device gate blocks integration;
keep this PR in Draft until a sanitized device validation report is reviewed.

## Implementation

P032 adds native commands `tokens`, `history`, `inspect`, and `quota`. They use
typed Rust reporting APIs over a separate `SqliteReadStore` opened with
`SQLITE_OPEN_READ_ONLY` and `PRAGMA query_only = ON`. Migration history and the
required schema are checked without applying migrations. Version 5 databases
can be read as-is; `sync` applies migration 6, which adds timestamp-normalized
global event and quota indexes. Evidence returned or aggregated is decoded
from the checksummed ledger rows. Reporting never reads an analytics
projection, checkpoint, or raw source payload as a substitute for evidence.

The existing default database-location helper is shared by `sync` and all new
commands. Missing history is actionable and does not create a database.

Token sums include only canonical `per_response` records. Direct source totals
remain separate from component sums; missing values, source semantics,
configuration availability, and checked-overflow errors are explicit.
History pages use deterministic timestamp/event-ID keysets and have bounded
limits. Quota samples are read from `quota_snapshot_evidence` only, with
five-hour and weekly latest samples independent. A 24-hour freshness label is
documented as a presentation policy. JSON uses reporting schema version 1.0.0
and does not change the analytics request/result contracts.

## Local validation

The P032 fixture suite covers canonical versus snapshot aggregation, missing
versus zero counters, cached input semantics, reasoning/output separation,
partial totals, model/time filters, stable fractional timestamp ordering,
equal timestamp ties, keyset continuation, independent quota meters, reset
availability, unknown versus corrupt IDs, integer overflow, empty databases,
read-only v5 compatibility, unchanged database bytes, privacy-safe structured
output, and malformed/conflicting CLI options.

Quality-gate results, full command output, diff review, and PR checks are
recorded in the final closeout for this manifest after validation completes.

## Reference Arch live-device gate

The device is unavailable in this execution environment. No actual Codex home,
installed CLI version, rollout, personal token total, or quota snapshot was
read. Do not present synthetic fixtures as real-device evidence and do not
create paid Codex activity just for validation.

On the reference device, after checking the installed version, run from this
checkout:

```bash
codex --version
cargo run --manifest-path rust/Cargo.toml -- sync
cargo run --manifest-path rust/Cargo.toml -- tokens
cargo run --manifest-path rust/Cargo.toml -- history --limit 20
cargo run --manifest-path rust/Cargo.toml -- quota
cargo run --manifest-path rust/Cargo.toml -- tokens --json > /tmp/codex-meter-tokens.json
```

Keep those outputs local. They can contain personal usage counts and opaque
IDs. Compare the token totals with an independently run count-only query over
`token_usage_evidence` constrained to `record_semantic = 'per_response'`; do
not print raw rollout records or include the database in a report. Repeat
`sync` in a separate process to check replay, then rely on normal user activity
for any new-event check. Do not request a paid action solely for the gate.

Record only the installed version, whether it is the audited 0.157.1 format,
sanitized command statuses, aggregate match/mismatch, replay stability, and
whether privacy checks passed. Keep unsupported versions, warnings, or
incomplete evidence visible. Until this gate is completed and reviewed, P032
integration remains blocked under the current project validation policy.

## P033 handoff

P033 may consume the typed history and evidence APIs when its scope is
authorized. It can add a TUI or plugin adapter, but must retain the read-only
ledger boundary, per-response versus snapshot separation, missing-value
semantics, and source-version gate. Project metadata must use its separate
authorized adapter; do not infer project names from stored paths. Do not start
P033 or publish v0.2 as part of P032.
