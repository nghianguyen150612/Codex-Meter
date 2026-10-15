"""Immutable internal models for historical metering-regime signals."""

import hashlib
import json
import math
from dataclasses import dataclass
from enum import StrEnum

from codex_meter.estimation import (
    ESTIMATOR_METHOD_VERSION,
    Confidence,
    ReasonCode,
    SampleAccounting,
)
from codex_meter.history import ConfigurationKey, QuotaMeterType, canonical_time_key

from .models import TrendDirection

REGIME_SIGNAL_METHOD_VERSION = "1.0.0"
MAXIMUM_REGIME_INPUT_OBSERVATIONS = 100_000


class RegimeAnalysisError(ValueError):
    """Base error for invalid regime-analysis input or output calculations."""


class RegimeDatasetError(RegimeAnalysisError):
    """Invalid, mixed-configuration, nonterminal, or oversized history input."""


class RegimePolicyError(RegimeAnalysisError):
    """Invalid regime-detection policy values."""


class RegimeNumericalError(RegimeAnalysisError):
    """P018–P020 produced invalid or non-finite numerical evidence."""


@dataclass(frozen=True, slots=True)
class RegimeDetectionPolicy:
    cohort_observations: int = 20
    minimum_used_observations: int = 8
    minimum_relative_change: float = 0.25
    maximum_input_observations: int = 500

    def __post_init__(self) -> None:
        for name, value in (
            ("cohort_observations", self.cohort_observations),
            ("minimum_used_observations", self.minimum_used_observations),
            ("maximum_input_observations", self.maximum_input_observations),
        ):
            if (
                isinstance(value, bool)
                or not isinstance(value, int)
                or not 1 <= value <= MAXIMUM_REGIME_INPUT_OBSERVATIONS
            ):
                raise RegimePolicyError(
                    f"{name} must be an integer in [1, {MAXIMUM_REGIME_INPUT_OBSERVATIONS}]"
                )
        if self.maximum_input_observations < self.cohort_observations:
            raise RegimePolicyError(
                "maximum_input_observations must be at least cohort_observations"
            )
        if self.minimum_used_observations < 2:
            raise RegimePolicyError("minimum_used_observations must be at least 2")
        if self.minimum_used_observations > self.cohort_observations:
            raise RegimePolicyError("minimum_used_observations cannot exceed cohort_observations")
        value = self.minimum_relative_change
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise RegimePolicyError("minimum_relative_change must be a finite positive number")
        try:
            normalized = float(value)
        except (OverflowError, ValueError) as exc:
            raise RegimePolicyError(
                "minimum_relative_change must be a finite positive number"
            ) from exc
        if not math.isfinite(normalized) or normalized <= 0:
            raise RegimePolicyError("minimum_relative_change must be a finite positive number")
        object.__setattr__(self, "minimum_relative_change", normalized)


class RegimeSignalStatus(StrEnum):
    INSUFFICIENT_EVIDENCE = "insufficient_evidence"
    NO_DETECTED_SHIFT = "no_detected_shift"
    AMBIGUOUS_VARIATION = "ambiguous_variation"
    SHIFT_SIGNAL = "shift_signal"


class RegimeSignalReason(StrEnum):
    INCOMPLETE_COHORT = "incomplete_cohort"
    INSUFFICIENT_SAMPLES = "insufficient_samples"
    LOW_CAPACITY_CONFIDENCE = "low_capacity_confidence"
    UNAVAILABLE_METER = "unavailable_meter"
    RESET_CROSSED = "reset_crossed"
    TELEMETRY_INCOMPLETE = "telemetry_incomplete"
    QUALITY_DEGRADATION = "quality_degradation"
    OUTLIERS_REMOVED = "outliers_removed"
    EFFECT_BELOW_THRESHOLD = "effect_below_threshold"
    DISTRIBUTION_OVERLAP = "distribution_overlap"
    NONFINITE_COMPARISON = "nonfinite_comparison"


