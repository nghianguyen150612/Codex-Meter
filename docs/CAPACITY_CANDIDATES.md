# Raw Capacity Candidates

P018 derives empirical raw-token capacity candidates from the immutable,
read-only `ObservationDataset` loaded by P017. It does not claim an official
provider allowance. The product wording for later presentation remains
**Estimated 5-hour raw-token capacity**.

## Candidate meaning

For one Observation and one target quota meter, an included candidate describes
the empirical full-window raw-token capacity implied by that local measurement:

```text
raw_tokens_per_percentage_point = raw_total / delta_percentage_points
full_capacity_raw_tokens = raw_total * 100 / delta_percentage_points
```

`delta_percentage_points` is a percentage-point change, not percent growth. For
example, `4,500,000 / 20` gives `225,000` raw tokens per percentage point and a
`22,500,000` raw-token candidate for a hypothetical 100-point window.

Candidates use `ObservationRecord.raw_total` only. Input, cached, output,
reasoning, weighted, effective, and plan-quota values are not substituted or
combined. The source raw total stays an integer; both derived values are normal
Python IEEE-754 `float` values and are not rounded.

## Eligibility

A candidate requires all of the following:

- token validity is `valid`;
- token quality is `A`, `B`, or `C`;
- raw total is present and greater than zero;
- the requested meter validity is `valid`;
- requested meter quality is `A`, `B`, or `C`;
- requested meter delta is present and strictly positive;
- the requested meter has not crossed a reset boundary; and
- both derived values are finite and positive.

Quality `D` and `X` are excluded. The candidate quality is the worse of the
relevant token and target-meter qualities using `A < B < C < D < X`. No
numeric quality weights are applied.

A missing raw total and a zero raw total are distinct: missing evidence emits
`RAW_TOTAL_UNAVAILABLE`, while a measured zero emits `ZERO_RAW_TOTAL`. A zero
quota delta is retained as evidence but emits `ZERO_QUOTA_DELTA`; it is never
converted to infinity, epsilon, or missing data. A tiny positive delta remains
a candidate, regardless of its magnitude.

Reset crossings emit `RESET_CROSSED` in addition to the other applicable meter
reasons. The unrelated meter is never consulted for target-meter eligibility.
In particular, a summary quality of `X` caused by an invalid weekly reset does
not exclude an otherwise eligible five-hour candidate, and vice versa.

## Decisions and accounting

`derive_capacity_candidates(dataset, meter_type)` returns one immutable
`CandidateDecision` per supplied Observation, in the dataset's existing
chronological order. An included decision has one `RawCapacityCandidate` and no
reasons. An excluded decision has no candidate and a stable, unique tuple of
all applicable `CandidateExclusionReason` values. Reasons are ordered by the
enum definition, not by set iteration.

The result also contains the included candidates in decision order and
`SampleAccounting`:

```text
candidate_observations = every supplied Observation row
valid_observations = rows producing a candidate
excluded_observations = rows producing no candidate
outliers_removed = 0
used_observations = valid_observations
```

Thus `candidate_observations = valid_observations + excluded_observations`
and `used_observations = valid_observations - outliers_removed`. Reason counts,
when queried, can overlap because one row may have multiple reasons; row
accounting never double-counts such reasons.

## Configuration and privacy

Candidate derivation requires one exact `ConfigurationKey` regime. A selected
configuration must match every Observation. Without a selected configuration,
a non-empty dataset must be homogeneous; mixed configurations raise a typed
error rather than being pooled. An empty dataset preserves a selected
configuration, if present, and otherwise has no configuration.

The estimator module is pure and consumes only normalized P017 records. It does
not access SQLite, the filesystem, the network, prompts, responses, reasoning
text, tool output, rollout records, source paths, credentials, or provider
payloads. Candidate and exclusion diagnostics may retain an Observation ID,
but no conversational data.

## Numerical and lifecycle behavior

P017's validated finite delta range remains authoritative. P018 still handles
missing and zero deltas explicitly and rejects any non-finite derived value.
Lifecycle labels, including `incomplete` and `invalid`, do not independently
remove a row; relevant token and target-meter evidence controls eligibility.
No age filter, minimum delta, minimum raw total, candidate cap, cross-task merge,
or aggregation is applied.

## Inputs for P019

P019 may assume:

- one positive-delta valid Observation yields at most one candidate per meter;
- candidate formula is fixed;
- five-hour and weekly candidates are independent;
- raw token count is unweighted;
- A/B/C candidates are retained with categorical quality;
- D/X observations are excluded;
- zero-delta observations are explicitly excluded from finite candidate derivation;
- zero raw-token observations are explicitly excluded;
- all excluded rows retain deterministic reason codes;
- incompatible configurations cannot be pooled;
- candidate sets have exact sample accounting; and
- candidates preserve observation IDs and chronological order.

P019 will implement robust candidate aggregation: median, quality weighting
policy, weighted median, MAD-based outlier handling, and P25/P50/P75. P018
does not implement any of those operations, confidence intervals, used or
remaining-token estimates, or AnalyticsResult serialization.
