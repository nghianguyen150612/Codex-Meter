# Prompt 018 Manifest

- Starting SHA: `4fa5d6ea8e55cfe0882765358d392602d9d377ef`.
- Verified P017 baseline: fetched `origin/main`, checked out `main`, confirmed
  `main == origin/main`, confirmed the expected P017 SHA, and confirmed a clean
  worktree before editing.
- Workflow: direct-to-main only; no feature branch and no pull request.
- Module structure: `codex_meter.estimation.candidates` contains pure candidate
  models, typed errors, deterministic eligibility, configuration guards,
  per-meter derivation, and accounting; `codex_meter.estimation` exports the
  stable public API.
- Candidate formula: `raw_total / delta_percentage_points` per percentage
  point and `raw_total * 100 / delta_percentage_points` for a hypothetical
  full window; deltas are percentage points, not percent growth.
- Eligibility: token and target meter must be valid; raw total must be present
  and positive; target delta must be present and positive; reset crossings are
  excluded; A/B/C are eligible and D/X are excluded.
- Quality policy: candidate quality is the worse of token and target-meter
  quality; no numeric weights are defined.
- Zero policies: zero raw total emits `ZERO_RAW_TOTAL`; zero quota delta emits
  `ZERO_QUOTA_DELTA`; neither is substituted, clamped, or divided.
- Reset policy: `RESET_CROSSED` is retained independently and the reset branch
  cannot produce a candidate.
- Configuration guard: selected configurations must match every row; unselected
  non-empty datasets must be homogeneous; mixed configurations raise
  `MixedConfigurationDatasetError`; empty selected datasets preserve selection.
- Exclusion enum: `TOKEN_NOT_VALID`, `TOKEN_QUALITY_INELIGIBLE`,
  `RAW_TOTAL_UNAVAILABLE`, `ZERO_RAW_TOTAL`, `METER_NOT_VALID`,
  `METER_QUALITY_INELIGIBLE`, `DELTA_UNAVAILABLE`, `ZERO_QUOTA_DELTA`,
  `RESET_CROSSED`, and `NONFINITE_CANDIDATE`.
- Sample accounting: all supplied terminal Observation rows are counted per
  meter; valid rows produce candidates; excluded rows produce decisions with
  reasons; `outliers_removed = 0`; `used_observations = valid_observations`.
- Numerical representation: raw totals remain `int`; derived values are
  unrounded finite Python `float` values; no numerical runtime dependency was
  added.
- Tests: focused P018 coverage includes formulas, independent meters, summary
  quality independence, A/B/C/D/X policy, all key exclusions, reset and zero
  semantics, tiny deltas, configuration guards, empty datasets, accounting,
  ordering, replay, and lifecycle behavior.
- Dependencies: runtime dependencies remain `[]`; only standard-library
  dataclasses, enum, math, and existing history models are used.
- Privacy: the module consumes only normalized P017 records and may retain
  observation IDs in diagnostics; it adds no conversational or credential data.
- P019 deferrals: no median, weighted median, quality weights, MAD, outliers,
  percentiles, confidence interval, used/remaining estimates, or
  AnalyticsResult serialization.
- Full validation: Python lint/format/tests, Rust format/clippy/tests/build,
  contract validation, and `git diff --check` are run before completion.
- Final SHA handling: the final commit SHA, push result, equality with
  `origin/main`, and clean-worktree status are recorded in the completion
  report after the single logical P018 commit is created; this manifest does
  not self-embed its own commit hash.
- Unrelated-work confirmation: no legitimate newer work was present; P017
  read-only behavior, schemas, Rust storage/migrations, and existing tests are
  preserved. No SQLite write path, capacity aggregation, quality weight,
  outlier filter, or AnalyticsResult serialization was added.