def _ordered_signal_reasons(
    reasons: tuple[RegimeSignalReason, ...] | set[RegimeSignalReason],
) -> tuple[RegimeSignalReason, ...]:
    return tuple(reason for reason in RegimeSignalReason if reason in set(reasons))


def _finite_positive_float(value: object) -> bool:
    return (
        not isinstance(value, bool)
        and isinstance(value, float)
        and math.isfinite(value)
        and value > 0
    )


def _finite_nonnegative_float(value: object) -> bool:
    return (
        not isinstance(value, bool)
        and isinstance(value, float)
        and math.isfinite(value)
        and value >= 0
    )


@dataclass(frozen=True, slots=True)
class RegimeCohortSummary:
    cohort_index: int
    first_observation_id: str
    last_observation_id: str
    started_at: str
    ended_at: str
    meter_type: QuotaMeterType
    configuration: ConfigurationKey | None
    estimator_method_version: str | None
    complete: bool
    confidence: Confidence | None
    estimated_full_capacity_raw_tokens: float | None
    p25_raw_tokens: float | None
    p50_raw_tokens: float | None
    p75_raw_tokens: float | None
    sample_accounting: SampleAccounting | None
    reason_codes: tuple[RegimeSignalReason, ...]
    observation_ids: tuple[str, ...]
    mad_raw_tokens: float | None = None
    relative_iqr: float | None = None
    outlier_rate: float | None = None
    estimator_reason_codes: tuple[ReasonCode, ...] = ()

    @property
    def observation_count(self) -> int:
        return len(self.observation_ids)

    @property
    def cohort_id(self) -> str:
        members = json.dumps(self.observation_ids, ensure_ascii=False, separators=(",", ":"))
        digest = hashlib.sha256(("codex-meter-regime-v1\0" + members).encode("utf-8"))
        return digest.hexdigest()

    def __post_init__(self) -> None:
        if (
            isinstance(self.cohort_index, bool)
            or not isinstance(self.cohort_index, int)
            or self.cohort_index < 1
            or not isinstance(self.first_observation_id, str)
            or not self.first_observation_id
            or not isinstance(self.last_observation_id, str)
            or not self.last_observation_id
            or not isinstance(self.meter_type, QuotaMeterType)
            or (
                self.configuration is not None
                and not isinstance(self.configuration, ConfigurationKey)
            )
            or not isinstance(self.complete, bool)
            or not isinstance(self.observation_ids, tuple)
            or not self.observation_ids
            or any(not isinstance(item, str) or not item for item in self.observation_ids)
            or len(set(self.observation_ids)) != len(self.observation_ids)
            or self.first_observation_id != self.observation_ids[0]
            or self.last_observation_id != self.observation_ids[-1]
            or not isinstance(self.reason_codes, tuple)
            or any(not isinstance(reason, RegimeSignalReason) for reason in self.reason_codes)
            or self.reason_codes != _ordered_signal_reasons(self.reason_codes)
            or not isinstance(self.estimator_reason_codes, tuple)
            or any(not isinstance(reason, ReasonCode) for reason in self.estimator_reason_codes)
        ):
            raise RegimeAnalysisError("cohort identity and diagnostics must use typed values")
        try:
            if (
                canonical_time_key(self.started_at) != self.started_at
                or canonical_time_key(self.ended_at) != self.ended_at
            ):
                raise RegimeAnalysisError("cohort boundaries must use canonical UTC timestamps")
        except ValueError as exc:
            raise RegimeAnalysisError(
                "cohort boundaries must use canonical UTC timestamps"
            ) from exc
        if self.started_at > self.ended_at:
            raise RegimeAnalysisError("cohort boundaries must be chronologically ordered")

        statistics = (
            self.estimated_full_capacity_raw_tokens,
            self.p25_raw_tokens,
            self.p50_raw_tokens,
            self.p75_raw_tokens,
        )
        if not self.complete:
            if (
                self.estimator_method_version is not None
                or self.confidence is not None
                or any(value is not None for value in statistics)
                or self.sample_accounting is not None
                or self.mad_raw_tokens is not None
                or self.relative_iqr is not None
                or self.outlier_rate is not None
                or self.estimator_reason_codes
                or RegimeSignalReason.INCOMPLETE_COHORT not in self.reason_codes
            ):
                raise RegimeAnalysisError("partial cohorts cannot contain fabricated statistics")
            return

        if (
            self.estimator_method_version != ESTIMATOR_METHOD_VERSION
            or not isinstance(self.confidence, Confidence)
            or not isinstance(self.sample_accounting, SampleAccounting)
            or self.outlier_rate is None
            or not _finite_nonnegative_float(self.outlier_rate)
            or any(
                value is not None and not _finite_nonnegative_float(value)
                for value in (self.mad_raw_tokens, self.relative_iqr)
            )
        ):
            raise RegimeAnalysisError("complete cohorts require typed estimator diagnostics")
        if self.confidence is Confidence.INSUFFICIENT:
            if any(value is not None for value in statistics):
                raise RegimeAnalysisError("insufficient cohorts cannot contain capacity statistics")
        elif (
            self.sample_accounting.used_observations < 2
            or not _finite_positive_float(self.estimated_full_capacity_raw_tokens)
            or any(not _finite_positive_float(value) for value in statistics[1:])
            or not self.p25_raw_tokens <= self.p50_raw_tokens <= self.p75_raw_tokens
        ):
            raise RegimeAnalysisError("cohort capacity statistics must be finite and ordered")


