# Codex Telemetry Discovery

## Scope and status

This document is the P005 evidence audit for Codex Meter. It records what was
observable on **October 3, 2026**, from the installed Codex CLI and from the
official `openai/codex` source pinned below. It is a discovery report, not a
parser design or a claim about undocumented provider quota behavior.

The existing product and data contracts remain authoritative:

```text
Track → Measure → Estimate → Compare
```

```text
raw tokens != weighted/effective usage != plan quota
```

### Discovery environment

| Item | Result |
| --- | --- |
| Platform | Linux x86_64 |
| Installed CLI | `codex-cli 0.157.1` |
| CLI executable | Present; `codex` resolved successfully |
| Codex storage root | `CODEX_HOME` was set and resolved to a Codex home; the personal absolute path is intentionally omitted |
| Upstream repository | Official `openai/codex` repository |
| Upstream revision | `8f7a0f7a878199c6886600370e5be6bd37ca38a3` (`main`, inspected October 3, 2026) |
| Repository state | Clean detached checkout used outside this repository |
| Local session files | Rollout JSONL files were present under the configured home |
| Local archive directory | `archived_sessions` was not present in the inspected home |
| Local SQLite surfaces | Versioned state, thread-history, and log databases were present |

### Privacy method

Local inspection used file names, sizes, timestamps, SQLite table/column names,
JSON keys, discriminator names, value types, and numeric-field presence only.
No prompt, response, reasoning, source code, repository content, working
directory, Git remote, email, account identifier, credential, cookie, token, or
raw unfiltered telemetry record is reproduced here. Examples below are
synthetic structural descriptions, not copied records.

## Evidence classification

- **Confirmed** means directly observed locally or established by the pinned upstream source/tests.
- **Likely but not yet proven** means a reasonable implementation hypothesis that still needs a controlled adapter test.
- **Unknown** means the evidence did not establish the behavior.
- **Deferred** means deliberately outside P005, even when a related field exists.

## Confirmed findings

### Primary persistence surface

The primary candidate is the Codex rollout stream: append-oriented JSONL session
files. The pinned source defines `sessions` and `archived_sessions` as rollout
subdirectories and describes the active layout as:

```text
<codex-home>/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread-id>.jsonl
```

The inspected local home contained dated `sessions/YYYY/MM/DD/` directories and
plain `.jsonl` rollout files. No local `archived_sessions` directory was found;
that absence is only a local observation, not a universal behavior.

Current upstream also supports a compressed sibling form, `.jsonl.zst`, through
the rollout line reader. The source applies plain-file precedence when both
representations exist and can materialize a compressed rollout back to plain
JSONL before append operations.

### Secondary local surfaces

The inspected Codex home also contained versioned SQLite databases. The relevant
confirmed structures are:

| Surface | Confirmed contents | P005 relevance |
| --- | --- | --- |
| `state_5.sqlite` | `threads` metadata, including `tokens_used`, model, reasoning effort, CLI version, provider, timestamps, archive state, and rollout path | Useful index/repair metadata; not a replacement for raw rollout usage records |
| `thread_history_1.sqlite` | Projected thread items/turns and `next_rollout_byte_offset` plus `next_rollout_ordinal` | Strong evidence for upstream projection cursors; contains copied item payloads and therefore has high privacy risk |
| `logs_2.sqlite` | Operational log rows with timestamps, levels, targets, and optional thread/process identifiers | Not a token source; avoid as a primary source |

Upstream `state::extract::apply_event_msg` updates the
`threads.tokens_used` aggregate from `TokenCount.info.total_token_usage.total_tokens`.
This makes the state database a derived index of one aggregate snapshot, not a
source for the full component breakdown or per-response usage.

### Rollout record envelope

The pinned upstream `RolloutLine` and `RolloutItemWire` establish this generic
shape:

