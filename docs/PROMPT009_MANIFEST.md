# Prompt 009 Manifest

## Identification and baseline

- **Prompt:** P009 — Ordered Session, Task, and Configuration Assembly
- **Title:** Assemble Codex session and task telemetry
- **Starting SHA:** `3eefe3820eee66bdc2d93f7b20bc0273cb5b0d49`
- **Verified P008 baseline:** `origin/main` matched the expected P008 merge baseline before editing.
- **Feature branch:** `codex/p009-session-task-assembly`
- **Compatibility:** Codex CLI `0.157.1`, upstream revision `8f7a0f7a878199c6886600370e5be6bd37ca38a3`.

## Modules and identity

- Added `telemetry::assembly` for transactional ordered interpretation.
- Added `telemetry::identity` and moved P008 SHA-256 identity derivation into shared helpers.
- Extended `telemetry::normalized` with typed session-detected and configuration-evidence serializers.
- Preserved P008 `src:`, `evt:`, `cursor:`, `session:`, and `task:` domains and fixed them with a hard-coded regression test.
- No new dependency was added; the existing `serde`, `serde_json`, and `sha2` dependencies remain sufficient.

## State and assembly policy

- `TelemetryState` contains safe session context, thread/default configuration, and a deterministic task map.
- Task lifecycle is internal typed evidence: observed, active, completed, failed, aborted, or conflicting terminal evidence.
- Starts, terminal evidence, anomalies, attribution wrappers, and snapshot evidence are not v1 task events.
- Configuration precedence is task-specific turn context, then thread settings, then session metadata.
- Configuration updates are prospective and never look ahead or rewrite earlier token attribution.
- Mixed configuration is based only on fingerprints attached to canonical per-response token events.
- `plan` and `speed_mode` remain unavailable; service tier remains internal and is not mapped to speed.

## Normalized events

- `session_meta` emits one deterministic `session_detected` event for a logical session.
- Ordered session metadata, thread settings, and turn context can emit `configuration_evidence_observed`.
- No `session_started`, `session_ended`, or task lifecycle normalized events are emitted.
- P008 remains the only token normalization path; cumulative snapshots never create consumption.

## Fixtures and tests

- Added session/task and mixed-configuration synthetic JSONL streams.
- Added Codex-specific session-detected and configuration-event contract fixtures.
- Updated the strict contract fixture inventory; expected inventory is 11 positive and 5 negative fixtures.
- Rust tests cover normal flow, replay, rollback, no-lookahead configuration, mixed configuration, snapshot no-double-counting, completion without start, and stable P008 identities.
- Current Rust test count is 47 library tests, plus the zero-test binary target, before the final full gate.

## Privacy and deferrals

State and outputs retain no raw JSON, source paths, prompts, responses, account IDs, credentials, or provider response IDs. P009 does not implement quota interpretation, persistence, storage, discovery, watchers, compression, reconciliation, observation finalization, estimation, benchmarks, or CLI UI.

## Validation

The final validation record will include Cargo formatting, Clippy, Rust build/tests, Ruff, Pytest, Draft 2020-12 contract validation, fixture/privacy review, and `git diff --check`.

## Final SHA handling

The required single logical commit is:

```text
feat: assemble Codex session and task telemetry
```

The final SHA, push result, and PR number/URL are recorded after validation and remote operations. `main` is not modified or merged by this work.

## Unrelated-work confirmation

No unrelated product, schema, persistence, quota, or runtime work is included.
