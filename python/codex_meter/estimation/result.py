"""Final empirical capacity estimates and AnalyticsResult v1 serialization."""

from __future__ import annotations

import json
import math
import re
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import datetime
from enum import StrEnum

from codex_meter.history import ConfigurationKey, QualityGrade, QuotaMeterType

from .candidates import CandidateExclusionReason, MeterCandidateSet, SampleAccounting
from .robust import (
    ESTIMATOR_METHOD_VERSION,
    AggregationStatus,
    RobustAggregationError,
    aggregate_capacity_candidates,
)

SCHEMA_VERSION = "1.0.0"
ANALYSIS_TYPE = "current_capacity_estimate"
METRIC_KIND = "estimated_raw_tokens"
PROVIDER_AUTHORITY = "codex_meter_empirical_estimate"
_OPAQUE_ID_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$")
_UTC_TIMESTAMP_PATTERN = re.compile(
    r"^(?P<base>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(?P<fraction>\d{1,9}))?Z$"
)
_SAFE_INTEGER_MAX = 9_007_199_254_740_991


class FinalEstimateError(ValueError):
    """Base class for invalid P020 result inputs or finalization state."""


class CurrentQuotaPositionRequiredError(FinalEstimateError):
    """Raised when a sufficient estimate has no explicit current meter position."""


class ResultConfigurationMismatchError(FinalEstimateError):
    """Raised when candidate evidence is not for the requested result configuration."""


class ResultInputMismatchError(FinalEstimateError):
    """Raised when target meters and supplied result inputs do not match exactly."""


class EstimatorInvariantError(FinalEstimateError):
    """Raised when P019 output cannot be finalized into a valid v1 estimate."""


class ConfigurationProvenance(StrEnum):
    """How a result configuration value was established."""

    OBSERVED = "observed"
    DERIVED = "derived"
    UNAVAILABLE = "unavailable"


@dataclass(frozen=True, slots=True)
class ResultConfigurationValue:
    """One schema configuration value with explicit availability provenance."""

    value: str | None
    provenance: ConfigurationProvenance

    def __post_init__(self) -> None:
        if not isinstance(self.provenance, ConfigurationProvenance):
            raise FinalEstimateError("configuration provenance must be a v1 enum value")
        if self.provenance is ConfigurationProvenance.UNAVAILABLE:
            if self.value is not None:
                raise FinalEstimateError("unavailable configuration values must be None")
            return
        if not isinstance(self.value, str) or not self.value or len(self.value) > 128:
            raise FinalEstimateError("available configuration values must be non-empty strings")

    @property
    def available(self) -> bool:
        return self.provenance is not ConfigurationProvenance.UNAVAILABLE

    def to_dict(self) -> dict[str, str]:
        if not self.available:
            return {"availability": "unavailable", "provenance": "unavailable"}
        assert self.value is not None
        return {
            "availability": "available",
            "value": self.value,
            "provenance": self.provenance.value,
        }


@dataclass(frozen=True, slots=True)
class ResultConfigurationIdentity:
    """Full v1 configuration identity retained at the result boundary."""

    plan: ResultConfigurationValue
    model: ResultConfigurationValue
    reasoning_level: ResultConfigurationValue
    speed_mode: ResultConfigurationValue
    codex_version: ResultConfigurationValue

    def __post_init__(self) -> None:
        if any(
            not isinstance(value, ResultConfigurationValue)
            for value in (
                self.plan,
                self.model,
                self.reasoning_level,
                self.speed_mode,
                self.codex_version,
            )
        ):
            raise FinalEstimateError("dataset configuration must use typed result values")

    @property
    def configuration_key(self) -> ConfigurationKey:
        return ConfigurationKey(
            self.plan.value,
            self.model.value,
            self.reasoning_level.value,
            self.speed_mode.value,
            self.codex_version.value,
        )

    def to_dict(self) -> dict[str, dict[str, str]]:
        return {
            "plan": self.plan.to_dict(),
            "model": self.model.to_dict(),
            "reasoning_level": self.reasoning_level.to_dict(),
            "speed_mode": self.speed_mode.to_dict(),
            "codex_version": self.codex_version.to_dict(),
        }