```text
{
  "timestamp": <string>,
  "ordinal": <optional integer>,
  "type": <snake_case rollout discriminator>,
  "payload": <object>,
  "metadata": <optional object for response items>
}
```

`type` and `payload` are the important persistence envelope fields. The
`metadata` member is optional and is used by response-item records. The
persisted top-level item discriminators defined by the pinned source are:

```text
session_meta
response_item
inter_agent_communication
inter_agent_communication_metadata
compacted
turn_context
token_usage_record
world_state
retained_context
security_risk_score
event_msg
realtime_item
```

`event_msg.payload` is a second, internally tagged event envelope using a
`type` discriminator. The upstream enum uses stable-looking snake_case names,
but the set is version-sensitive and must not be treated as permanently closed.

### Local record observations

The local rollout files matched the upstream envelope. Structural inspection
confirmed these token-bearing records and nested paths without retaining values:

```text
event_msg / payload.type = token_count
  payload.info.total_token_usage.*
  payload.info.last_token_usage.*
  payload.info.model_context_window
  payload.rate_limits.*

token_usage_record
  payload.usage.*
  payload.turn_token_usage.*
  payload.thread_token_usage.*
  payload.thread_id, payload.turn_id, payload.session_id,
  payload.root_turn_id, payload.response_id
```

Local files also contained `session_meta`, `turn_context`, `task_started`,
`task_complete`, `thread_settings_applied`, `context_compacted`, and
`turn_aborted` records. These observations support the upstream source reading;
they do not establish that every Codex version emits every record.

## Token telemetry audit

### Confirmed token fields

The upstream `TokenUsage` structure has these numeric fields, serialized as
integers in the current source:

| Upstream field | Containing structure | P005 semantic classification |
| --- | --- | --- |
| `input_tokens` | `TokenUsage` | Direct upstream usage component |
| `cached_input_tokens` | `TokenUsage` | Direct upstream usage component |
| `cache_write_input_tokens` | `TokenUsage` | Direct upstream usage component; outside the current five-domain Meter model |
| `output_tokens` | `TokenUsage` | Direct upstream usage component |
| `reasoning_output_tokens` | `TokenUsage` | Direct upstream usage component when supplied |
| `total_tokens` | `TokenUsage` | Direct upstream total field; exact identity to components is not established |

`TokenUsage` also has an internal optional `codex_rollout_budget_units` field in
the pinned source. It is skipped from normal serialization and is not an input
to P005 or the Codex Meter raw-token contract.

### Two different usage families

The source distinguishes two useful families:

1. **Per-response usage:** `RawResponseCompletedEvent.token_usage` is described
   by upstream as exact usage reported by one completed Responses API response.
   `Session::record_observed_response_completed` persists the same usage in a
   `TokenUsageRecord` when available.
2. **Session/UI token snapshots:** `TokenCountEvent.info` contains
   `TokenUsageInfo`, whose `total_token_usage` and `last_token_usage` are
   maintained by Codex session state and emitted in `event_msg/token_count`.

The per-response record is the strongest raw-token candidate. The snapshot is
useful for current UI context and aggregate recovery, but must not be summed as
though every snapshot were a new delta.

### Cumulative versus incremental semantics

The pinned source proves the following:

| Structure | Semantic | Classification |
| --- | --- | --- |
| `token_usage_record.payload.usage` | Usage for the one completed response represented by `response_id` | Event-local/per-response value; do not treat as a cumulative snapshot |
| `token_usage_record.payload.turn_token_usage` | `usage` added to the previous record when the `turn_id` is unchanged | Cumulative within the current turn; not a delta |
| `token_usage_record.payload.thread_token_usage` | `usage` added to the previous persisted record in session state | Cumulative thread/session snapshot; not a delta |
| `token_count.payload.info.last_token_usage` | The latest usage appended to `TokenUsageInfo` | Latest-value field; not a lifetime total |
| `token_count.payload.info.total_token_usage` | Prior total plus the latest usage in `TokenUsageInfo::append_last_usage` | Cumulative snapshot, not a delta |
| `state_5.sqlite.threads.tokens_used` | Derived from the latest `total_token_usage.total_tokens` snapshot | Aggregate index value; not a component or response delta |

