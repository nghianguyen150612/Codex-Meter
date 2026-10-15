"""Conservative metering-regime signals from disjoint historical cohorts."""

import math

from codex_meter.estimation import (
    ESTIMATOR_METHOD_VERSION,
    CandidateDerivationError,
    CandidateExclusionReason,
    Confidence,
    EstimatorInvariantError,
    ReasonCode,
    RobustAggregationError,
    aggregate_capacity_candidates,
    assess_capacity_evidence,
    derive_capacity_candidates,
)
from codex_meter.history import (
    ConfigurationKey,
    ObservationDataset,
    ObservationRecord,
    QuotaMeterType,
)

from .historical import _ordered_observations
from .models import HistoricalAnalysisError, HistoricalAnalysisPolicy, TrendDirection
from .regime_models import (
    REGIME_SIGNAL_METHOD_VERSION,
    MeteringRegimeAnalysis,
    MeterRegimeAnalysis,
    RegimeChangeComparison,
    RegimeCohortSummary,
    RegimeDatasetError,
    RegimeDetectionPolicy,
    RegimeNumericalError,
    RegimePolicyError,
    RegimeSignalReason,
    RegimeSignalStatus,
)


def _cohort_reasons(
    *,
    decision_reasons: set[CandidateExclusionReason],
    estimator_reasons: tuple[ReasonCode, ...],
    used_observations: int,
    minimum_used_observations: int,
    confidence: Confidence,
    outliers_removed: int,
) -> tuple[RegimeSignalReason, ...]:
    reasons: set[RegimeSignalReason] = set()
    if used_observations < minimum_used_observations:
        reasons.add(RegimeSignalReason.INSUFFICIENT_SAMPLES)
    if confidence in (Confidence.INSUFFICIENT, Confidence.LOW):
        reasons.add(RegimeSignalReason.LOW_CAPACITY_CONFIDENCE)
    if CandidateExclusionReason.RESET_CROSSED in decision_reasons:
        reasons.add(RegimeSignalReason.RESET_CROSSED)
    if {
        CandidateExclusionReason.METER_NOT_VALID,
        CandidateExclusionReason.DELTA_UNAVAILABLE,
        CandidateExclusionReason.ZERO_QUOTA_DELTA,
    } & decision_reasons or ReasonCode.METER_UNAVAILABLE in estimator_reasons:
        reasons.add(RegimeSignalReason.UNAVAILABLE_METER)
    if {
        CandidateExclusionReason.TOKEN_NOT_VALID,
        CandidateExclusionReason.RAW_TOTAL_UNAVAILABLE,
        CandidateExclusionReason.ZERO_RAW_TOTAL,
    } & decision_reasons or ReasonCode.TELEMETRY_INCOMPLETE in estimator_reasons:
        reasons.add(RegimeSignalReason.TELEMETRY_INCOMPLETE)
    if {
        CandidateExclusionReason.TOKEN_QUALITY_INELIGIBLE,
        CandidateExclusionReason.METER_QUALITY_INELIGIBLE,
    } & decision_reasons or {
        ReasonCode.METER_UNSTABLE,
        ReasonCode.CONCURRENT_USAGE_POSSIBLE,
    } & set(estimator_reasons):
        reasons.add(RegimeSignalReason.QUALITY_DEGRADATION)
    if outliers_removed:
        reasons.add(RegimeSignalReason.OUTLIERS_REMOVED)
    return tuple(reason for reason in RegimeSignalReason if reason in reasons)


def _partial_cohort_summary(
    *,
    index: int,
    members: tuple[tuple[str, ObservationRecord], ...],
    meter: QuotaMeterType,
    configuration: ConfigurationKey | None,
) -> RegimeCohortSummary:
    first_time, first_record = members[0]
    last_time, last_record = members[-1]
    observation_ids = tuple(record.observation_id for _, record in members)
    return RegimeCohortSummary(
        cohort_index=index,
        first_observation_id=first_record.observation_id,
        last_observation_id=last_record.observation_id,
        started_at=first_time,
        ended_at=last_time,
        meter_type=meter,
        configuration=configuration,
        estimator_method_version=None,
        complete=False,
        confidence=None,
        estimated_full_capacity_raw_tokens=None,
        p25_raw_tokens=None,
        p50_raw_tokens=None,
        p75_raw_tokens=None,
        sample_accounting=None,
        reason_codes=(RegimeSignalReason.INCOMPLETE_COHORT,),
        observation_ids=observation_ids,
    )