@dataclass(frozen=True, slots=True)
class CurrentQuotaPosition:
    """Explicit caller-provided current percentage for one quota meter."""

    meter_type: QuotaMeterType
    used_percent: float

    def __post_init__(self) -> None:
        if not isinstance(self.meter_type, QuotaMeterType):
            raise FinalEstimateError("current quota position meter must be a v1 meter type")
        if isinstance(self.used_percent, bool) or not isinstance(self.used_percent, (int, float)):
            raise FinalEstimateError("current used percentage must be numeric")
        if not math.isfinite(self.used_percent) or not 0 <= self.used_percent <= 100:
            raise FinalEstimateError("current used percentage must be finite between 0 and 100")


@dataclass(frozen=True, slots=True)
class CurrentCapacityResultContext:
    """Deterministic metadata supplied by the analytics runtime boundary."""

    request_id: str
    dataset_configuration: ResultConfigurationIdentity
    target_quota_windows: tuple[QuotaMeterType, ...]
    generated_at: str

    def __post_init__(self) -> None:
        if (
            not isinstance(self.request_id, str)
            or _OPAQUE_ID_PATTERN.fullmatch(self.request_id) is None
        ):
            raise FinalEstimateError("request ID does not match v1 opaque ID semantics")
        if not isinstance(self.dataset_configuration, ResultConfigurationIdentity):
            raise FinalEstimateError("dataset configuration must be typed")
        if not isinstance(self.target_quota_windows, tuple):
            raise FinalEstimateError("target quota windows must be a tuple")
        if any(not isinstance(meter, QuotaMeterType) for meter in self.target_quota_windows):
            raise FinalEstimateError("target quota windows must use v1 meter types")
        if len(set(self.target_quota_windows)) != len(self.target_quota_windows):
            raise ResultInputMismatchError("target quota windows must be unique")
        _validate_utc_timestamp(self.generated_at)


class Confidence(StrEnum):
    """Codex Meter's assessment of statistical empirical evidence."""

    INSUFFICIENT = "insufficient"
    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"


class AnalyticsResultStatus(StrEnum):
    """Overall availability of requested empirical estimates."""

    SUCCEEDED = "succeeded"
    INSUFFICIENT_EVIDENCE = "insufficient_evidence"
    FAILED = "failed"


class ReasonCode(StrEnum):
    """Compact public v1 diagnostics, in their required serialization order."""

    QUOTA_RESET_CROSSED = "quota_reset_crossed"
    METER_UNAVAILABLE = "meter_unavailable"
    METER_UNSTABLE = "meter_unstable"
    TELEMETRY_INCOMPLETE = "telemetry_incomplete"
    CONCURRENT_USAGE_POSSIBLE = "concurrent_usage_possible"
    PROCESS_INTERRUPTED = "process_interrupted"
    SOURCE_ACQUISITION_FAILURE = "source_acquisition_failure"
    UNKNOWN_REASON = "unknown_reason"


_REASON_ORDER = tuple(ReasonCode)


def _ordered_reasons(reasons: set[ReasonCode] | tuple[ReasonCode, ...]) -> tuple[ReasonCode, ...]:
    reason_set = set(reasons)
    if any(not isinstance(reason, ReasonCode) for reason in reason_set):
        raise FinalEstimateError("reason codes must use v1 enum values")
    return tuple(reason for reason in _REASON_ORDER if reason in reason_set)


def _validate_utc_timestamp(value: str) -> None:
    if not isinstance(value, str):
        raise FinalEstimateError("generated_at must be a UTC RFC3339 timestamp")
    match = _UTC_TIMESTAMP_PATTERN.fullmatch(value)
    if match is None:
        raise FinalEstimateError(
            "generated_at must use UTC RFC3339 with at most nine fraction digits"
        )
    try:
        datetime.strptime(match.group("base"), "%Y-%m-%dT%H:%M:%S")
    except ValueError as exc:
        raise FinalEstimateError("generated_at is not a valid UTC date-time") from exc