Two qualifications are important:

- Codex can recompute or fill token information for context-window/compaction
  behavior. The pinned `recompute_token_usage` and `fill_to_context_window`
  paths can update the snapshot with estimated/context values. Therefore a
  `token_count` snapshot is not proven to be provider-only accounting.
- Resume restores the latest token usage record from rollout history or a
  compaction checkpoint. This supports continuity across normal resume, but
  fork/revert lineage and cross-thread attribution need adapter tests before
  being treated as one simple lifetime total.

### Mapping toward Codex Meter raw-token domains

| Codex Meter domain | P005 result | Reason |
| --- | --- | --- |
| `uncached_input` | Not yet proven safely derivable | `input_tokens` and `cached_input_tokens` exist, but the source does not establish that cached input is a disjoint partition suitable for subtraction in every provider/model case |
| `cached_input` | Directly observable | `TokenUsage.cached_input_tokens` is present when a usage object is present |
| `output` | Directly observable | `TokenUsage.output_tokens` is present when a usage object is present |
| `reasoning_output` | Directly observable when supplied | `TokenUsage.reasoning_output_tokens` is a direct field; absence remains missing, not zero |
| `raw_total` | Directly observable as upstream `total_tokens` | The field exists in per-response and aggregate structures, but its accounting identity is not proven |

`cache_write_input_tokens` must remain a separate upstream field until a later
contract decision. P005 does not fold it into cached or uncached input.

### Raw-total warning

The evidence does **not** prove this identity:

```text
total_tokens = uncached_input + cached_input + output + reasoning_output
```

It also does not prove whether `reasoning_output_tokens` is already included in
`output_tokens` for every provider. P006/P007 must preserve upstream totals and
components separately, record missingness explicitly, and avoid double-counting
reasoning output or summing cumulative snapshots.

## Model and configuration evidence

| Field/concept | Evidence | Classification |
| --- | --- | --- |
| Model provider | `SessionMeta.model_provider`; `ThreadSettingsSnapshot.model_provider_id` | Directly observable when present |
| Model | `TurnContextItem.model`; `ThreadSettingsSnapshot.model`; model-reroute event fields | Directly observable, potentially changing within a thread |
| Reasoning level | `TurnContextItem.effort`; `ThreadSettingsSnapshot.reasoning_effort` | Directly observable when present |
| Service tier/speed setting | `ThreadSettingsSnapshot.service_tier` | Directly observable as an optional configuration field in the pinned revision; semantic mapping to Standard/Fast is not established |
| Codex version | `SessionMeta.cli_version`; indexed in the local state database | Directly observable at session creation |
| Provider plan | `RateLimitSnapshot.plan_type` exists under token-count rate-limit data | Structurally observable when present, but quota/plan semantics are deferred and must not be inferred from unrelated config |
| Timing-derived speed | No evidence | Unavailable; P005 does not infer speed from latency |

No source evidence establishes that the local CLI rollout is a complete account
usage ledger, or that a local model/provider field identifies a ChatGPT plan.

## Session and task lifecycle

### Strong lifecycle markers

| Lifecycle fact | Evidence | Strength |
| --- | --- | --- |
| Session created/detected | First `session_meta` record and rollout filename timestamp | Strong for file/session discovery |
| Task/turn started | `event_msg` with `payload.type = task_started`; `turn_id`, optional `root_turn_id`, `started_at`, and context-window data | Strong |
| Task/turn completed | `event_msg` with `payload.type = task_complete`; `turn_id`, optional error, start/end, duration, and time-to-first-token | Strong |
| Item started/completed | `item_started`/`item_completed` event structures | Strong for item lifecycle, not a replacement for turn segmentation |
| Turn aborted | `turn_aborted` event | Strong interruption evidence |
| Context compacted | `context_compacted` event and `compacted` rollout item | Strong compaction evidence, not session end |

