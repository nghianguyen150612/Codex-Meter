# v0.2 token usage and quota history CLI

P032 adds native Rust reporting over the checksummed P031 SQLite evidence
ledger. The commands do not start Python, run a Codex session, contact a
provider, read credentials, or change evidence. Run `codex-meter sync` first to
collect currently supported local rollout evidence.

## Commands

```text
codex-meter tokens [--today | --week | --all] [--model MODEL] [--json]
codex-meter tasks [--today | --week | --all] [--model MODEL]
    [--session SESSION_ID] [--limit N] [--json]
codex-meter history [--today | --week] [--since UTC] [--until UTC]
    [--limit N] [--model MODEL] [--session SESSION_ID] [--after EVENT_ID] [--json]
codex-meter inspect EVENT_ID [--json]
codex-meter quota [--history [--limit N] [--after SAMPLE_ID]] [--database PATH] [--json]
codex-meter quota --live [--json]
```

Every command also accepts `--database PATH` as an optional advanced override.
Without it, reporting uses exactly the default database path used by `sync`:

- Linux: `$XDG_DATA_HOME/codex-meter/codex-meter.sqlite3`, falling back to
  `~/.local/share/codex-meter/codex-meter.sqlite3` if `XDG_DATA_HOME` is absent
  or relative.
- macOS: `~/Library/Application Support/codex-meter/codex-meter.sqlite3`.
- Windows: `%LOCALAPPDATA%\codex-meter\codex-meter.sqlite3`.

`tokens` defaults to the current UTC calendar day. `--week` selects the trailing
seven days, and `--all` removes the time bound. `history` defaults to all
recorded history with a page size of 20. `--today` selects the UTC calendar day;
`--week` selects the trailing seven days. Its explicit `--since` bound is
inclusive and `--until` is exclusive. Both timestamps must be valid UTC RFC
3339 values ending in `Z`. Time-scope flags cannot be mixed with explicit
bounds.

History uses a deterministic reverse-chronological keyset `(timestamp, event
ID)`. If another page exists, the human summary prints the exact opaque ID to
pass to `--after`; versioned JSON returns both `has_more` and `next_cursor`.
Limits are 1 through 1,000. Model and session filters match exact persisted
values. Terminal history shortens opaque session IDs; JSON and `inspect` retain
the exact IDs.

`tasks` uses the same UTC period defaults as `tokens`, accepts exact model and
opaque session filters, and returns the highest known directly observed raw
totals first. The result is bounded to 1–1,000 tasks (`--limit`, default 20);
JSON reports `has_more` when additional lower-ranked tasks exist. A task key is
the tuple `(source_id, session_id, task_id)`. The source ID prevents collisions
between rollout sources, and the session ID prevents collisions between
sessions. Only persisted task and session identities are accepted. Missing task
or session identity is summarized as unattributed and never attached to a
nearby task. The command does not join P033 App Server thread/turn identifiers
to P031 rollout IDs; no validated mapping currently exists.

Task summaries show response count, first/last observation times, direct source
`total_tokens`, each directly observed component counter, and distinct model,
reasoning, service-tier, and Codex-version configurations. A configuration
tuple that differs in any field remains a separate observation, including
explicitly missing fields. At most 64 configurations are included per task;
`configurations_truncated` marks larger sets. Raw totals rank tasks; missing
raw totals remain unavailable and are not reconstructed from components.
`has_more` is a bounded top-task result indicator rather than a cursor, so
raise `--limit` to inspect more (up to 1,000).

## Token accounting

Only rows with `record_semantic = per_response` count as canonical consumed
responses. Turn cumulative, thread cumulative, and token-count snapshots remain
visible in `inspect` but never enter token totals or response history.

Every displayed counter is a sum of that source-reported field. `input_tokens`
is not relabeled as uncached input, and cached input is not subtracted from it.
Reasoning output is shown separately and is not added to output. A raw total is
only a direct source `total_tokens` value; Codex Meter does not construct it
from components. Therefore the component values may overlap and their simple
sum does not have to equal raw total. Each metric reports a known sum, known
event count, missing event count, and coverage (`complete`, `partial`,
`unavailable`, or `no_observations`). Missing values remain `NULL`; recorded
zero remains zero. Checked integer addition returns a sanitized overflow error
instead of wrapping.

