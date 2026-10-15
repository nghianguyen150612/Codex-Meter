import json
import math
from dataclasses import replace
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource

from codex_meter.estimation import (
    AnalyticsResultStatus,
    AnalyticsResultV1,
    CandidateDecision,
    CandidateExclusionReason,
    Confidence,
    ConfigurationProvenance,
    CurrentCapacityResultContext,
    CurrentQuotaPosition,
    CurrentQuotaPositionRequiredError,
    EstimatorInvariantError,
    MeterCandidateSet,
    QuotaCapacityEstimate,
    RawCapacityCandidate,
    ReasonCode,
    ResultConfigurationIdentity,
    ResultConfigurationMismatchError,
    ResultConfigurationValue,
    ResultInputMismatchError,
    SampleAccounting,
    build_current_capacity_result,
)
from codex_meter.history import ConfigurationKey, QualityGrade, QuotaMeterType

CONFIGURATION = ConfigurationKey("plus", "gpt-5.6-sol", "high", "standard", None)
OTHER_CONFIGURATION = ConfigurationKey("pro", "gpt-5.6-sol", "high", "standard", None)
GENERATED_AT = "2026-10-01T03:00:01.123456789Z"


def _configuration_identity(
    *,
    plan_provenance: ConfigurationProvenance = ConfigurationProvenance.OBSERVED,
) -> ResultConfigurationIdentity:
    return ResultConfigurationIdentity(
        ResultConfigurationValue("plus", plan_provenance),
        ResultConfigurationValue("gpt-5.6-sol", ConfigurationProvenance.OBSERVED),
        ResultConfigurationValue("high", ConfigurationProvenance.OBSERVED),
        ResultConfigurationValue("standard", ConfigurationProvenance.DERIVED),
        ResultConfigurationValue(None, ConfigurationProvenance.UNAVAILABLE),
    )


def _context(
    *meters: QuotaMeterType,
    configuration: ResultConfigurationIdentity | None = None,
) -> CurrentCapacityResultContext:
    return CurrentCapacityResultContext(
        request_id="request-020",
        dataset_configuration=configuration or _configuration_identity(),
        target_quota_windows=meters,
        generated_at=GENERATED_AT,
    )


def _candidate(
    index: int,
    value: float,
    quality: QualityGrade = QualityGrade.A,
    *,
    meter_type: QuotaMeterType = QuotaMeterType.FIVE_HOUR,
    configuration: ConfigurationKey = CONFIGURATION,
) -> RawCapacityCandidate:
    return RawCapacityCandidate(
        observation_id=f"observation-{index}",
        meter_type=meter_type,
        configuration=configuration,
        quality=quality,
        raw_total=1,
        delta_percentage_points=value / 100,
        raw_tokens_per_percentage_point=value / 100,
        full_capacity_raw_tokens=value,
    )


def _candidate_set(
    values: tuple[float, ...],
    qualities: tuple[QualityGrade, ...] | None = None,
    *,
    meter_type: QuotaMeterType = QuotaMeterType.FIVE_HOUR,
    configuration: ConfigurationKey = CONFIGURATION,
    exclusions: tuple[tuple[CandidateExclusionReason, ...], ...] = (),
) -> MeterCandidateSet:
    qualities = qualities or (QualityGrade.A,) * len(values)
    candidates = tuple(
        _candidate(
            index,
            value,
            quality,
            meter_type=meter_type,
            configuration=configuration,
        )
        for index, (value, quality) in enumerate(zip(values, qualities, strict=True))
    )
    decisions = tuple(
        CandidateDecision(candidate.observation_id, meter_type, candidate, ())
        for candidate in candidates
    )
    decisions += tuple(
        CandidateDecision(
            f"excluded-{index}",
            meter_type,
            None,
            reasons,
        )
        for index, reasons in enumerate(exclusions)
    )
    accounting = SampleAccounting(
        candidate_observations=len(decisions),
        valid_observations=len(candidates),
        excluded_observations=len(exclusions),
        outliers_removed=0,
        used_observations=len(candidates),
    )
    return MeterCandidateSet(meter_type, configuration, decisions, candidates, accounting)


