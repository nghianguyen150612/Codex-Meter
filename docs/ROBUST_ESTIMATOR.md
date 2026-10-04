# Robust Capacity Estimator

P019 aggregates one homogeneous `MeterCandidateSet` produced by P018. The
module is pure: it does not open SQLite, reload observations, inspect another
meter, use the filesystem or network, or calculate a final user-facing
capacity estimate.

## Method version

The estimator behavior is identified by:

```text
ESTIMATOR_METHOD_VERSION = "1.0.0"
```

Method `1.0.0` defines the quality policy, medians, MAD detector, zero-MAD
fallback, outlier threshold, percentile interpolation, accounting behavior,
and numerical rules in this document.

## Quality weights

P018 supplies only A, B, and C candidates. P019 uses the immutable categorical
policy:

| Quality | Integer weight |
| --- | ---: |
| A | 10 |
| B | 8 |
| C | 3 |

D and X have no weight because P018 excludes them. These weights affect only
the weighted median's ordering influence. They never multiply, alter, or create
a token count: a candidate remains its original `full_capacity_raw_tokens`
value. The weights are not derived from summary quality, token validity, quota
delta, task size, or recency.

## Central statistics

The pre-filter ordinary median sorts candidate capacity values and takes the
middle value for odd `N`, or the arithmetic mean of the two middle values for
even `N`. It is exposed as `pre_filter_median_raw_tokens`; the explicit alias
`unweighted_median_raw_tokens` has the same meaning.

The weighted median sorts candidates by ascending capacity and then ascending
`observation_id`. It accumulates integer quality weights and compares
`2 * cumulative_weight` with the integer total weight. A strict crossing
selects the current value. An exact half-weight boundary averages the current
and following sorted candidate values. Candidate values are never numerically
weighted. This rule makes equal-weight even-sized input reduce to the ordinary
median, including duplicate values.

## MAD and outliers

MAD is unweighted and is calculated from all eligible P018 candidate values
before filtering:

```text
m = median(values)
MAD = median(abs(value - m) for value in values)
```

This deliberately separates geometric disagreement from evidence influence:
quality changes the weighted median, but cannot make a C candidate easier to
classify as a geometric outlier than an A or B candidate.

Outlier rejection is disabled for fewer than three eligible candidates. For
`N >= 3` and positive MAD, P019 uses the full constant
`0.6744897501960817` and rejects only when:

```text
0.6744897501960817 * abs(value - m) / MAD > 3.5
```

Exactly `3.5` is retained. The detector's `m` and MAD are never recomputed
after outliers are removed.

When `N >= 3` and MAD is zero, the explicit method `1.0.0` fallback retains
values exactly equal to the pre-filter median and rejects every value that is
not equal to it. No division by zero, epsilon, or silent detector disablement
is used. The policy therefore retains all identical values and removes an
isolated value from a zero-spread median cluster.

The retained candidates must be non-empty. Removing every candidate is an
explicit `RobustAggregationError`, even though the specified median/MAD policy
normally prevents that state.

## Post-filter percentiles

The retained candidates are the used candidates. P25, P50, and P75 are
unweighted descriptive percentiles using deterministic Type-7 linear
interpolation. For sorted values and probability `p`:

```text
h = (n - 1) * p
lower = floor(h)
upper = ceil(h)
q(p) = values[lower] * (1 - (h - lower)) + values[upper] * (h - lower)
```

P50 equals the ordinary median of the retained candidates. The weighted
median is reported separately and is not used to define percentile positions.

`pre_filter_median_raw_tokens` and `mad_raw_tokens` describe the detector's
input. `weighted_median_raw_tokens` and P25/P50/P75 describe the retained,
post-filter distribution. P019 intentionally does not select one of the two
medians as the final capacity estimate.

## Samples and accounting

At least two retained candidates are required for `SUFFICIENT` aggregation.
Zero or one P018 candidate returns `INSUFFICIENT_SAMPLES` with all capacity
summary fields unavailable; one observation is never treated as a full quota
estimate. Two candidates are sufficient for descriptive P019 statistics and
never undergo outlier removal, even if they are far apart.

P018 exclusions remain exclusions. Only valid candidates can become P019
outliers. P019 preserves `candidate_observations`, `valid_observations`, and
`excluded_observations`, then updates:

```text
outliers_removed = number of rejected valid candidates
used_observations = valid_observations - outliers_removed
```

Diagnostic used and outlier observation IDs follow the original chronological
candidate-set order, not value-sorted order. Candidate IDs must be unique, and
manually constructed sets with mismatched decisions, accounting, meter type,
configuration, quality, or finite-positive capacity fail closed with a typed
aggregation error.

## Determinism, numerical behavior, and privacy

The same immutable candidate set produces equality-equivalent output. P019
uses no time, randomness, sampling, database access, network access, or
filesystem state. Candidate capacities and all aggregate results use normal
finite Python `float` values without rounding intermediate or output values.
Integer weights and integer doubled-cumulative comparisons avoid binary
floating-point threshold ambiguity in weighted-median decisions.

The module receives normalized P018 candidates and retains observation IDs for
diagnostics. It adds no prompts, responses, credentials, raw event payloads,
or conversational content. It does not write SQLite or change any schema.

## Inputs for P020

P020 may assume:

- P018 supplies only A/B/C eligible candidates;
- one-observation candidate sets never become robust estimates;
- quality policy v1 is A=10, B=8, C=3;
- candidate values are never numerically weighted;
- weighted median is deterministic;
- MAD/outlier policy is deterministic and versioned;
- zero-MAD behavior is defined;
- outliers are separated from eligibility exclusions;
- P25/P50/P75 use Type-7 interpolation;
- all robust statistics use post-outlier candidates except the detector's
  pre-filter median/MAD;
- sample accounting includes outlier removals; and
- meter/configuration identity remains isolated.

P020 will implement **final capacity estimate selection, sample
sufficiency/stability assessment, confidence classification, and v1
`AnalyticsResult` construction**. P019 does not implement a final capacity
field, confidence grade, confidence interval, used/remaining estimates,
current meter reading, quota-change detection, or result serialization.
