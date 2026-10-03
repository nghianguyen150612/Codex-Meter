# Prompt 006 Manifest

## Identification and baseline

- **Prompt:** P006 — Codex Rollout Source Model and Synthetic Fixtures
- **Title:** Model Codex rollout telemetry records
- **Starting SHA:** `70e132054e2045a7f2f5ede23b512994b559960d`
- **Verified P005 baseline:** `HEAD` matched the expected merged P005 baseline;
  the starting worktree was clean.
- **Feature branch:** `codex/p006-rollout-source-model`

`main` was left unchanged. No reset or rewrite of legitimate work was used.

## Compatibility target

- Codex CLI: `0.157.1`.
- Official upstream `openai/codex` revision:
  `8f7a0f7a878199c6886600370e5be6bd37ca38a3`.
- The upstream checkout was temporary and outside this repository.

## Implementation

- Added the `telemetry::codex_rollout` source-format module.
- Added a one-complete-record `parse_rollout_record` API.
- Added privacy-filtered models for session metadata, turn context, token usage
  records, token-count snapshots, configuration, and lifecycle events.
- Added safe known-ignored and unknown record/event classifications.
- Added safe-integer-bounded token counters with negative/fractional/oversized
  value rejection.
- Added only the `serde_json` runtime dependency; no filesystem, async,
  compression, database, watcher, quota, or CLI dependencies were introduced.

## Supported source variants

- Top-level: `session_meta`, `turn_context`, `token_usage_record`, `event_msg`.
- Event messages: `token_count`, `task_started`/`turn_started`,
  `task_complete`/`turn_complete`, `turn_aborted`, `context_compacted`, and
  `thread_settings_applied`.
- Known content-heavy top-level records are classified as ignored; unknown
  future records and event variants retain only bounded discriminators.

## Fixtures and tests

Synthetic fixtures were added under `fixtures/codex-rollout/v0.157.1/` for:

- session metadata and turn configuration;
- token usage with distinct per-response, turn-cumulative, and
  thread-cumulative values;
- token-count cumulative/latest snapshots;
- missing versus zero reasoning counters;
- task/turn start and completion, abort, compaction, and thread settings;
- ignored content-heavy, unknown future, and unknown event records.

Rust tests cover supported decoding, optional ordinals, semantic separation,
missing-vs-zero preservation, lifecycle aliases, privacy filtering, unknown
handling, structural errors, payload-safe error display, and numeric safety.

## Privacy decisions

No real rollout records, prompts, responses, reasoning, source code, paths,
working directories, Git metadata, account identifiers, credentials, or rate-
limit payloads were copied into the repository. The source model has no generic
raw payload field and ignores response IDs, arbitrary errors, and rate limits.

## Explicit deferrals and limitations

P006 does not implement filesystem/session discovery, JSONL iteration, partial
line buffering, file watching, Zstandard decoding, cursors/checkpoints,
token-delta calculation, lifecycle state machines, SQLite, quota/rate-limit
interpretation, reconciliation, estimation, normalization, or telemetry CLI
commands. Future Codex-version compatibility remains unproven beyond the pinned
target.

## Validation

- `cargo fmt --manifest-path rust/Cargo.toml --all`
- `cargo check --manifest-path rust/Cargo.toml --workspace`
- `cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --all-features -- -D warnings`
- `cargo test --manifest-path rust/Cargo.toml --workspace --all-targets`
- Synthetic fixture JSON validation and privacy review
- Full project quality gates and final Git checks are recorded in the completion
  report after implementation is committed.

## Files

Created:

- `docs/CODEX_ROLLOUT_SOURCE_MODEL.md`
- `docs/PROMPT006_MANIFEST.md`
- `fixtures/codex-rollout/v0.157.1/*.json`
- `rust/crates/codex-meter/src/telemetry/mod.rs`
- `rust/crates/codex-meter/src/telemetry/codex_rollout.rs`

Modified:

- `rust/crates/codex-meter/Cargo.toml`
- `rust/Cargo.lock`
- `rust/crates/codex-meter/src/lib.rs`

No P005 authoritative documentation, normalized schema, roadmap, or prior
manifest was modified.

## Final commit handling

The required single logical commit message is:

```text
feat: model Codex rollout telemetry records
```

The final SHA, push result, and genuine PR URL/number are reported after the
commit and remote operations. No commit SHA is embedded here before creation.