def _result(
    values: tuple[float, ...],
    *,
    used_percent: float = 24,
    qualities: tuple[QualityGrade, ...] | None = None,
    meter_type: QuotaMeterType = QuotaMeterType.FIVE_HOUR,
    candidate_configuration: ConfigurationKey = CONFIGURATION,
    result_configuration: ResultConfigurationIdentity | None = None,
    exclusions: tuple[tuple[CandidateExclusionReason, ...], ...] = (),
):
    candidate_set = _candidate_set(
        values,
        qualities,
        meter_type=meter_type,
        configuration=candidate_configuration,
        exclusions=exclusions,
    )
    context = _context(meter_type, configuration=result_configuration)
    return build_current_capacity_result(
        context=context,
        candidate_sets={meter_type: candidate_set},
        current_positions={meter_type: CurrentQuotaPosition(meter_type, used_percent)},
    )


def test_weighted_center_is_primary_and_p50_remains_unweighted() -> None:
    result = _result(
        (10.0, 20.0, 30.0, 40.0),
        qualities=(QualityGrade.A, QualityGrade.A, QualityGrade.B, QualityGrade.C),
    )
    estimate = result.quota_capacity_estimates[0]

    assert estimate.estimated_full_capacity_raw_tokens == 20.0
    assert estimate.p50_raw_tokens == 25.0
    assert estimate.estimated_used_raw_tokens == 4.8
    assert estimate.estimated_remaining_raw_tokens == 15.2


def test_required_arithmetic_and_edge_positions() -> None:
    for used_percent, expected_used, expected_remaining in (
        (24, 5_400_000.0, 17_100_000.0),
        (0, 0.0, 22_500_000.0),
        (100, 22_500_000.0, 0.0),
    ):
        result = _result((22_500_000.0, 22_500_000.0), used_percent=used_percent)
        estimate = result.quota_capacity_estimates[0]
        assert estimate.estimated_full_capacity_raw_tokens == 22_500_000.0
        assert estimate.estimated_used_raw_tokens == expected_used
        assert estimate.estimated_remaining_raw_tokens == expected_remaining
        assert (
            estimate.estimated_used_raw_tokens + estimate.estimated_remaining_raw_tokens
            == 22_500_000.0
        )


def test_high_confidence_inclusive_relative_iqr_boundary() -> None:
    result = _result((80.0, 90.0, 100.0, 100.0, 100.0, 115.0, 125.0, 140.0))

    assert result.quota_capacity_estimates[0].confidence is Confidence.HIGH


def test_high_confidence_inclusive_outlier_boundary() -> None:
    result = _result((100.0,) * 8 + (1_000.0, 1_100.0))

    estimate = result.quota_capacity_estimates[0]
    assert estimate.confidence is Confidence.HIGH
    assert estimate.sample_accounting.outliers_removed == 2


def test_medium_confidence_inclusive_boundaries() -> None:
    result = _result((50.0, 100.0, 100.0, 150.0, 200.0))
    estimate = result.quota_capacity_estimates[0]

    assert estimate.confidence is Confidence.MEDIUM

    result = _result((100.0,) * 13 + (1_000.0,) * 7)
    estimate = result.quota_capacity_estimates[0]
    assert estimate.confidence is Confidence.MEDIUM
    assert estimate.sample_accounting.outliers_removed == 7


def test_low_confidence_by_outlier_rate() -> None:
    result = _result((100.0,) * 12 + (1_000.0,) * 8)
    estimate = result.quota_capacity_estimates[0]

    assert estimate.confidence is Confidence.LOW
    assert estimate.sample_accounting.outliers_removed == 8


@pytest.mark.parametrize(
    "values, qualities",
    [
        ((20.0, 20.0), (QualityGrade.A, QualityGrade.A)),
        ((20.0, 20.0, 20.0), (QualityGrade.A, QualityGrade.A, QualityGrade.A)),
        ((20.0, 20.0, 20.0, 20.0), (QualityGrade.C,) * 4),
        ((20.0, 100.0, 200.0, 300.0), (QualityGrade.A,) * 4),
    ],
)
def test_low_confidence_policies(values, qualities) -> None:
    result = _result(values, qualities=qualities)

    assert result.quota_capacity_estimates[0].confidence is Confidence.LOW


