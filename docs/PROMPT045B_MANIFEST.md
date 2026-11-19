# P045B: opt-in live account quota persistence and history

## Scope and base

P045B adds explicit local persistence and a separately queryable, read-only
history for the normalized P045A `account/rateLimits/read` result. It reuses
`CodexAppServer::read_rate_limits()` and keeps the rollout quota evidence table,
qualified task reconciliation, Observations, and estimator eligibility
unchanged.

Starting P045A HEAD: `50359c8e8099ca3d1d2968702e37a7a1991fefb2` on
`feat/p045-live-quota-readonly`. The delivery branch is stacked on that branch.
The final commit, Draft PR, and exact-head validation links are recorded in the
P045B PR closeout.

## Storage contract

Migration `0007_live_account_quota_snapshots.sql` creates a new `STRICT` table,
`live_account_quota_snapshots`, and an acquisition-time index. Migrations 1–6
are unchanged. Each row represents one successful normalized RPC response and
stores both meters from that same acquisition: UTC acquisition timestamp and
sortable Unix nanoseconds, schema version, fixed source and provenance fields,
overall and bucket statuses, each meter's availability/observed percentage/
optional observed reset, bounded typed warning codes, and a SHA-256 checksum.
The source is fixed to `codex_app_server_account_rate_limits`; evidence scope is
`account_level`; task attribution and forecast qualification are fixed to
`not_established`.

Unavailable meter values remain SQL `NULL`; observed 0 and 100 remain observed
values. Remaining percentage is derived after reading and is not persisted.
The storage boundary independently validates the P045A schema/source,
timestamps, status relationships, main-bucket provenance, finite in-range
percentages, including fractional values, reset timestamps, warning count and
uniqueness. It never
accepts raw JSON or provider labels.

Every new save gets a distinct `lqs_` ID with 128 random bits from SQLite's
OS-seeded pseudo-random generator. IDs include no account, email, credential,
task, path, Git, or provider data. A repeated user acquisition is not
value-deduplicated. The typed retry operation accepts the same ID: same ID and
same canonical payload return idempotent success, while conflicting content
returns an explicit conflict. The insert is one immediate transaction; failure
rolls back the row. A canonical serialization of only approved fields is
hashed with SHA-256 and verified on reads.

The remote App Server read and local SQLite transaction are not a distributed
atomic operation. A process crash cannot guarantee exactly-once acquisition
across the RPC and database boundary. A later user-initiated read is a new
acquisition with a new ID.

## CLI and history

`codex-meter quota --live` and `--live --json` retain P045A behavior: one live
read, no SQLite open, and no persistence. Only `--live --save` opts into one
snapshot write. `--json` alone never saves. An explicit `--database PATH` is
accepted for a save, and default saves use the existing private data-directory
and database-permission preparation. An RPC, auth, malformed-response, or
transport failure occurs before database access and creates no row. A storage
failure returns a sanitized error without claiming a successful save.
The saved JSON envelope is separately versioned and reports a snapshot ID only
after commit.

`quota --history --source live` reads only
`live_account_quota_snapshots` through `SqliteReadStore`; it never unions the
table with rollout samples. Without the source selector, existing rollout
history is unchanged. On schema 5 or 6 databases, read-only live history is
empty until a later writer explicitly applies migration 7; reading never
migrates or creates a file.

Pages are capped at 1,000 records and ordered by exact acquisition Unix
nanoseconds descending, then snapshot ID descending. Keyset cursors must have
the `lqs_` format, identify an existing row in this dataset, and pass record
integrity validation. Fractional seconds and equal timestamps are covered by
tests; `OFFSET` pagination is not used. Every row retains both meters and their
statuses from one RPC event, so a missing value is never carried forward from a
different acquisition.

## Evidence separation

**An App Server quota snapshot is account-level evidence and does not
authorize P044 task-specific reconciliation.**

P045B adds no `AssociationCredential`, `QualifiedQuotaEvent`,
`TaskQuotaSampling`, `QualifiedSamplingCheckpoint`, P012 reconciliation,
P013/P016 Observation, quality grade, task association, or estimator input.
There is no timestamp-based join or inferred exclusive consumption. P020,
P022, P041, and P042 algorithms and eligibility remain unchanged. Live account
history is not empirical per-task capacity evidence.

## Privacy and security

Persistence retains only normalized numeric meter evidence, validated times,
fixed enums, typed warning codes, an opaque event ID, and a payload checksum.
It stores no prompt, response, reasoning, tool data, source code, repository
path, Git remote, account ID, email, credentials, raw RPC JSON, raw rollout
record, transcript, or arbitrary provider bucket label. P045A transport
redaction and authentication handling remain in use. No login, logout, turn
execution, polling, reset-credit use, or hidden save path is added.

## Tests and validation

File-backed SQLite and fake-server tests cover fresh migration, version-6
upgrade with existing quota and Observation rows preserved, previous migration
checksums, future-schema rejection, read-only file stability, typed validation,
missing/partial/ambiguous meters, 0/100 values, idempotent retries, conflict
rejection, rollback, checksum verification, fractional ordering, equal-time
tie-breaking, keyset continuation, invalid and foreign cursors, and separation
from rollout quota samples.

CLI tests cover flag combinations, an explicit fake App Server read through
normalization and persistence to read-only history, one request per save,
unsupported RPC with no database creation, privacy redaction, unchanged
non-saving P045A JSON, and unchanged rollout history output. The final PR
closeout reports exact Rust/Python/contracts results and exact-head push, PR,
ConPTY, macOS, and four-platform release-validation workflows.

No real account or physical device is used. Installed Codex runtime support,
authenticated live response correctness, and G4/G5 device compatibility remain
**UNVERIFIED**; fake-server and cross-platform tests do not qualify production
quota accuracy.