@dataclass(frozen=True, slots=True)
class RegimeChangeComparison:
    meter_type: QuotaMeterType
    previous_cohort_index: int
    current_cohort_index: int
    status: RegimeSignalStatus
    direction: TrendDirection
    previous_capacity_raw_tokens: float | None
    current_capacity_raw_tokens: float | None
    absolute_change_raw_tokens: float | None
    relative_change_percent: float | None
    previous_confidence: Confidence
    current_confidence: Confidence
    reason_codes: tuple[RegimeSignalReason, ...]
    previous_p25_raw_tokens: float | None = None
    previous_p75_raw_tokens: float | None = None
    current_p25_raw_tokens: float | None = None
    current_p75_raw_tokens: float | None = None
    previous_used_observations: int = 0
    current_used_observations: int = 0

    def __post_init__(self) -> None:
        if (
            not isinstance(self.meter_type, QuotaMeterType)
            or isinstance(self.previous_cohort_index, bool)
            or not isinstance(self.previous_cohort_index, int)
            or self.previous_cohort_index < 1
            or isinstance(self.current_cohort_index, bool)
            or not isinstance(self.current_cohort_index, int)
            or self.current_cohort_index != self.previous_cohort_index + 1
            or not isinstance(self.status, RegimeSignalStatus)
            or not isinstance(self.direction, TrendDirection)
            or not isinstance(self.previous_confidence, Confidence)
            or not isinstance(self.current_confidence, Confidence)
            or not isinstance(self.reason_codes, tuple)
            or any(not isinstance(reason, RegimeSignalReason) for reason in self.reason_codes)
            or self.reason_codes != _ordered_signal_reasons(self.reason_codes)
            or any(
                isinstance(value, bool) or not isinstance(value, int) or value < 0
                for value in (self.previous_used_observations, self.current_used_observations)
            )
        ):
            raise RegimeAnalysisError("comparison identity and diagnostics must use typed values")

        capacities = (self.previous_capacity_raw_tokens, self.current_capacity_raw_tokens)
        for value in capacities:
            if value is not None and not _finite_positive_float(value):
                raise RegimeAnalysisError("comparison capacities must be positive and finite")
        intervals = (
            (self.previous_p25_raw_tokens, self.previous_p75_raw_tokens),
            (self.current_p25_raw_tokens, self.current_p75_raw_tokens),
        )
        for lower, upper in intervals:
            if (lower is None) != (upper is None) or (
                lower is not None
                and (
                    not _finite_positive_float(lower)
                    or not _finite_positive_float(upper)
                    or lower > upper
                )
            ):
                raise RegimeAnalysisError("comparison IQR endpoints must be finite and ordered")

        deltas = (self.absolute_change_raw_tokens, self.relative_change_percent)
        if self.status is RegimeSignalStatus.INSUFFICIENT_EVIDENCE:
            if self.direction is not TrendDirection.UNAVAILABLE or any(
                value is not None for value in deltas
            ):
                raise RegimeAnalysisError(
                    "insufficient comparisons require unavailable direction and no deltas"
                )
        elif (
            self.previous_capacity_raw_tokens is None
            or self.current_capacity_raw_tokens is None
            or deltas[0] is None
            or isinstance(deltas[0], bool)
            or not isinstance(deltas[0], float)
            or not math.isfinite(deltas[0])
            or deltas[1] is None
            or isinstance(deltas[1], bool)
            or not isinstance(deltas[1], float)
            or not math.isfinite(deltas[1])
            or self.direction is TrendDirection.UNAVAILABLE
        ):
            raise RegimeAnalysisError("available comparisons require finite endpoints and deltas")

        if self.status is not RegimeSignalStatus.INSUFFICIENT_EVIDENCE and (
            self.previous_confidence not in (Confidence.MEDIUM, Confidence.HIGH)
            or self.current_confidence not in (Confidence.MEDIUM, Confidence.HIGH)
        ):
            raise RegimeAnalysisError("available comparisons require medium or high confidence")

        if self.status is not RegimeSignalStatus.INSUFFICIENT_EVIDENCE:
            assert self.previous_capacity_raw_tokens is not None
            assert self.current_capacity_raw_tokens is not None
            assert self.absolute_change_raw_tokens is not None
            assert self.relative_change_percent is not None
            expected_change = self.current_capacity_raw_tokens - self.previous_capacity_raw_tokens
            expected_direction = (
                TrendDirection.INCREASE
                if expected_change > 0
                else TrendDirection.DECREASE
                if expected_change < 0
                else TrendDirection.UNCHANGED
            )
            expected_percent = (expected_change / self.previous_capacity_raw_tokens) * 100
            if (
                self.absolute_change_raw_tokens != expected_change
                or self.relative_change_percent != expected_percent
                or self.direction is not expected_direction
            ):
                raise RegimeAnalysisError("comparison direction and deltas must match endpoints")

        if self.status in (RegimeSignalStatus.SHIFT_SIGNAL, RegimeSignalStatus.AMBIGUOUS_VARIATION):
            if self.direction not in (TrendDirection.INCREASE, TrendDirection.DECREASE):
                raise RegimeAnalysisError("shift classifications require an available direction")
            if any(
                value is None
                for value in (
                    self.previous_p75_raw_tokens,
                    self.current_p25_raw_tokens,
                    self.current_p75_raw_tokens,
                    self.previous_p25_raw_tokens,
                )
            ):
                raise RegimeAnalysisError("shift classifications require both cohort IQRs")
            separated = (
                self.previous_p75_raw_tokens < self.current_p25_raw_tokens
                if self.direction is TrendDirection.INCREASE
                else self.current_p75_raw_tokens < self.previous_p25_raw_tokens
            )
            if (self.status is RegimeSignalStatus.SHIFT_SIGNAL) != separated:
                raise RegimeAnalysisError("shift classification must match strict IQR separation")
        elif self.status is RegimeSignalStatus.NO_DETECTED_SHIFT:
            if self.direction is TrendDirection.UNAVAILABLE:
                raise RegimeAnalysisError("no-shift comparisons require a descriptive direction")


