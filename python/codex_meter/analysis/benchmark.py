"""Pure, exploratory benchmark calibration over caller-provided projections."""

import math
from dataclasses import fields

from codex_meter.estimation import (
    CandidateDerivationError,
    CandidateExclusionReason,
    Confidence,
    EstimatorInvariantError,
    RobustAggregationError,
    aggregate_capacity_candidates,
    assess_capacity_evidence,
    derive_capacity_candidates,
)
from codex_meter.history import (
    ConfigurationKey,
    ObservationDataset,
    ObservationRecord,
    QualityGrade,
    QuotaMeterType,
)

from .benchmark_models import (
    BENCHMARK_ANALYSIS_METHOD_VERSION,
    BenchmarkAnalysisPolicy,
    BenchmarkCalibrationAnalysis,
    BenchmarkCalibrationComparison,
    BenchmarkCapacitySummary,
    BenchmarkCohortKind,
    BenchmarkDatasetError,
    BenchmarkEvidenceAccounting,
    BenchmarkEvidenceDataset,
    BenchmarkEvidenceProvenance,
    BenchmarkIsolationStatus,
    BenchmarkMeasurementStatus,
    BenchmarkNumericalError,
    BenchmarkPolicyError,
    BenchmarkRegimeBoundary,
    _configuration,
    _identifier,
    _meters,
    _time,
)
from .benchmark_models import (
    BenchmarkAnalysisWarning as Warning,
)
from .benchmark_models import (
    BenchmarkCalibrationStatus as Status,
)
from .benchmark_models import (
    BenchmarkRecommendation as Recommendation,
)
from .historical import _ordered_observations
from .models import HistoricalAnalysisError, HistoricalAnalysisPolicy


def _warnings(items: set[Warning]) -> tuple[Warning, ...]:
    return tuple(item for item in Warning if item in items)


def _source_warnings(provenance: BenchmarkEvidenceProvenance) -> set[Warning]:
    return {
        Warning.NO_TRUSTED_PRODUCTION_EXPORT,
        Warning.SYNTHETIC_EVIDENCE
        if provenance is BenchmarkEvidenceProvenance.SYNTHETIC_FIXTURE
        else Warning.UNVERIFIED_PROVENANCE,
    }


def _validate_dataset(
    dataset: ObservationDataset, policy: BenchmarkAnalysisPolicy
) -> ConfigurationKey | None:
    if not isinstance(dataset, ObservationDataset):
        raise BenchmarkDatasetError("analysis requires an ObservationDataset")
    if type(dataset.migration_version) is not int or dataset.migration_version < 1:
        raise BenchmarkDatasetError("migration version requires a positive integer")
    try:
        configuration, _, _ = _ordered_observations(
            dataset,
            HistoricalAnalysisPolicy(),
            maximum_observations=policy.maximum_input_observations,
        )
    except HistoricalAnalysisError:
        raise BenchmarkDatasetError("ObservationDataset failed structural validation") from None
    if configuration is not None:
        _configuration(configuration)
    for row in dataset.observations:
        _identifier(row.observation_id, observation=True)
        times = [
            _time(t) for t in (row.started_at, row.ended_at, row.finalized_at) if t is not None
        ]
        if times != sorted(times):
            raise BenchmarkDatasetError("Observation timestamps are reversed")
    return configuration


def _time_range(
    rows: tuple[ObservationRecord, ...],
) -> tuple[str | None, str | None, int]:
    times = [
        _time(value)
        for row in rows
        for value in (row.started_at, row.ended_at, row.finalized_at)
        if value is not None
    ]
    missing = sum(
        all(value is None for value in (row.started_at, row.ended_at, row.finalized_at))
        for row in rows
    )
    return min(times, default=None), max(times, default=None), missing


