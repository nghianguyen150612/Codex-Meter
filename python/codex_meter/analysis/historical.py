"""Bounded as-of windows over immutable observations, with no storage access."""

import math
from dataclasses import fields

from codex_meter.estimation import (
    Confidence,
    aggregate_capacity_candidates,
    assess_capacity_evidence,
    derive_capacity_candidates,
)
from codex_meter.history import (
    ConfigurationKey,
    EvidenceValidity,
    ObservationDataset,
    ObservationLifecycle,
    ObservationRecord,
    QualityGrade,
    QueryValidationError,
    QuotaEvidenceRecord,
    QuotaMeterType,
    ResetStatus,
    canonical_time_key,
)

from .models import (
    ComparisonUnavailableReason,
    HistoricalAnalysisError,
    HistoricalAnalysisPolicy,
    HistoricalCapacityAnalysis,
    HistoricalCapacityChange,
    HistoricalCapacitySeries,
    HistoricalCapacitySnapshot,
    HistoricalDatasetError,
    HistoricalPolicyError,
    TrendDirection,
)


def compare_historical_snapshots(
    previous: HistoricalCapacitySnapshot,
    current: HistoricalCapacitySnapshot,
) -> HistoricalCapacityChange:
    """Compare a caller-supplied adjacent pair; incompatible identities fail closed."""

    if not isinstance(previous, HistoricalCapacitySnapshot) or not isinstance(
        current, HistoricalCapacitySnapshot
    ):
        raise HistoricalAnalysisError("comparisons require typed snapshots")
    if (
        previous.meter_type is not current.meter_type
        or previous.configuration != current.configuration
        or previous.estimator_method_version != current.estimator_method_version
    ):
        raise HistoricalAnalysisError(
            "comparisons require matching meter, configuration, and method"
        )
    if (previous.observed_at, previous.observation_id) >= (
        current.observed_at,
        current.observation_id,
    ):
        raise HistoricalAnalysisError("comparison snapshots must be chronologically ordered")

    before = previous.estimated_full_capacity_raw_tokens
    after = current.estimated_full_capacity_raw_tokens
    reason = None
    absolute = relative = None
    direction = TrendDirection.UNAVAILABLE
    if (
        previous.confidence is Confidence.INSUFFICIENT
        or current.confidence is Confidence.INSUFFICIENT
    ):
        reason = ComparisonUnavailableReason.INSUFFICIENT_EVIDENCE
    else:
        assert before is not None and after is not None
        absolute = after - before
        relative = 100 * absolute / before
        if not math.isfinite(absolute) or not math.isfinite(relative):
            reason = ComparisonUnavailableReason.NONFINITE_CHANGE
            absolute = relative = None
        else:
            direction = (
                TrendDirection.INCREASE
                if absolute > 0
                else TrendDirection.DECREASE
                if absolute < 0
                else TrendDirection.UNCHANGED
            )
    return HistoricalCapacityChange(
        previous.meter_type,
        previous.observation_id,
        current.observation_id,
        before,
        after,
        absolute,
        relative,
        direction,
        reason,
    )


def _validate_configuration(configuration: ConfigurationKey) -> None:
    if not isinstance(configuration, ConfigurationKey) or any(
        value is not None and (not isinstance(value, str) or not value)
        for field in fields(ConfigurationKey)
        for value in (getattr(configuration, field.name),)
    ):
        raise HistoricalDatasetError("historical configuration must use exact typed values")