@dataclass(frozen=True, slots=True)
class MeterRegimeAnalysis:
    meter_type: QuotaMeterType
    configuration: ConfigurationKey | None
    cohorts: tuple[RegimeCohortSummary, ...]
    comparisons: tuple[RegimeChangeComparison, ...]

    def __post_init__(self) -> None:
        if (
            not isinstance(self.meter_type, QuotaMeterType)
            or (
                self.configuration is not None
                and not isinstance(self.configuration, ConfigurationKey)
            )
            or not isinstance(self.cohorts, tuple)
            or any(not isinstance(cohort, RegimeCohortSummary) for cohort in self.cohorts)
            or any(
                cohort.meter_type is not self.meter_type
                or cohort.configuration != self.configuration
                for cohort in self.cohorts
            )
            or tuple(cohort.cohort_index for cohort in self.cohorts)
            != tuple(sorted(cohort.cohort_index for cohort in self.cohorts))
            or not isinstance(self.comparisons, tuple)
            or any(not isinstance(item, RegimeChangeComparison) for item in self.comparisons)
            or any(item.meter_type is not self.meter_type for item in self.comparisons)
        ):
            raise RegimeAnalysisError("meter analysis must preserve typed chronological cohorts")
        indices = tuple(cohort.cohort_index for cohort in self.cohorts)
        if indices != tuple(range(1, len(indices) + 1)):
            raise RegimeAnalysisError("cohort indices must be consecutive and start at one")
        if any(not cohort.complete for cohort in self.cohorts[:-1]):
            raise RegimeAnalysisError("only the final cohort may be incomplete")
        member_ids = tuple(
            observation_id for cohort in self.cohorts for observation_id in cohort.observation_ids
        )
        if len(member_ids) != len(set(member_ids)):
            raise RegimeAnalysisError("an Observation cannot belong to multiple cohorts")
        for previous, current in zip(self.cohorts, self.cohorts[1:]):
            if previous.ended_at > current.started_at or (
                previous.ended_at == current.started_at
                and previous.last_observation_id >= current.first_observation_id
            ):
                raise RegimeAnalysisError("cohorts must preserve canonical chronological order")
        complete_indices = tuple(cohort.cohort_index for cohort in self.cohorts if cohort.complete)
        expected_comparisons = tuple(zip(complete_indices, complete_indices[1:]))
        actual_comparisons = tuple(
            (comparison.previous_cohort_index, comparison.current_cohort_index)
            for comparison in self.comparisons
        )
        if actual_comparisons != expected_comparisons:
            raise RegimeAnalysisError("comparisons must cover adjacent complete cohorts only")


