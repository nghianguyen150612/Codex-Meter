"""Immutable internal historical outputs; these are not AnalyticsResult v1."""

import math
from dataclasses import dataclass
from enum import StrEnum

from codex_meter.estimation import Confidence, ReasonCode, SampleAccounting
from codex_meter.history import (
    ConfigurationKey,
    QueryValidationError,
    QuotaMeterType,
    canonical_time_key,
)

MAXIMUM_HISTORICAL_SNAPSHOTS = 500


class HistoricalAnalysisError(ValueError):
    """Privacy-safe failure for invalid historical inputs or calculations."""


class HistoricalDatasetError(HistoricalAnalysisError):
    """Invalid, ambiguous, or oversized historical dataset."""


class HistoricalPolicyError(HistoricalAnalysisError):
    """Invalid computation bounds."""


@dataclass(frozen=True, slots=True)
class HistoricalAnalysisPolicy:
    window_observations: int = 50
    maximum_snapshots: int = 500

    def __post_init__(self) -> None:
        for value in (self.window_observations, self.maximum_snapshots):
            if (
                isinstance(value, bool)
                or not isinstance(value, int)
                or not 1 <= value <= MAXIMUM_HISTORICAL_SNAPSHOTS
            ):
                raise HistoricalPolicyError("historical policy limits must be integers in [1, 500]")


@dataclass(frozen=True, slots=True)
class HistoricalCapacitySnapshot:
    observation_id: str
    observed_at: str
    meter_type: QuotaMeterType
    configuration: ConfigurationKey
    estimator_method_version: str
    confidence: Confidence
    estimated_full_capacity_raw_tokens: float | None
    p25_raw_tokens: float | None
    p50_raw_tokens: float | None
    p75_raw_tokens: float | None
    sample_accounting: SampleAccounting
    reason_codes: tuple[ReasonCode, ...]
    mad_raw_tokens: float | None
    relative_iqr: float | None
    outlier_rate: float

    def __post_init__(self) -> None:
        if (
            not isinstance(self.observation_id, str)
            or not self.observation_id
            or not isinstance(self.meter_type, QuotaMeterType)
            or not isinstance(self.configuration, ConfigurationKey)
            or not isinstance(self.estimator_method_version, str)
            or not self.estimator_method_version
            or not isinstance(self.sample_accounting, SampleAccounting)
            or not isinstance(self.reason_codes, tuple)
            or any(not isinstance(reason, ReasonCode) for reason in self.reason_codes)
        ):
            raise HistoricalAnalysisError(
                "snapshot identity, accounting, and reasons must be typed"
            )
        try:
            if canonical_time_key(self.observed_at) != self.observed_at:
                raise HistoricalAnalysisError("snapshot time must be a canonical UTC key")
        except QueryValidationError as exc:
            raise HistoricalAnalysisError("snapshot time must be a canonical UTC key") from exc
        values = (
            self.estimated_full_capacity_raw_tokens,
            self.p25_raw_tokens,
            self.p50_raw_tokens,
            self.p75_raw_tokens,
        )
        if not isinstance(self.confidence, Confidence):
            raise HistoricalAnalysisError("snapshot confidence must be typed")
        if self.confidence is Confidence.INSUFFICIENT:
            if any(value is not None for value in values) or not self.reason_codes:
                raise HistoricalAnalysisError(
                    "insufficient snapshots require reasons and no capacity"
                )
            if self.sample_accounting.used_observations >= 2:
                raise HistoricalAnalysisError(
                    "insufficient snapshots must have fewer than two samples"
                )
        else:
            if self.sample_accounting.used_observations < 2 or any(
                isinstance(value, bool)
                or not isinstance(value, float)
                or not math.isfinite(value)
                or value <= 0
                for value in values
            ):
                raise HistoricalAnalysisError(
                    "sufficient snapshots require positive finite statistics"
                )
            if not self.p25_raw_tokens <= self.p50_raw_tokens <= self.p75_raw_tokens:
                raise HistoricalAnalysisError("snapshot percentiles must be ordered")
        for value in (self.mad_raw_tokens, self.relative_iqr, self.outlier_rate):
            if value is not None and (
                isinstance(value, bool)
                or not isinstance(value, float)
                or not math.isfinite(value)
                or value < 0
            ):
                raise HistoricalAnalysisError("snapshot dispersion must be finite and non-negative")


class TrendDirection(StrEnum):
    INCREASE = "increase"
    DECREASE = "decrease"
    UNCHANGED = "unchanged"
    UNAVAILABLE = "unavailable"


class ComparisonUnavailableReason(StrEnum):
    INSUFFICIENT_EVIDENCE = "insufficient_evidence"
    NONFINITE_CHANGE = "nonfinite_change"


@dataclass(frozen=True, slots=True)
class HistoricalCapacityChange:
    meter_type: QuotaMeterType
    previous_observation_id: str
    current_observation_id: str
    previous_capacity_raw_tokens: float | None
    current_capacity_raw_tokens: float | None
    absolute_change_raw_tokens: float | None
    relative_change_percent: float | None
    direction: TrendDirection
    unavailable_reason: ComparisonUnavailableReason | None

    def __post_init__(self) -> None:
        for value in (self.previous_capacity_raw_tokens, self.current_capacity_raw_tokens):
            if value is not None and (
                not isinstance(value, float) or not math.isfinite(value) or value <= 0
            ):
                raise HistoricalAnalysisError("comparison endpoints must be positive finite floats")
        deltas = (self.absolute_change_raw_tokens, self.relative_change_percent)
        if self.direction is TrendDirection.UNAVAILABLE:
            if any(value is not None for value in deltas) or not isinstance(
                self.unavailable_reason, ComparisonUnavailableReason
            ):
                raise HistoricalAnalysisError(
                    "unavailable comparisons require reasons and no deltas"
                )
        elif (
            not isinstance(self.direction, TrendDirection)
            or self.unavailable_reason is not None
            or self.previous_capacity_raw_tokens is None
            or self.current_capacity_raw_tokens is None
            or any(not isinstance(value, float) or not math.isfinite(value) for value in deltas)
        ):
            raise HistoricalAnalysisError(
                "available comparisons require finite deltas and endpoints"
            )


@dataclass(frozen=True, slots=True)
class HistoricalCapacitySeries:
    meter_type: QuotaMeterType
    configuration: ConfigurationKey | None
    snapshots: tuple[HistoricalCapacitySnapshot, ...]
    comparisons: tuple[HistoricalCapacityChange, ...]


@dataclass(frozen=True, slots=True)
class HistoricalCapacityAnalysis:
    series: tuple[HistoricalCapacitySeries, ...]
    unanchored_observations: int