The current source accepts `turn_started`/`turn_complete` as aliases for the
persisted v1 names `task_started`/`task_complete`. P006 must accept the proven
names and fail explicitly on unknown lifecycle variants rather than silently
guessing.

### Weak or unavailable lifecycle markers

- There is no confirmed dedicated `session_started` or `session_ended` record
  in the persisted rollout contract.
- `ShutdownComplete` exists as an event variant, but it is process shutdown
  evidence, not proof that a logical session ended cleanly.
- File creation, modification time, close, or a watcher delete event is a
  heuristic only.
- Resume is represented by opening an existing rollout/history lineage; no
  dedicated persisted `session_resumed` event was confirmed.

P009 should therefore distinguish a completed turn from an ended session and
leave an interrupted/open session explicitly incomplete.

## Identifier audit

| Identifier | Scope/evidence | Resume behavior | Privacy decision |
| --- | --- | --- | --- |
| Session ID | `SessionMeta.session_id`; source says it equals the root thread ID | Intended to persist with the logical session | Candidate safe local correlation ID |
| Thread ID | `SessionMeta.id`, filename, `TokenUsageRecord.thread_id` | Stable for the logical thread; revert can keep it while changing rollout ID | Candidate safe local correlation ID |
| Rollout ID | Filename; differs from thread ID for revert files | Identifies an immutable physical rollout | Candidate safe local correlation ID |
| Turn/task ID | `TurnStartedEvent`, `TurnCompleteEvent`, `TokenUsageRecord.turn_id` | Stable for that task/turn | Candidate safe local correlation ID |
| Root turn ID | `TurnStartedEvent.root_turn_id`, `TokenUsageRecord.root_turn_id` | Used for parent/root attribution | Candidate safe local correlation ID |
| Response ID | `RawResponseCompletedEvent` and `TokenUsageRecord.response_id` | Provider response correlation | Present but unnecessary for the initial privacy-minimal store; do not persist by default |
| Trace/event ID | Optional trace IDs exist; no generic persisted event ID was confirmed | Unknown | Exclude unless a later adapter proves necessity |
| Creator/account IDs | `SessionMeta.creator_user_id` and `creator_account_id` can exist | Account-linked | Explicitly prohibited from Codex Meter persistence |

Random local IDs do not make a record content-free, so P006 should still keep
them in an allowlist and never store adjacent prompt/response payloads.

## File lifecycle, append, archive, and resume

### Confirmed

- Rollout writers use a background recorder and append `RolloutLine` records to
  JSONL. The recorder has explicit create and resume parameters.
- Each record has an optional monotonically assigned `ordinal` in current
  paginated persistence. The source also uses byte offsets in history positions
  and thread-history projection state.
- Active rollouts are discovered recursively below `sessions/YYYY/MM/DD`.
- The source defines a separate flat `archived_sessions` root, lists archived
  threads, resolves archived paths, and current upstream tests exercise moving
  rollout files between active and archived locations and reading/resuming them.
- Current upstream supports plain JSONL and compressed JSONL Zstandard files.
  A plain file takes precedence over its compressed sibling.
- Resume/reconstruction can use the latest `TokenUsageRecord` or the same
  record captured in a compaction checkpoint.

### Likely but not yet proven

- A file may be incrementally visible before a turn completes because the
  recorder persists events during execution. The writer architecture and local
  file growth support this, but a controlled write/tail experiment was not run.
- A watcher can receive duplicate notifications, especially around writes,
  compression, and rename operations. This is an OS/filesystem integration
  concern, not an upstream rollout semantic established in P005.

### Unknown