@dataclass(frozen=True, slots=True)
class MeteringRegimeAnalysis:
    method_version: str
    configuration: ConfigurationKey | None
    meter_analyses: tuple[MeterRegimeAnalysis, ...]
    unanchored_observations: int
    incomplete_tail_observations: int

    def __post_init__(self) -> None:
        if (
            self.method_version != REGIME_SIGNAL_METHOD_VERSION
            or (
                self.configuration is not None
                and not isinstance(self.configuration, ConfigurationKey)
            )
            or not isinstance(self.meter_analyses, tuple)
            or any(not isinstance(item, MeterRegimeAnalysis) for item in self.meter_analyses)
            or any(item.configuration != self.configuration for item in self.meter_analyses)
            or len({item.meter_type for item in self.meter_analyses}) != len(self.meter_analyses)
            or any(
                isinstance(value, bool) or not isinstance(value, int) or value < 0
                for value in (self.unanchored_observations, self.incomplete_tail_observations)
            )
        ):
            raise RegimeAnalysisError("regime analysis identity and accounting must be valid")
        if self.meter_analyses:
            for analysis in self.meter_analyses:
                incomplete = (
                    analysis.cohorts[-1].observation_count
                    if (analysis.cohorts and not analysis.cohorts[-1].complete)
                    else 0
                )
                if incomplete != self.incomplete_tail_observations:
                    raise RegimeAnalysisError("meter analyses must agree on incomplete tail size")


__all__ = [
    "MAXIMUM_REGIME_INPUT_OBSERVATIONS",
    "REGIME_SIGNAL_METHOD_VERSION",
    "MeterRegimeAnalysis",
    "MeteringRegimeAnalysis",
    "RegimeAnalysisError",
    "RegimeChangeComparison",
    "RegimeCohortSummary",
    "RegimeDatasetError",
    "RegimeDetectionPolicy",
    "RegimeNumericalError",
    "RegimePolicyError",
    "RegimeSignalReason",
    "RegimeSignalStatus",
]