def _crosses_boundary(
    start: str | None,
    end: str | None,
    missing: int,
    meter: QuotaMeterType,
    boundaries: tuple[BenchmarkRegimeBoundary, ...],
) -> bool:
    relevant = tuple(item for item in boundaries if item.meter_type is meter)
    if not relevant:
        return False
    if missing:
        return True  # Cannot establish segmentation of every sample.
    return start is not None and any(start <= _time(item.observed_at) <= end for item in relevant)


def _summary(
    *,
    rows: tuple[ObservationRecord, ...],
    configuration: ConfigurationKey | None,
    experiment_id: str | None,
    workload_key: str | None,
    meter: QuotaMeterType,
    evidence: BenchmarkEvidenceDataset,
    warnings: set[Warning],
) -> BenchmarkCapacitySummary:
    rows = tuple(
        sorted(
            rows,
            key=lambda row: (
                _time(row.finalized_at or row.ended_at or row.started_at) or "",
                row.observation_id,
            ),
        )
    )
    dataset = ObservationDataset(rows, selected_configuration=configuration)
    try:
        candidates = derive_capacity_candidates(dataset, meter)
        aggregation = aggregate_capacity_candidates(candidates)
        assessment = assess_capacity_evidence(candidates, aggregation)
    except (
        CandidateDerivationError,
        RobustAggregationError,
        EstimatorInvariantError,
        OverflowError,
    ):
        raise BenchmarkNumericalError(
            "P018–P020 could not produce finite benchmark statistics"
        ) from None
    values = (
        aggregation.weighted_median_raw_tokens,
        aggregation.p25_raw_tokens,
        aggregation.p50_raw_tokens,
        aggregation.p75_raw_tokens,
    )
    if any(value is not None and (not math.isfinite(value) or value <= 0) for value in values):
        raise BenchmarkNumericalError("capacity statistics must be finite and positive")
    if all(value is not None for value in values) and not values[1] <= values[2] <= values[3]:
        raise BenchmarkNumericalError("capacity percentiles must be ordered")
    if any(
        value is not None and (not math.isfinite(value) or value < 0)
        for value in (aggregation.mad_raw_tokens, assessment.relative_iqr, assessment.outlier_rate)
    ):
        raise BenchmarkNumericalError("dispersion must be finite and nonnegative")
    start, end, missing = _time_range(rows)
    crossed = _crosses_boundary(start, end, missing, meter, evidence.regime_boundaries)
    warnings = set(warnings) | _source_warnings(evidence.provenance)
    if missing:
        warnings.add(Warning.TEMPORAL_EVIDENCE_INCOMPLETE)
    if crossed:
        warnings.add(Warning.REGIME_BOUNDARY)
    if configuration is not None and any(
        getattr(configuration, f.name) is None for f in fields(configuration)
    ):
        warnings.add(Warning.UNKNOWN_CONFIGURATION)
    used = set(aggregation.used_observation_ids)
    missing_reasons = {
        CandidateExclusionReason.METER_NOT_VALID,
        CandidateExclusionReason.DELTA_UNAVAILABLE,
    }
    return BenchmarkCapacitySummary(
        cohort_kind=(
            BenchmarkCohortKind.BENCHMARK
            if experiment_id is not None
            else BenchmarkCohortKind.OBSERVATIONAL_REFERENCE
        ),
        experiment_id=experiment_id,
        workload_key=workload_key,
        meter_type=meter,
        configuration=configuration,
        provenance=evidence.provenance,
        confidence=assessment.confidence,
        sample_accounting=aggregation.accounting,
        weighted_median_raw_tokens=aggregation.weighted_median_raw_tokens,
        p25_raw_tokens=aggregation.p25_raw_tokens,
        p50_raw_tokens=aggregation.p50_raw_tokens,
        p75_raw_tokens=aggregation.p75_raw_tokens,
        mad_raw_tokens=aggregation.mad_raw_tokens,
        relative_iqr=assessment.relative_iqr,
        outlier_rate=assessment.outlier_rate,
        c_quality_used=sum(
            c.quality is QualityGrade.C and c.observation_id in used for c in candidates.candidates
        ),
        missing_meter_evidence=sum(
            bool(set(d.exclusion_reasons) & missing_reasons) for d in candidates.decisions
        ),
        reset_crossings=candidates.excluded_by_reason(CandidateExclusionReason.RESET_CROSSED),
        decisions=candidates.decisions,
        used_observation_ids=aggregation.used_observation_ids,
        outlier_observation_ids=aggregation.outlier_observation_ids,
        estimator_reason_codes=assessment.reason_codes,
        observed_from=start,
        observed_through=end,
        observations_without_time=missing,
        spans_regime_boundary=crossed,
        warnings=_warnings(warnings),
    )


