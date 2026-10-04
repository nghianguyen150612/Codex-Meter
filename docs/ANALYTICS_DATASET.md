# Analytics Dataset

P017 establishes the read-only foundation for Phase E analytics. Rust remains
the owner of the SQLite database, migrations, canonical Observation JSON, and
all durable writes. Python opens the database only to construct an in-memory,
immutable analytics dataset:

```text
Rust writes → SQLite → Python read-only connection → typed Observation records
```

Python does not migrate, repair, normalize, rewrite, or persist analytics
results. It also does not calculate capacity candidates, weights, percentiles,
outliers, or confidence.

## Connection and storage compatibility

`codex_meter.history.ObservationDatabase` opens a resolved `pathlib.Path` as a
SQLite URI with `mode=ro` and `uri=True`. The resolved URI escapes spaces,
`#`, non-ASCII characters, and platform-specific path details. The normal
reader does not use `immutable=1`, so a committed WAL state remains visible
while Rust is writing. A finite five-second busy timeout is used by default.

Immediately after opening, the connection enables and verifies:

```sql
PRAGMA query_only = ON;
PRAGMA query_only;
```

The second statement must return `1`. The production package exposes no
writable or general-purpose connection API. A missing path raises a typed
error and never creates an empty database file.

Before reading rows, the reader requires `schema_migrations` and
`observations`, then validates migration history versions 1 through 4 in
order. Each version must have the expected name and SHA-256 checksum. A gap,
name/checksum drift, missing migration, migration version below 4, or a
database newer than 4 is rejected. The reader does not execute migration SQL;
Rust is the only migration owner. The required Observation projection columns
are also checked before loading.

## Typed projections and corruption checks

The public history surface exports immutable dataclasses and string enums:

- `ConfigurationKey` preserves all five exact configuration dimensions;
- `ObservationRecord` preserves lifecycle, canonical SQL time projections,
  token evidence, raw token total, and summary quality;
- `QuotaEvidenceRecord` keeps five-hour and weekly evidence independent;
- `ObservationDataset` contains a tuple in deterministic chronological order;
- `ObservationSelector` describes exact configuration, inclusive time bounds,
  and an optional bounded result count.

`NULL` configuration values and `NULL` raw totals become Python `None`.
Unavailable configuration is never replaced with an invented value. A valid
quota delta of `0.0` remains `0.0`; SQL `NULL` remains `None`. A reset branch
must be `invalid / X / None`, and valid quota evidence must have a delta. The
reader retains incomplete, invalid, quality-D, quality-X, and reset-invalid
rows; estimator eligibility is deferred.

Every loaded row is checked for a non-empty identity, Observation schema
version `1.0.0`, known enum values, non-negative raw totals, finite quota
deltas in `[0, 100]`, validity/quality invariants, reset invariants, and a
summary quality equal to the worst component quality. The exact UTF-8 bytes of
`observation_json` are hashed with SHA-256 and compared with
`observation_sha256`. A checksum match is not treated as sufficient: the JSON
is parsed and its identity, lifecycle, timing, configuration, token, and both
quota projections are compared with SQL.

Canonical JSON timestamps may retain original RFC3339 precision, including no
fraction or one through nine fractional digits. Python uses the same strict
UTC-only fixed-width key as migration 0004:

```text
YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ
```

It does not route timestamps through Python's microsecond-limited `datetime`
representation. Non-UTC offsets and malformed timestamps fail closed.

## Dataset selection

The normal loader selects only terminal lifecycle states:

```text
finalized, incomplete, invalid
```

Provisional states are excluded. The observed time for a terminal row is the
first available value in this order:

```text
finalized_at → ended_at → started_at
```

Rows with none of those values cannot satisfy a bounded observed-time range.
`observed_from` and `observed_through` are inclusive, and both are normalized
to the canonical timestamp key before bound parameters are sent to SQLite.
Returned records are ordered by observed time ascending, then
`observation_id` ascending. A configuration selector compares all five
dimensions exactly, including `None == None` and `None != "fast"`; an empty
exact group remains empty and never falls back to another configuration.

All selector values are bound SQL parameters. `None` configuration dimensions
use `IS NULL`, never `= NULL`.

## Bounded loading and snapshots

Without a limit, rows are consumed in batches rather than with an unbounded
`fetchall()`. `maximum_observations` must be at least 1 and no greater than the
reader's conservative safety ceiling of 100,000. When supplied, the loader
selects the most recent matching N rows by observed time descending and
`observation_id` descending, then returns those rows in chronological
ascending order. This makes truncation stable and appropriate for recent
history analysis.

Each dataset load starts one normal deferred read transaction, reads all
selected pages, and commits it. No `BEGIN IMMEDIATE` or write is issued. The
transaction gives a single consistent SQLite/WAL snapshot while Rust may
continue to write committed observations. The dataset is materialized before
the connection can be closed, so records do not depend on a hidden lazy
iterator or a closed connection.

## Privacy and deferrals

Errors are privacy-safe: they may identify an Observation ID and a schema or
migration version, but never include canonical JSON, prompts, responses,
credentials, source payloads, or database row contents. The checksum is an
integrity check, not authentication or encryption.

P017 deliberately does not calculate `raw_total / delta_percentage_points`,
handle zero-delta capacity candidates, assign quality weights, remove outliers,
compute MAD/percentiles, detect quota changes, serialize analytics results, or
invoke Python from Rust.

## Inputs for P018

P018 may assume:

- Python opens SQLite strictly read-only;
- storage migration v4 is validated before analytics;
- terminal Observation history loads deterministically;
- loaded records are typed and corruption checked;
- configuration groups are exact and immutable;
- raw token totals preserve unavailable vs zero;
- five-hour and weekly evidence remain independent;
- quota delta preserves NULL vs valid zero;
- reset-invalid/incomplete observations are retained for exclusion accounting;
- analytical datasets support exact configuration/time selection;
- selected datasets are chronologically deterministic;
- recent-N truncation is deterministic;
- Python does not mutate SQLite.

P018 will implement **per-observation raw-capacity candidate derivation,
estimator eligibility/exclusion rules, and explicit sample accounting**.