def _finite_nonnegative(value: float, label: str) -> float:
    if not isinstance(value, (int, float)) or not math.isfinite(value) or value < 0:
        raise EstimatorInvariantError(f"{label} must be finite and non-negative")
    return 0.0 if value == 0 else float(value)


def _validate_sample_accounting(accounting: SampleAccounting) -> None:
    if not isinstance(accounting, SampleAccounting):
        raise EstimatorInvariantError("sample accounting must be P019 sample accounting")
    counts = (
        accounting.candidate_observations,
        accounting.valid_observations,
        accounting.excluded_observations,
        accounting.outliers_removed,
        accounting.used_observations,
    )
    if any(
        isinstance(count, bool)
        or not isinstance(count, int)
        or count < 0
        or count > _SAFE_INTEGER_MAX
        for count in counts
    ):
        raise EstimatorInvariantError("sample accounting counts exceed the v1 safe integer range")


@dataclass(frozen=True, slots=True)
class QuotaCapacityEstimate:
    """One immutable per-meter empirical capacity estimate."""

    meter_type: QuotaMeterType
    confidence: Confidence
    sample_accounting: SampleAccounting
    reason_codes: tuple[ReasonCode, ...]
    estimated_full_capacity_raw_tokens: float | None
    estimated_used_raw_tokens: float | None
    estimated_remaining_raw_tokens: float | None
    p25_raw_tokens: float | None
    p50_raw_tokens: float | None
    p75_raw_tokens: float | None

    def __post_init__(self) -> None:
        if not isinstance(self.meter_type, QuotaMeterType):
            raise EstimatorInvariantError("estimate meter must be a v1 meter type")
        if not isinstance(self.confidence, Confidence):
            raise EstimatorInvariantError("estimate confidence must be a v1 enum value")
        _validate_sample_accounting(self.sample_accounting)
        ordered_reasons = _ordered_reasons(self.reason_codes)
        object.__setattr__(self, "reason_codes", ordered_reasons)

        values = (
            self.estimated_full_capacity_raw_tokens,
            self.estimated_used_raw_tokens,
            self.estimated_remaining_raw_tokens,
            self.p25_raw_tokens,
            self.p50_raw_tokens,
            self.p75_raw_tokens,
        )
        if self.confidence is Confidence.INSUFFICIENT:
            if any(value is not None for value in values):
                raise EstimatorInvariantError(
                    "insufficient estimates must omit all capacity numbers"
                )
            return
        if any(value is None for value in values):
            raise EstimatorInvariantError("sufficient estimates require every capacity number")
        normalized = tuple(_finite_nonnegative(value, "capacity estimate") for value in values)
        object.__setattr__(self, "estimated_full_capacity_raw_tokens", normalized[0])
        object.__setattr__(self, "estimated_used_raw_tokens", normalized[1])
        object.__setattr__(self, "estimated_remaining_raw_tokens", normalized[2])
        object.__setattr__(self, "p25_raw_tokens", normalized[3])
        object.__setattr__(self, "p50_raw_tokens", normalized[4])
        object.__setattr__(self, "p75_raw_tokens", normalized[5])

    def to_dict(self) -> dict[str, object]:
        result: dict[str, object] = {
            "meter_type": self.meter_type.value,
            "metric_kind": METRIC_KIND,
            "provider_authority": PROVIDER_AUTHORITY,
            "confidence": self.confidence.value,
            "sample_accounting": {
                "candidate_observations": self.sample_accounting.candidate_observations,
                "valid_observations": self.sample_accounting.valid_observations,
                "excluded_observations": self.sample_accounting.excluded_observations,
                "outliers_removed": self.sample_accounting.outliers_removed,
                "used_observations": self.sample_accounting.used_observations,
            },
            "reason_codes": [reason.value for reason in self.reason_codes],
        }
        if self.confidence is not Confidence.INSUFFICIENT:
            assert self.estimated_full_capacity_raw_tokens is not None
            assert self.estimated_used_raw_tokens is not None
            assert self.estimated_remaining_raw_tokens is not None
            assert self.p25_raw_tokens is not None
            assert self.p50_raw_tokens is not None
            assert self.p75_raw_tokens is not None
            result.update(
                {
                    "estimated_full_capacity_raw_tokens": self.estimated_full_capacity_raw_tokens,
                    "estimated_used_raw_tokens": self.estimated_used_raw_tokens,
                    "estimated_remaining_raw_tokens": self.estimated_remaining_raw_tokens,
                    "capacity_percentiles_raw_tokens": {
                        "p25": self.p25_raw_tokens,
                        "p50": self.p50_raw_tokens,
                        "p75": self.p75_raw_tokens,
                    },
                }
            )
        return result