def _compare(
    benchmark: BenchmarkCapacitySummary,
    reference: BenchmarkCapacitySummary | None,
    policy: BenchmarkAnalysisPolicy,
    boundaries: tuple[BenchmarkRegimeBoundary, ...],
) -> BenchmarkCalibrationComparison:
    warnings = set(benchmark.warnings)
    recommendations = set()
    absolute = relative = percent = ratio = overlap = None
    if reference is None:
        status = Status.COMPARISON_UNAVAILABLE
        warnings.add(Warning.NO_REFERENCE)
    else:
        warnings.update(reference.warnings)
        insufficient = False
        for summary in (benchmark, reference):
            if summary.sample_accounting.used_observations < policy.minimum_used_observations:
                warnings.add(Warning.INSUFFICIENT_SAMPLES)
                insufficient = True
            if summary.confidence not in (Confidence.MEDIUM, Confidence.HIGH):
                warnings.add(Warning.LOW_CAPACITY_CONFIDENCE)
                insufficient = True
        start = min(
            (s.observed_from for s in (benchmark, reference) if s.observed_from is not None),
            default=None,
        )
        end = max(
            (s.observed_through for s in (benchmark, reference) if s.observed_through is not None),
            default=None,
        )
        crossed = _crosses_boundary(
            start,
            end,
            benchmark.observations_without_time + reference.observations_without_time,
            benchmark.meter_type,
            boundaries,
        )
        if crossed:
            warnings.add(Warning.REGIME_BOUNDARY)
        if insufficient:
            status = Status.INSUFFICIENT_EVIDENCE
            recommendations.add(Recommendation.COLLECT_MORE_SAMPLES)
        elif crossed:
            status = Status.COMPARISON_UNAVAILABLE
        else:
            b, r = benchmark.weighted_median_raw_tokens, reference.weighted_median_raw_tokens
            assert b is not None and r is not None
            absolute = b - r
            relative = absolute / r
            percent = relative * 100
            ratio = b / r
            if not all(math.isfinite(value) for value in (absolute, relative, percent, ratio)):
                absolute = relative = percent = ratio = None
                status = Status.COMPARISON_UNAVAILABLE
                warnings.add(Warning.NONFINITE_COMPARISON)
            else:
                overlap = (
                    benchmark.p25_raw_tokens <= reference.p75_raw_tokens
                    and reference.p25_raw_tokens <= benchmark.p75_raw_tokens
                )
                if abs(relative) < policy.material_relative_difference:
                    status = Status.DESCRIPTIVELY_CONSISTENT
                elif overlap:
                    status = Status.MATERIAL_DIFFERENCE_OVERLAPPING
                else:
                    status = Status.MATERIAL_DIFFERENCE_SEPARATED
                if status in (
                    Status.MATERIAL_DIFFERENCE_OVERLAPPING,
                    Status.MATERIAL_DIFFERENCE_SEPARATED,
                ):
                    recommendations.update(
                        (
                            Recommendation.INVESTIGATE_WORKLOAD_DIFFERENCE,
                            Recommendation.REVIEW_ESTIMATOR_CALIBRATION,
                        )
                    )
    if Warning.REGIME_BOUNDARY in warnings:
        recommendations.add(Recommendation.INVESTIGATE_POSSIBLE_REGIME_SHIFT)
    if any(
        s is not None
        and (
            s.c_quality_used
            or s.reset_crossings
            or s.missing_meter_evidence
            or s.outlier_rate
            or s.confidence is Confidence.LOW
        )
        for s in (benchmark, reference)
    ):
        recommendations.add(Recommendation.INVESTIGATE_METER_STABILITY)
    return BenchmarkCalibrationComparison(
        benchmark,
        reference,
        benchmark.provenance,
        status,
        absolute,
        relative,
        percent,
        ratio,
        overlap,
        _warnings(warnings),
        tuple(item for item in Recommendation if item in recommendations),
    )


