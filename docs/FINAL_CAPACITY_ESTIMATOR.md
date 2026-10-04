# Final Capacity Estimator

P020 completes the v1 empirical capacity pipeline. It consumes a P018
`MeterCandidateSet`, calls the P019 robust aggregation, and produces an
immutable `AnalyticsResultV1` without SQLite I/O, quota acquisition, or
historical-state mutation.

## Final center and percentiles

For a sufficient P019 aggregation, the final full-capacity estimate is the
P019 quality-weighted median. P019 weights are A=10, B=8, and C=3. The
weighted median is robust, lets higher-quality evidence influence the selected
center, and preserves candidate capacity values rather than shrinking them.

The serialized P50 remains P019's ordinary, unweighted median of retained
empirical candidates. Therefore `estimated_full_capacity_raw_tokens` and
`capacity_percentiles_raw_tokens.p50` may differ. P25/P50/P75 are copied from
P019 and are not recalculated by P020.

## Confidence policy

Confidence is Codex Meter's assessment of statistical empirical evidence. It
is not OpenAI confidence, quota certainty, or an account guarantee.

For sufficient aggregation:

- `high`: at least 8 retained observations, relative IQR at most 0.20, no
  retained C-quality candidates, and an outlier rate at most 0.20.
- `medium`: otherwise at least 4 retained observations, relative IQR at most
  0.50, at least half of retained observations are A/B, and an outlier rate at
  most 0.35.
- `low`: every other sufficient aggregation, including stable evidence with
  only 2–3 retained observations.

All boundaries are inclusive. Relative IQR is
`(p75 - p25) / p50`. Outlier rate is
`outliers_removed / valid_observations`, or zero when valid observations are
zero. Quality composition uses only P019's retained `used_observation_ids`,
never excluded candidates or removed outliers. If P019 reports insufficient
samples, confidence is `insufficient` regardless of current meter evidence.

## Current quota position

Current usage is explicit runtime context:

```python
CurrentQuotaPosition(meter_type=..., used_percent=...)
```

`used_percent` is a finite percentage-point value from 0 through 100; `24`
means 24 percentage points, not 0.24. P020 derives
`remaining_percent = 100 - used_percent` conceptually and calculates:

```text
estimated_used_raw_tokens = full_capacity * used_percent / 100
estimated_remaining_raw_tokens = full_capacity - estimated_used_raw_tokens
```

The values remain unrounded finite Python floats. Subtraction defines the
remaining value so used plus remaining equals the same floating calculation of
full capacity. No current position is inferred from historical observations.

A sufficient meter without an explicit current position raises
`CurrentQuotaPositionRequiredError`. An insufficient meter does not require a
position and never receives fabricated capacity numbers. Five-hour and weekly
positions are independent.

## Configuration and result status

`ResultConfigurationIdentity` retains each configuration value's availability
and caller-provided provenance (`observed`, `derived`, or `unavailable`).
Unavailable values omit `value` during serialization. P020 compares the
projected `ConfigurationKey` exactly with every candidate set and fails closed
on mismatch; it never infers provenance from availability.

`AnalyticsResultV1` supports only `current_capacity_estimate`, always emits
schema version `1.0.0`, metric kind `estimated_raw_tokens`, provider authority
`codex_meter_empirical_estimate`, and estimator method version `1.0.0`.

- `succeeded` means at least one requested meter is low, medium, or high.
- `insufficient_evidence` means every requested meter is insufficient, or no
  target meters were requested.
- `failed` is reserved for a later process boundary; pure P020 construction
  raises typed errors instead of manufacturing this status.

Insufficient estimates omit full capacity, used, remaining, and percentile
fields rather than serializing JSON nulls. Sufficient estimates include all of
those fields. Result and warning reason arrays are duplicate-free and use the
common-schema order.

## Reason mapping

P018 exclusions remain detailed internal diagnostics. P020 maps them
conservatively: reset crossings become `quota_reset_crossed`; invalid meter or
unavailable delta becomes `meter_unavailable`; ineligible meter quality becomes
`meter_unstable`; invalid/ineligible token evidence or unavailable raw totals
becomes `telemetry_incomplete`; zero raw totals, zero quota deltas, and
non-finite candidates become `unknown_reason`. Retained C-quality candidates
add `concurrent_usage_possible`. Insufficient evidence caused solely by too
few retained candidates adds `unknown_reason`. Outlier removal and low
confidence do not add warnings by themselves.

Global `warnings` is the deterministic union of all per-meter reason codes.
The result remains an empirical estimate of 5-hour or weekly raw-token
capacity; it is not an official provider allowance or a claim about a Plus
plan's token limit.

## Determinism and integration boundary

`generated_at` is required caller input, validated as UTC RFC3339 with up to
nine fractional digits. P020 never calls the clock, persists current state, or
reloads SQLite. Replaying the same context, candidate sets, and current
positions produces equal results and deterministic compact JSON.

AnalyticsResult v1 requires used/remaining estimates for non-insufficient
confidence, but AnalyticsRequest v1 does not currently carry a current meter
position. P020 therefore accepts current quota position as explicit runtime
context and never infers it from historical Observations. Rust↔Python process
integration must resolve/version this boundary before wiring the final
subprocess protocol.

The serialized request's `minimum_quality` execution path is also deferred:
P018/P019 candidate sets supplied to P020 are assumed to already represent the
caller's selected estimator dataset policy. P017's bounded loading remains a
mechanical dataset concern and is not reinterpreted by P020.

## Inputs for P021

P021 may assume:

- P017 loads strict read-only homogeneous history;
- P018 derives independently eligible meter candidates;
- P019 performs deterministic robust aggregation;
- P020 uses weighted median as method-v1 primary full-capacity estimate;
- P50 remains the unweighted retained median;
- confidence policy v1 is deterministic;
- used/remaining estimates require explicit current meter position;
- AnalyticsResult v1 can be constructed deterministically;
- success and insufficient results validate against the v1 contract;
- current AnalyticsRequest v1 still lacks current meter position and subprocess integration must resolve that boundary explicitly;
- no estimate is represented as an official provider allowance.

P021 begins **Phase F — historical estimate analysis / metering-regime change detection**.
