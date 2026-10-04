# SQLite Storage

## Ownership and boundary

Codex Meter has one Rust-owned SQLite writer. Rust opens the database, configures
the writer connection, owns schema migrations, and will own primary application
writes. Future CLI and Python analytics processes are read-only consumers; Python
must not become a migration owner or normal SQLite writer. Migration locking is
not the complete future daemon-singleton/process-lock mechanism.

P014 does not choose a production database location. The caller supplies an
explicit path, and the caller owns preparation of its parent directory. The
storage layer does not create arbitrary platform-specific directories. Tests and
callers that need an ephemeral database can use `SqliteStore::open_in_memory()`
or the explicit `:memory:` path.

## Dependency and connection configuration

The Rust crate uses `rusqlite` `0.40.2` with `default-features = false` and the
`bundled` feature. Bundled SQLite keeps availability and behavior consistent on
Linux, macOS, and Windows without requiring a system SQLite development package.
No ORM, async runtime, or async database layer is used.

Every writer connection:

- uses a finite five-second busy timeout;
- executes `PRAGMA foreign_keys = ON` and verifies that it reports `1`;
- leaves SQLite synchronous durability at its safe default; and
- uses `PRAGMA journal_mode = WAL` for file-backed databases, verifying the
  returned mode is `wal`.

WAL is not requested for in-memory databases. The implementation checks the
actual pragma result instead of assuming that a requested mode was accepted.
SQLite open failures, configuration failures, migration failures, and
compatibility failures return typed `StorageError` values. Display text is
privacy-safe and does not include row contents, parameters, payloads, secrets,
or full SQL text; the underlying SQLite error remains available through the
standard error source chain where applicable.

## Migration registry

Migration definitions are compiled into the binary with `include_str!`. Runtime
code never discovers SQL files from the current working directory. Each registry
entry has a positive version, stable name, and exact embedded SQL. The compiled
registry is validated before the database is mutated:

- versions are unique, strictly increasing, contiguous, and start at `1`;
- names are non-empty and unique; and
- an empty registry is rejected.

Each exact SQL text is hashed with SHA-256 and stored as lowercase hexadecimal.
Version, name, and checksum together identify an applied migration. Published
migration SQL is immutable: changing SQL or renaming a migration causes
`MigrationDrift` instead of silently updating history or re-running it.

## Migration history

The infrastructure history table is:

```sql
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER NOT NULL PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL UNIQUE CHECK (length(name) > 0),
    checksum_sha256 TEXT NOT NULL CHECK (
        length(checksum_sha256) = 64
        AND checksum_sha256 NOT GLOB '*[^0-9a-f]*'
    )
);
```

The table contains no account, task, telemetry, prompt, response, or other
application data. A successful normal writer open always returns a fully
migrated `StorageInfo` whose applied version equals the binary's latest
supported migration.

History is read again after obtaining a SQLite `BEGIN IMMEDIATE` writer lock.
The engine validates contiguous history, unique names, known versions, names,
and checksums while holding that lock. A database with a version newer than the
binary returns `DatabaseTooNew`; an impossible or missing-version history
returns `MigrationHistoryCorrupt`. Older binaries never downgrade, delete, or
recreate a database.

## Transaction and failure semantics

Each pending migration is applied in its own immediate transaction:

```text
BEGIN IMMEDIATE
    execute the embedded migration SQL
    insert version/name/checksum into schema_migrations
COMMIT
```

Any SQL or history-insertion error rolls the transaction back. Earlier
successfully committed migrations may remain. A failed migration is not
recorded as applied, and partial schema objects from that migration are rolled
back. A later open can retry the failed version after the underlying problem is
fixed. Reopening a fully migrated database validates history and performs no
migration writes.

The immediate transaction provides the migration-level serialization needed for
two simultaneous Rust writers. Pending decisions are not made from a history
snapshot taken before waiting for that lock. This is not a replacement for the
future process/agent single-writer coordination described by the architecture.

## Current schema

P016 extends the P015 storage infrastructure with durable Observation history:

- `schema_migrations`: versioned migration identity and checksums;
- `storage_metadata`: constrained infrastructure key/value space from migration
  `0001_storage_metadata`.
- `runtime_checkpoints`: atomic cursor/runtime/replay recovery state from
  migration `0002_runtime_checkpoints`.
- `observations`: canonical P013 Observation JSON plus derived indexed
  projections, checksum, and positive storage revision from migration
  `0003_observations`.

Migration 0001 creates:

```sql
CREATE TABLE storage_metadata (
    key TEXT NOT NULL PRIMARY KEY CHECK (length(key) > 0),
    value TEXT NOT NULL
);
```

The exact migration checksum is:

```text
1ffa336dcdc5abc63fdf74276c354c82a7b8f157af9412625723a7d8fe20c5aa
```

Migration 0002 has checksum:

```text
ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199
```

Migration 0003 has checksum:

```text
1ed907c9f124697b7f5620799672e49128dde7110860d513b0efaa9f57cd305c
```

The checkpoint table is a `STRICT` table keyed by `(rollout_id,
source_generation)`. It stores unsigned cursor values as decimal text, a
nullable decimal ordinal, positive revision, internal state format version,
typed JSON payload, and lowercase payload checksum. It has no source path and
is not an append-only history table.

The `observations` table is `STRICT`, keyed by `observation_id`, and contains
the complete canonical P013 payload. Its explicit projections cover identity,
lifecycle/timing, configuration, token validity/quality/raw total, and
independent five-hour and weekly validity/quality/delta/reset status. It has
deliberate history, lifecycle, configuration, quality, validity, and composite
configuration/time indexes. There is no generic JSON blob table, normalized
event archive, task/session content table, or analytics result table.

See `docs/RUNTIME_CHECKPOINTS.md` for the runtime recovery payload and
transaction contract, and `docs/OBSERVATION_STORAGE.md` for the Observation
repository contract.

SQLite migration versions are separate from JSON contract versions such as
`schema_version = 1.0.0`; one must not be inferred from the other.

## Tests and privacy

The Rust storage tests cover new-database bootstrap, reopen idempotency and
metadata preservation, explicit in-memory behavior, foreign keys, file-backed
WAL, registry validation, checksum regression, checksum drift, name drift,
newer databases, history gaps, pending migrations, and transactional failure
rollback/resume. Tests use in-memory SQLite or platform-neutral paths under the
system temporary directory; no Unix-only path is required by the core storage
layer.

Storage errors identify migration versions, stable names, and checksum
metadata only. They do not print arbitrary SQLite row values or application
payloads. Full SQLite physical integrity checking is intentionally separate
from migration compatibility and is deferred to a future diagnostics feature.

## Inputs for P016

P015 may assume:

- SQLite writer open is cross-platform;
- foreign keys are enabled;
- file-backed writer databases use WAL;
- migration definitions are embedded;
- migration history is versioned and checksummed;
- migrations apply transactionally;
- reopen is idempotent;
- migration drift is rejected;
- newer databases are rejected by older binaries;
- failed migrations do not record success;
- direct database path injection exists; and
- migration 0002 and runtime checkpoint CAS/transaction helpers exist; and
- Python remains a read-only consumer.

P016 implements **durable Observation persistence, deterministic history queries,
and atomic checkpoint-to-Observation handoff**. Estimation, weighting, capacity
calculation, and Python analytics remain deferred to Phase E.