def analyze_benchmark_calibration(
    *,
    benchmark_dataset: ObservationDataset,
    benchmark_links: BenchmarkEvidenceDataset,
    reference_dataset: ObservationDataset | None = None,
    target_meters: tuple[QuotaMeterType, ...] = (QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
    policy: BenchmarkAnalysisPolicy = BenchmarkAnalysisPolicy(),
) -> BenchmarkCalibrationAnalysis:
    """Analyze exploratory evidence; never authenticate runs or change an estimator."""
    if not isinstance(policy, BenchmarkAnalysisPolicy):
        raise BenchmarkPolicyError("benchmark analysis requires a typed policy")
    _meters(target_meters)
    if not isinstance(benchmark_links, BenchmarkEvidenceDataset):
        raise BenchmarkDatasetError("analysis requires a typed BenchmarkEvidenceDataset")
    evidence = benchmark_links
    if any(
        len(items) > policy.maximum_input_observations
        for items in (evidence.runs, evidence.experiments, evidence.regime_boundaries)
    ):
        raise BenchmarkDatasetError("evidence exceeds maximum_input_observations")
    configuration = _validate_dataset(benchmark_dataset, policy)
    reference_configuration = (
        _validate_dataset(reference_dataset, policy) if reference_dataset is not None else None
    )
    if (
        configuration is not None
        and reference_configuration is not None
        and configuration != reference_configuration
    ):
        raise BenchmarkDatasetError("benchmark and reference configurations differ")
    observations = {row.observation_id: row for row in benchmark_dataset.observations}
    references = reference_dataset.observations if reference_dataset is not None else ()
    # Include unresolved and incomplete claims in the disjointness check as well.
    claimed = {run.observation_id for run in evidence.runs}
    if (set(observations) | claimed) & {row.observation_id for row in references}:
        raise BenchmarkDatasetError("benchmark and reference evidence overlap")
    experiments = {}
    for experiment in evidence.experiments:
        if experiment.experiment_id in experiments:
            raise BenchmarkDatasetError("duplicate experiment identity")
        if configuration is None:
            configuration = experiment.configuration
        if experiment.configuration != configuration:
            raise BenchmarkDatasetError("experiment configuration differs from benchmark dataset")
        experiments[experiment.experiment_id] = experiment
    if (
        configuration is not None
        and reference_configuration is not None
        and configuration != reference_configuration
    ):
        raise BenchmarkDatasetError("experiment and reference configurations differ")
    run_ids, repetitions, observation_ids = set(), set(), set()
    grouped = {key: [] for key in experiments}
    group_warnings = {key: set() for key in experiments}
    resolved = unresolved = incomplete = invalid = eligible = unknown = concurrent = 0
    for run in evidence.runs:
        experiment = experiments.get(run.experiment_id)
        if experiment is None or run.workload_key != experiment.workload_key:
            raise BenchmarkDatasetError("run is outside declared experiment/workload scope")
        if run.repetition_index >= experiment.planned_run_count:
            raise BenchmarkDatasetError("run repetition exceeds declared experiment scope")
        repetition = (run.experiment_id, run.repetition_index)
        if (
            run.run_id in run_ids
            or repetition in repetitions
            or run.observation_id in observation_ids
        ):
            raise BenchmarkDatasetError("duplicate run, repetition, or Observation association")
        run_ids.add(run.run_id)
        repetitions.add(repetition)
        observation_ids.add(run.observation_id)
        warnings = group_warnings[run.experiment_id]
        if run.isolation_status is BenchmarkIsolationStatus.UNKNOWN:
            unknown += 1
            warnings.add(Warning.UNKNOWN_ISOLATION)
        elif run.isolation_status is BenchmarkIsolationStatus.POSSIBLE_CONCURRENT_USAGE:
            concurrent += 1
            warnings.add(Warning.POSSIBLE_CONCURRENT_USAGE)
        incomplete += run.measurement_status is BenchmarkMeasurementStatus.INCOMPLETE
        invalid += run.measurement_status is BenchmarkMeasurementStatus.INVALID
        row = observations.get(run.observation_id)
        if row is None:
            unresolved += 1
            warnings.add(Warning.UNRESOLVED_LINKS)
            continue
        resolved += 1
        # Compare available times without claiming the association is authenticated.
        pairs = (
            (run.started_at, row.started_at),
            (row.ended_at, run.ended_at),
            (run.ended_at, row.finalized_at),
            (run.started_at, row.ended_at),
            (row.started_at, run.ended_at),
            (run.started_at, row.finalized_at),
        )
        if any(_time(a) > _time(b) for a, b in pairs if a is not None and b is not None):
            raise BenchmarkDatasetError("run and Observation timing is inconsistent")
        if run.measurement_status is not BenchmarkMeasurementStatus.COMPLETE:
            warnings.add(Warning.MEASUREMENT_EXCLUDED)
            continue
        grouped[run.experiment_id].append(row)
        eligible += 1
    warnings = _source_warnings(evidence.provenance)
    for items in group_warnings.values():
        warnings.update(items)
    unlinked = len(set(observations) - observation_ids)
    if unlinked:
        warnings.add(Warning.UNLINKED_OBSERVATIONS)
    if len(experiments) > 1:
        warnings.add(Warning.SEPARATE_EXPERIMENTS)
        for items in group_warnings.values():
            items.add(Warning.SEPARATE_EXPERIMENTS)
    reference_summaries = (
        tuple(
            _summary(
                rows=references,
                configuration=reference_configuration or configuration,
                experiment_id=None,
                workload_key=None,
                meter=meter,
                evidence=evidence,
                warnings=set(),
            )
            for meter in target_meters
        )
        if reference_dataset is not None
        else ()
    )
    for summary in reference_summaries:
        warnings.update(summary.warnings)
    summaries = []
    comparisons = []
    for key, experiment in sorted(experiments.items()):
        for meter in target_meters:
            if meter not in experiment.target_meters:
                continue
            summary = _summary(
                rows=tuple(grouped[key]),
                configuration=configuration,
                experiment_id=key,
                workload_key=experiment.workload_key,
                meter=meter,
                evidence=evidence,
                warnings=group_warnings[key],
            )
            summaries.append(summary)
            reference = next((s for s in reference_summaries if s.meter_type is meter), None)
            comparison = _compare(summary, reference, policy, evidence.regime_boundaries)
            comparisons.append(comparison)
            warnings.update(comparison.warnings)
    return BenchmarkCalibrationAnalysis(
        BENCHMARK_ANALYSIS_METHOD_VERSION,
        configuration,
        evidence.provenance,
        tuple(summaries),
        reference_summaries,
        tuple(comparisons),
        BenchmarkEvidenceAccounting(
            len(experiments),
            len(evidence.runs),
            resolved,
            unresolved,
            incomplete,
            invalid,
            eligible,
            unknown,
            concurrent,
            unlinked,
        ),
        _warnings(warnings),
    )
