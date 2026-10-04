# Prompt 017 Manifest

- Starting SHA: `4b8a6001acea73aa5564926cc7d2616c73a22f9d`.
- Verified P016A baseline: fetched `origin/main`, checked out `main`, confirmed
  `main == origin/main`, confirmed the P016A migration-v4 baseline, and
  confirmed a clean worktree before editing.
- Workflow: direct-to-main only; no feature branch and no pull request.
- Python structure: `codex_meter.history.models` contains immutable typed
  records and enums; `codex_meter.history.sqlite` owns the read-only SQLite
  connection, compatibility checks, validation, and dataset selection.
- Runtime dependencies: no changes; production dependencies remain empty and
  the standard library supplies `sqlite3`, `json`, `hashlib`, `pathlib`,
  `dataclasses`, and `enum`.
- SQLite access: resolved `Path.as_uri()` plus `mode=ro` and `uri=True`; no
  `immutable=1`; five-second finite busy timeout; verified `PRAGMA query_only`
  result `1`.
- Compatibility: required migration history versions 1 through 4 are
  contiguous and checked against the four P016A names and SHA-256 constants;
  older, newer, missing, gapped, or drifted histories fail closed.
  Constants are `0001`=`1ffa336dcdc5abc63fdf74276c354c82a7b8f157af9412625723a7d8fe20c5aa`,
  `0002`=`ae4a25ab39f865e7d1d5f02c9d03cc95a2786b72a48733025db1a0abe01d7199`,
  `0003`=`1ed907c9f124697b7f5620799672e49128dde7110860d513b0efaa9f57cd305c`,
  and `0004`=`97aaebf9ca9c856b42cd88089ee17f84cbe3010ad4e37246ecd7118417d14f6c`.
- Dataset model: terminal typed observations only, exact immutable five-field
  configuration keys, independent five-hour and weekly evidence, and explicit
  `None` handling for unavailable values.
- Corruption checks: checksum, JSON shape, identity, lifecycle, timing,
  configuration, token, quota, reset, validity/quality, summary quality, and
  SQL projection checks.
- Timestamp normalization: strict UTC-only RFC3339 input is normalized to the
  nine-digit P016A canonical key without losing nanosecond digits.
- Selection: inclusive observed-time bounds use `finalized_at → ended_at →
  started_at`; configuration filters are exact and SQL-parameterized; rows are
  chronological with Observation-ID tie-breaking.
- Bounded loading: batched materialization by default; safety ceiling 100,000;
  recent N selected by descending observed time/ID and returned ascending.
- Snapshot: each load uses one deferred read transaction and materializes its
  dataset before connection close, allowing safe WAL-reader coexistence.
- Tests: 19 Python tests cover read-only/query-only behavior, missing files,
  migration compatibility/drift, checksums, projection/JSON corruption,
  null-vs-zero, exact configuration, mixed-precision chronology, recent-N,
  terminal filtering, inclusive ranges, and timestamp parsing.
- Privacy: exception messages omit payloads, prompts, responses, credentials,
  and row contents; no analytics result persistence or Python migration path.
- P018 deferrals: no capacity candidates, zero-delta estimator semantics,
  eligibility/exclusion policy, weights, robust statistics, outliers,
  percentiles, quota-change detection, result serialization, or Rust-to-Python
  invocation.
- Final SHA handling: final commit SHA, push result, equality with
  `origin/main`, and clean-worktree status are recorded in the completion
  report after the single logical P017 commit is created; the manifest does not
  self-embed its own commit hash.
- Unrelated-work confirmation: no legitimate newer work was present; Rust
  storage, migrations, schemas, and existing P008–P016A behavior were
  preserved.
