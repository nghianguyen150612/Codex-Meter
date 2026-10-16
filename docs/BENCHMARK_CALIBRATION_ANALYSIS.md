# Benchmark calibration analysis

P026 completes Phase G's Python statistical layer with
`BENCHMARK_ANALYSIS_METHOD_VERSION = "1.0.0"`. Results are unofficial,
exploratory empirical evidence. The invariant remains:

```text
raw tokens != weighted/effective usage != plan quota
```

**No production benchmark results are authenticated by P026 until a trusted Rust-originated export/integration boundary is implemented.**

## Input and provenance boundary

P024 validates benchmark plans, lifecycle, and trusted internal Observation
linkage. P025 coordinates authorized runtime attempts, protects durable dispatch,
and resolves existing Observations. Production execution remains disabled.
Neither component establishes an authenticated, persisted Rust-to-Python export.
P026 does not read the dispatch journal or treat an Observation ID as ownership
proof. Structural consistency checks are not authentication.

Public APIs are exported through `codex_meter.analysis`:

```python
analyze_benchmark_calibration(
    *,
    benchmark_dataset: ObservationDataset,
    benchmark_links: BenchmarkEvidenceDataset,
    reference_dataset: ObservationDataset | None = None,
    target_meters=(QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
    policy=BenchmarkAnalysisPolicy(),
) -> BenchmarkCalibrationAnalysis
```

The caller supplies all evidence. No measurements, timestamps, workload registry,
or reference history are generated or loaded automatically.

`BenchmarkEvidenceDataset` contains immutable tuples of:

- `BenchmarkExperimentEvidence`: experiment ID, workload key, exact
  `ConfigurationKey`, planned run count, and selected meters. This is a narrow
  caller-declared scope projection, not a Rust `BenchmarkPlan` transport.
- `BenchmarkRunEvidence`: experiment/run/workload/Observation IDs, repetition
  index, measurement status, isolation status, and optional UTC start/end times.
- `BenchmarkRegimeBoundary`: UTC timestamp and affected meter identifying a
  caller-supplied detected or suspected temporal boundary.

Dataset provenance accepts only typed `synthetic_fixture` or
`caller_supplied_unverified`. Synthetic outputs retain synthetic provenance;
external records retain unverified provenance even when every structural check
passes. There is no accepted runtime-attested enum, verified boolean,
cryptographic claim, or arbitrary metadata field. A future trusted integration
must establish authenticity independently before adding production provenance.

The `verified_isolated` isolation enum is a supplied claim, alongside
`possible_concurrent_usage` and `unknown`. It never authenticates a run or
promotes Observation quality. Unknown isolation remains explicit. Production
controlled calibration would require actual verified isolation and a trusted
origin; P026 cannot provide either through this API.

## Identity, scope, configuration, and grouping

Experiment, run, and workload identifiers follow P024's 1–64 ASCII character
rule: initial letter, followed by letters, digits, underscore, or hyphen.
Observation IDs require `obs:` and 64 lowercase hexadecimal characters, as
emitted by Rust. P026 cannot recompute that identity from private task/session
inputs and does not claim to do so.

The bounded Python projection uses repetition indices 0–99, strictly below the
experiment's declared run count (1–100). This is a P026 projection convention;
it does not change P024's Rust repetition type. Duplicate experiment IDs, run
IDs across the supplied evidence, experiment/repetition pairs, Observation rows,
and Observation associations are rejected. Every run must name its declared
experiment's workload. An unknown workload is rejected relative to this scope;
P026 has no execution registry and cannot verify a workload's actual content.

Each ObservationDataset is homogeneous across all five configuration dimensions:
plan, model, reasoning level, speed mode, and Codex version. Explicit selected
configuration, experiment scope, linked rows, and reference configuration must
match exactly. Unknown (`None`) differs from known values and is never inferred.
Matching unknown configurations retain an uncertainty warning. Missing constructor
fields, malformed values, unsupported enums, nonterminal rows, malformed raw
totals/deltas, and invalid meter identities fail with privacy-safe typed errors.

Summaries are isolated by **configuration, workload, experiment, and meter**.
Distinct experiments remain separate even when workload keys match, with an
explicit warning. This conservative refinement of workload grouping prevents
silent pooling of different periods or execution conditions. Global accounting
preserves experiment and run counts separately. Empty declared experiments
produce insufficient summaries; no declarations produce no benchmark summaries.
Results use sorted experiment IDs, caller-requested meter order, and canonical
Observation time then ID order. Input ordering does not affect numerical results.

## Link screening and sample accounting

Each link resolves to at most one supplied Observation. A missing association
is excluded and counted as unresolved; extra unlinked rows are counted and never
used as benchmarks. Link uniqueness is checked across every experiment.
Available run and Observation times must be consistent. UTC RFC3339 precision up
to nanoseconds is preserved through P017's canonical time keys, without clock
reads. Absent timestamps remain absent and are reported separately.