@dataclass(frozen=True, slots=True)
class AnalyticsResultV1:
    """Immutable AnalyticsResult v1 for current empirical capacity estimates."""

    request_id: str
    result_status: AnalyticsResultStatus
    dataset_configuration: ResultConfigurationIdentity
    generated_at: str
    estimator_method_version: str
    quota_capacity_estimates: tuple[QuotaCapacityEstimate, ...]
    warnings: tuple[ReasonCode, ...]

    analysis_type = ANALYSIS_TYPE
    schema_version = SCHEMA_VERSION

    def __post_init__(self) -> None:
        if (
            not isinstance(self.request_id, str)
            or _OPAQUE_ID_PATTERN.fullmatch(self.request_id) is None
        ):
            raise FinalEstimateError("request ID does not match v1 opaque ID semantics")
        if not isinstance(self.result_status, AnalyticsResultStatus):
            raise FinalEstimateError("result status must be a v1 enum value")
        if not isinstance(self.dataset_configuration, ResultConfigurationIdentity):
            raise FinalEstimateError("dataset configuration must be typed")
        _validate_utc_timestamp(self.generated_at)
        if self.estimator_method_version != ESTIMATOR_METHOD_VERSION:
            raise EstimatorInvariantError(
                "result estimator method version must be P019 version 1.0.0"
            )
        if not isinstance(self.quota_capacity_estimates, tuple):
            raise FinalEstimateError("quota capacity estimates must be a tuple")
        if any(
            not isinstance(estimate, QuotaCapacityEstimate)
            for estimate in self.quota_capacity_estimates
        ):
            raise FinalEstimateError("quota capacity estimates must be typed")
        meters = tuple(estimate.meter_type for estimate in self.quota_capacity_estimates)
        if len(set(meters)) != len(meters):
            raise ResultInputMismatchError("result estimates must contain unique meters")
        object.__setattr__(self, "warnings", _ordered_reasons(self.warnings))

        has_sufficient = any(
            estimate.confidence is not Confidence.INSUFFICIENT
            for estimate in self.quota_capacity_estimates
        )
        expected_status = (
            AnalyticsResultStatus.SUCCEEDED
            if has_sufficient
            else AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
        )
        if self.result_status is not expected_status:
            raise EstimatorInvariantError("result status does not match per-meter confidence")

    def to_dict(self) -> dict[str, object]:
        return {
            "schema_version": SCHEMA_VERSION,
            "request_id": self.request_id,
            "analysis_type": ANALYSIS_TYPE,
            "result_status": self.result_status.value,
            "dataset_configuration": self.dataset_configuration.to_dict(),
            "generated_at": self.generated_at,
            "estimator_method_version": self.estimator_method_version,
            "quota_capacity_estimates": [
                estimate.to_dict() for estimate in self.quota_capacity_estimates
            ],
            "warnings": [reason.value for reason in self.warnings],
        }

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), separators=(",", ":"), ensure_ascii=False)