def test_insufficient_result_omits_capacity_numbers_and_explains_shortage() -> None:
    candidate_set = _candidate_set((20.0,))
    result = build_current_capacity_result(
        context=_context(QuotaMeterType.FIVE_HOUR),
        candidate_sets={QuotaMeterType.FIVE_HOUR: candidate_set},
        current_positions={},
    )
    estimate = result.quota_capacity_estimates[0]

    assert result.result_status is AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
    assert estimate.confidence is Confidence.INSUFFICIENT
    assert estimate.reason_codes == (ReasonCode.UNKNOWN_REASON,)
    serialized = estimate.to_dict()
    assert "estimated_full_capacity_raw_tokens" not in serialized
    assert "capacity_percentiles_raw_tokens" not in serialized


def test_missing_position_only_fails_for_sufficient_evidence() -> None:
    with pytest.raises(CurrentQuotaPositionRequiredError):
        build_current_capacity_result(
            context=_context(QuotaMeterType.FIVE_HOUR),
            candidate_sets={QuotaMeterType.FIVE_HOUR: _candidate_set((20.0, 20.0))},
            current_positions={},
        )

    result = build_current_capacity_result(
        context=_context(QuotaMeterType.FIVE_HOUR),
        candidate_sets={QuotaMeterType.FIVE_HOUR: _candidate_set((20.0,))},
        current_positions={},
    )
    assert result.result_status is AnalyticsResultStatus.INSUFFICIENT_EVIDENCE


@pytest.mark.parametrize("used_percent", (-1.0, 101.0, math.nan, math.inf))
def test_current_position_rejects_invalid_percentages(used_percent: float) -> None:
    with pytest.raises(ValueError):
        CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, used_percent)


def test_current_position_meter_identity_is_strict() -> None:
    with pytest.raises(ResultInputMismatchError):
        build_current_capacity_result(
            context=_context(QuotaMeterType.FIVE_HOUR),
            candidate_sets={QuotaMeterType.FIVE_HOUR: _candidate_set((20.0, 20.0))},
            current_positions={
                QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.WEEKLY, 20)
            },
        )


def test_configuration_provenance_is_preserved_without_inference() -> None:
    identity = _configuration_identity(plan_provenance=ConfigurationProvenance.DERIVED)
    result = _result((20.0, 20.0), result_configuration=identity)
    serialized = result.to_dict()["dataset_configuration"]

    assert serialized["plan"] == {
        "availability": "available",
        "value": "plus",
        "provenance": "derived",
    }
    assert serialized["speed_mode"]["provenance"] == "derived"
    assert serialized["codex_version"] == {
        "availability": "unavailable",
        "provenance": "unavailable",
    }
    assert "value" not in serialized["codex_version"]


def test_configuration_mismatch_fails_closed() -> None:
    with pytest.raises(ResultConfigurationMismatchError):
        _result((20.0, 20.0), candidate_configuration=OTHER_CONFIGURATION)


def test_reason_mapping_has_fixed_order_and_deduplication() -> None:
    exclusions = (
        (CandidateExclusionReason.ZERO_QUOTA_DELTA,),
        (CandidateExclusionReason.TOKEN_NOT_VALID,),
        (CandidateExclusionReason.RESET_CROSSED, CandidateExclusionReason.METER_NOT_VALID),
        (CandidateExclusionReason.METER_QUALITY_INELIGIBLE,),
    )
    result = _result((20.0, 20.0), exclusions=exclusions)

    assert result.quota_capacity_estimates[0].reason_codes == (
        ReasonCode.QUOTA_RESET_CROSSED,
        ReasonCode.METER_UNAVAILABLE,
        ReasonCode.METER_UNSTABLE,
        ReasonCode.TELEMETRY_INCOMPLETE,
        ReasonCode.UNKNOWN_REASON,
    )
    assert result.warnings == result.quota_capacity_estimates[0].reason_codes


