# Observation Storage

P016 owns durable persistence for the typed P013 `NormalizedObservation`.
Telemetry and Observation assembly remain Rust domain code; SQL is confined to
`storage/observations.rs`. Rust is the only migration and database writer.
Python may later open the database read-only.

## Canonical payload

Each row is keyed by the P013 `observation_id`. The complete typed Observation
is encoded with `serde_json::to_vec` in struct/field order, without pretty
printing or debug formatting. The exact UTF-8 bytes are stored as
`observation_json`. Its lowercase SHA-256 is stored in `observation_sha256`.

Load verifies the checksum before decoding. Decoding is closed for the v1
Observation model and explicit validation rejects impossible states, including
unsupported schema versions, meter identity mismatches, reset branches with a
delta, invalid timing, inconsistent quality/validity, and unsafe numeric values.
The checksum detects accidental or inconsistent mutation; it is not
authentication or encryption. Projection verification is a second structural
check: projections are derived again from the decoded Observation and must
match every stored projection. P016 never repairs a corrupt row.

## Projections and schema

Migration `0003_observations` creates one `STRICT` table named `observations`.
SQLite migration version `4`, Observation schema version `1.0.0`, and checkpoint
state format version `1` are independent version domains.

The intentional application tables after migration 4 are exactly
`schema_migrations`, `storage_metadata`, `runtime_checkpoints`, and
`observations` (plus SQLite internal indexes/objects).

The table includes:

- `observation_id`, `schema_version`, `task_id`, `session_id`, and
  `source_instance_id`;
- lifecycle state and `started_at`, `ended_at`, `finalized_at`, `duration_ms`;
- summary quality;
- available-only `plan`, `model`, `reasoning_level`, `speed_mode`, and
  `codex_version`;
- token validity/quality and available-only `raw_total`;
- independent five-hour validity/quality/delta/reset status; and
- independent weekly validity/quality/delta/reset status;
- canonical JSON, checksum, and positive `storage_revision`.

The `started_at`, `ended_at`, and `finalized_at` SQL projections are fixed-width
UTC query keys in the form `YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ`. The migration 0004
backfill normalizes existing projections and leaves `observation_json` byte-for-
byte unchanged; the JSON retains the exact domain timestamp supplied by P013.
New writes derive the same query keys from the typed Observation.

Configuration and numeric projections come only from the typed Observation.
Unavailable configuration is SQL `NULL`; unavailable/reset/incomplete quota
branches have SQL `NULL` deltas. A real valid zero delta remains `0.0`.
`raw_total` and duration use checked conversion to SQLite signed integers.

Indexes cover finalized and ended time, lifecycle, each configuration dimension,
summary quality, token validity, each quota validity, and a composite
configuration/finalized-time history grouping. Indexes do not encode estimator
eligibility or quality weights.

## Revision and lifecycle policy

`SqliteStore::save_observation` uses one `BEGIN IMMEDIATE` writer transaction.
An initial insert requires `expected_revision = None` and starts at revision 1.
A non-identical update requires the exact current revision and increments it
with checked arithmetic. Stale revisions and overflow return typed errors.

The lifecycle order is:

```text
detected → active → task_ended → awaiting_meter → reconciling
                                                        ↓
                                          finalized | incomplete | invalid
```

Same-lifecycle provisional updates are allowed for `awaiting_meter` and
`reconciling`; lifecycle regression is rejected. Terminal rows are immutable:
an exact replay succeeds, but any different payload under the same ID returns a
terminal conflict. Identity-defining `task_id`, `session_id`, and
`source_instance_id` cannot change. P016 does not generate or overwrite
`timing.finalized_at`.

Provisional lifecycle rows must omit `timing.finalized_at`. Terminal rows may
include it or omit it; storage never fabricates a finalization timestamp. If
both task endpoints exist, `duration_ms` must equal the exact floor of their
elapsed interval in milliseconds. Missing either endpoint requires missing
duration, and inconsistent timing is rejected as corruption.

An exact canonical replay is an idempotent no-op even when the caller's
expected revision is stale. It returns the durable row, preserves its revision,
and creates no duplicate history entry.

## Load and history APIs

`load_observation` returns a typed `StoredObservation` containing the decoded
Observation, storage revision, and payload checksum. SQL rows are not exposed.
Missing IDs return `Ok(None)`.

`ObservationQuery` supports exact lifecycle, plan, model, reasoning-level,
speed-mode, summary-quality, and finalized-time range filters. Limits are
required to be bounded (1 through 1000); an empty filter is never unbounded.
Filters use bound parameters and exact comparisons. `list_terminal_history`
selects finalized, incomplete, and invalid rows only. `list_active_observations`
selects provisional lifecycle rows separately.

Results are ordered deterministically by
`COALESCE(finalized_at, ended_at, started_at, '') DESC, observation_id DESC`.
Because each non-empty projection is fixed-width UTC, this lexical ordering is
chronological even when source timestamps use different fractional precision.
`ObservationPageCursor` stores that stable order-time plus the ID and uses
keyset predicates, so pages do not rely on `OFFSET`, wall-clock tokens, or
unstable row order. Terminal history normally has finalized timestamps and is
therefore ordered by `finalized_at DESC, observation_id DESC`.

All load and query APIs are read-only. They do not update access metadata,
revision, or schema.

## Atomic checkpoint handoff

`SqliteStore::commit_observation_and_checkpoint` accepts both expected
revisions and performs Observation validation, checkpoint validation, both
writes, and commit in one `BEGIN IMMEDIATE` transaction. It reuses the P015
checkpoint writer helper without nesting transactions. Standalone
`save_runtime_checkpoint` retains strict P015 CAS behavior.

If either Observation or checkpoint CAS fails, both writes roll back. If the
Observation is already an identical durable replay, it is a no-op while a valid
checkpoint CAS can still advance. If the checkpoint payload is also exactly
identical (including source, cursor, format, JSON, and checksum), its write is
a no-op and its existing revision is returned even when the caller's expected
revision is stale. This makes the safe runtime sequence
straightforward: persist the terminal Observation while removing its completed
reconciliation from the next checkpoint. A crash before commit leaves the old
checkpoint available for deterministic replay; a crash after commit leaves both
Observation and next checkpoint durable.

## Privacy and read-only boundary

Observation storage contains only data already permitted by P013: normalized
IDs, configuration identity, timing, token evidence, quota evidence, quality,
and lifecycle. It never stores prompts, responses, reasoning text, tool
contents, source paths, repository paths, account email, credentials, OAuth
tokens, raw rollout lines, or source-event archives.

Rust owns migrations and writes. Phase E Python code may read the stable SQLite
schema read-only and load canonical Observations; it must not write rows or
become a migration owner.

## Inputs for P017

P017 may assume:

- migration 0004 exists;
- Observations persist losslessly;
- finalized history is immutable;
- provisional Observation updates are CAS-protected;
- exact replay is idempotent;
- observation corruption fails closed;
- estimator-critical fields have indexed projections;
- terminal/history queries are deterministic and paginated;
- checkpoint-to-Observation handoff can be atomic;
- Python may read SQLite but does not write it.

P017 begins:

**Phase E — Python estimator foundation and read-only Observation dataset loading.**
