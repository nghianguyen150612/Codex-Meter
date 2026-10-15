# Remaining-workload prediction

P022 adds a pure, internal Python analysis that combines a bounded P017
`ObservationDataset` with an explicitly supplied P020 `AnalyticsResultV1`.
It estimates how many similarly sized historical tasks may fit in each meter's
estimated remaining raw-token capacity. Results are empirical planning
estimates, not guarantees, official allowances, or exact counts of future Work
calls.

## Method and inputs

The method version is `WORKLOAD_METHOD_VERSION = "1.0.0"`. It versions task
sample eligibility, Type-7 task-size quantiles, and whole-task count
calculations. It is independent of P019/P020's
`ESTIMATOR_METHOD_VERSION = "1.0.0"`.

```python
from codex_meter.analysis import predict_remaining_workload

prediction = predict_remaining_workload(
    dataset=selected_task_history,
    capacity_result=current_capacity_result,
)
```

The caller selects history through P017, including any configuration, date
range, and row bound. P022 consumes exactly those rows. It does not query
SQLite, widen a window, call the clock, acquire quota information, or derive a
current meter position from historical quota observations. Capacity comes
only from each P020 meter's `estimated_remaining_raw_tokens`.

The P020 result must be a typed, validated `AnalyticsResultV1`. Its
`dataset_configuration.configuration_key` is the authoritative identity. The
selected dataset configuration and every nonempty row must match all five
dimensions exactly: plan, model, reasoning level, speed mode, and Codex
version. Unknown (`None`) and known values differ. An empty, unselected dataset
uses the P020 identity and remains insufficient task history. Task-size rows
and P020 capacity samples need not share Observation IDs or time windows;
that possible time-window mismatch is a limitation of the estimate.

P022 accepts only terminal observations (`finalized`, `incomplete`, or
`invalid`). An incomplete or invalid lifecycle does not by itself discard
otherwise valid token evidence. A missing task timestamp is accepted; P022
does not create timestamps or infer chronology. Duplicate Observation IDs,
nonterminal rows, malformed typed fields, and more than P017's 100,000-row
reader limit fail with typed errors. The caller must explicitly choose any
recent or time-bounded history through P017.

## Task-size evidence

Raw task sizes are the unweighted integer `raw_total` from token telemetry.
They stay separate from quality-weighted statistical influence, quota
percentage, and official plan allowance:

```text
raw token telemetry != weighted capacity aggregation != observed quota percentage != official plan allowance
```

A task sample is eligible when all of these conditions hold:

- `token_validity` is `valid`;
- token quality is A, B, or C; and
- `raw_total` is a present positive integer.

Quality is a categorical eligibility decision. P022 never multiplies task
sizes by P019's A=10, B=8, or C=3 capacity-aggregation weights. A- and
B-quality tasks are eligible. C-quality tasks are also eligible, and the
distribution reports their count because possible concurrent usage may make
their task-size evidence less certain. D- and X-quality tasks are excluded.

Task-size eligibility is independent of quota-meter validity and global
summary quality. A valid task-token total may contribute when either or both
quota meters are unavailable, a quota reset crossed the observation, quota
deltas are missing or zero, or summary quality is X. P022 does not consult
quota deltas or reset status when deciding task-size eligibility.

Each terminal row receives exactly one deterministic decision, sorted by
Observation ID for stable replay. Eligible rows become immutable
`TaskSizeSample` values. Excluded rows retain every applicable typed reason:

- `token_not_valid`;
- `token_quality_ineligible`;
- `raw_total_unavailable`; and
- `zero_raw_total`.

Missing and zero raw totals remain distinct. Neither is replaced by one or by
an epsilon. An Observation with several reasons counts once in
`excluded_observations`. `TaskSizeAccounting` enforces:

```text
candidate_observations = eligible_observations + excluded_observations
```

There is no minimum task-size threshold: one raw token is valid evidence. A
positive raw total that cannot be represented safely in the quantile
arithmetic fails closed with `WorkloadNumericalError`.

## Historical task-size distribution

At least three eligible positive task samples are required. This three-sample
minimum is a P022 versioned design policy, not an official statistical rule or
provider guarantee. With zero, one, or two samples, the typed distribution is
`insufficient`, its P25/P50/P75 fields are `None`, and no task-count
predictions are emitted. Decision, exclusion, and accounting data remain
available.