def _mapped_reasons(candidate_set: MeterCandidateSet) -> set[ReasonCode]:
    reasons: set[ReasonCode] = set()
    for decision in candidate_set.decisions:
        exclusion_reasons = set(decision.exclusion_reasons)
        if CandidateExclusionReason.RESET_CROSSED in exclusion_reasons:
            reasons.add(ReasonCode.QUOTA_RESET_CROSSED)
        if {
            CandidateExclusionReason.METER_NOT_VALID,
            CandidateExclusionReason.DELTA_UNAVAILABLE,
        } & exclusion_reasons:
            reasons.add(ReasonCode.METER_UNAVAILABLE)
        if CandidateExclusionReason.METER_QUALITY_INELIGIBLE in exclusion_reasons:
            reasons.add(ReasonCode.METER_UNSTABLE)
        if {
            CandidateExclusionReason.TOKEN_NOT_VALID,
            CandidateExclusionReason.TOKEN_QUALITY_INELIGIBLE,
            CandidateExclusionReason.RAW_TOTAL_UNAVAILABLE,
        } & exclusion_reasons:
            reasons.add(ReasonCode.TELEMETRY_INCOMPLETE)
        if {
            CandidateExclusionReason.ZERO_RAW_TOTAL,
            CandidateExclusionReason.ZERO_QUOTA_DELTA,
            CandidateExclusionReason.NONFINITE_CANDIDATE,
        } & exclusion_reasons:
            reasons.add(ReasonCode.UNKNOWN_REASON)
    return reasons


def _quality_reasons(
    candidate_set: MeterCandidateSet,
    used_observation_ids: tuple[str, ...],
) -> set[ReasonCode]:
    candidates_by_id: dict[str, list[QualityGrade]] = {}
    for candidate in candidate_set.candidates:
        candidates_by_id.setdefault(candidate.observation_id, []).append(candidate.quality)

    qualities: list[QualityGrade] = []
    for observation_id in used_observation_ids:
        matches = candidates_by_id.get(observation_id, [])
        if len(matches) != 1:
            raise EstimatorInvariantError(
                "P019 used observation IDs do not map exactly once to P018"
            )
        qualities.append(matches[0])
    if QualityGrade.C in qualities:
        return {ReasonCode.CONCURRENT_USAGE_POSSIBLE}
    return set()


def _confidence_and_reasons(
    candidate_set: MeterCandidateSet,
    aggregation,
) -> tuple[Confidence, set[ReasonCode]]:
    reasons = _mapped_reasons(candidate_set)
    reasons.update(_quality_reasons(candidate_set, aggregation.used_observation_ids))

    if aggregation.status is AggregationStatus.INSUFFICIENT_SAMPLES:
        if aggregation.accounting.used_observations < 2 and not reasons:
            reasons.add(ReasonCode.UNKNOWN_REASON)
        return Confidence.INSUFFICIENT, reasons
    if aggregation.status is not AggregationStatus.SUFFICIENT:
        raise EstimatorInvariantError("P019 returned an unknown aggregation status")

    values = (
        aggregation.weighted_median_raw_tokens,
        aggregation.p25_raw_tokens,
        aggregation.p50_raw_tokens,
        aggregation.p75_raw_tokens,
    )
    if any(value is None for value in values):
        raise EstimatorInvariantError("P019 sufficient aggregation omitted a statistic")
    weighted_median, p25, p50, p75 = values
    assert weighted_median is not None
    assert p25 is not None
    assert p50 is not None
    assert p75 is not None
    if not all(math.isfinite(value) for value in values) or p50 <= 0 or p25 <= 0:
        raise EstimatorInvariantError("P019 sufficient statistics must be finite and positive")
    if not p25 <= p50 <= p75:
        raise EstimatorInvariantError("P019 percentiles are not ordered")
    relative_iqr = (p75 - p25) / p50
    if not math.isfinite(relative_iqr) or relative_iqr < 0:
        raise EstimatorInvariantError("relative IQR must be finite and non-negative")
    valid_observations = aggregation.accounting.valid_observations
    outlier_rate = (
        aggregation.accounting.outliers_removed / valid_observations
        if valid_observations > 0
        else 0.0
    )
    if not math.isfinite(outlier_rate) or outlier_rate < 0:
        raise EstimatorInvariantError("outlier rate must be finite and non-negative")

    qualities = _used_qualities(candidate_set, aggregation.used_observation_ids)
    c_count = sum(quality is QualityGrade.C for quality in qualities)
    strong_quality_count = sum(quality in (QualityGrade.A, QualityGrade.B) for quality in qualities)
    used_observations = aggregation.accounting.used_observations
    if used_observations >= 8 and relative_iqr <= 0.20 and c_count == 0 and outlier_rate <= 0.20:
        return Confidence.HIGH, reasons
    if (
        used_observations >= 4
        and relative_iqr <= 0.50
        and 2 * strong_quality_count >= used_observations
        and outlier_rate <= 0.35
    ):
        return Confidence.MEDIUM, reasons
    return Confidence.LOW, reasons


