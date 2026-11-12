# P044 manifest: P044A in progress

P044 remains incomplete. P044A introduces a conservative coverage assessment;
P044B quota sampling, reconciliation integration, and its durable lifecycle are
not implemented.

## Pinned source evidence

The supported compatibility baseline is Codex CLI 0.157.1, audited against
upstream revision `8f7a0f7a878199c6886600370e5be6bd37ca38a3` (see
`V02_REAL_TELEMETRY_INGESTION.md`). At that revision:

- `codex-rs/protocol/src/protocol.rs`, `TokenUsageRecord`, explicitly calls
  the per-response usage best effort. It has response and turn identities and
  separate response, turn, and thread usage fields.
- `codex-rs/core/src/session/turn.rs`, `ResponseEvent::Completed`, passes
  optional usage to `record_observed_response_completed`.
- `codex-rs/core/src/session/mod.rs`, `record_observed_response_completed`,
  persists a `TokenUsageRecord` only when usage is present. A completed response
  without usage therefore need not yield such a record.
- `TurnCompleteEvent` in `protocol.rs` contains turn identity and optional
  timing, but no response count or token coverage watermark. Its terminal event
  alone does not establish exhaustive response usage coverage.
- `TokenCountEvent` carries optional aggregate usage and rate-limit snapshots;
  `send_token_count_event` reads the current session state. In `turn.rs`, a
  rate-limit update can cause this event to be emitted. This is not evidence of
  a qualified before/after task sampling protocol or of exclusive task usage.

These observations do not establish exhaustive stream completion, a last
canonical token event, generation continuity, task-level source closure, or
sampling stability. The audit has not established a task-associated baseline or
post-task quota sample. More source and recovery analysis is needed before any
new proof can be accepted. Existing P043 incomplete classification remains the
default; the presence of individual response usage does not change it.

## Baseline

P043 HEAD was `a2c5cab2521b9831c47cd4e104f9fc29ab1f6495`; its push and PR CI
and release-candidate validation passed. These runs do not validate P044.
P043 persists terminal observations without task-associated quota samples;
P044 must resolve lifecycle ordering and crash recovery without modifying those
historical terminal records.

## P044A implementation

`telemetry::coverage` separates reported source continuity from task coverage.
It assesses identity, boundary presence, lifecycle anomalies, mixed configuration,
and canonical event identity and duplicate conflicts. Its status distinguishes
unavailable, partial, continuous but unproven, and conflicting evidence. A
reserved verified-complete state has no production construction path: there is
no audited closure contract or synthetic proof interface yet. Even a constructed
assessment cannot promote P013 completeness. Continuity supplied by a caller is
not closure, and the recorder uses unknown continuity because a cursor alone
cannot prove the whole task history. The boundary passed at assembly is a
candidate committed atomically with the observation and checkpoint, not an
independently durable guarantee.

The recorder continues to select canonical per-response records from the current
source generation and delegates checked accounting and deduplication to P013.
Snapshots are excluded by that existing selection; raw totals and missingness
are unchanged. The assessment is transient and introduces no migration or
historical row rewrite. P043 checkpoint and terminal immutability rules remain
in force. Unit tests cover continuity without closure, missing boundary/start,
lifecycle conflicts, identity, duplicate replay and conflicting duplicate data.
Existing sync and storage regression tests exercise the recorder, checkpoint
replay, source replacement, incomplete tails and terminal immutability.

This is a conservative first increment, not a complete authoritative proof
framework. More tests and an audited, provenance-bound closure contract are
needed before verified completeness could be supported. P044B must implement
explicit task-associated quota samples, P012 qualification, bounded sampling,
and atomic durable pending reconciliation without upgrading terminal rows.