For three or more samples, P022 computes unweighted Type-7 linear interpolation
over sorted raw task sizes. For sample count `n` and probability `p`, it uses
`h = (n - 1) * p`, then interpolates between `floor(h)` and `ceil(h)`. The
quantiles are descriptive historical sizes; there is no capacity outlier
filter. Large tasks remain in the distribution because they can be legitimate
heavy-task evidence. P019's capacity outlier policy is not applied to task
sizes.

The scenarios mean only relative historical task size in the selected
configuration and dataset:

| Scenario | Historical raw-token task size |
| --- | --- |
| Light | P25 |
| Normal | P50 |
| Heavy | P75 |

**These names do not identify task types.** P013/P017 observations do not have
a stable, verified semantic task classification, so P022 cannot say whether a
task was coding, debugging, research, refactoring, planning, or a benchmark.
Light, normal, and heavy do not make those claims.

For task sizes `100`, `200`, `300`, and `400`, Type-7 yields P25 `175`, P50
`250`, and P75 `325` raw tokens.

## Remaining task counts

For each P020 meter independently, let `R` be that meter's finite, non-negative
estimated remaining raw-token capacity and let `T` be the positive historical
task-size quantile for a scenario. P022 computes:

```text
estimated_task_count = floor(R / T)
```

The count represents whole tasks only. It is never rounded to the nearest
integer and never fractional. For example, 2,500,000 estimated remaining raw
tokens with light, normal, and heavy sizes of 25,000, 50,000, and 100,000
produces 100, 50, and 25 estimated similarly sized tasks. A positive budget
smaller than a scenario task produces zero. Zero remaining capacity also
produces zero for all three scenarios.

Because P25 <= P50 <= P75, available predictions enforce:

```text
light_task_count >= normal_task_count >= heavy_task_count
```

Counts above the v1 safe-integer maximum `9_007_199_254_740_991`, nonfinite
calculations, and unsafe division fail closed with a typed numerical error.
P022 never clamps a count to an arbitrary ceiling.

Five-hour and weekly estimates are separate. Each meter prediction retains its
P020 confidence, remaining raw-token amount, compact P020 reason codes, and
empirical provider-authority label. P020 `low`, `medium`, and `high` confidence
passes through unchanged; more task samples never raise capacity confidence.

## Insufficient evidence

Task history and remaining capacity are independent evidence domains. A
prediction is unavailable when either domain is insufficient, and its task
counts are `None`, never a substitute zero. A valid zero remaining capacity
with sufficient task history is available and has three zero counts.

Per-meter status preserves the distinction between insufficient task history,
insufficient P020 capacity, and both at once. P020's `insufficient` confidence
never borrows the other meter's remaining capacity. Conversely, a sufficient
P020 result cannot replace the three-sample task-history minimum.

## Limitations, boundaries, and privacy

The capacity history and task-size dataset may cover different caller-selected
time periods. P022 preserves the P020 request ID, generation time, and
estimator method version as provenance, but it does not claim the history is
contemporaneous with the current capacity estimate. It does not infer official
plan token limits or future execution counts. Both estimated capacity and
historical task sizes are uncertain.

Outputs are internal Python models, not an extension to `AnalyticsResultV1`.
Current `AnalyticsResultV1` remains `current_capacity_estimate`; P022 changes
no schema, fixture, process protocol, SQLite schema, or database content. The
analysis does no database reads or writes, network operations, prompt or
response logging, or clock access. Its diagnostics contain only configuration
keys, opaque Observation IDs, raw task-size values, counts, confidence, and
compact reason codes. No prompt, response, reasoning content, source code,
repository path, account identifier, email, credential, or raw rollout payload
is introduced.

## Inputs for P023

P023 may assume:

- Historical capacity trends are available from P021.
- Historical task-size distributions and workload scenarios are available from P022.
- Capacity and task-size uncertainty remain separate.
- Five-hour and weekly meters remain independent.
- Configurations cannot be silently pooled.
- Historical capacity trends never imply official quota changes by themselves.
- No remaining-workload prediction claims an official allowance.

P023 will implement:

**Historical metering-regime and quota-change detection, using defensible statistical evidence and explicit uncertainty.**
