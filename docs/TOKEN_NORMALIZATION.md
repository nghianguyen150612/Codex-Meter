# Codex Token Normalization

## Scope

P008 implements the raw-token boundary between the P006 source model and the
v1 `token_counters_updated` normalized event:

```text
P006 source record + P007 source-position evidence
        ↓
raw token evidence classification
        ↓
double-count-safe normalization
        ↓
v1 token_counters_updated event
```

The compatibility target remains Codex CLI `0.157.1` and official upstream
revision `8f7a0f7a878199c6886600370e5be6bd37ca38a3`. P008 does not broaden that
claim or change the v1 JSON Schema.

The project domains remain separate:

```text
raw tokens != weighted/effective usage != plan quota
```

This work covers raw token telemetry only. It does not implement quotas,
weighting, capacity estimation, lifecycle assembly, or persistence.

## Canonical consumption source

For the pinned source format, only:

```text
token_usage_record.payload.usage
```

is canonical event-local consumption evidence. It is classified as
`TokenEvidenceSemantic::PerResponse` and may produce one normalized token event.

These structures are extracted but never independently normalized as new
consumption:

- `turn_token_usage` — `TurnCumulativeSnapshot`;
- `thread_token_usage` — `ThreadCumulativeSnapshot`;
- `token_count.info.total_token_usage` — `TokenCountTotalSnapshot`;
- `token_count.info.last_token_usage` — `TokenCountLatestSnapshot`.

P008 never subtracts cumulative snapshots, sums snapshots, or emits a second
event from `last_token_usage`. Snapshot evidence remains available to later
recovery or consistency work.

## Raw evidence model

`RawTokenEvidence` contains the original P006 `TokenUsage` plus an explicit
`TokenEvidenceSemantic`. `TokenEvidenceSet` exposes named optional fields:

| Field | Source meaning | Consumption role |
| --- | --- | --- |
| `per_response` | one response usage | canonical consumption |
| `turn_cumulative` | cumulative turn usage | snapshot only |
| `thread_cumulative` | cumulative thread/session usage | snapshot only |
| `total_snapshot` | token-count cumulative total | snapshot only |
| `latest_snapshot` | token-count latest usage | snapshot only |

Every upstream counter remains an `Option<TokenCount>`. P008 does not turn
missing values into zero and does not recompute source totals.

## Source-to-normalized mapping

The v1 normalized contract exposes:

```text
uncached_input
cached_input
output
reasoning_output
raw_total
```

For canonical per-response usage, P008 maps these fields as follows:

| Normalized metric | Source field | Behavior | Provenance |
| --- | --- | --- | --- |
| `uncached_input` | none | unavailable; `input_tokens - cached_input_tokens` is not proven safe | unavailable |
| `cached_input` | `cached_input_tokens` | direct copy when present | observed |
| `output` | `output_tokens` | direct copy when present | observed |
| `reasoning_output` | `reasoning_output_tokens` | direct copy when present | observed |
| `raw_total` | `total_tokens` | direct copy when present | observed |

`input_tokens` remains available in `RawTokenEvidence` but is not relabeled as
`uncached_input`. `cache_write_input_tokens` also remains available in raw
evidence and is not added to cached input, uncached input, output, reasoning
output, or raw total.

P008 preserves upstream `total_tokens` even when it differs from any naïve
component sum. It never adds reasoning output onto output and never derives a
replacement total. An absent upstream total remains unavailable.

The generic v1 fixture remains unchanged. The Codex-specific fixture
`fixtures/contracts/v1/normalized-token-event-codex-rollout.json` demonstrates
that `uncached_input` is unavailable while directly observed counters remain
available.

## Normalized event model

`NormalizedTokenEvent` serializes to the existing normalized-event contract
with:

- `schema_version: "1.0.0"`;
- `event_type: "token_counters_updated"`;
- `payload.kind: "token_counters"`;
- schema-shaped `TokenMetric` availability/value/provenance objects.

The Rust model is a typed serializer for this subset; the repository's Python
validator remains the contract authority. No competing schema or contract
version was added.

## Deterministic identities

P008 uses SHA-256 with explicit domain tags and length-delimited components.
Generated IDs use schema-valid prefixes:

- `src:<digest>` from logical rollout ID plus source generation;
- `evt:<digest>` from source identity, start/end offsets, ordinal, and the
  token-normalization domain/version;
- `cursor:<digest>` from source identity plus the processed end offset;
- `session:<digest>` from the upstream session ID;
- `task:<digest>` from the upstream turn ID.

The IDs are bounded opaque values, contain no paths or payloads, and are
stable across replay. Different source positions produce different event and
cursor identities, while a replacement source generation produces distinct
source, event, and cursor identities. No random UUID or process-local hash is
used, and no in-memory deduplication table is required.

## Timestamp boundary

A per-response event uses the P006 rollout timestamp unchanged after validating
its UTC RFC 3339 shape, calendar date, time range, and optional fractional
seconds. Invalid timestamps produce a structural normalization error and never
produce an invalid normalized event. The source timestamp value is not included
in the error text.

## Privacy

The normalized event contains only hashed opaque IDs, timestamp, token metrics,
and source correlation IDs. It does not retain paths, usernames, CWD, Git data,
creator/account identifiers, response IDs, prompts, responses, reasoning text,
source code, rate limits, or raw JSON lines. Rejected P007 lines are classified
without creating zero-token events.

## Current limitations

- `uncached_input` remains unavailable because P005 did not prove subtraction
  semantics across the supported source/provider set.
- `cache_write_input_tokens` is preserved only in raw evidence because v1 has no
  normalized field for it.
- Configuration timeline association is not implemented; model, reasoning, and
  service-tier state are not attached to token events here.
- No token-event persistence, durable deduplication, cursor storage, quota or
  rate-limit interpretation, filesystem discovery, watcher, compression,
  lifecycle assembly, observation, reconciliation, estimator, or CLI UI exists.
- Compatibility remains evidence-backed only to the pinned Codex target.

## Inputs for P009

P009 may assume:

- P006 decodes safe complete source records;
- P007 delivers complete replay-safe records with source positions;
- `token_usage_record.payload.usage` is the canonical event-local consumption
  source;
- cumulative snapshots never become independent consumption events;
- normalized event IDs, source IDs, cursor IDs, session IDs, and task IDs are
  deterministic and replay-stable;
- normalized token events conform to the existing v1 schema;
- raw `input_tokens` and `cache_write_input_tokens` remain available internally;
- `uncached_input` remains unavailable under the current evidence baseline.

P009 owns ordered session/task lifecycle assembly and configuration state.