def _ordered_observations(
    dataset: ObservationDataset,
    policy: HistoricalAnalysisPolicy,
    *,
    maximum_observations: int | None = None,
) -> tuple[ConfigurationKey | None, list[tuple[str, ObservationRecord]], int]:
    if not isinstance(dataset, ObservationDataset) or not isinstance(dataset.observations, tuple):
        raise HistoricalDatasetError("historical analysis requires an immutable ObservationDataset")
    limit = policy.maximum_snapshots if maximum_observations is None else maximum_observations
    if isinstance(limit, bool) or not isinstance(limit, int) or limit < 1:
        raise HistoricalPolicyError("historical observation limit must be a positive integer")
    if len(dataset.observations) > limit:
        message = (
            "dataset exceeds maximum_snapshots; select a bounded dataset using P017 "
            "ObservationSelector(maximum_observations=...)"
            if maximum_observations is None
            else "dataset exceeds the configured observation limit; select a bounded dataset using "
            "P017 ObservationSelector(maximum_observations=...)"
        )
        raise HistoricalDatasetError(message)
    configuration = dataset.selected_configuration
    if configuration is not None:
        _validate_configuration(configuration)
    seen = set()
    anchored: list[tuple[str, ObservationRecord]] = []
    unanchored = 0
    for record in dataset.observations:
        if not isinstance(record, ObservationRecord):
            raise HistoricalDatasetError("historical observations must be typed")
        if not isinstance(record.observation_id, str) or not record.observation_id:
            raise HistoricalDatasetError("historical observation ID must be non-empty")
        if record.observation_id in seen:
            raise HistoricalDatasetError("duplicate historical observation ID")
        seen.add(record.observation_id)
        if not isinstance(record.lifecycle, ObservationLifecycle) or record.lifecycle not in (
            ObservationLifecycle.FINALIZED,
            ObservationLifecycle.INCOMPLETE,
            ObservationLifecycle.INVALID,
        ):
            raise HistoricalDatasetError("historical observations must be terminal")
        if (
            not isinstance(record.token_validity, EvidenceValidity)
            or not isinstance(record.token_quality, QualityGrade)
            or not isinstance(record.summary_quality, QualityGrade)
            or (
                record.raw_total is not None
                and (
                    isinstance(record.raw_total, bool)
                    or not isinstance(record.raw_total, int)
                    or record.raw_total < 0
                )
            )
        ):
            raise HistoricalDatasetError(
                "historical token evidence must use typed valid projections"
            )
        _validate_configuration(record.configuration)
        if configuration is None:
            configuration = record.configuration
        elif record.configuration != configuration:
            raise HistoricalDatasetError("historical analysis cannot pool multiple configurations")
        for meter, quota in (
            (QuotaMeterType.FIVE_HOUR, record.five_hour),
            (QuotaMeterType.WEEKLY, record.weekly),
        ):
            if not isinstance(quota, QuotaEvidenceRecord) or quota.meter_type is not meter:
                raise HistoricalDatasetError("historical quota evidence has an unsupported meter")
            if (
                not isinstance(quota.validity, EvidenceValidity)
                or not isinstance(quota.quality, QualityGrade)
                or not isinstance(quota.reset_status, ResetStatus)
                or (
                    quota.delta_percentage_points is not None
                    and (
                        isinstance(quota.delta_percentage_points, bool)
                        or not isinstance(quota.delta_percentage_points, (int, float))
                        or not 0 <= quota.delta_percentage_points <= 100
                    )
                )
            ):
                raise HistoricalDatasetError(
                    "historical quota evidence must use typed valid projections"
                )
        try:
            times = tuple(
                canonical_time_key(value) if value is not None else None
                for value in (record.finalized_at, record.ended_at, record.started_at)
            )
        except QueryValidationError as exc:
            raise HistoricalDatasetError("historical observation timestamp is invalid") from exc
        anchor = next((value for value in times if value is not None), None)
        if anchor is None:
            unanchored += 1
        else:
            anchored.append((anchor, record))
    anchored.sort(key=lambda item: (item[0], item[1].observation_id))
    return configuration, anchored, unanchored


def analyze_historical_capacity(
    dataset: ObservationDataset,
    target_meters: tuple[QuotaMeterType, ...] = (QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
    policy: HistoricalAnalysisPolicy = HistoricalAnalysisPolicy(),
) -> HistoricalCapacityAnalysis:
    """Recompute each as-of window independently, preserving requested meter order."""

    if not isinstance(policy, HistoricalAnalysisPolicy):
        raise HistoricalAnalysisError("historical policy must be typed")
    if not isinstance(target_meters, tuple) or any(
        not isinstance(meter, QuotaMeterType) for meter in target_meters
    ):
        raise HistoricalAnalysisError("historical targets must be a tuple of supported meters")
    if len(set(target_meters)) != len(target_meters):
        raise HistoricalAnalysisError("historical target meters must be unique")
    configuration, anchored, unanchored = _ordered_observations(dataset, policy)
    if len(anchored) * len(target_meters) > policy.maximum_snapshots:
        raise HistoricalDatasetError(
            "requested meters exceed maximum_snapshots in total; select a bounded dataset "
            "using P017 ObservationSelector(maximum_observations=...)"
        )
    series: list[HistoricalCapacitySeries] = []
    for meter in target_meters:
        snapshots: list[HistoricalCapacitySnapshot] = []
        for index, (anchor, record) in enumerate(anchored):
            window = ObservationDataset(
                tuple(
                    row
                    for _, row in anchored[
                        max(0, index + 1 - policy.window_observations) : index + 1
                    ]
                ),
                migration_version=dataset.migration_version,
                selected_configuration=configuration,
            )
            candidates = derive_capacity_candidates(window, meter)
            aggregation = aggregate_capacity_candidates(candidates)
            assessment = assess_capacity_evidence(candidates, aggregation)
            snapshots.append(
                HistoricalCapacitySnapshot(
                    observation_id=record.observation_id,
                    observed_at=anchor,
                    meter_type=meter,
                    configuration=record.configuration,
                    estimator_method_version=aggregation.estimator_method_version,
                    confidence=assessment.confidence,
                    estimated_full_capacity_raw_tokens=aggregation.weighted_median_raw_tokens,
                    p25_raw_tokens=aggregation.p25_raw_tokens,
                    p50_raw_tokens=aggregation.p50_raw_tokens,
                    p75_raw_tokens=aggregation.p75_raw_tokens,
                    sample_accounting=aggregation.accounting,
                    reason_codes=assessment.reason_codes,
                    mad_raw_tokens=aggregation.mad_raw_tokens,
                    relative_iqr=assessment.relative_iqr,
                    outlier_rate=assessment.outlier_rate,
                )
            )
        comparisons = tuple(
            compare_historical_snapshots(previous, current)
            for previous, current in zip(snapshots, snapshots[1:])
        )
        series.append(HistoricalCapacitySeries(meter, configuration, tuple(snapshots), comparisons))
    return HistoricalCapacityAnalysis(tuple(series), unanchored)
