# v0.2 local Codex telemetry ingestion

P031 adds the first native, one-shot ingestion path for local Codex rollout
files. It stores privacy-filtered token and quota evidence in Rust-owned SQLite
and resumes each source from a durable checkpoint. It is an on-demand command,
not a daemon or automatic background tracker.

## Run a local sync

From a development checkout:

```bash
cargo run --manifest-path rust/Cargo.toml -- sync
cargo run --manifest-path rust/Cargo.toml -- sync --json
cargo run --manifest-path rust/Cargo.toml -- sync --codex-home '/path/to/.codex'
cargo run --manifest-path rust/Cargo.toml -- sync --database '/path/to/history.sqlite3'
cargo run --manifest-path rust/Cargo.toml -- sync --help
```

The built native binary accepts the same `codex-meter sync` forms. The default
command needs no manually created database, Python executable, request JSON,
timestamp, or quota percentage. `--database` is an advanced override; the
SQLite file is opened and migrated by Rust.

The first sync creates this local database when a Codex home is found:

| Platform | Default database |
| --- | --- |
| Linux | `$XDG_DATA_HOME/codex-meter/codex-meter.sqlite3`, or `~/.local/share/codex-meter/codex-meter.sqlite3` when `XDG_DATA_HOME` is absent or relative |
| macOS | `~/Library/Application Support/codex-meter/codex-meter.sqlite3` |
| Windows | `%LOCALAPPDATA%\codex-meter\codex-meter.sqlite3` |

On Unix, Codex Meter requests mode `0700` for its default data directory and
`0600` for its database. It does not change permissions on Codex files.

## Discovery and supported compatibility

Discovery chooses `--codex-home`, then a valid `CODEX_HOME`, then the current
user's default `~/.codex` location. Active plain rollouts are discovered below
`sessions/YYYY/MM/DD/`, including bounded nested rollout directories. The pinned
source audit describes `archived_sessions` as a separate flat root, so P031
scans plain rollout files directly inside that directory rather than assuming
the active date layout applies there.

P031 accepts only the telemetry format identified by a complete `session_meta`
record with Codex CLI version **0.157.1**. That is the format audited against
upstream revision
`8f7a0f7a878199c6886600370e5be6bd37ca38a3` and the repository's structural
fixtures. It is a historical compatibility baseline, not a claim about newer
Codex versions. P031 does not invoke `codex` to determine its version; it only
checks whether a Codex executable is present on `PATH` for the human summary.
Unknown versions are skipped with a visible coverage warning.

Plain `.jsonl` files are supported. `.jsonl.zst` files are detected and counted
but not decompressed in P031. If a source has only a compressed representation,
its token and quota evidence is absent from the saved history. When both plain
and compressed siblings exist, the supported plain file is used. Discovery
limits directory entries, file count, nesting depth, header size, and symlink
traversal. It does not follow symlinked source entries. A file that cannot be
opened does not stop processing unrelated sources.

The incremental reader accepts only newline-complete records. Its limits are
1 MiB per record, 4 MiB and 2,048 records per read batch, 128 MiB and 100,000
records per source per invocation, 50,000 directory entries, and 10,000 rollout
files. Limits and skipped sources appear in the summary. An incomplete trailing
record remains unread and can be completed by a later append.

## What is recorded

For each supported `token_usage_record`, the canonical consumption evidence is
only `payload.usage`. The ledger preserves directly reported input tokens,
cached input tokens, cache-write input tokens, output tokens, reasoning output
tokens, and `total_tokens` independently. Missing counters are SQLite `NULL`;
an observed zero remains zero. No raw total is reconstructed from components.
`input_tokens` is not interpreted as uncached input, cached input is not
subtracted from it, and reasoning output is not added to output.

The following are stored as separate snapshot evidence and never summed as new
consumption: `payload.turn_token_usage`, `payload.thread_token_usage`, and
`event_msg/token_count`'s `info.total_token_usage` and `info.last_token_usage`.
The CLI's raw token total is the sum of known, directly reported per-response
`total_tokens` values in saved evidence. If some events omit the direct total,
the summary labels the result partial or unavailable.