- Whether every CLI release uses exactly the same archive move timing.
- Whether a crash can leave a final partial UTF-8/JSON line in every writer path.
  Upstream tests explicitly exercise invalid UTF-8 tails and incomplete resume
  prefixes, so an incremental reader must tolerate and quarantine an incomplete
  trailing record; the normal-frequency guarantee is unknown.
- Whether all platforms expose identical rename/mtime notification sequences.

### Cursor recommendation for later work

P006/P007 should evaluate a cursor composed from:

```text
logical rollout identity + physical file identity + byte offset + ordinal
```

The final persisted cursor schema is deferred. Timestamp alone is insufficient,
and an event ID is unavailable. On truncation/replacement, the adapter should
detect that the stored byte offset is no longer valid and replay from a safe
boundary. For compressed files, a byte offset must be interpreted in the
decompressed logical stream or the source should be materialized/read through a
compression-aware path; a raw compressed-file offset is not interchangeable.

## Privacy suitability

The rollout stream mixes safe telemetry with high-risk content. It can contain
user messages, model output, reasoning, tool arguments/results, paths, Git
metadata, and workspace context alongside token and lifecycle records.

The following allowlist is suitable as a P006 starting boundary:

```text
rollout envelope: timestamp, ordinal, top-level type
session metadata: session_id, id, timestamp, cli_version, model_provider, source
turn context/settings: turn_id, root_turn_id, model, effort, service_tier
lifecycle: task_started/task_complete/turn_aborted fields and timestamps
token usage: the six TokenUsage numeric fields plus explicit missingness
correlation: local session/thread/rollout/turn/root-turn IDs
```

The following must be excluded unless a later explicit contract says otherwise:

```text
prompt/user message text
assistant/model response text
reasoning text or encrypted reasoning payloads
tool arguments and tool output
source code, paths, CWD, workspace roots, Git data
creator user/account identifiers
provider response IDs and trace IDs by default
credentials, cookies, OAuth/API material, and raw logs
```

This is a privacy boundary, not a statement that the upstream files are
privacy-safe by themselves.

## Rate-limit and quota-related evidence

`TokenCountEvent` contains an optional `rate_limits` structure. The current
source defines nested rate-limit windows, credits, plan type, limit identifiers,
and spend-control fields. Local `token_count` records structurally contained a
`rate_limits` object in the inspected files.

P010 now retains a narrow privacy-filtered subset and converts supported local
meter windows into v1 quota snapshots. It accepts only the main Codex bucket
(missing `limit_id` or case-insensitive `codex`), classifies windows by observed
duration, and does not retain credits or spend-control monetary values. P010
does not scrape a provider UI, authenticate, calculate quota deltas, or infer
reset/window continuity.

## Work/web surface limitation

The discovered local source is Codex CLI-local session telemetry. It can observe
CLI rollouts that write to the configured Codex home. No evidence shows that it
captures all ChatGPT Work/web activity, other clients, or concurrent account
usage. Therefore:

```text
local Codex session telemetry != complete account usage
```

Concurrent Work/web activity can contaminate any later quota reconciliation and
may lower Observation quality to `C`. This limitation must remain visible in
P010+ quota and estimator work.

## Likely, unknown, and deferred findings

### Likely but not yet proven

- JSONL file growth can be tailed for near-real-time ingestion while the CLI is
  running.
- A combined source adapter using rollout files plus optional SQLite index
  metadata will be more robust than using SQLite alone.
- The event discriminator names documented here will remain recognizable across
  nearby releases, but exact payload fields and enum variants are versioned
  implementation details.

### Unknown

- A provider-independent mathematical relationship between `input_tokens`,
  `cached_input_tokens`, `output_tokens`, `reasoning_output_tokens`, and
  `total_tokens`.
- Whether all providers include reasoning output inside output totals.
- Exact archive timing, cross-platform watcher behavior, and crash-tail
  guarantees.
- Whether `service_tier` maps to any product-facing speed label.
- Whether all local session sources expose identical metadata or only CLI
  sessions use the observed rollout format.

### Deferred

