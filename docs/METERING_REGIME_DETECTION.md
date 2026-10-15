# Historical metering-regime detection

P023 adds a deterministic, internal analysis for finding **empirical shift
signals** in the relationship between raw-token totals and observed quota-meter
percentage deltas. It does not establish an official capacity or identify a
provider policy change.

The product invariant remains:

```text
raw tokens != weighted/effective usage != plan quota
```

The output describes estimated raw-token capacity derived from observations. It
does not describe quota percentage-point consumption as tokens, and it does not
verify an undocumented OpenAI quota-policy change.

## Method and API

The public pure API is:

```python
detect_metering_regime_signals(
    *,
    dataset: ObservationDataset,
    target_meters=(QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
    policy: RegimeDetectionPolicy = RegimeDetectionPolicy(),
) -> MeteringRegimeAnalysis
```

`REGIME_SIGNAL_METHOD_VERSION` is `1.0.0`. It versions cohort formation,
sample and confidence requirements, effect threshold, IQR separation, and
classification rules. It is independent of `ESTIMATOR_METHOD_VERSION` and
`WORKLOAD_METHOD_VERSION`, which remain `1.0.0`.

The default policy is:

| Setting | Default | Meaning |
| --- | ---: | --- |
| `cohort_observations` | 20 | Anchored terminal rows in a complete cohort |
| `minimum_used_observations` | 8 | P019 retained candidate rows required in each cohort |
| `minimum_relative_change` | 0.25 | Inclusive 25% material-change threshold |
| `maximum_input_observations` | 500 | Maximum input rows accepted by the detector |

The maximum input bound cannot exceed P017's 100,000-row reader safety limit.
Policy values are fixed for a call; the detector does not tune thresholds from
the observed data. These are Codex Meter method choices, not universal
statistical significance guarantees.

## Validation, configuration, and chronology

P023 reuses P021's strict dataset checks and chronological normalization. Input
must be an immutable `ObservationDataset` of terminal records, with unique
non-empty Observation IDs, typed token and quota evidence, finite valid quota
deltas, valid timestamps, and one exact configuration. The five configuration
dimensions are plan, model, reasoning level, speed mode, and Codex version.
Missing values remain distinct from known values, and a selected configuration
must match every record. Mixed configurations fail with a typed privacy-safe
error.

All present timestamps are normalized by P017 `canonical_time_key()`. The
anchor is selected in this order:

```text
finalized_at → ended_at → started_at
```

Rows are sorted by canonical UTC timestamp, then Observation ID. Equal-time ID
ordering makes replay deterministic; it does not imply causal order.
Observations missing all three timestamps are counted as
`unanchored_observations`; they are not assigned to a cohort or given a
fabricated time.

## Disjoint cohorts

P021 rolling snapshots overlap heavily. For example, snapshots over rows 1–50
and 2–51 share 49 source observations. P023 does not treat those snapshots as
independent evidence. It forms consecutive, non-overlapping groups directly
from the chronologically ordered P017 Observation rows:

```text
cohort 1: rows 1–20
cohort 2: rows 21–40
cohort 3: rows 41–60
```

Every anchored terminal row counts toward these fixed boundaries, including
rows with excluded, incomplete, invalid, reset-crossed, zero-delta, or missing
meter evidence. Eligibility does not move cohort boundaries. A completed
cohort's membership, deterministic ID, boundaries, P018–P020 statistics, and
confidence are prefix-stable when later rows are appended and the original
prefix is preserved.

A final cohort smaller than the configured size is retained with its member
IDs, count, canonical start/end times, and `incomplete_cohort` diagnostic. It
has no fabricated capacity statistics and cannot be compared. For example, 45
anchored rows at size 20 produce two complete cohorts and one visible 5-row
tail. Only the two complete cohorts can be compared.

## Capacity evidence for each cohort

Five-hour and weekly analysis is independent. For each complete cohort and each
requested meter, P023 makes a homogeneous immutable `ObservationDataset` and
calls, in order:

1. P018 `derive_capacity_candidates()`.
2. P019 `aggregate_capacity_candidates()`.
3. P020 `assess_capacity_evidence()`.

P023 does not replace the candidate eligibility, A/B/C quality handling, reset
rules, quality weights, weighted median, MAD/outlier filter, or percentile
calculations. The primary capacity is the P019 weighted median. The cohort's
P25, P50, and P75 remain P019's unweighted percentiles of retained candidates.
Sample accounting preserves:

```text
candidate_observations = valid_observations + excluded_observations
used_observations = valid_observations - outliers_removed
```

The minimum sample count applies to `used_observations` after P019 outlier
removal, independently in both cohorts. Total cohort size alone is not enough.
Each cohort must also have P020 `medium` or `high` confidence. P023 never
upgrades `low` or `insufficient` confidence based on a large numerical
difference. Reset crossings invalidate only the affected meter's candidates;
other valid rows can still support that cohort. A reset is a window-boundary
event, not evidence of a changed quota allowance.