Run statuses are `complete`, `incomplete`, and `invalid`. Incomplete/invalid
run metadata is excluded before statistical candidate derivation and counted
separately from P018 exclusions and P019 outliers. A complete status is only a
claim enabling exploratory evaluation of the linked terminal Observation.
It does not finalize that Observation. Stored terminal lifecycle and summary
quality never override the independent P018 token and target-meter evidence.
For example, a reset-crossed five-hour contribution does not exclude valid
weekly evidence in a linked terminal record. Nonterminal rows are rejected.

The report retains resolved/unresolved links, incomplete/invalid runs, eligible
linked runs, unknown/possible-concurrent isolation, and unlinked Observations.
For each statistical group it preserves P018 decisions with all exclusion
reasons, missing meter count, reset count, retained C-quality count, used/outlier
IDs, and P020 reasons. A row with several reasons is one excluded Observation:

```text
candidate_observations = valid_observations + excluded_observations
used_observations = valid_observations - outliers_removed
```

Here candidate observations are complete, resolved run records passed to P018;
pre-statistical run/link exclusions remain in evidence accounting. Ordinary
reference rows are separately identified by `observational_reference` cohort
kind, with no experiment or workload identity. They are never relabeled controlled
benchmarks. Provenance on reference summaries describes the enclosing exploratory
analysis limitation, not authenticated benchmark ownership of ordinary history.

## Statistical reuse and confidence

For every benchmark group and each reference meter independently, P026 calls:

1. P018 `derive_capacity_candidates()`.
2. P019 `aggregate_capacity_candidates()`.
3. P020 `assess_capacity_evidence()`.

P018 owns positive raw-total/quota-delta capacity derivation, valid telemetry,
A/B/C eligibility, D/X exclusion, reset exclusion, and numerical candidate checks.
P026 never derives another capacity formula or promotes quality from a benchmark
label, workload key, execution claim, authorization, or isolation claim.

P019 owns A/B/C weights (10/8/3), weighted median, unweighted median, pre-filter
MAD, modified-Z outlier handling including zero-MAD behavior, and retained-sample
Type-7 P25/P50/P75. Weighted median is the comparison center; P50 remains
unweighted. Detector MAD describes the pre-filter candidates. P026 neither
recomputes nor changes these algorithms.

P020 confidence is copied unchanged:

- HIGH: at least 8 retained samples, relative IQR ≤ 0.20, no retained C quality,
  outlier rate ≤ 0.20.
- MEDIUM: otherwise at least 4 retained samples, relative IQR ≤ 0.50, at least
  half retained A/B quality, outlier rate ≤ 0.35.
- LOW: other sufficient aggregations.
- INSUFFICIENT: fewer than two robust candidates; capacity remains unavailable.

This is empirical measurement stability, never a probability that a calibration
conclusion is correct. All existing component versions remain `1.0.0`.

## Reference comparisons and sufficiency

Reference observations must have the exact configuration and target meter.
The entire benchmark dataset and every claimed benchmark Observation ID
(including unresolved/incomplete claims) must be disjoint from the reference
dataset. Proven overlap is a typed input error; no comparison to itself is
reported as independent evidence. Reference capacity is independently derived
through P018–P020 and may be shared as context across several comparisons.
Those comparisons are not independent confirmations.

Method v1 requires at least four retained candidates on **each** side and
independent P020 MEDIUM/HIGH confidence on each side. Ordinary insufficiency
returns `insufficient_evidence`, not an exception. Policy may raise the sample
minimum; it cannot weaken the four-sample floor. No reference produces
`comparison_unavailable` with `no_reference`.

For finite positive benchmark and reference weighted medians B and R:

```text
absolute_difference_raw_tokens = B - R
relative_difference = (B - R) / R
relative_difference_percent = 100 * relative_difference
benchmark_to_reference_ratio = B / R
```

For B=25,000,000 and R=20,000,000 the difference is +5,000,000 raw tokens,
+25%, and ratio 1.25. This means benchmark-derived empirical capacity is 25%
higher than the selected observational reference. It does not establish any
provider quota increase or official estimator error; neither cohort is provider
side ground truth.

Both retained-sample IQRs are preserved. Overlap means:

```text
benchmark.p25 <= reference.p75 and reference.p25 <= benchmark.p75
```

Touching endpoints overlap. Strict separation requires one P75 strictly below
the other's P25. These IQRs describe distributions; they are not confidence
intervals, hypothesis tests, p-values, or certainty percentages.

## Classification and temporal confounders

The policy defaults to `minimum_used_observations=4`,
`material_relative_difference=0.20`, and `maximum_input_observations=500`.
Select the material threshold before inspecting the evidence; it is a
method heuristic, not a universal theorem. Classification precedence is:

1. Integrity violations (configuration, duplicates, overlap) reject input.
2. An absent reference is unavailable.
3. Either side failing retained count or MEDIUM/HIGH confidence yields
   `insufficient_evidence`, retaining any confounder warnings.