def test_mixed_meter_status_and_current_position_isolation() -> None:
    five_hour = _candidate_set((100.0, 100.0, 100.0, 100.0))
    weekly = _candidate_set((), meter_type=QuotaMeterType.WEEKLY)
    result = build_current_capacity_result(
        context=_context(QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
        candidate_sets={QuotaMeterType.FIVE_HOUR: five_hour, QuotaMeterType.WEEKLY: weekly},
        current_positions={
            QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, 20),
        },
    )

    assert result.result_status is AnalyticsResultStatus.SUCCEEDED
    assert [estimate.confidence for estimate in result.quota_capacity_estimates] == [
        Confidence.MEDIUM,
        Confidence.INSUFFICIENT,
    ]
    assert result.quota_capacity_estimates[1].to_dict().keys() >= {
        "meter_type",
        "sample_accounting",
    }


def test_empty_targets_are_insufficient_with_unknown_warning() -> None:
    result = build_current_capacity_result(
        context=_context(),
        candidate_sets={},
        current_positions={},
    )

    assert result.result_status is AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
    assert result.quota_capacity_estimates == ()
    assert result.warnings == (ReasonCode.UNKNOWN_REASON,)


def test_replay_and_current_position_change_only_change_arithmetic() -> None:
    candidate_set = _candidate_set((100.0, 120.0, 110.0, 105.0))
    context = _context(QuotaMeterType.FIVE_HOUR)
    first = build_current_capacity_result(
        context=context,
        candidate_sets={QuotaMeterType.FIVE_HOUR: candidate_set},
        current_positions={
            QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, 20)
        },
    )
    replay = build_current_capacity_result(
        context=context,
        candidate_sets={QuotaMeterType.FIVE_HOUR: candidate_set},
        current_positions={
            QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, 20)
        },
    )
    changed = build_current_capacity_result(
        context=context,
        candidate_sets={QuotaMeterType.FIVE_HOUR: candidate_set},
        current_positions={
            QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, 60)
        },
    )

    assert first == replay
    assert first.to_json() == replay.to_json()
    assert (
        first.quota_capacity_estimates[0].confidence
        is changed.quota_capacity_estimates[0].confidence
    )
    assert (
        first.quota_capacity_estimates[0].estimated_full_capacity_raw_tokens
        == changed.quota_capacity_estimates[0].estimated_full_capacity_raw_tokens
    )
    assert (
        first.quota_capacity_estimates[0].p50_raw_tokens
        == changed.quota_capacity_estimates[0].p50_raw_tokens
    )
    assert (
        first.quota_capacity_estimates[0].sample_accounting
        == changed.quota_capacity_estimates[0].sample_accounting
    )
    assert (
        first.quota_capacity_estimates[0].estimated_used_raw_tokens
        != changed.quota_capacity_estimates[0].estimated_used_raw_tokens
    )


def test_result_schema_validation_for_success_mixed_and_insufficient() -> None:
    root = Path(__file__).parents[2]
    schema_dir = root / "schemas" / "v1"
    schemas = {
        path.name: json.loads(path.read_text(encoding="utf-8"))
        for path in schema_dir.glob("*.schema.json")
    }
    base_uri = schema_dir.resolve().as_uri().rstrip("/") + "/"
    registry = Registry().with_resources(
        [(base_uri + name, Resource.from_contents(schema)) for name, schema in schemas.items()]
    )
    schema = {
        "$id": base_uri + "analytics-result.schema.json",
        **schemas["analytics-result.schema.json"],
    }
    validator = Draft202012Validator(
        schema,
        registry=registry,
        format_checker=FormatChecker(),
    )
    success = _result((100.0, 110.0, 105.0, 100.0)).to_dict()
    both_success = build_current_capacity_result(
        context=_context(QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
        candidate_sets={
            QuotaMeterType.FIVE_HOUR: _candidate_set((100.0, 100.0)),
            QuotaMeterType.WEEKLY: _candidate_set(
                (1_000.0, 1_000.0), meter_type=QuotaMeterType.WEEKLY
            ),
        },
        current_positions={
            QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, 20),
            QuotaMeterType.WEEKLY: CurrentQuotaPosition(QuotaMeterType.WEEKLY, 42),
        },
    ).to_dict()
    mixed = build_current_capacity_result(
        context=_context(QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY),
        candidate_sets={
            QuotaMeterType.FIVE_HOUR: _candidate_set((100.0, 100.0)),
            QuotaMeterType.WEEKLY: _candidate_set((), meter_type=QuotaMeterType.WEEKLY),
        },
        current_positions={
            QuotaMeterType.FIVE_HOUR: CurrentQuotaPosition(QuotaMeterType.FIVE_HOUR, 20)
        },
    ).to_dict()
    insufficient = build_current_capacity_result(
        context=_context(QuotaMeterType.FIVE_HOUR),
        candidate_sets={QuotaMeterType.FIVE_HOUR: _candidate_set((100.0,))},
        current_positions={},
    ).to_dict()

    assert list(validator.iter_errors(success)) == []
    assert list(validator.iter_errors(both_success)) == []
    assert list(validator.iter_errors(mixed)) == []
    assert list(validator.iter_errors(insufficient)) == []


