# Prompt 016A Manifest

- Starting SHA: `fb02d91bef5c6f23ffeb051b01bd7e8180558dd1`.
- Verified workflow: fetched `origin/main`, checked out `main`, confirmed the
  P016 baseline, confirmed `main == origin/main`, and confirmed a clean
  worktree before editing.
- Findings corrected: variable-width RFC3339 TEXT ordering, over-constrained
  terminal `finalized_at`, missing exact duration validation, and composite
  checkpoint replay mutating or rejecting identical state.
- Migration: added version `4`, `0004_observation_time_keys`.
- Migration 0001 checksum: `1ffa336dcdc5abc63fdf74276c354c82a7b8f157af9412625723a7d8fe20c5aa`.
- Migration 0002 checksum: `ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199`.
- Migration 0003 checksum: `1ed907c9f124697b7f5620799672e49128dde7110860d513b0efaa9f57cd305c`.
- Migration 0004 checksum: `97aaebf9ca9c856b42cd88089ee17f84cbe3010ad4e37246ecd7118417d14f6c`.
- Time keys: SQL timing projections use exactly
  `YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ`; Rust parses RFC3339, requires UTC `Z`, and
  formats deterministic nine-digit nanoseconds.
- Backfill: migration 3→4 normalizes existing `started_at`, `ended_at`, and
  `finalized_at` projection columns without modifying canonical JSON or its
  payload checksum.
- Terminal timing: provisional lifecycle rows must omit `finalized_at`; terminal
  rows may omit it; storage never fabricates finalization time.
- Duration: complete task endpoints require exact floor elapsed milliseconds;
  missing endpoints require missing duration; safe-integer bounds remain active.
- Composite replay: exact checkpoint payload equality includes source, cursor,
  format, JSON, and checksum. Exact checkpoint replay returns the existing
  revision without requiring expected revision equality. Standalone P015
  checkpoint CAS remains strict.
- Tests: mixed-precision ordering, keyset pagination, exclusive time filters,
  migration backfill, terminal-without-finalized, provisional-finalized
  rejection, duration mismatch/missing-duration rejection, exact composite
  replay, one-sided composite progression, stale checkpoint rollback, UTC
  validation, and prior P008–P016 regressions.
- Contract status: public Observation schema remains v1 `1.0.0`; no
  `schemas/v1/*` files were changed.
- P017 remains deferred; no estimator, Python loader, capacity calculation,
  weighting, or analytics result persistence was added.
- Final SHA handling: final commit SHA, push result, equality check, and clean
  worktree result are recorded in the completion report after the single
  corrective commit.