4. A supplied temporal boundary spanning either distribution or the combined
   comparison time range yields `comparison_unavailable`.
5. Absolute relative difference strictly below the threshold yields
   `descriptively_consistent`.
6. Threshold met, including equality, with touching/overlapping IQRs yields
   `material_difference_overlapping`.
7. Threshold met with strict IQR separation yields
   `material_difference_separated`.

Boundaries are per meter. Inclusive time endpoints fail conservatively. If a
relevant boundary is supplied and any member lacks all timing, segmentation
cannot be established and the comparison is unavailable. Descriptive summaries
remain available with `spans_regime_boundary`; they cannot support calibration.
The caller should provide separately segmented datasets to proceed.

Without supplied boundaries, P026 does not invent a common metering regime.
Time ranges and missing-time diagnostics remain available. P023 signals can
inform caller-supplied boundaries; P026 does not automatically run P023 or
interpret no signal as proof of no provider change. Workload mix, caching,
concurrent usage, meter latency, quality, unrepresented deployment differences,
and different time periods remain possible confounders. A matching workload
label does not prove the same execution bytes or environment.

Recommendations have fixed enum ordering: collect more samples; investigate
meter stability when supported by quality/reset/missingness/outlier diagnostics;
investigate workload differences and review estimator calibration for material
comparable differences; investigate possible regime shift for supplied temporal
confounding. These are investigation labels, not causal conclusions.

## Numerical safety, reporting, and privacy

Available capacities/percentiles are finite and positive; percentiles are ordered.
MAD, dispersion, and outlier rate are finite and nonnegative. P018 nonfinite
candidates remain explicit exclusions. Impossible P019/P020 arithmetic raises
`BenchmarkNumericalError`; nonfinite comparison arithmetic returns unavailable
with all comparison numbers absent. There is no NaN, Infinity, clamping,
rounding, zero-substitution, or automatic recalibration.

Frozen, slotted report models retain provenance at analysis, summary, and
comparison level. `BenchmarkCalibrationAnalysis` contains benchmark/reference
summaries, comparisons, evidence accounting, and ordered warnings. Each
comparison retains both summaries, status, descriptive metrics, overlap, and
ordered investigations. No current used/remaining token estimate is included.
No human-facing CLI renderer or public JSON serialization protocol is provided.

Only opaque identifiers, UTC times, configuration, numerical telemetry, evidence
classifications, statistics, and compact diagnostics are accepted. There are no
prompts, responses, reasoning, tool outputs, source/repository content, remotes,
credentials, emails, account IDs, rollout JSONL, payload fields, or generic
metadata dictionaries. Errors do not dump input records; analysis emits no logs.

The analyzer is deterministic and performs no file/SQLite access, writes,
network calls, environment lookup, current-clock reads, provider calls, workload
execution, or automatic synthesis. Runtime dependencies remain `[]`; standard
library only. `schemas/v1`, published migrations/checksums, Rust runtime, existing
analysis policies, and estimator outputs are unchanged. No result is persisted
or automatically applied to the main estimator.

## Computational bounds and Phase H handoff

Each Observation dataset, run tuple, experiment tuple, and boundary tuple is
bounded by the policy limit: default 500, maximum 100,000. Bounds reject booleans,
malformed/nonfinite numbers, invalid ranges, and oversized input without
truncation. Use P017's `ObservationSelector(maximum_observations=...)` to bound
history before calling. Each experiment has at most 100 runs. For n observations,
e experiments, b boundaries, and m≤2 meters, validation/ordering and aggregation
cost approximately O(n log n + m n log n + m e b); output has O(n + m e) evidence
and summary entries. Reference statistics are calculated once per meter and
shared by immutable comparisons.

P026 completes Phase G's analytical scope. Trusted provenance export,
subprocess representation, runtime/analytics wiring, presentation, and estimator
policy changes are separate work. No P027 implementation is included.

## Inputs for P027

P027 may assume:

- P017 loads read-only historical Observations.
- P018 derives quota-capacity candidates.
- P019 performs robust aggregation.
- P020 constructs confidence-aware capacity results.
- P021 provides historical time-series estimates.
- P022 provides remaining-workload scenarios.
- P023 detects conservative observational regime-change signals.
- P024 defines benchmark plans, lifecycle, and evidence contracts.
- P025 provides safe runtime orchestration, durable dispatch guards, and trusted internal Observation linkage.
- P026 provides typed benchmark statistical comparisons and calibration diagnostics.
- No provider quota or policy changes are claimed as official.
- Runtime benchmark execution remains disabled in production.
- Benchmark statistical results are not automatically applied to the main estimator.
- Trusted Rust-to-Python benchmark provenance and subprocess contract integration remain outstanding.

P027 begins **Phase H — CLI and Runtime/Analytics Integration**. It is not
implemented or authorized by P026.