def _direct_estimate() -> QuotaCapacityEstimate:
    return QuotaCapacityEstimate(
        meter_type=QuotaMeterType.FIVE_HOUR,
        confidence=Confidence.LOW,
        sample_accounting=SampleAccounting(2, 2, 0, 0, 2),
        reason_codes=(),
        estimated_full_capacity_raw_tokens=100.0,
        estimated_used_raw_tokens=24.0,
        estimated_remaining_raw_tokens=76.0,
        p25_raw_tokens=80.0,
        p50_raw_tokens=100.0,
        p75_raw_tokens=120.0,
    )


def _direct_result(
    estimates: tuple[QuotaCapacityEstimate, ...],
    warnings: tuple[ReasonCode, ...],
    status: AnalyticsResultStatus = AnalyticsResultStatus.SUCCEEDED,
) -> AnalyticsResultV1:
    return AnalyticsResultV1(
        request_id="request-020",
        result_status=status,
        dataset_configuration=_configuration_identity(),
        generated_at=GENERATED_AT,
        estimator_method_version="1.0.0",
        quota_capacity_estimates=estimates,
        warnings=warnings,
    )


@pytest.mark.parametrize("confidence", (Confidence.LOW, Confidence.MEDIUM, Confidence.HIGH))
def test_direct_sufficient_estimate_and_result_are_valid(confidence: Confidence) -> None:
    estimate = replace(_direct_estimate(), confidence=confidence)
    result = _direct_result((estimate,), ())

    assert result.quota_capacity_estimates == (estimate,)
    assert result.result_status is AnalyticsResultStatus.SUCCEEDED


@pytest.mark.parametrize("full", (50.0, 100.0, 150.0))
def test_direct_weighted_center_is_independent_of_percentiles(full: float) -> None:
    estimate = replace(
        _direct_estimate(),
        estimated_full_capacity_raw_tokens=full,
        estimated_used_raw_tokens=0.0,
        estimated_remaining_raw_tokens=full,
    )

    assert estimate.estimated_full_capacity_raw_tokens == full
    assert estimate.p50_raw_tokens == 100.0


@pytest.mark.parametrize(
    "field, value",
    [
        ("estimated_full_capacity_raw_tokens", 0.0),
        ("estimated_full_capacity_raw_tokens", -1.0),
        ("estimated_used_raw_tokens", 25.0),
        ("estimated_used_raw_tokens", -1.0),
        ("estimated_remaining_raw_tokens", 77.0),
        ("estimated_remaining_raw_tokens", -1.0),
        ("p25_raw_tokens", 101.0),
        ("p75_raw_tokens", 99.0),
    ],
)
def test_direct_estimate_rejects_inconsistent_capacity(field: str, value: float) -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_estimate(), **{field: value})


_NUMERIC_FIELDS = (
    "estimated_full_capacity_raw_tokens",
    "estimated_used_raw_tokens",
    "estimated_remaining_raw_tokens",
    "p25_raw_tokens",
    "p50_raw_tokens",
    "p75_raw_tokens",
)


@pytest.mark.parametrize("field", _NUMERIC_FIELDS)
@pytest.mark.parametrize("value", (True, False, math.nan, math.inf, -math.inf, None, 10**400))
def test_direct_estimate_rejects_invalid_numbers(field: str, value) -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_estimate(), **{field: value})


@pytest.mark.parametrize("field", ("p25_raw_tokens", "p50_raw_tokens", "p75_raw_tokens"))
@pytest.mark.parametrize("value", (0.0, -0.0, -1.0))
def test_direct_estimate_rejects_nonpositive_percentiles(field: str, value: float) -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_estimate(), **{field: value})


