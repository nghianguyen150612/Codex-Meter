"""Immutable analytics-side projections of stored observations."""

from dataclasses import dataclass
from enum import StrEnum


class ObservationLifecycle(StrEnum):
    DETECTED = "detected"
    ACTIVE = "active"
    TASK_ENDED = "task_ended"
    AWAITING_METER = "awaiting_meter"
    RECONCILING = "reconciling"
    FINALIZED = "finalized"
    INCOMPLETE = "incomplete"
    INVALID = "invalid"


class EvidenceValidity(StrEnum):
    VALID = "valid"
    INCOMPLETE = "incomplete"
    INVALID = "invalid"
    UNAVAILABLE = "unavailable"


class QualityGrade(StrEnum):
    A = "A"
    B = "B"
    C = "C"
    D = "D"
    X = "X"

    @property
    def severity(self) -> int:
        return ("A", "B", "C", "D", "X").index(self.value)


class ResetStatus(StrEnum):
    NOT_DETECTED = "not_detected"
    DETECTED = "detected"
    UNAVAILABLE = "unavailable"


class QuotaMeterType(StrEnum):
    FIVE_HOUR = "five_hour"
    WEEKLY = "weekly"


@dataclass(frozen=True, slots=True)
class ConfigurationKey:
    plan: str | None
    model: str | None
    reasoning_level: str | None
    speed_mode: str | None
    codex_version: str | None


@dataclass(frozen=True, slots=True)
class QuotaEvidenceRecord:
    meter_type: QuotaMeterType
    validity: EvidenceValidity
    quality: QualityGrade
    delta_percentage_points: float | None
    reset_status: ResetStatus


@dataclass(frozen=True, slots=True)
class ObservationRecord:
    observation_id: str
    lifecycle: ObservationLifecycle
    started_at: str | None
    ended_at: str | None
    finalized_at: str | None
    configuration: ConfigurationKey
    summary_quality: QualityGrade
    token_validity: EvidenceValidity
    token_quality: QualityGrade
    raw_total: int | None
    five_hour: QuotaEvidenceRecord
    weekly: QuotaEvidenceRecord

    def quota(self, meter_type: QuotaMeterType) -> QuotaEvidenceRecord:
        if meter_type is QuotaMeterType.FIVE_HOUR:
            return self.five_hour
        if meter_type is QuotaMeterType.WEEKLY:
            return self.weekly
        raise ValueError(f"unsupported quota meter type: {meter_type!r}")


@dataclass(frozen=True, slots=True)
class ObservationSelector:
    configuration: ConfigurationKey | None = None
    observed_from: str | None = None
    observed_through: str | None = None
    maximum_observations: int | None = None


@dataclass(frozen=True, slots=True)
class ObservationDataset:
    observations: tuple[ObservationRecord, ...]
    migration_version: int = 4
    selected_configuration: ConfigurationKey | None = None

    @property
    def loaded_rows(self) -> int:
        return len(self.observations)


__all__ = [
    "ConfigurationKey",
    "EvidenceValidity",
    "ObservationDataset",
    "ObservationLifecycle",
    "ObservationRecord",
    "ObservationSelector",
    "QualityGrade",
    "QuotaEvidenceRecord",
    "QuotaMeterType",
    "ResetStatus",
]