- Filesystem discovery and watching implementation.
- JSONL/JSONL.zst parser implementation.
- Token extraction, task segmentation, cursors, and SQLite storage in Codex
  Meter.
- Quota acquisition and reconciliation.
- Weighting coefficients and effective-usage formulas.

## Source evidence table

All upstream references below are relative to the pinned official repository at
`8f7a0f7a878199c6886600370e5be6bd37ca38a3`. Local observations are described
structurally and do not copy values.

| Finding | Evidence source | Revision/location | Confidence |
| --- | --- | --- | --- |
| Rollout root names and canonical parser | Official Codex source | `codex-rs/rollout/src/lib.rs`, `SESSIONS_SUBDIR`, `ARCHIVED_SESSIONS_SUBDIR`, `parse_rollout_line` | Confirmed |
| JSONL envelope and item discriminators | Official Codex source | `codex-rs/history/src/lib.rs`, `RolloutLine`; `codex-rs/history/src/rollout_payload.rs`, `RolloutItemWire` | Confirmed |
| Dated active layout and filename IDs | Official Codex source | `codex-rs/rollout/src/list.rs`, `traverse_directories_for_paths`; `rollout_file_name.rs`, `RolloutFileName` | Confirmed |
| Plain/compressed rollout handling | Official Codex source/tests | `codex-rs/rollout/src/compression.rs`; `compression_tests.rs` | Confirmed |
| Session metadata fields | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `SessionMeta` and `SessionMetaLine` | Confirmed |
| Token component fields | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `TokenUsage` | Confirmed |
| Per-response usage record | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `RawResponseCompletedEvent`, `TokenUsageRecord`; `codex-rs/core/src/session/mod.rs`, `record_observed_response_completed` | Confirmed |
| Turn/thread cumulative records | Official Codex source | `codex-rs/core/src/state/session.rs`, `record_token_usage` | Confirmed |
| Snapshot cumulative/last semantics | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `TokenUsageInfo::append_last_usage`; `codex-rs/core/src/context_manager/history.rs`, `update_token_info` | Confirmed |
| Context-window recomputation caveat | Official Codex source | `codex-rs/core/src/session/mod.rs`, `recompute_token_usage`; `TokenUsageInfo::fill_to_context_window` | Confirmed |
| Lifecycle event names and fields | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `EventMsg`, `TurnStartedEvent`, `TurnCompleteEvent`, `TokenCountEvent` | Confirmed |
| Model/reasoning/service-tier evidence | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `TurnContextItem`, `ThreadSettingsSnapshot`; `codex-rs/state/src/extract.rs` | Confirmed |
| Derived `threads.tokens_used` | Official Codex source | `codex-rs/state/src/extract.rs`, `apply_event_msg`; `codex-rs/state/src/sqlite.rs`, `STATE_DB` | Confirmed |
| Active/archive path lookup and resume | Official Codex source/tests | `codex-rs/rollout/src/list.rs`, archive lookup functions; `codex-rs/app-server/tests/suite/v2/thread_archive.rs`, `thread_resume.rs` | Confirmed |
| Cursor inputs | Official Codex source | `codex-rs/protocol/src/protocol.rs`, `HistoryPosition`; `codex-rs/state/thread_history_migrations/0001_thread_history.sql`; local `thread_history_1.sqlite` schema | Confirmed as candidate inputs, final contract deferred |
| Local CLI surface | Local read-only inspection | Installed `codex-cli 0.157.1`; configured Codex home file names, SQLite schema names, and redacted JSON structural keys | Confirmed for this environment only |

## Telemetry source suitability matrix