@pytest.mark.parametrize("used, remaining", ((-0.0, 100.0), (100.0, -0.0)))
def test_direct_estimate_accepts_edge_usage_and_normalizes_negative_zero(
    used: float, remaining: float
) -> None:
    estimate = replace(
        _direct_estimate(),
        estimated_used_raw_tokens=used,
        estimated_remaining_raw_tokens=remaining,
    )

    assert estimate.estimated_used_raw_tokens == used
    assert estimate.estimated_remaining_raw_tokens == remaining
    assert math.copysign(1.0, estimate.estimated_used_raw_tokens) == 1.0
    assert math.copysign(1.0, estimate.estimated_remaining_raw_tokens) == 1.0


def test_direct_estimate_capacity_sum_uses_relative_tolerance_only() -> None:
    estimate = replace(_direct_estimate(), estimated_remaining_raw_tokens=76.0 + 5e-11)
    assert estimate.estimated_remaining_raw_tokens > 76.0
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_estimate(), estimated_remaining_raw_tokens=76.0 + 2e-10)
    with pytest.raises(EstimatorInvariantError):
        replace(
            _direct_estimate(),
            estimated_full_capacity_raw_tokens=1e-20,
            estimated_used_raw_tokens=0.0,
            estimated_remaining_raw_tokens=0.0,
        )


@pytest.mark.parametrize("samples", (0, 1))
@pytest.mark.parametrize("confidence", (Confidence.LOW, Confidence.MEDIUM, Confidence.HIGH))
def test_direct_sufficient_estimate_requires_two_retained_samples(
    samples: int, confidence: Confidence
) -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(
            _direct_estimate(),
            confidence=confidence,
            sample_accounting=SampleAccounting(samples, samples, 0, 0, samples),
        )


def _direct_insufficient_estimate(samples: int = 1) -> QuotaCapacityEstimate:
    return QuotaCapacityEstimate(
        meter_type=QuotaMeterType.FIVE_HOUR,
        confidence=Confidence.INSUFFICIENT,
        sample_accounting=SampleAccounting(samples, samples, 0, 0, samples),
        reason_codes=(ReasonCode.UNKNOWN_REASON,),
        estimated_full_capacity_raw_tokens=None,
        estimated_used_raw_tokens=None,
        estimated_remaining_raw_tokens=None,
        p25_raw_tokens=None,
        p50_raw_tokens=None,
        p75_raw_tokens=None,
    )


@pytest.mark.parametrize("samples", (0, 1))
def test_direct_insufficient_estimate_is_valid(samples: int) -> None:
    estimate = _direct_insufficient_estimate(samples)
    result = _direct_result(
        (estimate,), (ReasonCode.UNKNOWN_REASON,), AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
    )
    assert result.quota_capacity_estimates == (estimate,)


@pytest.mark.parametrize("samples", (2, 8))
def test_direct_insufficient_estimate_rejects_multiple_retained_samples(samples: int) -> None:
    with pytest.raises(EstimatorInvariantError):
        _direct_insufficient_estimate(samples)


def test_direct_insufficient_estimate_requires_reason() -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_insufficient_estimate(), reason_codes=())


@pytest.mark.parametrize("reason", ("unknown_reason", 1, []))
def test_direct_estimate_rejects_untyped_reasons(reason) -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_insufficient_estimate(), reason_codes=(reason,))


@pytest.mark.parametrize("field", _NUMERIC_FIELDS)
def test_direct_insufficient_estimate_rejects_every_capacity_field(field: str) -> None:
    with pytest.raises(EstimatorInvariantError):
        replace(_direct_insufficient_estimate(), **{field: 1.0})


@pytest.mark.parametrize(
    "warnings",
    (
        (),
        (ReasonCode.UNKNOWN_REASON,),
        (ReasonCode.METER_UNAVAILABLE, ReasonCode.UNKNOWN_REASON),
        (ReasonCode.METER_UNAVAILABLE, ReasonCode.METER_UNAVAILABLE),
    ),
)
def test_direct_result_rejects_missing_extra_or_duplicate_warnings(warnings) -> None:
    estimate = replace(_direct_estimate(), reason_codes=(ReasonCode.METER_UNAVAILABLE,))
    with pytest.raises(EstimatorInvariantError):
        _direct_result((estimate,), warnings)