The ledger also keeps normalized event/session/task identities, observation
timestamps, model and reasoning metadata, Codex version, observed service tier,
and typed malformed/unsupported evidence diagnostics. Upstream `service_tier` is
preserved as its own field; P031 does not treat it as the product's `speed_mode`.
Configuration labels must use a short identifier alphabet; free-text-looking
labels are rejected before they can enter checkpoint state or evidence rows.
Plan is recorded when it accompanies an observed quota sample; it is not copied
onto a response without a validated response-level plan association. Speed
mode remains unavailable in the audited source format.
Prompt and response bodies, reasoning text, tool arguments/results, source code,
repository and working-directory paths, Git remotes, account identifiers,
credentials, and raw rollout JSON are neither stored nor emitted.

Supported `event_msg/token_count/rate_limits` snapshots are retained separately
for five-hour and weekly windows when the existing quota normalizer can classify
them. Captured values include observed used percentage, sample and reset times,
source identity, audited Codex version, and plan when supplied. Missing
percentages remain unavailable.
Ambiguous or unsupported windows are diagnosed rather than mapped by primary
or secondary slot. A quota snapshot describes an account meter at one time; it
does not establish a task's quota cost. P031 does not invent before/after quota
samples or task deltas. Task-completion Observations can therefore contain
observed token evidence while both quota domains remain unavailable/incomplete.

## Replay, checkpoints, and history

P031 adds SQLite migration 5, `0005_usage_evidence.sql`. It creates typed
`token_usage_evidence`, `quota_snapshot_evidence`, `ingestion_diagnostics`, and
`source_boundary_fingerprints` tables. Existing migrations 1–4 are unchanged;
all migration checksums remain recorded and verified. Rust alone runs
migrations. The database rejects token counters outside the safe integer
range, and every row is checksummed over its typed privacy-safe values.

P032 adds migration 6, `0006_usage_history_indexes.sql`, with normalized-time
indexes for global token and quota history ordering. It does not change P031
evidence rows or their checksums. The native `tokens`, `history`, `inspect`, and
`quota` commands read the evidence ledger through a separate strictly read-only
SQLite connection. See the [usage history CLI guide](V02_USAGE_HISTORY_CLI.md)
for filters, pagination, missing-value behavior, JSON output, and quota
freshness policy.

Each source has a hashed rollout identity and generation, an incremental reader
cursor, the last complete line fingerprint, and a platform file-identity
fingerprint where available. Token/quota/record-level diagnostic evidence,
applicable task Observation, checkpoint state, boundary fingerprint, and
committed cursor advance together in one SQLite transaction per complete source
batch. A failure before commit replays the batch. Sanitized source-boundary and
discovery diagnostics that explain a skipped source use a separate small
transaction that leaves the cursor unchanged. A subsequent scan of unchanged
input adds no consumption events.

Moving a file without replacing it (for example, an in-volume rename into
`archived_sessions`) retains its file identity and resumes the prior checkpoint.
Truncation, changed committed bytes, or a replacement file is reported and is
not silently rewound. If multiple physical files share a rollout identity, or
source continuity cannot be proved, the ambiguous evidence is skipped rather
than merged into trusted totals. Local SHA-256 fingerprints detect accidental
change; they are not independent proof against someone who controls the
machine.

The CLI reports observed scan counts and saved-history totals without including
private source/database paths. `sync --json` emits a typed, path-free summary.
SQLite locked, corrupt, or newer-than-supported databases fail with sanitized
categories. Codex Meter never writes to or deletes Codex-owned files.

## Validation gates and limitations

Automated fixtures cover the pinned 0.157.1 structural contract, discovery
precedence, active/archive movement, compressed-source warnings, bounded
newline reading, replay, incomplete tails, source replacement, transaction
rollback, symlink skips, token snapshots and missingness, quota separation,
task Observations, database privacy, and Unicode/space paths. These tests are
not proof of current-format compatibility on another Codex version.

The reference Arch desktop is not attached to the implementation environment.
P031 therefore has no live-device ingestion or restart result. Before treating
the device gate as passed, run the safe commands in
[`PROMPT031_MANIFEST.md`](PROMPT031_MANIFEST.md) on that device and record only
sanitized counts, version, replay result, and privacy checks. Do not generate
an extra paid request for validation.

P031 provides durable token history and directly observed quota snapshots. It
does not provide continuous tracking, authenticated quota acquisition,
automatic quota stabilization, or estimator-ready per-task quota evidence.
Those remain later work. P032 should build history and token inspection over
this typed ledger, preserve its raw/snapshot distinction, and only add task
quota attribution after valid baseline, time-window, stabilization, and
reconciliation evidence exists.
