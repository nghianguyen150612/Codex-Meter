"""Internal P026 evidence and report models; no authenticated ingestion protocol."""

import math
import re
from dataclasses import dataclass, fields
from enum import StrEnum

from codex_meter.estimation import CandidateDecision, Confidence, ReasonCode, SampleAccounting
from codex_meter.history import (
    ConfigurationKey,
    QueryValidationError,
    QuotaMeterType,
    canonical_time_key,
)

BENCHMARK_ANALYSIS_METHOD_VERSION = "1.0.0"
MAXIMUM_BENCHMARK_INPUT_OBSERVATIONS = 100_000


class BenchmarkAnalysisError(ValueError):
    """Privacy-safe invalid input or impossible analysis."""


class BenchmarkDatasetError(BenchmarkAnalysisError):
    """Malformed or incompatible evidence projection."""


class BenchmarkPolicyError(BenchmarkAnalysisError):
    """Unsupported analysis policy."""


class BenchmarkNumericalError(BenchmarkAnalysisError):
    """Unable to produce finite statistics through P018–P020."""


class BenchmarkEvidenceProvenance(StrEnum):
    SYNTHETIC_FIXTURE = "synthetic_fixture"
    CALLER_SUPPLIED_UNVERIFIED = "caller_supplied_unverified"


class BenchmarkCohortKind(StrEnum):
    BENCHMARK = "benchmark"
    OBSERVATIONAL_REFERENCE = "observational_reference"


class BenchmarkMeasurementStatus(StrEnum):
    COMPLETE = "complete"
    INCOMPLETE = "incomplete"
    INVALID = "invalid"


class BenchmarkIsolationStatus(StrEnum):
    """Supplied claims only, including VERIFIED_ISOLATED; never authentication."""

    VERIFIED_ISOLATED = "verified_isolated"
    POSSIBLE_CONCURRENT_USAGE = "possible_concurrent_usage"
    UNKNOWN = "unknown"


class BenchmarkCalibrationStatus(StrEnum):
    INSUFFICIENT_EVIDENCE = "insufficient_evidence"
    COMPARISON_UNAVAILABLE = "comparison_unavailable"
    DESCRIPTIVELY_CONSISTENT = "descriptively_consistent"
    MATERIAL_DIFFERENCE_OVERLAPPING = "material_difference_overlapping"
    MATERIAL_DIFFERENCE_SEPARATED = "material_difference_separated"


class BenchmarkAnalysisWarning(StrEnum):
    SYNTHETIC_EVIDENCE = "synthetic_evidence"
    UNVERIFIED_PROVENANCE = "unverified_provenance"
    NO_TRUSTED_PRODUCTION_EXPORT = "no_trusted_production_export"
    UNRESOLVED_LINKS = "unresolved_links"
    UNLINKED_OBSERVATIONS = "unlinked_observations"
    MEASUREMENT_EXCLUDED = "measurement_excluded"
    UNKNOWN_ISOLATION = "unknown_isolation"
    POSSIBLE_CONCURRENT_USAGE = "possible_concurrent_usage"
    SEPARATE_EXPERIMENTS = "separate_experiments"
    UNKNOWN_CONFIGURATION = "unknown_configuration"
    TEMPORAL_EVIDENCE_INCOMPLETE = "temporal_evidence_incomplete"
    REGIME_BOUNDARY = "regime_boundary"
    NO_REFERENCE = "no_reference"
    INSUFFICIENT_SAMPLES = "insufficient_samples"
    LOW_CAPACITY_CONFIDENCE = "low_capacity_confidence"
    NONFINITE_COMPARISON = "nonfinite_comparison"


class BenchmarkRecommendation(StrEnum):
    COLLECT_MORE_SAMPLES = "collect_more_samples"
    INVESTIGATE_METER_STABILITY = "investigate_meter_stability"
    INVESTIGATE_WORKLOAD_DIFFERENCE = "investigate_workload_difference"
    INVESTIGATE_POSSIBLE_REGIME_SHIFT = "investigate_possible_regime_shift"
    REVIEW_ESTIMATOR_CALIBRATION = "review_estimator_calibration"