def _complete_cohort_summary(
    *,
    index: int,
    members: tuple[tuple[str, ObservationRecord], ...],
    meter: QuotaMeterType,
    dataset: ObservationDataset,
    configuration: ConfigurationKey | None,
    policy: RegimeDetectionPolicy,
) -> RegimeCohortSummary:
    first_time, first_record = members[0]
    last_time, last_record = members[-1]
    observation_ids = tuple(record.observation_id for _, record in members)
    cohort_dataset = ObservationDataset(
        observations=tuple(record for _, record in members),
        migration_version=dataset.migration_version,
        selected_configuration=configuration,
    )
    try:
        candidate_set = derive_capacity_candidates(cohort_dataset, meter)
        if any(
            CandidateExclusionReason.NONFINITE_CANDIDATE in decision.exclusion_reasons
            for decision in candidate_set.decisions
        ):
            raise RegimeNumericalError("P018 produced a non-finite capacity candidate")
        aggregation = aggregate_capacity_candidates(candidate_set)
        assessment = assess_capacity_evidence(candidate_set, aggregation)
    except RegimeNumericalError:
        raise
    except (CandidateDerivationError, RobustAggregationError, EstimatorInvariantError) as exc:
        raise RegimeNumericalError(
            "P018–P020 could not produce valid finite cohort statistics"
        ) from exc

    if aggregation.estimator_method_version != ESTIMATOR_METHOD_VERSION:
        raise RegimeNumericalError("cohort estimator method version is unsupported")
    decision_reasons = {
        reason for decision in candidate_set.decisions for reason in decision.exclusion_reasons
    }
    reason_codes = _cohort_reasons(
        decision_reasons=decision_reasons,
        estimator_reasons=assessment.reason_codes,
        used_observations=aggregation.accounting.used_observations,
        minimum_used_observations=policy.minimum_used_observations,
        confidence=assessment.confidence,
        outliers_removed=aggregation.accounting.outliers_removed,
    )
    return RegimeCohortSummary(
        cohort_index=index,
        first_observation_id=first_record.observation_id,
        last_observation_id=last_record.observation_id,
        started_at=first_time,
        ended_at=last_time,
        meter_type=meter,
        configuration=configuration,
        estimator_method_version=aggregation.estimator_method_version,
        complete=True,
        confidence=assessment.confidence,
        estimated_full_capacity_raw_tokens=aggregation.weighted_median_raw_tokens,
        p25_raw_tokens=aggregation.p25_raw_tokens,
        p50_raw_tokens=aggregation.p50_raw_tokens,
        p75_raw_tokens=aggregation.p75_raw_tokens,
        sample_accounting=aggregation.accounting,
        reason_codes=reason_codes,
        observation_ids=observation_ids,
        mad_raw_tokens=aggregation.mad_raw_tokens,
        relative_iqr=assessment.relative_iqr,
        outlier_rate=assessment.outlier_rate,
        estimator_reason_codes=assessment.reason_codes,
    )


