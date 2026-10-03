# Incremental Codex Rollout Reader

## Scope

P007 adds the incremental boundary between a caller-supplied plain Codex
rollout JSONL source and the P006 one-record source decoder:

```text
plain rollout JSONL source
        ↓
incremental byte reader
        ↓
complete newline-delimited record
        ↓
P006 parse_rollout_record(...)
        ↓
privacy-filtered RolloutRecord
```

The reader accepts an already-open `Read + Seek` source. It does not discover
Codex files, inspect `CODEX_HOME`, watch directories, decompress `.jsonl.zst`,
normalize token usage, or persist cursors.

The compatibility target remains Codex CLI `0.157.1` and upstream revision
`8f7a0f7a878199c6886600370e5be6bd37ca38a3` from P005/P006.

## API and source identity

`read_available(reader, source, cursor)` reads one currently available source
generation and returns a `ReadBatch` containing safe `ReadItem` outcomes, a
candidate `next_cursor`, and an `has_incomplete_tail` flag.

The caller supplies `SourceIdentity`:

- `rollout_id`: opaque logical rollout identity;
- `source_generation`: opaque physical-generation identity.

Both values are bounded, non-empty, and control-character checked. P007 does
not derive them from paths, usernames, inodes, mtimes, or platform APIs.

The reader supports plain newline-delimited JSONL bytes only. Compressed
physical offsets are not interchangeable with offsets in a decompressed logical
stream and are outside this module.

## Cursor invariant

`RolloutCursor` contains:

- the opaque logical rollout ID;
- the opaque source-generation ID;
- `committed_offset`;
- `last_ordinal: Option<u64>`.

The authoritative offset invariant is:

```text
committed_offset = first byte not committed as part of a complete
                   newline-terminated record
```

After `record\n` or `record\r\n`, the candidate offset points immediately after
both newline bytes. A cursor created with `RolloutCursor::at_start(source)` has
offset zero and no ordinal.

P007 does not mutate a durable checkpoint. The returned cursor is only a
candidate. The caller processes the batch, then commits the candidate by
retaining it. If processing fails or the process crashes before retention, the
old cursor intentionally replays the same complete records.

## Line boundaries

The reader processes one line at a time and never loads the complete source into
memory. LF and CRLF are supported. For CRLF, the `\r` is removed before the
line is passed to P006, while both bytes count toward the next cursor offset.

An EOF fragment without `\n` is always an incomplete trailing record, even if
its current bytes form valid JSON. It is not passed to P006 and its starting
offset remains uncommitted. Once bytes are appended, a later call from that
cursor rereads and decodes the completed line exactly once.

An empty complete line is a `RejectedLineReason::EmptyLine`. It is reported as
a complete rejected item and its newline is committed, so it cannot block later
records.

## Complete-line outcomes

Each complete line becomes one `ReadItem` with safe start/end offsets and an
optional decoded ordinal:

- `ReadItemOutcome::Decoded(RolloutRecord)` for a P006-decoded record;
- `ReadItemOutcome::Rejected(RejectedLine)` for an empty line, invalid UTF-8,
  or a complete line rejected by P006.

Complete malformed JSON is therefore distinct from an incomplete tail. The
reader reports the structural P006 error and advances the candidate cursor past
the malformed line, allowing later valid lines to be inspected.

No raw line bytes are retained in a batch, rejection, cursor, or error. Errors
identify only structural conditions, safe offsets/ordinals, bounded opaque IDs,
and I/O error kinds.

## Replay and duplicate reads

Calling the reader with an older cursor intentionally replays records from that
byte position. This is the crash-recovery behavior; downstream storage must
provide idempotency when it eventually exists.

Calling it again with an already advanced cursor and no new complete bytes
returns an empty batch with the same cursor. Duplicate future watcher
notifications therefore do not themselves create duplicate decoded records.

## Ordinals

Decoded P006 records expose an optional ordinal, and P007 carries the greatest
accepted ordinal in `last_ordinal`. Missing ordinals remain missing and do not
become zero. Gaps are accepted as evidence only; P007 does not infer lost
records or require contiguous numbering.

Within the same logical rollout and source generation, an observed ordinal less
than or equal to `last_ordinal` is an `OrdinalRegression` discontinuity. The
reader returns an error rather than silently interpreting that record as new.
The caller can replay from its prior cursor or choose another explicit recovery
strategy.

## Truncation and replacement

Before seeking, the reader checks the current source length. If it is below the
cursor's committed offset, it returns `SourceTruncated` and never resets to
zero automatically.

If the caller supplies a different logical rollout ID, the reader returns a
rollout identity mismatch. If the logical ID is the same but the generation
changes, it returns `SourceReplaced`. No record is interpreted under the old
cursor in either case.

To intentionally restart a new generation, the caller constructs a new
`SourceIdentity` and uses `RolloutCursor::at_start(&new_source)`, which clears
the offset and ordinal explicitly.

A same-length physical replacement presented with the same generation cannot be
reliably detected from byte offsets alone. The future discovery/platform layer
must provide a new generation identity whenever physical identity changes;
mtime alone is not treated as a sufficient guarantee here.

## Current limitations

- No filesystem or Codex-home discovery;
- no recursive active/archive scanning;
- no watcher integration;
- no platform file-ID acquisition;
- no `.jsonl.zst` support;
- no persisted cursor/checkpoint store;
- no token delta calculation or cumulative snapshot reconciliation;
- no normalized events, task/session assembly, SQLite, quota, or estimator;
- no broad compatibility claim beyond the pinned P006 target.

## Inputs for P008

P008 may assume:

- P006 safely decodes one complete rollout record;
- P007 incrementally produces complete source records from a supplied plain
  JSONL source;
- partial trailing records are never prematurely parsed or committed;
- LF and CRLF offsets are deterministic;
- malformed complete lines are safely rejected while later records continue;
- replay from an older cursor is intentional and deterministic;
- source truncation and replacement are explicit conditions;
- optional ordinal evidence is preserved without fabricated values;
- cumulative and per-response source semantics remain distinct;
- no raw line content survives the ingestion boundary.

P008 owns extraction of raw token evidence, mapping to Codex Meter token
concepts, event-local accounting, cumulative-snapshot handling, avoiding
double-counting, and normalized telemetry events.
