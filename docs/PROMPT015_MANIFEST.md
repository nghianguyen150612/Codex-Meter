# Prompt 015 Manifest

- Starting SHA: `f69ddcd1de16bf667de982a0ee38e54cf0a3fd8f`.
- Verified P014 baseline: fetched `origin/main`, checked out `main`, confirmed
  `main == origin/main`, confirmed the P014 SQLite migration/registry commit was
  present, and confirmed a clean worktree before editing.
- Workflow: direct-to-main only; no feature branch and no pull request.
- Migration: added `0002_runtime_checkpoints` as migration version `2` without
  modifying published migration 0001.
- Migration checksum: `ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199`.
- Table schema: strict `runtime_checkpoints` table keyed by
  `(rollout_id, source_generation)` with decimal cursor text, nullable ordinal,
  positive revision, state format, typed JSON, and lowercase SHA-256 checksum.
- Checkpoint key: exact P007 `rollout_id + source_generation`; generations are
  isolated and no generation ordering or cleanup is inferred.
- Cursor encoding: canonical `u64::to_string()` decimal text; strict parsing;
  `NULL` ordinal preserves `None`; full `u64` range is covered by regression.
- State format: internal `state_format_version = 1`, represented in both the
  row and the closed typed `PersistedRuntimeStateV1` envelope.
- DTO design: explicit storage envelope and policy/duration DTOs retain typed
  P009 telemetry, P011 quota/replay, and P012 reconciliation state while
  keeping in-memory layout separate from the durable version boundary.
- Checksum design: SHA-256 of exact `state_json` UTF-8 bytes, lowercase hex,
  verified before deserialization; checksum is corruption detection only.
- Revision/CAS: initial insert requires `None`; updates require exact revision,
  checked increment, and typed stale/overflow errors.
- Atomicity: encoding and recovery validation precede one `BEGIN IMMEDIATE`
  transaction that writes cursor, state, checksum, format, and revision.
- P009 recovery: session, thread configuration, task lifecycle/evidence,
  anomalies, overrides, token fingerprints, and consistency survive restart.
- P011 recovery: both meters, windows, last samples, and private seen sample IDs
  survive restart, preserving duplicate-sample behavior.
- P012 recovery: active task policy and complete per-meter reconciliation,
  including tracker, accumulated delta, stabilization candidate, attempt IDs,
  timestamps, and terminal handoff state, survive restart.
- Privacy: no source paths, raw lines, prompts, responses, tool content,
  credentials, account IDs, or public event archive are persisted.
- Tests: migration 1→2 upgrade, migration checksum regressions, strict cursor
  parsing/full-u64 round trip, generation isolation, load read-only behavior,
  revision CAS, checksum/decode/unknown-field/format failures, telemetry and
  reconciliation round trips, and quota replay recovery.
- Dependencies: no new dependency; existing `serde`, `serde_json`, `sha2`,
  `time`, and bundled `rusqlite` are used.
- Explicit P016 deferral: no Observation table, observation history, analytics,
  benchmark, or normalized-event archive was added. Terminal reconciliation may
  remain temporarily for the P016 crash-gap handoff.
- Final SHA handling: this manifest intentionally does not embed its own commit
  SHA. The final commit SHA and push/equality result are recorded in the
  completion report after the single logical P015 commit.
- Unrelated-work confirmation: no legitimate newer work was overwritten; public
  JSON contracts and prior P008–P014 domain behavior remain unchanged.
