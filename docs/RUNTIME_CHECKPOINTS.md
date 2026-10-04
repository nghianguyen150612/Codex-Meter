# Runtime Checkpoints

## Purpose

P015 persists the latest recoverable runtime state for one exact source
identity. A checkpoint is keyed by `rollout_id + source_generation`; it is not a
source-path record, an event archive, or a measurement history table.

The durable invariant is:

```text
accepted source position + runtime state + replay/idempotency state
```

are committed together. A cursor never advances durably without the state
produced from that cursor, and state never advances durably without its
corresponding cursor.

## Key and cursor representation

`runtime_checkpoints` uses `(rollout_id, source_generation)` as its primary key.
P007 identity values are validated through `SourceIdentity::new` when loaded.
The database stores no rollout path, `CODEX_HOME`, working directory, repository
path, or other filesystem location.

P007's `u64` cursor fields are stored as canonical unsigned decimal text:
`committed_offset` and nullable `last_ordinal` use `value.to_string()` and are
loaded with strict `u64` parsing. Empty, signed, whitespace-containing,
malformed, negative, and overflowing values fail closed. `NULL` represents
`last_ordinal = None`; ordinal zero remains distinct from no ordinal.

`RolloutCursor::from_checkpoint` creates a cursor only from a validated
`SourceIdentity`, so the cursor cannot be restored with a different rollout or
generation.

## Runtime payload

The dedicated typed JSON envelope is `PersistedRuntimeStateV1`. It contains:

- P009 `TelemetryState`, including session and thread configuration, task
  lifecycle, root-task evidence, terminal evidence, anomalies, overrides,
  token-configuration fingerprints, and consistency state;
- P011 `QuotaTrackingState`, independently for `five_hour` and `weekly`,
  including current windows, last samples, and private seen-sample replay IDs;
- a deterministic `BTreeMap` of active task reconciliations, each containing
  its P012 `ReconciliationPolicy` and complete `TaskQuotaReconciliation` state.

The payload is made from typed DTOs and closed deserialization rejects unknown
fields. P012 policy durations use exact `{seconds, nanoseconds}` values rather
than floating-point seconds. Restored policies pass `ReconciliationPolicy::validate`
and domain-owned recovery validation before becoming runtime state.

`state_format_version = 1` is an internal checkpoint format version. It is
separate from the SQLite migration version (`4`) and public JSON contract
versions such as `schema_version = 1.0.0`. The format is present in both the
row metadata and the typed payload envelope. A newer or unsupported format is
rejected rather than decoded as version 1.

P015 retains terminal reconciliation state in the active-reconciliation map
when the caller has not yet handed its completed measurement to P016. This is a
short crash-gap handoff, not permanent Observation storage. P016 can remove or
replace the retained entry after durable Observation finalization.

## Checksums and privacy

`state_sha256` is the lowercase SHA-256 digest of the exact UTF-8 bytes in
`state_json`. Load verifies the digest before deserializing. A mismatch returns
`CheckpointCorrupt`; malformed or incompatible JSON returns a typed decode
error. The checksum detects accidental or inconsistent row mutation. It is not
encryption, a MAC, a signature, or tamper-proof security.

The checkpoint contains privacy-filtered P009/P011/P012 state only. It never
stores raw rollout lines, prompts, responses, reasoning text, tool arguments or
outputs, source paths, credentials, OAuth tokens, cookies, account IDs, or
public contract event archives.

## Revision and transactions

`checkpoint_revision` starts at `1` and is constrained to be positive. Initial
writes require `expected_revision = None` and fail if a row already exists.
Updates require the exact current revision and increment it with checked
arithmetic. A stale or missing row returns `CheckpointRevisionConflict`; an
unrepresentable increment returns `CheckpointRevisionOverflow`.

State encoding and validation happen before opening the write transaction. The
write then runs under one `BEGIN IMMEDIATE` transaction and updates the cursor,
payload, checksum, format, and revision together. Any SQLite or commit failure
leaves the old row authoritative. The API does not split cursor and runtime
state across commits. Revision CAS improves stale in-process writer safety; it
is not a complete process-level singleton lock.

Loading is read-only and never creates a row. A missing exact source-generation
row returns `Ok(None)`.

The standalone `SqliteStore::save_runtime_checkpoint` API retains the strict
P015 rule that an expected revision must match before a non-identical update.
The P016/P016A composite Observation handoff additionally recognizes an exact
durable checkpoint replay by comparing source identity, cursor, state format,
state JSON, and state checksum. An exact replay returns the existing checkpoint
without incrementing its revision, even if the caller's expected revision is
stale. Corrupt checkpoint rows are decoded and checksum-validated before they
can qualify as an exact replay.

## Ingestion sequence

Future runtime orchestration follows this sequence:

1. load the committed checkpoint;
2. read from its committed cursor;
3. process the `ReadBatch` into candidate telemetry state;
4. process candidate quota tracking and reconciliation state;
5. if every candidate operation succeeds, save one checkpoint transaction; and
6. only after commit, treat the candidate cursor and state as durable.

A P009, P011, P012, serialization, validation, or downstream failure leaves the
old durable checkpoint unchanged. A newer source generation is an independent
checkpoint stream; P015 never infers generation ordering or deletes an older
generation.

## Current tables

After the production migrations, the intended durable tables are:

- `schema_migrations`;
- `storage_metadata`; and
- `runtime_checkpoints`.

There is no `observations`, analytics, benchmark, normalized-event archive, or
generic `records(kind, json)` table in P015. Python remains a read-only
consumer; checkpoint writes remain inside Rust `SqliteStore`.

## Inputs for P016

P016 may assume:

- migration 0002 exists;
- exact source-generation checkpoints are durable;
- cursor and runtime state commit atomically;
- checkpoint writes use revision CAS;
- corrupt payloads and checksums fail closed;
- P009 telemetry state survives restart;
- P011 replay/idempotency state survives restart;
- active P012 reconciliation and stabilization can survive restart;
- no raw telemetry or content is persisted; and
- no Observation table exists yet.

P016 will implement **durable Observation storage, idempotent Observation
upserts/finalization, indexes, and history/query APIs**.