@pytest.mark.parametrize(
    "warnings", ((), (ReasonCode.METER_UNAVAILABLE,), (ReasonCode.UNKNOWN_REASON,) * 2)
)
def test_direct_empty_result_requires_exact_unknown_warning(warnings) -> None:
    with pytest.raises(EstimatorInvariantError):
        _direct_result((), warnings, AnalyticsResultStatus.INSUFFICIENT_EVIDENCE)


def test_direct_empty_result_with_unknown_reason_is_valid() -> None:
    result = _direct_result(
        (), (ReasonCode.UNKNOWN_REASON,), AnalyticsResultStatus.INSUFFICIENT_EVIDENCE
    )
    assert result.quota_capacity_estimates == ()
    assert result.to_dict()["warnings"] == ["unknown_reason"]


def test_direct_result_normalizes_union_to_common_reason_order() -> None:
    first = replace(
        _direct_estimate(),
        reason_codes=(ReasonCode.UNKNOWN_REASON, ReasonCode.QUOTA_RESET_CROSSED),
    )
    second = replace(
        _direct_insufficient_estimate(),
        meter_type=QuotaMeterType.WEEKLY,
        reason_codes=(ReasonCode.METER_UNAVAILABLE, ReasonCode.UNKNOWN_REASON),
    )
    result = _direct_result(
        (first, second),
        (ReasonCode.UNKNOWN_REASON, ReasonCode.METER_UNAVAILABLE, ReasonCode.QUOTA_RESET_CROSSED),
    )
    assert first.reason_codes == (ReasonCode.QUOTA_RESET_CROSSED, ReasonCode.UNKNOWN_REASON)
    assert result.warnings == (
        ReasonCode.QUOTA_RESET_CROSSED,
        ReasonCode.METER_UNAVAILABLE,
        ReasonCode.UNKNOWN_REASON,
    )
    assert result.to_dict()["warnings"] == [
        "quota_reset_crossed",
        "meter_unavailable",
        "unknown_reason",
    ]


def test_direct_result_preserves_unique_meter_and_status_rules() -> None:
    estimate = _direct_estimate()
    with pytest.raises(ResultInputMismatchError):
        _direct_result((estimate, estimate), ())
    with pytest.raises(EstimatorInvariantError):
        _direct_result((estimate,), (), AnalyticsResultStatus.INSUFFICIENT_EVIDENCE)
    with pytest.raises(EstimatorInvariantError):
        _direct_result((), (ReasonCode.UNKNOWN_REASON,), AnalyticsResultStatus.SUCCEEDED)
    with pytest.raises(EstimatorInvariantError):
        _direct_result((estimate,), (), AnalyticsResultStatus.FAILED)


def test_builder_serialization_remains_compatible() -> None:
    result = _result((100.0, 100.0))
    expected = {
        "schema_version": "1.0.0",
        "request_id": "request-020",
        "analysis_type": "current_capacity_estimate",
        "result_status": "succeeded",
        "dataset_configuration": _configuration_identity().to_dict(),
        "generated_at": GENERATED_AT,
        "estimator_method_version": "1.0.0",
        "quota_capacity_estimates": [
            {
                "meter_type": "five_hour",
                "metric_kind": "estimated_raw_tokens",
                "provider_authority": "codex_meter_empirical_estimate",
                "confidence": "low",
                "sample_accounting": {
                    "candidate_observations": 2,
                    "valid_observations": 2,
                    "excluded_observations": 0,
                    "outliers_removed": 0,
                    "used_observations": 2,
                },
                "reason_codes": [],
                "estimated_full_capacity_raw_tokens": 100.0,
                "estimated_used_raw_tokens": 24.0,
                "estimated_remaining_raw_tokens": 76.0,
                "capacity_percentiles_raw_tokens": {"p25": 100.0, "p50": 100.0, "p75": 100.0},
            }
        ],
        "warnings": [],
    }
    assert result.to_dict() == expected
    assert result.to_json() == json.dumps(expected, allow_nan=False, separators=(",", ":"))
