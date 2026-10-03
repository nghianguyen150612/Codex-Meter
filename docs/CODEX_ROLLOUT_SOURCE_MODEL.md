# Codex Rollout Source Model

## Scope

This document describes the privacy-filtered Rust source model introduced by
P006. It decodes one complete Codex rollout JSON object and does not implement
filesystem discovery, JSONL iteration, cursors, normalization, or persistence.

The boundary is intentionally:

```text
raw Codex rollout record
        ↓
one-record source decoding
        ↓
privacy-filtered source model
        ↓
future normalization
```

## Compatibility target

The model is evidence-backed against:

- Codex CLI `0.157.1`;
- official `openai/codex` revision
  `8f7a0f7a878199c6886600370e5be6bd37ca38a3`;
- the rollout structures documented by P005.

This is not a claim of compatibility with future Codex versions. Unknown
discriminators are classified safely rather than interpreted optimistically.

## API

`codex_meter::telemetry::codex_rollout::parse_rollout_record` accepts one
complete JSON object and returns either a `RolloutRecord` or a structural
`RolloutDecodeError`. It does not accept a JSONL stream and does not retain the
input `serde_json::Value` after decoding.

The envelope model contains:

- `timestamp: String`;
- `ordinal: Option<u64>`;
- a typed `RolloutRecordKind` selected by the top-level `type` discriminator.

Known source records require the expected payload shape. Unknown records do not
require payload decoding.

## Supported records

The privacy-filtered model supports these top-level records:

| Top-level type | Safe model | Retained fields |
| --- | --- | --- |
| `session_meta` | `SessionMetaRecord` | session/thread IDs, CLI version, optional provider and source classification |
| `turn_context` | `TurnContextRecord` | optional turn IDs, model, optional reasoning effort |
| `token_usage_record` | `TokenUsageRecord` | session/thread/turn IDs, optional root-turn ID, and three named usage structures |
| `event_msg` | `EventMessage` | only the event variants listed below |

Known content-heavy records such as `response_item`, `realtime_item`,
`world_state`, `retained_context`, `compacted`, and inter-agent records are
classified as `KnownButIgnored`. A future top-level discriminator is classified
as `Unknown`. Both classifications retain only the bounded discriminator.

## Supported events

`event_msg.payload.type` supports:

- `token_count`;
- `task_started` and compatibility alias `turn_started`;
- `task_complete` and compatibility alias `turn_complete`;
- `turn_aborted`;
- `context_compacted`;
- `thread_settings_applied`.

Unknown event discriminators become `EventMessage::Unsupported` with only the
bounded event type. Event payloads are not retained.

Lifecycle models retain IDs, timestamps/durations, completion-vs-failure
status, known abort reasons, and the fact of compaction. Arbitrary error text,
trace IDs, collaboration details, summaries, and similar content are ignored.
An unrecognized abort reason is represented as `TurnAbortReason::Unknown`
without retaining the source text.

`service_tier` is exposed as the upstream string in
`ThreadSettingsAppliedRecord`. It is not mapped to Codex Meter's `standard` or
`fast` speed modes.

## Token semantics

`TokenUsage` preserves the six confirmed upstream counters:

- `input_tokens`;
- `cached_input_tokens`;
- `cache_write_input_tokens`;
- `output_tokens`;
- `reasoning_output_tokens`;
- `total_tokens`.

Each counter is `Option<TokenCount>` so missing and explicit zero remain
distinct. Present counters must be non-negative integers no larger than the
JSON safe-integer maximum `9_007_199_254_740_991`. Fractions, negative values,
and larger values are rejected; no value is coerced or truncated.

The source model uses distinct field names for distinct upstream semantics:

| Field | Meaning | P006 behavior |
| --- | --- | --- |
| `TokenUsageRecord::usage` | usage for one completed response | preserved as a separate structure |
| `TokenUsageRecord::turn_token_usage` | cumulative turn snapshot | preserved as a separate structure |
| `TokenUsageRecord::thread_token_usage` | cumulative thread/session snapshot | preserved as a separate structure |
| `TokenCountInfo::total_token_usage` | cumulative token-count snapshot | preserved as a separate structure |
| `TokenCountInfo::last_token_usage` | latest appended usage snapshot | preserved as a separate structure |

P006 does not calculate uncached input, replacement totals, deltas, weighted
usage, or effective usage. It also does not assume that reasoning output is
additive to output totals or that upstream `total_tokens` equals a sum of the
six source fields. Those are P008 normalization questions and remain
evidence-dependent.

The `rate_limits` member of `token_count` is now retained through a narrow
P010-safe model. It contains only the optional `limit_id`, `primary` and
`secondary` windows (`used_percent`, `window_minutes`, and `resets_at`), and a
recognized provider `plan_type`. Credits, spend-control monetary values,
model-slug metadata, arbitrary fields, and raw JSON remain discarded. Quota
semantics are applied only by the separate P010 quota normalizer.

## Privacy filtering

The public source model contains no generic payload field. It intentionally
does not retain prompts, responses, reasoning text, source code, paths, working
directories, Git metadata, account identifiers, response IDs, raw errors,
credits, monetary spend values, or provider request payloads. Synthetic
fixtures include fake sensitive-looking values to regression-test their
removal; no local rollout data is committed.

String discriminators and allowlisted IDs/configuration values are bounded and
control-character checked. They are retained only where the P005 evidence
identified them as useful source metadata.

## Error and unknown policy

Malformed JSON, non-object top-level values, missing required fields, wrong
known-record shapes, invalid numeric values, and unsafe allowlisted strings
produce structural errors. Error display identifies record/field structure and
numeric reason, never the complete input record or payload value.

Unknown future top-level records and event variants are non-fatal and
non-content-retaining. This keeps later stream readers forward-safe while
leaving version-specific interpretation explicit.

## Current limitations

- The model is pinned to the P005 evidence target; it has no broad version
  negotiation or schema-version dispatch.
- It parses one complete JSON object only.
- It does not discover sessions, read files, handle partial trailing lines,
  watch for changes, decompress rollout files, or persist cursors.
- It does not calculate token deltas or assemble task/session state.
- It does not normalize into `schemas/v1/normalized-event.schema.json`.
- It does not acquire or interpret quotas/rate limits and cannot observe all
  Codex Work/web or account activity.

## Inputs for P007

P007 may safely assume:

- `parse_rollout_record` decodes one complete JSON object;
- the envelope includes a timestamp, optional ordinal, and top-level type;
- supported records return typed privacy-filtered source models;
- known ignored and unknown records are non-content-retaining;
- supported event aliases are classified consistently;
- per-response, turn-cumulative, thread-cumulative, total-snapshot, and
  latest-usage structures have distinct names and types;
- token counters preserve missingness, preserve zero, and reject unsafe values;
- decoding errors expose structure rather than raw payload content;
- synthetic source fixtures are available under
  `fixtures/codex-rollout/v0.157.1/`.

P007 still owns complete-line boundaries, partial trailing-line behavior,
incremental reading, replay/cursor mechanics, and file replacement or
truncation handling.
