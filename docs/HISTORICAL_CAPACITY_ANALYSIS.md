# Historical capacity analysis

P021 provides pure, internal typed historical estimates in `codex_meter.analysis`.
They are unofficial empirical raw-token capacity estimates. The product invariant
remains `raw tokens != weighted/effective usage != plan quota`.

## API and as-of semantics

```python
from codex_meter.analysis import HistoricalAnalysisPolicy, analyze_historical_capacity
from codex_meter.history import ObservationDatabase, ObservationSelector, QuotaMeterType

# The caller owns loading; analytical functions never open a database.
with ObservationDatabase.open(database_path) as database:
    dataset = database.load_dataset(ObservationSelector(
        configuration=configuration,
        maximum_observations=250,
    ))

analysis = analyze_historical_capacity(
    dataset,
    target_meters=(QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
    policy=HistoricalAnalysisPolicy(window_observations=50, maximum_snapshots=500),
)
```

The result is an immutable `HistoricalCapacityAnalysis`, with a tuple of
`HistoricalCapacitySeries` in caller-requested meter order and an
`unanchored_observations` count. Each series preserves configuration, ordered
snapshots, and explicit adjacent comparisons. Targets must be unique typed
`QuotaMeterType` values; an empty target tuple is allowed and still validates
and accounts for the dataset. Empty datasets return empty series and retain
only an explicitly selected configuration, if any.

A snapshot is generated for every anchored terminal Observation per requested
meter, including Observations that cannot yield a capacity candidate. At sorted
index `i`, its evidence is the prefix through `i`, inclusive. The analyzer then
selects at most the most recent `window_observations` rows of that prefix.
Later rows never enter an earlier snapshot. Rebuilding with identical earlier
evidence and policy leaves those earlier snapshots exactly equal.

## Rolling windows and bounds

Both policy limits are positive integers in `[1, 500]`; booleans are rejected.
The default window is 50 terminal Observations and the default total snapshot
limit is 500. The window counts all anchored terminal rows, including excluded,
incomplete, and invalid evidence. It never counts just eligible candidates.
Unanchored rows do not participate in any point-in-time window.

`maximum_snapshots` bounds both input rows and the total snapshots across all
requested meters. Two requested meters with 250 anchored rows produce the
maximum 500 snapshots at the default limit. Input-row accounting includes
unanchored rows, keeping validation and missing-time accounting bounded too.
Oversized datasets fail with
`HistoricalDatasetError`, instructing callers to use P017's bounded selector;
the analyzer never silently truncates manually supplied data. Choose a larger
per-call limit up to 500 if needed. P017 may select recent rows before analysis;
estimates then describe only that selected history, without claiming earlier
evidence was loaded.

Every window is rebuilt as an immutable `ObservationDataset`. On eviction the
analyzer recomputes eligibility, MAD, medians, percentiles, and confidence from
that window. It never subtracts previously aggregated medians or MADs.

## Chronological integrity and missing time

Each Observation uses the first available time in this order:

```text
finalized_at → ended_at → started_at
```

All supplied timestamps, including non-anchor fields, are validated with P017's
`canonical_time_key()`. Strict UTC RFC3339 timestamps with zero through nine
fractional digits become `YYYY-MM-DDTHH:MM:SS.NNNNNNNNNZ`. Nanoseconds are
preserved without conversion through microsecond-limited datetime arithmetic.
Variable-width RFC3339 strings are never compared directly.

Rows sort by canonical anchor ascending and Observation ID ascending. Equal-time
ID order is a deterministic tie-breaker, not evidence of causal ordering within
that instant. At an equal-time position only IDs already visited are in the
prefix. Unordered inputs are normalized without modifying the original dataset.
Duplicate IDs, nonterminal rows, invalid timestamps, invalid dataset types, and
unsupported meter identities fail with typed errors.

If all three timestamps are absent, the Observation contributes only to
`unanchored_observations`. It has no fabricated timestamp, snapshot, or rolling
window membership. If these are the only rows, the series remain empty while
preserving their known configuration. There are no clock calls. P017/P018's
nonhistorical treatment of absent timing evidence is unchanged.

## Configuration and meter isolation

All rows, including unanchored rows, must share the exact five-dimensional
`ConfigurationKey`: plan, model, reasoning level, speed mode, and Codex version.
An explicitly selected configuration must match every row. `None` remains
unknown and differs from every known value. Mixed configurations fail closed;
the caller must select separate datasets.

Five-hour and weekly windows use separate P018 candidates, P019 robust
aggregations, P020 confidence assessments, and comparisons. A reset or invalid
meter affects only that meter's contribution; summary quality from the other
meter does not remove otherwise usable evidence.

## Estimator and confidence reuse