def _identifier(value: str, *, observation: bool = False) -> None:
    pattern = r"obs:[0-9a-f]{64}" if observation else r"[A-Za-z][A-Za-z0-9_-]{0,63}"
    if not isinstance(value, str) or re.fullmatch(pattern, value) is None:
        raise BenchmarkDatasetError("invalid opaque benchmark or Observation identifier")


def _configuration(value: ConfigurationKey) -> None:
    if not isinstance(value, ConfigurationKey) or any(
        item is not None and (not isinstance(item, str) or not item or len(item) > 128)
        for field in fields(ConfigurationKey)
        for item in (getattr(value, field.name),)
    ):
        raise BenchmarkDatasetError("configuration requires five typed, bounded dimensions")


def _meters(value: tuple[QuotaMeterType, ...]) -> None:
    if (
        not isinstance(value, tuple)
        or not value
        or any(not isinstance(meter, QuotaMeterType) for meter in value)
        or len(set(value)) != len(value)
    ):
        raise BenchmarkDatasetError(
            "meters require a nonempty unique tuple of supported identities"
        )


def _time(value: str | None) -> str | None:
    if value is None:
        return None
    try:
        return canonical_time_key(value)
    except QueryValidationError:
        raise BenchmarkDatasetError("invalid UTC evidence timestamp") from None


@dataclass(frozen=True, slots=True)
class BenchmarkAnalysisPolicy:
    minimum_used_observations: int = 4
    material_relative_difference: float = 0.20
    maximum_input_observations: int = 500

    def __post_init__(self) -> None:
        for value in (self.minimum_used_observations, self.maximum_input_observations):
            if type(value) is not int or not 4 <= value <= MAXIMUM_BENCHMARK_INPUT_OBSERVATIONS:
                raise BenchmarkPolicyError("sample bounds require integers in [4, 100000]")
        if self.minimum_used_observations > self.maximum_input_observations:
            raise BenchmarkPolicyError("minimum samples exceed input bound")
        value = self.material_relative_difference
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            raise BenchmarkPolicyError("material threshold must be finite and positive")
        try:
            valid = math.isfinite(value) and value > 0
        except OverflowError:
            valid = False
        if not valid:
            raise BenchmarkPolicyError("material threshold must be finite and positive")


@dataclass(frozen=True, slots=True)
class BenchmarkExperimentEvidence:
    """Caller-declared scope; a projection, not a Rust plan or attestation."""

    experiment_id: str
    workload_key: str
    configuration: ConfigurationKey
    planned_run_count: int = 100
    target_meters: tuple[QuotaMeterType, ...] = (QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY)

    def __post_init__(self) -> None:
        _identifier(self.experiment_id)
        _identifier(self.workload_key)
        _configuration(self.configuration)
        _meters(self.target_meters)
        if type(self.planned_run_count) is not int or not 1 <= self.planned_run_count <= 100:
            raise BenchmarkDatasetError("planned run count must be an integer in [1, 100]")


@dataclass(frozen=True, slots=True)
class BenchmarkRunEvidence:
    experiment_id: str
    run_id: str
    repetition_index: int
    workload_key: str
    observation_id: str
    measurement_status: BenchmarkMeasurementStatus
    isolation_status: BenchmarkIsolationStatus
    started_at: str | None = None
    ended_at: str | None = None

    def __post_init__(self) -> None:
        for value in (self.experiment_id, self.run_id, self.workload_key):
            _identifier(value)
        _identifier(self.observation_id, observation=True)
        if type(self.repetition_index) is not int or not 0 <= self.repetition_index < 100:
            raise BenchmarkDatasetError("repetition index must be an integer in [0, 99]")
        if not isinstance(self.measurement_status, BenchmarkMeasurementStatus) or not isinstance(
            self.isolation_status, BenchmarkIsolationStatus
        ):
            raise BenchmarkDatasetError("measurement and isolation require supported enum types")
        start, end = _time(self.started_at), _time(self.ended_at)
        if start is not None and end is not None and start > end:
            raise BenchmarkDatasetError("run timestamps are reversed")


