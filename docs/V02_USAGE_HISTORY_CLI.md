# v0.2 token usage and quota history CLI

P032 adds native Rust reporting over the checksummed P031 SQLite evidence
ledger. The commands do not start Python, run a Codex session, contact a
provider, read credentials, or change evidence. Run `codex-meter sync` first to
collect currently supported local rollout evidence.

## Commands

```text
codex-meter tokens [--today | --week | --all] [--model MODEL] [--json]
codex-meter history [--today | --week] [--since UTC] [--until UTC]
    [--limit N] [--model MODEL] [--session SESSION_ID] [--after EVENT_ID] [--json]
codex-meter inspect EVENT_ID [--json]
codex-meter quota [--history [--limit N] [--after SAMPLE_ID]] [--json]
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
tables, and never creates a missing file or runs migrations. Version 5 P031
databases remain reportable without modification; `sync` applies migration 6
to add timestamp-normalized history indexes. Evidence rows returned or used in
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