def _comparison(
    previous: RegimeCohortSummary,
    current: RegimeCohortSummary,
    policy: RegimeDetectionPolicy,
) -> RegimeChangeComparison:
    assert previous.complete and current.complete
    assert previous.confidence is not None and current.confidence is not None
    assert previous.sample_accounting is not None and current.sample_accounting is not None
    reasons = set(previous.reason_codes) | set(current.reason_codes)
    before = previous.estimated_full_capacity_raw_tokens
    after = current.estimated_full_capacity_raw_tokens
    enough_samples = (
        previous.sample_accounting.used_observations >= policy.minimum_used_observations
        and current.sample_accounting.used_observations >= policy.minimum_used_observations
    )
    enough_confidence = previous.confidence in (Confidence.MEDIUM, Confidence.HIGH) and (
        current.confidence in (Confidence.MEDIUM, Confidence.HIGH)
    )
    has_statistics = all(
        value is not None and math.isfinite(value) and value > 0
        for value in (
            before,
            after,
            previous.p25_raw_tokens,
            previous.p50_raw_tokens,
            previous.p75_raw_tokens,
            current.p25_raw_tokens,
            current.p50_raw_tokens,
            current.p75_raw_tokens,
        )
    )
    if not enough_samples:
        reasons.add(RegimeSignalReason.INSUFFICIENT_SAMPLES)
    if not enough_confidence:
        reasons.add(RegimeSignalReason.LOW_CAPACITY_CONFIDENCE)
    if not has_statistics:
        reasons.add(RegimeSignalReason.UNAVAILABLE_METER)
    if not (enough_samples and enough_confidence and has_statistics):
        return RegimeChangeComparison(
            meter_type=previous.meter_type,
            previous_cohort_index=previous.cohort_index,
            current_cohort_index=current.cohort_index,
            status=RegimeSignalStatus.INSUFFICIENT_EVIDENCE,
            direction=TrendDirection.UNAVAILABLE,
            previous_capacity_raw_tokens=before,
            current_capacity_raw_tokens=after,
            absolute_change_raw_tokens=None,
            relative_change_percent=None,
            previous_confidence=previous.confidence,
            current_confidence=current.confidence,
            reason_codes=tuple(reason for reason in RegimeSignalReason if reason in reasons),
            previous_p25_raw_tokens=previous.p25_raw_tokens,
            previous_p75_raw_tokens=previous.p75_raw_tokens,
            current_p25_raw_tokens=current.p25_raw_tokens,
            current_p75_raw_tokens=current.p75_raw_tokens,
            previous_used_observations=previous.sample_accounting.used_observations,
            current_used_observations=current.sample_accounting.used_observations,
        )

    assert before is not None and after is not None
    assert previous.p25_raw_tokens is not None and previous.p75_raw_tokens is not None
    assert current.p25_raw_tokens is not None and current.p50_raw_tokens is not None
    assert current.p75_raw_tokens is not None
    absolute = after - before
    relative = absolute / before
    percent = relative * 100
    if not all(math.isfinite(value) for value in (absolute, relative, percent)):
        reasons.add(RegimeSignalReason.NONFINITE_COMPARISON)
        return RegimeChangeComparison(
            meter_type=previous.meter_type,
            previous_cohort_index=previous.cohort_index,
            current_cohort_index=current.cohort_index,
            status=RegimeSignalStatus.INSUFFICIENT_EVIDENCE,
            direction=TrendDirection.UNAVAILABLE,
            previous_capacity_raw_tokens=before,
            current_capacity_raw_tokens=after,
            absolute_change_raw_tokens=None,
            relative_change_percent=None,
            previous_confidence=previous.confidence,
            current_confidence=current.confidence,
            reason_codes=tuple(reason for reason in RegimeSignalReason if reason in reasons),
            previous_p25_raw_tokens=previous.p25_raw_tokens,
            previous_p75_raw_tokens=previous.p75_raw_tokens,
            current_p25_raw_tokens=current.p25_raw_tokens,
            current_p75_raw_tokens=current.p75_raw_tokens,
            previous_used_observations=previous.sample_accounting.used_observations,
            current_used_observations=current.sample_accounting.used_observations,
        )

    direction = (
        TrendDirection.INCREASE
        if absolute > 0
        else TrendDirection.DECREASE
        if absolute < 0
        else TrendDirection.UNCHANGED
    )
    if abs(relative) < policy.minimum_relative_change:
        status = RegimeSignalStatus.NO_DETECTED_SHIFT
        reasons.add(RegimeSignalReason.EFFECT_BELOW_THRESHOLD)
    else:
        separated = (
            previous.p75_raw_tokens < current.p25_raw_tokens
            if direction is TrendDirection.INCREASE
            else current.p75_raw_tokens < previous.p25_raw_tokens
            if direction is TrendDirection.DECREASE
            else False
        )
        if separated:
            status = RegimeSignalStatus.SHIFT_SIGNAL
        else:
            status = RegimeSignalStatus.AMBIGUOUS_VARIATION
            reasons.add(RegimeSignalReason.DISTRIBUTION_OVERLAP)

    return RegimeChangeComparison(
        meter_type=previous.meter_type,
        previous_cohort_index=previous.cohort_index,
        current_cohort_index=current.cohort_index,
        status=status,
        direction=direction,
        previous_capacity_raw_tokens=before,
        current_capacity_raw_tokens=after,
        absolute_change_raw_tokens=absolute,
        relative_change_percent=percent,
        previous_confidence=previous.confidence,
        current_confidence=current.confidence,
        reason_codes=tuple(reason for reason in RegimeSignalReason if reason in reasons),
        previous_p25_raw_tokens=previous.p25_raw_tokens,
        previous_p75_raw_tokens=previous.p75_raw_tokens,
        current_p25_raw_tokens=current.p25_raw_tokens,
        current_p75_raw_tokens=current.p75_raw_tokens,
        previous_used_observations=previous.sample_accounting.used_observations,
        current_used_observations=current.sample_accounting.used_observations,
    )


