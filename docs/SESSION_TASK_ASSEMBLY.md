# Ordered Session, Task, and Configuration Assembly

P009 is the ordered interpretation boundary above the privacy-filtered P006 source model, the replay-safe P007 reader, and the P008 token normalizer. It consumes complete `ReadBatch` items in source order and returns outputs plus a candidate `TelemetryState`; it does not persist either result.

## Ordered architecture

`assemble_batch(current_state, source, batch)` clones the supplied state, processes every item in order, and returns `AssembledBatch { outputs, next_state }`. A fatal error discards the candidate, leaving the caller's state unchanged. `BTreeMap` and `BTreeSet` keep replay results deterministic.

Rejected P007 lines are safe diagnostics only. Unsupported P006 records leave state unchanged. P009 never re-parses source JSON and always routes token records through `normalize_token_item`.

## Session detection

`session_meta` is the only session-detection evidence used here. It establishes a hashed session ID, safe thread identity, CLI version, and optional model-provider evidence, and emits one `session_detected` event with a lifecycle payload. Repeated metadata for the same logical session does not emit another detection event. A conflicting session identity is a structural error.

P009 deliberately does not infer `session_started` or `session_ended` from the first task, final task, EOF, close, shutdown, or inactivity. Process lifetime is not logical session lifetime.

## Task lifecycle

Task lifecycle is an internal typed domain: `Observed`, `Active`, `Completed`, `Failed`, `Aborted`, and `ConflictingTerminalEvidence`. Starts, completions, failures, and aborts produce deterministic internal lifecycle evidence, not v1 normalized task events. Completion or abort without a start is retained with an anomaly and does not fabricate a start. Contradictory terminal evidence is retained and marked conflicting. Token evidence remains valid after terminal evidence and is marked `TokenAfterTerminal`.

Root-task evidence may arrive after a task is first observed. Later confirmed root evidence backfills unknown lineage; repeated agreeing evidence is idempotent. Distinct normalized roots are all retained in the task's root-evidence set and mark `ConflictingRootTaskEvidence` rather than silently overwriting an earlier root.

## Configuration timeline

Thread settings update ordered defaults for model, provider, reasoning effort, and service tier. Turn context updates only the associated task's model and reasoning override. Precedence is task-specific turn context, then thread defaults, then session metadata. Updates are prospective; no later record is applied retroactively.

The normalized configuration identity always reports `plan` and `speed_mode` as unavailable. `service_tier` remains internal and is never mapped to speed. Model, reasoning level, and Codex version are observed only when ordered source evidence establishes them. Configuration events contain the effective known state after the current record is applied.

## Token attribution

P008 remains the sole token normalization path. Only per-response `usage` creates an attributed token event. Turn/thread cumulative usage and token-count snapshots are preserved as snapshot evidence and never create consumption. P009 attaches the effective configuration, a deterministic configuration fingerprint, and lifecycle status at the token's source position; it does not aggregate token totals.

A task is `NoTokenConsumption`, `Consistent`, or `Mixed` based only on fingerprints attached to canonical per-response token events. Settings changes without token events do not make a task mixed.

## Deterministic IDs

Session, token, cursor, task, lifecycle, configuration, snapshot, and configuration-fingerprint IDs use SHA-256 with domain separation. Existing P008 `src:`, `evt:`, `cursor:`, `session:`, and `task:` domains are unchanged. Source paths, raw payloads, prompts, response IDs, account identifiers, and credentials are not retained in state or normalized output.

## Limitations

P009 does not infer plan or quota, map service tier to speed, assemble session end state, persist state, deduplicate in storage, discover files, watch files, read compressed rollouts, reconcile observations, estimate capacity, or expose task lifecycle through the v1 normalized-event schema.

## Inputs for P010

P010 may assume Phase B provides safe source decoding, replay-safe incremental records, canonical event-local token evidence, deterministic token events, session detection, task terminal evidence, ordered configuration, token attribution, mixed-configuration classification, and explicit incomplete/conflicting lifecycle evidence. P010 begins quota acquisition and must not reinterpret cumulative token snapshots as quota.
