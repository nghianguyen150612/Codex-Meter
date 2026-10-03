# Prompt 008 Manifest

## Identification and baseline

- **Prompt:** P008 — Raw Token Extraction, Normalization, and Double-Count Prevention
- **Title:** Normalize Codex raw token telemetry
- **Starting SHA:** `a2ef3903e5565b1362cca7c104ba268997261332`
- **Verified P007 baseline:** `origin/main` exactly matched the expected merged
  P007 baseline after fetching; the starting worktree was clean.
- **Feature branch:** `codex/p008-token-normalization`

`main` was not modified. No reset or rewrite of legitimate work was used.

## Compatibility target

- Codex CLI: `0.157.1`.
- Official upstream `openai/codex` revision:
  `8f7a0f7a878199c6886600370e5be6bd37ca38a3`.
- P006 remains the source decoder and P007 remains the incremental source-position boundary.

## Dependencies

- Added direct `serde` dependency with derive support for typed normalized
  serialization.
- Added direct `sha2` dependency for stable SHA-256 opaque IDs.
- Existing direct `serde_json` dependency remains in use for source decoding and
  tests.
- No async runtime, watcher, compression, database, network, quota, or CLI
  dependency was added.

## Modules and models

- Added `telemetry::normalized` with typed v1 normalized token event structures,
  token metrics, availability, provenance, and payload types.
- Added `telemetry::token_normalization` with raw evidence semantics, extraction,
  canonical per-response normalization, deterministic ID derivation, and
  timestamp validation.
- Updated `telemetry::mod` exports.

## Token policy

- Canonical consumption source: `token_usage_record.payload.usage`.
- `turn_token_usage`, `thread_token_usage`, `total_token_usage`, and
  `last_token_usage` remain named snapshot evidence only.
- No cumulative subtraction, snapshot summing, or latest-snapshot second event.
- `input_tokens` remains raw evidence and does not become `uncached_input`.
- `cache_write_input_tokens` remains raw evidence and does not alter normalized
  metrics.
- `cached_input`, `output`, `reasoning_output`, and `raw_total` copy present
  source values with `observed` provenance.
- Missing values remain unavailable; upstream `total_tokens` is preserved and
  never recomputed.

## Deterministic identities

SHA-256 domain-separated, length-delimited digests derive:

- `source_instance_id` from rollout ID plus source generation;
- `event_id` from source identity, start/end offsets, optional ordinal, and
  normalization domain/version;
- `safe_cursor_id` from source identity plus end offset;
- normalized session/task IDs from upstream session/turn IDs.

The strategy is replay-stable, source-generation-aware, privacy-safe, and does
not use random IDs or a process-local deduplication set.

## Fixtures and contract inventory

- Added `fixtures/contracts/v1/normalized-token-event-codex-rollout.json`.
- Updated `scripts/validate_contracts.py` with explicit fixture inventory.
- The contract validator now reports 9 positive and 5 negative fixtures.
- The existing generic `normalized-token-event.json` fixture remains unchanged.

## Tests

Rust tests cover:

- semantic extraction of per-response and cumulative evidence;
- per-response-only normalized event creation;
- token-count snapshot no-double-count behavior;
- cumulative values not replacing canonical usage;
- missing versus explicit-zero reasoning output;
- unavailable uncached input;
- cache-write preservation;
- observed raw-total preservation and missing-total behavior;
- deterministic replay, position, generation, session, and task IDs;
- non-token and rejected source items producing no zero-token event;
- invalid timestamp rejection without source-value leakage;
- serialized event payload compatibility with the Codex-specific fixture.

## Privacy and explicit deferrals

No real telemetry, prompts, responses, reasoning text, source code, paths,
account identifiers, credentials, response IDs, rate-limit payloads, or raw
source lines were added. P008 does not implement task/session lifecycle
assembly, configuration timelines, discovery, watchers, compression, SQLite,
persistent deduplication, quotas, reconciliation, observations, estimation,
benchmarks, or CLI UI.

## Validation

The completion report records Cargo formatting, Clippy, Rust tests/build,
Ruff, Pytest, contract validation, fixture checks, privacy review, and Git
checks after commit creation.

## Files

Created:

- `docs/TOKEN_NORMALIZATION.md`
- `docs/PROMPT008_MANIFEST.md`
- `fixtures/contracts/v1/normalized-token-event-codex-rollout.json`
- `rust/crates/codex-meter/src/telemetry/normalized.rs`
- `rust/crates/codex-meter/src/telemetry/token_normalization.rs`

Modified:

- `rust/Cargo.lock`
- `rust/crates/codex-meter/Cargo.toml`
- `rust/crates/codex-meter/src/telemetry/mod.rs`
- `scripts/validate_contracts.py`

No existing normalized schema, generic contract fixture, P005/P007 evidence
document, roadmap, or lifecycle implementation was modified.

## Final commit handling

The required single logical commit message is:

```text
feat: normalize Codex raw token telemetry
```

The final SHA, push result, and genuine PR URL/number are reported after commit
and remote operations. No commit SHA is embedded here before creation.