def detect_metering_regime_signals(
    *,
    dataset: ObservationDataset,
    target_meters: tuple[QuotaMeterType, ...] = (
        QuotaMeterType.FIVE_HOUR,
        QuotaMeterType.WEEKLY,
    ),
    policy: RegimeDetectionPolicy = RegimeDetectionPolicy(),
) -> MeteringRegimeAnalysis:
    """Detect possible meter-specific shifts using disjoint chronological cohorts."""

    if not isinstance(policy, RegimeDetectionPolicy):
        raise RegimePolicyError("regime detection requires a typed policy")
    if not isinstance(target_meters, tuple) or any(
        not isinstance(meter, QuotaMeterType) for meter in target_meters
    ):
        raise RegimeDatasetError("target meters must be a tuple of supported meter identities")
    if len(set(target_meters)) != len(target_meters):
        raise RegimeDatasetError("target meters must be unique")
    if not isinstance(dataset, ObservationDataset):
        raise RegimeDatasetError("regime detection requires an ObservationDataset")
    if (
        isinstance(dataset.migration_version, bool)
        or not isinstance(dataset.migration_version, int)
        or dataset.migration_version < 1
    ):
        raise RegimeDatasetError("dataset migration version must be a positive integer")
    if not isinstance(dataset.observations, tuple):
        raise RegimeDatasetError("regime detection requires an immutable ObservationDataset")
    if len(dataset.observations) > policy.maximum_input_observations:
        raise RegimeDatasetError(
            "dataset exceeds maximum_input_observations; select a bounded dataset using P017 "
            "ObservationSelector(maximum_observations=...)"
        )

    try:
        configuration, anchored, unanchored = _ordered_observations(
            dataset,
            HistoricalAnalysisPolicy(),
            maximum_observations=policy.maximum_input_observations,
        )
    except HistoricalAnalysisError as exc:
        raise RegimeDatasetError("regime analysis history failed P021 validation") from exc

    cohort_size = policy.cohort_observations
    cohort_members = tuple(
        tuple(anchored[offset : offset + cohort_size])
        for offset in range(0, len(anchored), cohort_size)
    )
    incomplete_tail_observations = (
        len(cohort_members[-1]) if cohort_members and len(cohort_members[-1]) < cohort_size else 0
    )

    meter_analyses: list[MeterRegimeAnalysis] = []
    for meter in target_meters:
        summaries: list[RegimeCohortSummary] = []
        for index, members in enumerate(cohort_members, start=1):
            if len(members) < cohort_size:
                summary = _partial_cohort_summary(
                    index=index,
                    members=members,
                    meter=meter,
                    configuration=configuration,
                )
            else:
                summary = _complete_cohort_summary(
                    index=index,
                    members=members,
                    meter=meter,
                    dataset=dataset,
                    configuration=configuration,
                    policy=policy,
                )
            summaries.append(summary)
        complete_summaries = tuple(summary for summary in summaries if summary.complete)
        comparisons = tuple(
            _comparison(previous, current, policy)
            for previous, current in zip(complete_summaries, complete_summaries[1:])
        )
        meter_analyses.append(
            MeterRegimeAnalysis(meter, configuration, tuple(summaries), comparisons)
        )

    return MeteringRegimeAnalysis(
        method_version=REGIME_SIGNAL_METHOD_VERSION,
        configuration=configuration,
        meter_analyses=tuple(meter_analyses),
        unanchored_observations=unanchored,
        incomplete_tail_observations=incomplete_tail_observations,
    )


__all__ = ["detect_metering_regime_signals"]
