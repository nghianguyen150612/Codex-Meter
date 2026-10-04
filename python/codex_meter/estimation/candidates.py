"""Pure derivation of per-observation raw-token capacity candidates."""

import math
from dataclasses import dataclass
from enum import StrEnum

from codex_meter.history import (
    ConfigurationKey,
    EvidenceValidity,
    ObservationDataset,
    ObservationRecord,
    QualityGrade,
    QuotaMeterType,
    ResetStatus,
)


class CandidateExclusionReason(StrEnum):
    """Stable reasons why an observation cannot produce a candidate."""

    TOKEN_NOT_VALID = "token_not_valid"
    TOKEN_QUALITY_INELIGIBLE = "token_quality_ineligible"
    RAW_TOTAL_UNAVAILABLE = "raw_total_unavailable"
    ZERO_RAW_TOTAL = "zero_raw_total"
    METER_NOT_VALID = "meter_not_valid"
    METER_QUALITY_INELIGIBLE = "meter_quality_ineligible"
    DELTA_UNAVAILABLE = "delta_unavailable"
    ZERO_QUOTA_DELTA = "zero_quota_delta"
    RESET_CROSSED = "reset_crossed"
    NONFINITE_CANDIDATE = "nonfinite_candidate"


class CandidateDerivationError(ValueError):
    """Base error for a dataset that cannot be used for candidate derivation."""


class MixedConfigurationDatasetError(CandidateDerivationError):
    """Raised when one dataset contains multiple configuration regimes."""


class DatasetConfigurationMismatchError(CandidateDerivationError):
    """Raised when a selected configuration does not match every observation."""


@dataclass(frozen=True, slots=True)
class RawCapacityCandidate:
    """One empirical full-window raw-token capacity point."""

    observation_id: str
    meter_type: QuotaMeterType
    configuration: ConfigurationKey
    quality: QualityGrade
    raw_total: int
    delta_percentage_points: float
    raw_tokens_per_percentage_point: float
    full_capacity_raw_tokens: float


@dataclass(frozen=True, slots=True)
class CandidateDecision:
    """The inclusion decision and all applicable diagnostic reasons."""

    observation_id: str
    meter_type: QuotaMeterType
    candidate: RawCapacityCandidate | None
    exclusion_reasons: tuple[CandidateExclusionReason, ...]

    def __post_init__(self) -> None:
        if (self.candidate is not None) != (not self.exclusion_reasons):
            raise ValueError("candidate and exclusion reasons must be complementary")


@dataclass(frozen=True, slots=True)
class SampleAccounting:
    """Per-meter row accounting before robust aggregation begins."""

    candidate_observations: int
    valid_observations: int
    excluded_observations: int
    outliers_removed: int
    used_observations: int

    def __post_init__(self) -> None:
        counts = (
            self.candidate_observations,
            self.valid_observations,
            self.excluded_observations,
            self.outliers_removed,
            self.used_observations,
        )
        if any(count < 0 for count in counts):
            raise ValueError("sample accounting counts must be non-negative")
        if self.candidate_observations != self.valid_observations + self.excluded_observations:
            raise ValueError("candidate observations must equal valid plus excluded observations")
        if self.used_observations != self.valid_observations - self.outliers_removed:
            raise ValueError("used observations must equal valid minus removed outliers")


@dataclass(frozen=True, slots=True)
class MeterCandidateSet:
    """Chronologically ordered candidate decisions for one quota meter."""

    meter_type: QuotaMeterType
    configuration: ConfigurationKey | None
    decisions: tuple[CandidateDecision, ...]
    candidates: tuple[RawCapacityCandidate, ...]
    accounting: SampleAccounting

    def excluded_by_reason(self, reason: CandidateExclusionReason) -> int:
        """Count decisions containing ``reason``; rows are counted at most once."""

        return sum(reason in decision.exclusion_reasons for decision in self.decisions)


_ELIGIBLE_QUALITY = frozenset({QualityGrade.A, QualityGrade.B, QualityGrade.C})


def _candidate_quality(token_quality: QualityGrade, meter_quality: QualityGrade) -> QualityGrade:
    return token_quality if token_quality.severity >= meter_quality.severity else meter_quality


def _configuration_for_dataset(dataset: ObservationDataset) -> ConfigurationKey | None:
    selected = dataset.selected_configuration
    if selected is not None and any(
        record.configuration != selected for record in dataset.observations
    ):
        raise DatasetConfigurationMismatchError(
            "an observation does not match the dataset selected configuration"
        )

    configurations = {record.configuration for record in dataset.observations}
    if len(configurations) > 1:
        raise MixedConfigurationDatasetError(
            "candidate derivation cannot pool multiple configurations"
        )
    if selected is not None:
        return selected
    return next(iter(configurations), None)


