# Prompt 007 Manifest

## Identification and baseline

- **Prompt:** P007 — Incremental Rollout JSONL Reader and Replay Cursors
- **Title:** Add incremental Codex rollout reader
- **Starting SHA:** `42b36a74a74d1c55a7f271c469fe537edd54dd2e`
- **Verified P006 baseline:** `origin/main` exactly matched the expected merged
  P006 baseline after fetching; the starting worktree was clean.
- **Feature branch:** `codex/p007-incremental-rollout-reader`

`main` was not modified. No reset or rewrite of legitimate work was used.

## Compatibility target

- Codex CLI: `0.157.1`.
- Official upstream `openai/codex` revision:
  `8f7a0f7a878199c6886600370e5be6bd37ca38a3`.
- P006 remains the complete-record decoder and compatibility boundary.

## Implementation

- Added `telemetry::incremental_jsonl`.
- Added synchronous `read_available<R: Read + Seek>` API.
- Added bounded opaque `SourceIdentity` and `RolloutCursor` types.
- Defined `committed_offset` as the first byte not committed as part of a
  complete newline-terminated line.
- Added candidate cursor semantics; the reader does not persist or mutate a
  hidden checkpoint.
- Added LF/CRLF handling, one-line-at-a-time processing, incomplete-tail
  preservation, and safe rejected-line outcomes.
- Added explicit rollout mismatch, source replacement, source truncation,
  ordinal regression, I/O, and offset-overflow errors.
- Added no new dependency; standard-library synchronous I/O is sufficient.

## Policies

- A non-newline EOF fragment is never sent to P006, even if currently valid
  JSON, and its start offset remains uncommitted.
- Complete malformed or empty lines are reported as rejected items and advance
  the candidate cursor.
- Older cursors intentionally replay records; advanced cursors with no new
  complete bytes return an empty batch.
- Missing ordinals remain missing; ordinal gaps are accepted; ordinal regression
  is an explicit discontinuity.
- Cursor/source identities never contain paths, usernames, CWD, repository
  data, prompts, responses, or payload content.
- Plain `.jsonl` semantics only; compressed rollout handling remains deferred.

## Synthetic stream fixtures

Added under `fixtures/codex-rollout/v0.157.1/streams/`:

- `multiple-complete.jsonl` — multiple valid records;
- `partial-tail.jsonl` — complete record followed by an unterminated tail;
- `partial-tail-complete.jsonl` — corresponding completed stream state;
- `crlf.jsonl` — CRLF records;
- `malformed-and-empty.jsonl` — malformed and empty complete lines between valid records;
- `ordinal-progression.jsonl` — monotonic ordinals with an intentional gap;
- `ordinal-regression.jsonl` — repeated ordinal discontinuity example.

All fixtures use synthetic IDs and values. Truncation and generation replacement
are constructed in-memory in tests because they are source mutations rather than
static stream formats.

## Tests

Coverage includes:

- multiple complete records and byte offsets;
- valid JSON partial-tail withholding and append completion;
- crash replay from an old cursor;
- empty reread after an advanced cursor;
- complete malformed lines followed by valid records;
- empty line rejection and offset advancement;
- CRLF decoding and two-byte offset advancement;
- ordinal progression, gaps, missing ordinals, and regression;
- source truncation without automatic rewind;
- source-generation replacement;
- explicit restart cursor construction;
- bounded identity validation and safe error representation;
- fixture-backed stream reads.

## Explicit deferrals

P007 does not implement Codex-home discovery, recursive session/archive
scanning, watchers, platform file IDs, `.jsonl.zst`, persistent cursor storage,
token deltas, normalized events, task/session assembly, SQLite, quotas,
reconciliation, estimation, or CLI telemetry UI.

## Validation

The final completion report records the full direct-equivalent project gate,
fixture validation, privacy audit, and Git checks after commit creation.

## Files

Created:

- `.gitattributes` — preserves the intentional CRLF stream fixture bytes.
- `docs/INCREMENTAL_ROLLOUT_READER.md`
- `docs/PROMPT007_MANIFEST.md`
- `fixtures/codex-rollout/v0.157.1/streams/*.jsonl`
- `rust/crates/codex-meter/src/telemetry/incremental_jsonl.rs`

Modified:

- `rust/crates/codex-meter/src/telemetry/mod.rs`

No P005/P006 evidence documents, normalized schemas, roadmap, or existing
contract fixtures were modified.

## Final commit handling

The required single logical commit message is:

```text
feat: add incremental Codex rollout reader
```

The final SHA, push result, and genuine PR URL/number are reported after commit
and remote operations. No commit SHA is embedded here before creation.