Configuration counts are grouped independently by observed model, reasoning
level, and service tier. Missing configuration values have an explicit
Unavailable group. To keep memory and output bounded, at most 1,000 groups per
dimension are shown; structured output marks a truncated dimension.

## Quota evidence

`quota` reads only `quota_snapshot_evidence` and selects the latest supported
five-hour and weekly samples independently. It shows the observed percentage,
sample time, reset time when present, and a derived `100 - used` remaining
percentage. Percentage bars always include a numeric label. An absent sample
is Unavailable; it is never shown as zero.

Samples older than 24 hours are labeled potentially stale. A future timestamp
or unparsable time has unknown freshness. The 24-hour threshold is a Codex Meter
presentation policy, not a provider guarantee. Old samples are not presented
as the current live quota. A sample is account-meter evidence and cannot prove
that a particular task caused meter movement. Local token totals and account
quota snapshots describe different observation domains; Codex Cloud, ChatGPT
Work, or another concurrent client can affect an account meter without leaving
supported local rollout evidence.

`quota --history` lists persisted samples in reverse chronological order and
supports the same bounded keyset continuation using `--after SAMPLE_ID`.
Task totals do not use quota meter snapshots and cannot establish task-level
account quota consumption.

`quota --live` explicitly requests one in-memory read through the Codex App
Server. Without `--save`, it preserves P045A behavior and does not open or write
SQLite. Use Codex's existing auth state. An unsupported method, unavailable
authentication, or missing App Server is reported as a sanitized diagnostic.
The JSON result has schema version 1.0.0, source and acquisition time, snapshot
availability, five-hour and weekly availability, observed used percentages,
derived remaining percentages, validated UTC resets, typed warnings, and
account-level provenance. No email, account identifier, credential, provider
response, prompt, transcript, or process path is included.

`quota --live --save` explicitly stores the result of one successful read in
the separate `live_account_quota_snapshots` table (migration 7). `--json` does
not imply saving. The save result has its own versioned envelope with commit
status and snapshot ID; the ID is reported only after SQLite commits. Missing
or ambiguous meters remain missing/ambiguous and preserve typed warnings.
Storage failure returns a sanitized error and does not report a successful save.
A new explicit read
gets a new random ID even when its percentages equal an earlier snapshot; exact
same-ID retries are idempotent and conflicting payloads fail. App Server
acquisition and the local transaction are not distributed atomic, so a crash
does not promise exactly-once remote acquisition.

`quota --history --source live` reads only the live account snapshot table and
never unions it with local rollout quota evidence. It is strictly read-only,
bounded to 1,000 snapshots, ordered by precise acquisition nanoseconds and
snapshot ID descending, and paginated by a cursor that must resolve to a saved
live snapshot. Without `--source live`, `quota --history` retains its rollout
quota meaning. Both live outputs state account-level provenance and say task
attribution and forecast qualification are not established. **An App Server
quota snapshot is account-level evidence and does not authorize P044
task-specific reconciliation.**

`quota --compare-live --before-snapshot ID --after-snapshot ID` compares only
the two explicitly selected saved snapshots. `--after-snapshot` is separate
from the history pagination `--after` cursor. The comparator opens the existing
database through `SqliteReadStore`, validates each stored checksum and fixed
source/provenance contract, requires strictly increasing UTC acquisition
times, and never invokes Codex App Server. It rejects live, save, diagnose,
history, source, limit, and pagination-cursor combinations. `--database PATH`
selects an existing database and remains read-only.

Five-hour and weekly meters are classified independently. A movement is
reported only when both observed percentages have the same observed reset
timestamp, the reset is after both acquisition times, bucket evidence is the
main Codex bucket, and the percentage did not decrease. A crossed or changed
reset, missing reset metadata, unavailable meter, ambiguous bucket evidence,
or decreasing percentage suppresses movement. Missing evidence is never
treated as zero. A zero movement is one unchanged account-meter pair, not a
stability or task-consumption finding. Output is versioned and reproducible
from the two IDs and comparator version; results are not persisted.