def _used_qualities(
    candidate_set: MeterCandidateSet,
    used_observation_ids: tuple[str, ...],
) -> tuple[QualityGrade, ...]:
    candidates_by_id: dict[str, list[QualityGrade]] = {}
    for candidate in candidate_set.candidates:
        candidates_by_id.setdefault(candidate.observation_id, []).append(candidate.quality)
    qualities: list[QualityGrade] = []
    for observation_id in used_observation_ids:
        matches = candidates_by_id.get(observation_id, [])
        if len(matches) != 1:
            raise EstimatorInvariantError(
                "P019 used observation IDs do not map exactly once to P018"
            )
        qualities.append(matches[0])
    return tuple(qualities)


def _build_estimate(
    candidate_set: MeterCandidateSet,
    current_position: CurrentQuotaPosition | None,
) -> QuotaCapacityEstimate:
    try:
        aggregation = aggregate_capacity_candidates(candidate_set)
    except RobustAggregationError as exc:
        raise EstimatorInvariantError("P019 aggregation could not be finalized") from exc

    confidence, reasons = _confidence_and_reasons(candidate_set, aggregation)
    if confidence is Confidence.INSUFFICIENT:
        return QuotaCapacityEstimate(
            meter_type=candidate_set.meter_type,
            confidence=confidence,
            sample_accounting=aggregation.accounting,
            reason_codes=tuple(reasons),
            estimated_full_capacity_raw_tokens=None,
            estimated_used_raw_tokens=None,
            estimated_remaining_raw_tokens=None,
            p25_raw_tokens=None,
            p50_raw_tokens=None,
            p75_raw_tokens=None,
        )

    if current_position is None:
        raise CurrentQuotaPositionRequiredError(
            f"current quota position required for meter {candidate_set.meter_type.value}"
        )
    if current_position.meter_type is not candidate_set.meter_type:
        raise ResultInputMismatchError("current quota position meter does not match target meter")
    full_capacity = aggregation.weighted_median_raw_tokens
    p25 = aggregation.p25_raw_tokens
    p50 = aggregation.p50_raw_tokens
    p75 = aggregation.p75_raw_tokens
    if full_capacity is None or not math.isfinite(full_capacity) or full_capacity <= 0:
        raise EstimatorInvariantError("final full capacity must be finite and positive")
    if p25 is None or p50 is None or p75 is None:
        raise EstimatorInvariantError("sufficient P019 percentiles are required")
    used = full_capacity * current_position.used_percent / 100
    remaining = full_capacity - used
    full_capacity = _finite_nonnegative(full_capacity, "final full capacity")
    used = _finite_nonnegative(used, "estimated used capacity")
    remaining = _finite_nonnegative(remaining, "estimated remaining capacity")
    if not math.isclose(used + remaining, full_capacity, rel_tol=1e-12, abs_tol=0.0):
        raise EstimatorInvariantError("used and remaining estimates do not sum to full capacity")
    return QuotaCapacityEstimate(
        meter_type=candidate_set.meter_type,
        confidence=confidence,
        sample_accounting=aggregation.accounting,
        reason_codes=tuple(reasons),
        estimated_full_capacity_raw_tokens=full_capacity,
        estimated_used_raw_tokens=used,
        estimated_remaining_raw_tokens=remaining,
        p25_raw_tokens=_finite_nonnegative(p25, "P25 capacity"),
        p50_raw_tokens=_finite_nonnegative(p50, "P50 capacity"),
        p75_raw_tokens=_finite_nonnegative(p75, "P75 capacity"),
    )