def _derive_decision(
    record: ObservationRecord,
    meter_type: QuotaMeterType,
    configuration: ConfigurationKey,
) -> CandidateDecision:
    quota = record.quota(meter_type)
    reasons: list[CandidateExclusionReason] = []

    if record.token_validity is not EvidenceValidity.VALID:
        reasons.append(CandidateExclusionReason.TOKEN_NOT_VALID)
    if record.token_quality not in _ELIGIBLE_QUALITY:
        reasons.append(CandidateExclusionReason.TOKEN_QUALITY_INELIGIBLE)
    if record.raw_total is None:
        reasons.append(CandidateExclusionReason.RAW_TOTAL_UNAVAILABLE)
    elif record.raw_total == 0:
        reasons.append(CandidateExclusionReason.ZERO_RAW_TOTAL)

    if quota.validity is not EvidenceValidity.VALID:
        reasons.append(CandidateExclusionReason.METER_NOT_VALID)
    if quota.quality not in _ELIGIBLE_QUALITY:
        reasons.append(CandidateExclusionReason.METER_QUALITY_INELIGIBLE)
    if quota.delta_percentage_points is None:
        reasons.append(CandidateExclusionReason.DELTA_UNAVAILABLE)
    elif quota.delta_percentage_points == 0:
        reasons.append(CandidateExclusionReason.ZERO_QUOTA_DELTA)
    if quota.reset_status is ResetStatus.DETECTED:
        reasons.append(CandidateExclusionReason.RESET_CROSSED)

    if reasons:
        return CandidateDecision(record.observation_id, meter_type, None, tuple(reasons))

    assert record.raw_total is not None
    assert quota.delta_percentage_points is not None
    try:
        raw_tokens_per_percentage_point = record.raw_total / quota.delta_percentage_points
        full_capacity_raw_tokens = raw_tokens_per_percentage_point * 100
    except (OverflowError, ZeroDivisionError):
        raw_tokens_per_percentage_point = math.inf
        full_capacity_raw_tokens = math.inf

    if (
        not math.isfinite(raw_tokens_per_percentage_point)
        or raw_tokens_per_percentage_point <= 0
        or not math.isfinite(full_capacity_raw_tokens)
        or full_capacity_raw_tokens <= 0
    ):
        return CandidateDecision(
            record.observation_id,
            meter_type,
            None,
            (CandidateExclusionReason.NONFINITE_CANDIDATE,),
        )

    candidate = RawCapacityCandidate(
        observation_id=record.observation_id,
        meter_type=meter_type,
        configuration=configuration,
        quality=_candidate_quality(record.token_quality, quota.quality),
        raw_total=record.raw_total,
        delta_percentage_points=quota.delta_percentage_points,
        raw_tokens_per_percentage_point=raw_tokens_per_percentage_point,
        full_capacity_raw_tokens=full_capacity_raw_tokens,
    )
    return CandidateDecision(record.observation_id, meter_type, candidate, ())


def derive_capacity_candidates(
    dataset: ObservationDataset,
    meter_type: QuotaMeterType,
) -> MeterCandidateSet:
    """Derive independent raw-capacity decisions for one meter."""

    configuration = _configuration_for_dataset(dataset)
    if configuration is None and dataset.observations:
        raise CandidateDerivationError("non-empty dataset has no configuration")

    decisions = (
        tuple(
            _derive_decision(record, meter_type, configuration) for record in dataset.observations
        )
        if configuration is not None
        else ()
    )
    candidates = tuple(
        decision.candidate for decision in decisions if decision.candidate is not None
    )
    valid_observations = len(candidates)
    accounting = SampleAccounting(
        candidate_observations=len(dataset.observations),
        valid_observations=valid_observations,
        excluded_observations=len(dataset.observations) - valid_observations,
        outliers_removed=0,
        used_observations=valid_observations,
    )
    return MeterCandidateSet(meter_type, configuration, decisions, candidates, accounting)


__all__ = [
    "CandidateDecision",
    "CandidateDerivationError",
    "CandidateExclusionReason",
    "DatasetConfigurationMismatchError",
    "MeterCandidateSet",
    "MixedConfigurationDatasetError",
    "RawCapacityCandidate",
    "SampleAccounting",
    "derive_capacity_candidates",
]