The comparison remains account-level: fixed App Server source labels do not
prove that both snapshots belong to the same authenticated account. External
account activity, task causality, controlled isolation, token coverage,
production quota accuracy, official token allowance, and estimator eligibility
remain unverified or unavailable. No P044 event, Observation, or estimator
input is created.

This snapshot describes account quota state at the time of the explicit read.
It does not prove quota consumed by a task, complete token coverage, task
association, or capacity-estimator qualification. **An App Server quota
snapshot is account-level evidence and does not authorize P044 task-specific
reconciliation.** Offline fake-server tests do not qualify an authenticated
live account or all installed Codex versions; those gates remain UNVERIFIED.

## Fixture-only display examples

The following short examples use synthetic Rust test fixtures. They are not
from a Codex account and do not validate live quota accuracy:

```text
Codex Meter — Token Usage History
Period: All recorded history

Time (UTC)              | Session   | Model            | Reasoning    | Raw total          | Evidence
2026-10-20T12:00:00Z    | sess:fix1 | gpt-5.6-sol      | high         |                 15 | Observed
```

```text
5-hour meter
  [########............] 42%
  Used:      42% observed
  Remaining: 58% (derived)
  Observed:  2026-10-20T12:00:00Z (recent by 24-hour policy)
  Reset:     Unavailable
```

## Read-only, integrity, and privacy

Reporting opens the existing database using SQLite read-only flags and
`query_only`. It validates migration history and checksums, verifies required
tables, and never creates a missing file or runs migrations. Version 5 and 6
databases remain reportable without modification; `sync` applies migration 6
for timestamp-normalized rollout indexes and migration 7 for live account
snapshots. Evidence rows returned or used in
aggregates are decoded as typed evidence and checked against their stored
SHA-256 checksum. Corrupt rows fail with a sanitized database-integrity error.

The normal location is shared with `sync`; a missing database returns guidance
to run `codex-meter sync`. A valid empty database reports `No observations yet`
or `No supported sample` rather than implying zero account usage. JSON uses a
separate versioned `schema_version: "1.0.0"` reporting contract. It contains
explicit nulls for unavailable evidence, exact opaque IDs, source provenance,
time scope, and pagination status. It does not alter AnalyticsRequest v1 or
AnalyticsResult v1.

The inspector never displays source payloads, prompts, responses, account IDs,
credentials, or original filesystem paths. It reports only typed normalized
fields and hashed/opaque identities. Data coverage summarizes persistent
diagnostics for unsupported versions, compressed sources, malformed or
incomplete records, continuity ambiguity, and known partial collection. These
counts describe Codex Meter's local collection, not all ChatGPT account use.

P031's parser is verified only against the audited Codex CLI 0.157.1 rollout
format. P032 does not broaden that compatibility claim. The reference Arch
Linux live-device validation is still unresolved; fixture success is not live
compatibility or quota-accuracy validation. See
[`PROMPT032_MANIFEST.md`](PROMPT032_MANIFEST.md) and
[`PROMPT031_MANIFEST.md`](PROMPT031_MANIFEST.md) for validation status and safe
local checks.

## Handoff

P033 may add a future TUI or plugin adapter over the reusable typed reporting
repositories. Project metadata remains a separately authorized adapter; P032
does not derive project names from local paths. Task quota attribution requires
separate verified baseline, window, stabilization, and reconciliation evidence.

## Interactive task history (P040)

`codex-meter tui` exposes the same typed P039 report in History (`5`, then `t`).
The TUI loads the top 200 tasks and navigates that bounded list in 50-row pages;
`has_more` is a truncation notice, never a database cursor. For a larger report,
use `codex-meter tasks --all --limit 1000`. The task overlay shows full composite
identity and per-counter coverage. No task quota percentage or App Server ID
mapping is inferred. See [TUI navigation and details](V02_TUI_DASHBOARD.md#task-usage-p040).