Each window calls `derive_capacity_candidates()`, then
`aggregate_capacity_candidates()`, then the public P020
`assess_capacity_evidence(candidate_set, aggregation)`. The current-capacity
builder uses this same authoritative confidence function. No eligibility,
MAD, weighted-median, percentile, or confidence thresholds are reimplemented
in the historical module. Estimator method version remains `1.0.0`.

The primary full-capacity estimate is P019's quality-weighted median. P50 is
the unweighted median of retained candidates. P25/P50/P75 are retained-sample
Type-7 percentiles. Snapshots also expose pre-filter MAD, relative IQR
`(p75 - p25) / p50`, outlier rate, and ordered P020 reason diagnostics.

Confidence uses the existing inclusive P020 rules:

- HIGH: at least 8 retained observations, relative IQR at most 0.20, no retained
  C candidates, and outlier rate at most 0.20.
- MEDIUM: otherwise at least 4 retained observations, relative IQR at most 0.50,
  at least half retained A/B candidates, and outlier rate at most 0.35.
- LOW: other sufficient aggregations.
- INSUFFICIENT: fewer than two retained candidates.

Insufficient snapshots stay visible with Observation ID, canonical timestamp,
meter, configuration, method, sample accounting, and explanatory reasons.
Full-capacity and percentile fields are `None`; one candidate cannot produce
a numerical full-capacity estimate. No current quota percentage is accepted,
and historical snapshots have no estimated used or remaining tokens.

Accounting always preserves:

```text
candidate_observations = valid_observations + excluded_observations
used_observations = valid_observations - outliers_removed
```

Candidate observations never exceed the actual as-of window size. Outliers
remain distinct from eligibility exclusions. Diagnostics are compact P020
reason codes, not source content.

## Adjacent trend comparisons

`compare_historical_snapshots(previous, current)` compares a supplied ordered
pair with identical meter, configuration, and estimator method version. Series
construction calls it only for immediately adjacent snapshots, producing
exactly `max(0, snapshot_count - 1)` comparisons.

```text
absolute_change_raw_tokens = current_capacity - previous_capacity
relative_change_percent = 100 * absolute_change_raw_tokens / previous_capacity
```

The relative percentage measures change in the empirical estimate. It is not
quota consumption percentage points. Direction is `increase`, `decrease`, or
`unchanged` from exact float comparisons, without a significance threshold.
A real zero remains `0.0`.

If either adjacent snapshot is insufficient, direction is `unavailable`, both
deltas are `None`, and the reason is `insufficient_evidence`. Available endpoint
capacities remain available. The analyzer never bridges a gap to an older
sufficient estimate. Nonfinite trend calculations likewise omit both deltas
with `nonfinite_change`. Sufficient capacities are positive finite unrounded
floats; numerical aggregation failures fail closed. Outputs contain no NaN or
Infinity and may be converted with `dataclasses.asdict` for internal use.

A numerical increase or decrease, however large, does not confirm a provider
quota, policy, or metering-regime change. Rolling-window eviction, evidence
quality, outlier decisions, and sample count can all move an estimate. P023
owns change detection, quality checks, and uncertainty policy.

## Privacy, boundaries, and complexity

Outputs contain only opaque Observation IDs, configuration keys, UTC timestamps,
accounting, statistical raw-token estimates, confidence, and compact diagnostics.
They add no prompts, responses, reasoning, tool content, repository paths, Git
remotes, email addresses, account identifiers, credentials, or raw events.
Analytical functions perform no database access, writes, network calls,
randomness, or environment-dependent time operations.

Historical outputs are internal Python models. They are not serialized as
`AnalyticsResultV1`, whose supported analysis remains `current_capacity_estimate`.
This work does not complete a `history_summary` subprocess contract, add a CLI,
change schemas/migrations, or add runtime dependencies.

For `n <= 500` input rows, `m <= 2` meters, at most 500 total snapshots, and
window size `w <= 500`, ordering costs `O(n log n)` and straightforward recomputation costs approximately
`O(m n w log w)` time. Working storage is `O(n + w)` plus `O(m n)` returned
snapshots/comparisons. There is no incremental median/MAD data structure.

## Inputs for P022

P022 may assume:

- Historical Observation datasets load strictly read-only.
- Historical capacity estimates are time-anchored and deterministic.
- Snapshot windows never use future observations.
- Per-meter estimates remain independent.
- Configuration identity remains isolated.
- P020 method-v1 confidence policy is reused.
- Historical estimates retain sample accounting.
- Insufficient evidence remains explicit.
- Historical comparisons preserve uncertainty.
- No historical trend is treated as official provider behavior.

P022 will implement **Remaining-workload prediction using historical task-size
distributions and empirically estimated remaining raw-token capacity.** P021
does not begin that work. P023 owns metering/quota-change detection.