def build_current_capacity_result(
    *,
    context: CurrentCapacityResultContext,
    candidate_sets: Mapping[QuotaMeterType, MeterCandidateSet],
    current_positions: Mapping[QuotaMeterType, CurrentQuotaPosition],
) -> AnalyticsResultV1:
    """Build a deterministic current-capacity AnalyticsResult v1."""

    target_meters = set(context.target_quota_windows)
    candidate_keys = set(candidate_sets)
    position_keys = set(current_positions)
    if any(not isinstance(key, QuotaMeterType) for key in candidate_keys | position_keys):
        raise ResultInputMismatchError("result input mappings must use v1 meter types")
    if candidate_keys != target_meters:
        raise ResultInputMismatchError("candidate sets must exactly cover target quota windows")
    if not position_keys <= target_meters:
        raise ResultInputMismatchError("current positions must target requested quota windows")

    estimates: list[QuotaCapacityEstimate] = []
    for meter_type in context.target_quota_windows:
        candidate_set = candidate_sets[meter_type]
        if not isinstance(candidate_set, MeterCandidateSet):
            raise ResultInputMismatchError("candidate set input must use the P018 typed model")
        if candidate_set.meter_type is not meter_type:
            raise ResultInputMismatchError("candidate set meter does not match its mapping key")
        if candidate_set.configuration != context.dataset_configuration.configuration_key:
            raise ResultConfigurationMismatchError(
                "candidate configuration does not match result configuration for "
                f"{meter_type.value}"
            )
        position = current_positions.get(meter_type)
        if position is not None:
            if not isinstance(position, CurrentQuotaPosition):
                raise ResultInputMismatchError("current position input must use the typed model")
            if position.meter_type is not meter_type:
                raise ResultInputMismatchError(
                    "current quota position meter does not match its key"
                )
        estimates.append(_build_estimate(candidate_set, position))

    reasons = {reason for estimate in estimates for reason in estimate.reason_codes}
    if not estimates:
        reasons.add(ReasonCode.UNKNOWN_REASON)
    status = (
        AnalyticsResultStatus.SUCCEEDED
        if any(estimate.confidence is not Confidence.INSUFFICIENT for estimate in estimates)
        else AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
    )
    return AnalyticsResultV1(
        request_id=context.request_id,
        result_status=status,
        dataset_configuration=context.dataset_configuration,
        generated_at=context.generated_at,
        estimator_method_version=ESTIMATOR_METHOD_VERSION,
        quota_capacity_estimates=tuple(estimates),
        warnings=_ordered_reasons(reasons),
    )


__all__ = [
    "ANALYSIS_TYPE",
    "AnalyticsResultStatus",
    "AnalyticsResultV1",
    "ConfigurationProvenance",
    "Confidence",
    "CurrentCapacityResultContext",
    "CurrentQuotaPosition",
    "EstimatorInvariantError",
    "FinalEstimateError",
    "METRIC_KIND",
    "PROVIDER_AUTHORITY",
    "QuotaCapacityEstimate",
    "ReasonCode",
    "ResultConfigurationIdentity",
    "ResultConfigurationValue",
    "ResultConfigurationMismatchError",
    "ResultInputMismatchError",
    "SCHEMA_VERSION",
    "CurrentQuotaPositionRequiredError",
    "build_current_capacity_result",
]