Typed P023 diagnostics preserve reasons such as insufficient retained samples,
low capacity confidence, unavailable meter evidence, reset crossings,
incomplete telemetry, quality degradation, outliers removed, effect below the
threshold, overlapping distributions, and non-finite comparison arithmetic.
They are internal analytics diagnostics and do not extend `schemas/v1` reason
codes.

## Adjacent comparison and classification

Only neighboring complete cohorts are compared:

```text
C1 → C2
C2 → C3
```

Comparisons sharing a cohort also share its evidence; they are not independent
confirmations. P023 makes no repeated-confirmation probability or independent
p-value claim.

For qualifying cohorts, with previous and current P019 weighted-median raw-token
capacities, P023 computes:

```text
absolute_change_raw_tokens = current - previous
relative_change = (current - previous) / previous
relative_change_percent = 100 * relative_change
```

These values describe empirical capacity estimates. They are not quota
percentage-point consumption changes. The default material threshold is
`abs(relative_change) >= 0.25`; equality passes. The 25% cutoff is a method
heuristic, not a formal statistical test.

A material effect also needs strict separation of the unweighted P019 retained
capacity IQRs:

```text
increase: previous P75 < current P25
decrease: current P75 < previous P25
```

Touching endpoints count as overlap. The intervals are descriptive empirical
distributions, not confidence intervals. Large median differences with
overlapping IQRs are classified as `ambiguous_variation`.

The stable classifications and precedence are:

1. `insufficient_evidence`: either cohort is incomplete, has fewer than the
   configured retained samples, lacks medium/high P020 confidence, or lacks
   finite required statistics.
2. `no_detected_shift`: both cohorts qualify but the absolute relative change
   is below threshold. Direction may still describe the numerical increase,
   decrease, or equality.
3. `ambiguous_variation`: the material threshold passes but IQRs overlap or
   touch.
4. `shift_signal`: the material threshold passes and the appropriate IQRs are
   strictly separated.

`shift_signal` means that the observed empirical evidence supports
investigating a possible change in metering behavior. It is not a verified
provider policy change. Its temporal location is the boundary between the two
cohorts: a shift became observable between the previous cohort and the
subsequent cohort. P023 does not claim a precise change-point timestamp.

## Uncertainty and limits

Workload composition, cache behavior, telemetry changes, possible concurrent
usage, model deployments not represented by the five configuration fields,
meter update timing, and ordinary measurement noise can all affect observed
capacity candidates. P022 task-size distributions may provide separate
context; task counts and workload forecasts are not regime-change evidence.

P023 is a conservative heuristic evaluated over successive cohort pairs. It is
not a formal multiple-testing-controlled hypothesis test. It assigns no
provider-change probability, percentage confidence, p-value, or independent
confirmation count. False positives remain possible when different workloads
or measurement conditions happen to produce separated distributions. False
negatives remain possible when evidence is sparse, confidence is low, IQRs are
broad, changes are smaller than 25%, or the change is gradual. Gradual drift
may never cross the threshold between adjacent cohorts; method v1 does not
compare distant endpoints to manufacture a signal.

Identical candidate distributions produce `no_detected_shift`. Numerical
capacities, IQR endpoints, deltas, and percentages must be finite. The detector
fails with a typed numerical error if P018–P020 cannot produce valid cohort
statistics. If comparison arithmetic overflows, it returns an explicit
`insufficient_evidence` comparison with `nonfinite_comparison`, never NaN,
Infinity, a clamped value, or a substitute constant.

## Privacy, runtime, and integration boundary

The function is deterministic and uses only its dataset, target meter tuple,
and policy. It does not access SQLite, the clock, the network, randomness, or
additional history. Its internal results contain opaque Observation IDs,
canonical UTC boundaries, configuration keys, raw-token capacity summaries,
confidence, counts, percentiles, and compact diagnostics. It never processes
prompts, responses, reasoning traces, tool output, source code, repository
contents, Git remotes, account identifiers, email addresses, credentials, or
raw rollout JSONL content. No analysis result is persisted.

The default bound is 500 observations. For `n` anchored rows, `m` requested
meters, and cohort size `c`, sorting is `O(n log n)`; candidate and estimator
work is approximately `O(m n log c)`, with at most `ceil(n / c)` summaries per
meter. Python standard library only; runtime dependencies remain empty.
P023 changes no public schema, SQLite migration, write path, Rust-to-Python
transport, CLI, or dashboard.

## Inputs for P024

P024 may assume:

- P017 loads read-only historical Observations.
- P018 derives meter-specific capacity candidates.
- P019 performs robust aggregation.
- P020 supplies confidence-aware empirical capacity.
- P021 produces chronological historical trends.
- P022 predicts remaining tasks from raw task-size quantiles.
- P023 identifies conservative possible metering-regime shift signals from
  non-overlapping historical cohorts.
- Regime signals remain observational and unofficial.
- Benchmark experiments are needed to improve calibration and distinguish
  measurement variability from actual environmental changes.

P024 begins **Phase G — Controlled Benchmark Framework**. This document does
not begin that work.