| Candidate source | Token coverage | Lifecycle | Config/model | Realtime | Restart/replay | Privacy risk | Stability/version risk | Cross-platform | Decision |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Active rollout JSONL | Exact per-response components plus cumulative snapshots | Strong task/item events; weak session end | Session, turn, model, reasoning, provider, CLI version | Good candidate; incremental append evidence | Strong; ordinals, timestamps, resume history | High unless allowlisted | Medium/high; internal format | Source intends platform support; path handling varies | **Primary candidate for P006–P009** |
| Archived rollout JSONL/JSONL.zst | Same records if retained | Same historical evidence | Same | Not realtime | Strong historical replay | High unless allowlisted | Medium/high | Requires compression/archive handling | Secondary historical source |
| `state_5.sqlite` thread metadata | Aggregate `tokens_used` only; no component breakdown | Timestamps/archive flags and indexed IDs | Model/reasoning/provider/version columns | Database updates may lag projection | Good index/repair support | Medium/high; copied paths/Git/account fields | High; versioned schema | Platform-dependent SQLite path | Optional index, not primary |
| `thread_history_1.sqlite` | Copied item JSON may include all source content | Projected turns and statuses | Derived context | Projection may lag rollout | Strong byte-offset/ordinal evidence | Very high because `item_json` is mixed content | High; versioned schema | Platform-dependent SQLite path | Cursor/reference evidence only |
| `logs_2.sqlite` | No confirmed token source | Operational process/thread logs | No reliable config contract | Operationally realtime | Log retention/reclamation uncertain | High; arbitrary log payloads | High | Platform-dependent SQLite path | Not suitable |

The primary choice is evidence-based and not an immutable product contract:
allowlisted rollout JSONL records contain the best combination of raw usage,
lifecycle, IDs, configuration, and replay evidence while avoiding dependency on
derived SQLite projections.

## Inputs for P006

P006 may safely assume only the following confirmed facts from this report:

1. A configurable Codex home abstraction exists; this environment exposes it as
   `CODEX_HOME`. Do not hard-code a personal absolute path or assume that the
   default home is universal.
2. Current upstream uses an active `sessions` rollout root organized by
   `YYYY/MM/DD` and a separate `archived_sessions` root. Discovery must support
   both roots and must not assume the archive root exists.
3. Current upstream persists rollout records as JSONL, with `.jsonl.zst`
   support in the current revision. Each logical record has a timestamp, an
   optional ordinal, a top-level discriminator, and a payload.
4. `session_meta`, `turn_context`, `token_usage_record`, and `event_msg` are
   confirmed relevant discriminators. Unknown discriminators must be rejected or
   quarantined explicitly, not silently interpreted.
5. `token_usage_record.payload.usage` is the per-response usage candidate;
   `turn_token_usage` and `thread_token_usage` are cumulative snapshots. The
   `event_msg/token_count` `info.total_token_usage` is cumulative and
   `info.last_token_usage` is the latest appended usage.
6. The confirmed numeric upstream token fields are `input_tokens`,
   `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`,
   `reasoning_output_tokens`, and `total_tokens`. Missing fields remain missing;
   P006 must not fabricate zeros or a raw total.
7. Session/thread/rollout/turn/root-turn IDs are available for local
   correlation. Creator/account IDs, prompts, responses, reasoning, tool data,
   paths, Git data, response IDs, and trace IDs are not part of the initial
   privacy-safe persistence allowlist.
8. `task_started` and `task_complete` are the current persisted lifecycle names,
   with `turn_started` and `turn_complete` accepted by the upstream enum as
   compatibility aliases. A completed turn is not proof of a completed session.
9. Model, provider, reasoning effort, service tier, and CLI version may be
   available, but service-tier-to-speed semantics and plan/quota semantics are
   unresolved.
10. File identity, byte offset, optional ordinal, and logical rollout identity
    are candidate cursor inputs. P006 must still define behavior for append,
    replay, replacement/truncation, partial trailing records, compression, and
    duplicate notifications.

P006 must not assume the raw-total component identity, complete account usage,
universal event stability, a dedicated session-end event, or a universal archive
and watcher lifecycle. Those remain unresolved and must be represented as
unknown or unsupported rather than inferred.
