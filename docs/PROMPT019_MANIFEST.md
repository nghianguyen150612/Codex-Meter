# Prompt 019 Manifest

- Starting SHA: `b9f59927cbafd32c32d6b89b7a20f664701ca88e`.
- Verified P018 baseline: fetched `origin/main`, checked out `main`, confirmed
  `main == origin/main`, confirmed the expected P018 SHA, and confirmed a
  clean worktree before editing.
- Workflow: direct-to-main only; no feature branch and no pull request.
- Estimator method: `ESTIMATOR_METHOD_VERSION = "1.0.0"`.
- Quality policy: immutable integer weights A=10, B=8, C=3; D/X have no
  weights because P018 excludes them. Weights influence only weighted-median
  ordering and never alter candidate token values.
- Median algorithm: deterministic sorted ordinary median; even counts use the
  arithmetic mean of the two middle values.
- Weighted-median algorithm: sort by `(full_capacity_raw_tokens,
  observation_id)`, accumulate integer weights, compare
  `2 * cumulative_weight` with total weight, and average adjacent values at an
  exact half boundary.
- MAD algorithm: unweighted median absolute deviation over pre-filter eligible
  candidates.
- Outlier threshold: modified Z constant `0.6744897501960817`; reject only
  when modified Z is strictly greater than `3.5`; no rejection for fewer than
  three candidates.
- Zero-MAD policy: retain values equal to the pre-filter median and reject
  non-equal values when at least three candidates exist.
- Percentile algorithm: unweighted Type-7 linear interpolation for post-filter
  P25, P50, and P75.
- One-observation policy: zero or one candidate returns typed
  `INSUFFICIENT_SAMPLES` with no capacity summary statistics.
- Accounting semantics: P018 eligibility exclusions remain exclusions; P019
  updates only `outliers_removed` and `used_observations` after deterministic
  valid-candidate filtering.
- Module/API: `codex_meter.estimation.robust` provides immutable result/error
  types and `aggregate_capacity_candidates`; stable exports are provided by
  `codex_meter.estimation`.
- Tests: focused coverage includes method/version policy, ordinary and
  weighted medians, exact boundaries, duplicate values, MAD, zero-MAD behavior,
  modified-Z threshold, two-sample behavior, one/zero samples, Type-7
  percentiles, accounting, identity/configuration isolation, replay, and
  candidate-set invariant failures.
- Dependencies: runtime dependencies remain `[]`; implementation uses only
  the Python standard library and existing P017/P018 models.
- Privacy: aggregation consumes normalized candidate models and preserves only
  observation IDs needed for diagnostics; it does not access SQLite or add
  sensitive payloads.
- P020 deferrals: no final capacity selection, confidence interval, confidence
  classification, used/remaining estimates, current quota reading,
  quota-change detection, or `AnalyticsResult` serialization.
- Final SHA handling: the final commit SHA, push result, equality with
  `origin/main`, and clean-worktree status are recorded in the completion
  report after the single logical P019 commit is created; this manifest does
  not self-embed its own commit hash.
- Unrelated-work confirmation: no legitimate newer work was present; P017
  read-only behavior, P018 eligibility semantics, schemas, Rust code, and
  existing tests are preserved. No SQLite write path, final confidence logic,
  result serialization, quota-change analysis, or used/remaining calculation
  was added.