@dataclass(frozen=True, slots=True)
class BenchmarkRegimeBoundary:
    """Caller-supplied detected/suspected temporal boundary; no provider claim."""

    observed_at: str
    meter_type: QuotaMeterType

    def __post_init__(self) -> None:
        if _time(self.observed_at) is None or not isinstance(self.meter_type, QuotaMeterType):
            raise BenchmarkDatasetError("regime boundary requires UTC time and typed meter")


@dataclass(frozen=True, slots=True)
class BenchmarkEvidenceDataset:
    provenance: BenchmarkEvidenceProvenance
    experiments: tuple[BenchmarkExperimentEvidence, ...]
    runs: tuple[BenchmarkRunEvidence, ...]
    regime_boundaries: tuple[BenchmarkRegimeBoundary, ...] = ()

    def __post_init__(self) -> None:
        if not isinstance(self.provenance, BenchmarkEvidenceProvenance):
            raise BenchmarkDatasetError("unsupported benchmark evidence provenance")
        for items, kind in (
            (self.experiments, BenchmarkExperimentEvidence),
            (self.runs, BenchmarkRunEvidence),
            (self.regime_boundaries, BenchmarkRegimeBoundary),
        ):
            if not isinstance(items, tuple) or any(not isinstance(item, kind) for item in items):
                raise BenchmarkDatasetError("benchmark evidence requires immutable typed tuples")


@dataclass(frozen=True, slots=True)
class BenchmarkEvidenceAccounting:
    experiment_count: int
    run_count: int
    resolved_links: int
    unresolved_links: int
    incomplete_runs: int
    invalid_runs: int
    eligible_linked_runs: int
    unknown_isolation: int
    possible_concurrent_isolation: int
    unlinked_observations: int


@dataclass(frozen=True, slots=True)
class BenchmarkCapacitySummary:
    """One experiment/workload/meter distribution; also used for ordinary references."""

    cohort_kind: BenchmarkCohortKind
    experiment_id: str | None
    workload_key: str | None
    meter_type: QuotaMeterType
    configuration: ConfigurationKey | None
    provenance: BenchmarkEvidenceProvenance
    confidence: Confidence
    sample_accounting: SampleAccounting
    weighted_median_raw_tokens: float | None
    p25_raw_tokens: float | None
    p50_raw_tokens: float | None
    p75_raw_tokens: float | None
    mad_raw_tokens: float | None
    relative_iqr: float | None
    outlier_rate: float
    c_quality_used: int
    missing_meter_evidence: int
    reset_crossings: int
    decisions: tuple[CandidateDecision, ...]
    used_observation_ids: tuple[str, ...]
    outlier_observation_ids: tuple[str, ...]
    estimator_reason_codes: tuple[ReasonCode, ...]
    observed_from: str | None
    observed_through: str | None
    observations_without_time: int
    spans_regime_boundary: bool
    warnings: tuple[BenchmarkAnalysisWarning, ...]


@dataclass(frozen=True, slots=True)
class BenchmarkCalibrationComparison:
    benchmark: BenchmarkCapacitySummary
    reference: BenchmarkCapacitySummary | None
    provenance: BenchmarkEvidenceProvenance
    status: BenchmarkCalibrationStatus
    absolute_difference_raw_tokens: float | None
    relative_difference: float | None
    relative_difference_percent: float | None
    benchmark_to_reference_ratio: float | None
    iqr_overlap: bool | None
    warnings: tuple[BenchmarkAnalysisWarning, ...]
    recommendations: tuple[BenchmarkRecommendation, ...]


@dataclass(frozen=True, slots=True)
class BenchmarkCalibrationAnalysis:
    method_version: str
    configuration: ConfigurationKey | None
    provenance: BenchmarkEvidenceProvenance
    benchmark_summaries: tuple[BenchmarkCapacitySummary, ...]
    reference_summaries: tuple[BenchmarkCapacitySummary, ...]
    comparisons: tuple[BenchmarkCalibrationComparison, ...]
    evidence_accounting: BenchmarkEvidenceAccounting
    warnings: tuple[BenchmarkAnalysisWarning, ...]
