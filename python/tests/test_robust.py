import math
from dataclasses import replace

import pytest

from codex_meter.estimation import (
    ESTIMATOR_METHOD_VERSION,
    QUALITY_WEIGHTS_V1,
    AggregationStatus,
    CandidateDecision,
    CandidateExclusionReason,
    CandidateSetInvariantError,
    MeterCandidateSet,
    RawCapacityCandidate,
    RobustCapacityAggregation,
    SampleAccounting,
    aggregate_capacity_candidates,
)
from codex_meter.history import ConfigurationKey, QualityGrade, QuotaMeterType

CONFIGURATION = ConfigurationKey("pro", "gpt-5", "medium", "standard", "1.0")
METER = QuotaMeterType.FIVE_HOUR


def _candidate(
    observation_id: str,
    value: float,
    quality: QualityGrade = QualityGrade.A,
    *,
    meter_type: QuotaMeterType = METER,
    configuration: ConfigurationKey = CONFIGURATION,
) -> RawCapacityCandidate:
    return RawCapacityCandidate(
        observation_id=observation_id,
        meter_type=meter_type,
        configuration=configuration,
        quality=quality,
        raw_total=1,
        delta_percentage_points=1.0,
        raw_tokens_per_percentage_point=value / 100,
        full_capacity_raw_tokens=value,
    )


def _candidate_set(
    values: tuple[float, ...],
    qualities: tuple[QualityGrade, ...] | None = None,
    *,
    excluded: int = 0,
    meter_type: QuotaMeterType = METER,
    configuration: ConfigurationKey | None = CONFIGURATION,
) -> MeterCandidateSet:
    qualities = qualities or (QualityGrade.A,) * len(values)
    candidates = tuple(
        _candidate(
            f"observation-{index}",
            value,
            quality,
            meter_type=meter_type,
            configuration=configuration or CONFIGURATION,
        )
        for index, (value, quality) in enumerate(zip(values, qualities, strict=True))
    )
    decisions = tuple(
        CandidateDecision(candidate.observation_id, meter_type, candidate, ())
        for candidate in candidates
    ) + tuple(
        CandidateDecision(
            f"excluded-{index}",
            meter_type,
            None,
            (CandidateExclusionReason.TOKEN_NOT_VALID,),
        )
        for index in range(excluded)
    )
    accounting = SampleAccounting(
        candidate_observations=len(decisions),
        valid_observations=len(candidates),
        excluded_observations=excluded,
        outliers_removed=0,
        used_observations=len(candidates),
    )
    return MeterCandidateSet(meter_type, configuration, decisions, candidates, accounting)


def test_method_version_and_quality_policy() -> None:
    assert ESTIMATOR_METHOD_VERSION == "1.0.0"
    assert dict(QUALITY_WEIGHTS_V1) == {
        QualityGrade.A: 10,
        QualityGrade.B: 8,
        QualityGrade.C: 3,
    }
    assert set(QUALITY_WEIGHTS_V1) == {QualityGrade.A, QualityGrade.B, QualityGrade.C}


@pytest.mark.parametrize(
    ("values", "expected"),
    [((10.0, 20.0, 30.0), 20.0), ((10.0, 20.0, 30.0, 40.0), 25.0)],
)
def test_ordinary_median_and_post_filter_p50(values: tuple[float, ...], expected: float) -> None:
    result = aggregate_capacity_candidates(_candidate_set(values))

    assert result.pre_filter_median_raw_tokens == expected
    assert result.p50_raw_tokens == expected
    assert result.unweighted_median_raw_tokens == expected


def test_weighted_median_equal_weight_even_is_ordinary_median() -> None:
    result = aggregate_capacity_candidates(
        _candidate_set((10.0, 20.0, 30.0, 40.0), (QualityGrade.A,) * 4)
    )

    assert result.weighted_median_raw_tokens == 25.0


def test_weighted_median_mixed_quality_preserves_candidate_values() -> None:
    result = aggregate_capacity_candidates(
        _candidate_set(
            (10.0, 20.0, 30.0, 40.0),
            (QualityGrade.A, QualityGrade.A, QualityGrade.B, QualityGrade.C),
        )
    )

    assert result.weighted_median_raw_tokens == 20.0
    assert result.p50_raw_tokens == 25.0
    assert result.used_observation_ids == tuple(f"observation-{index}" for index in range(4))


@pytest.mark.parametrize(
    ("values", "qualities", "expected"),
    [
        ((10.0, 20.0), (QualityGrade.A, QualityGrade.A), 15.0),
        ((10.0, 20.0, 30.0), (QualityGrade.A, QualityGrade.B, QualityGrade.C), 20.0),
        ((10.0, 20.0, 20.0, 30.0), (QualityGrade.A,) * 4, 20.0),
    ],
)
def test_weighted_median_boundaries_and_duplicate_values(
    values: tuple[float, ...], qualities: tuple[QualityGrade, ...], expected: float
) -> None:
    assert (
        aggregate_capacity_candidates(_candidate_set(values, qualities)).weighted_median_raw_tokens
        == expected
    )


def test_mad_is_unweighted_and_reported_before_filtering() -> None:
    result = aggregate_capacity_candidates(_candidate_set((10.0, 20.0, 30.0, 40.0)))

    assert result.pre_filter_median_raw_tokens == 25.0
    assert result.mad_raw_tokens == 10.0


def test_all_identical_values_have_zero_mad() -> None:
    result = aggregate_capacity_candidates(_candidate_set((20.0, 20.0, 20.0)))

    assert result.mad_raw_tokens == 0.0
    assert result.outlier_observation_ids == ()
    assert result.p25_raw_tokens == result.p50_raw_tokens == result.p75_raw_tokens == 20.0


def test_zero_mad_removes_values_outside_median_cluster() -> None:
    result = aggregate_capacity_candidates(_candidate_set((100.0, 100.0, 100.0, 1000.0)))

    assert result.mad_raw_tokens == 0.0
    assert result.outlier_observation_ids == ("observation-3",)
    assert result.used_observation_ids == (
        "observation-0",
        "observation-1",
        "observation-2",
    )
    assert result.p50_raw_tokens == 100.0
    assert result.accounting.outliers_removed == 1


def test_modified_z_exact_threshold_is_retained() -> None:
    median = 115.0
    mad = 5.0
    value_at_threshold = math.nextafter(median + (3.5 * mad / 0.6744897501960817), 0)
    result = aggregate_capacity_candidates(
        _candidate_set((100.0, 110.0, 115.0, 120.0, value_at_threshold))
    )

    modified_z = 0.6744897501960817 * abs(value_at_threshold - median) / mad
    assert modified_z < 3.5
    assert result.outlier_observation_ids == ()


def test_modified_z_above_threshold_is_removed() -> None:
    result = aggregate_capacity_candidates(_candidate_set((100.0, 110.0, 115.0, 120.0, 141.0)))

    assert result.outlier_observation_ids == ("observation-4",)
    assert result.p50_raw_tokens == 112.5


def test_two_candidates_never_have_outliers() -> None:
    result = aggregate_capacity_candidates(_candidate_set((20.0, 24.0)))

    assert result.status is AggregationStatus.SUFFICIENT
    assert result.mad_raw_tokens == 2.0
    assert result.p25_raw_tokens == 21.0
    assert result.p50_raw_tokens == 22.0
    assert result.p75_raw_tokens == 23.0
    assert result.outlier_observation_ids == ()


@pytest.mark.parametrize("values", [(), (20.0,)])
def test_zero_and_one_candidate_are_insufficient_without_statistics(
    values: tuple[float, ...],
) -> None:
    result = aggregate_capacity_candidates(_candidate_set(values))

    assert result.status is AggregationStatus.INSUFFICIENT_SAMPLES
    assert result.pre_filter_median_raw_tokens is None
    assert result.mad_raw_tokens is None
    assert result.weighted_median_raw_tokens is None
    assert result.p25_raw_tokens is None
    assert result.p50_raw_tokens is None
    assert result.p75_raw_tokens is None


def test_type_7_percentiles() -> None:
    result = aggregate_capacity_candidates(_candidate_set((10.0, 20.0, 30.0, 40.0)))

    assert result.p25_raw_tokens == 17.5
    assert result.p50_raw_tokens == 25.0
    assert result.p75_raw_tokens == 32.5


def test_exclusions_remain_distinct_from_outliers_and_ids_are_chronological() -> None:
    result = aggregate_capacity_candidates(
        _candidate_set((20.0, 21.0, 19.0, 20.0, 200.0), excluded=3)
    )

    assert result.accounting.candidate_observations == 8
    assert result.accounting.valid_observations == 5
    assert result.accounting.excluded_observations == 3
    assert result.accounting.outliers_removed == 1
    assert result.accounting.used_observations == 4
    assert result.used_observation_ids == (
        "observation-0",
        "observation-1",
        "observation-2",
        "observation-3",
    )
    assert result.outlier_observation_ids == ("observation-4",)


def test_configuration_and_meter_are_preserved_and_meters_are_independent() -> None:
    weekly = _candidate_set(
        (100.0, 120.0),
        meter_type=QuotaMeterType.WEEKLY,
        configuration=CONFIGURATION,
    )
    five_hour = _candidate_set((10.0, 20.0))

    weekly_result = aggregate_capacity_candidates(weekly)
    five_hour_result = aggregate_capacity_candidates(five_hour)

    assert weekly_result.meter_type is QuotaMeterType.WEEKLY
    assert weekly_result.configuration is CONFIGURATION
    assert five_hour_result.meter_type is QuotaMeterType.FIVE_HOUR
    assert five_hour_result.p50_raw_tokens == 15.0


def test_replay_is_exactly_equal() -> None:
    candidate_set = _candidate_set((10.0, 20.0, 100.0))

    assert aggregate_capacity_candidates(candidate_set) == aggregate_capacity_candidates(
        candidate_set
    )


@pytest.mark.parametrize(
    "mutator",
    [
        lambda candidate_set: replace(
            candidate_set,
            candidates=(replace(candidate_set.candidates[0], meter_type=QuotaMeterType.WEEKLY),),
        ),
        lambda candidate_set: replace(
            candidate_set,
            candidates=(replace(candidate_set.candidates[0], configuration=None),),
        ),
        lambda candidate_set: replace(
            candidate_set,
            candidates=(replace(candidate_set.candidates[0], full_capacity_raw_tokens=math.inf),),
        ),
        lambda candidate_set: replace(
            candidate_set,
            decisions=(candidate_set.decisions[0], candidate_set.decisions[0]),
        ),
        lambda candidate_set: replace(
            candidate_set,
            accounting=SampleAccounting(
                candidate_observations=2,
                valid_observations=1,
                excluded_observations=1,
                outliers_removed=0,
                used_observations=1,
            ),
        ),
    ],
)
def test_candidate_set_invariants_fail_closed(mutator) -> None:
    candidate_set = _candidate_set((10.0,))

    with pytest.raises(CandidateSetInvariantError):
        aggregate_capacity_candidates(mutator(candidate_set))


def test_non_eligible_quality_fails_closed() -> None:
    candidate_set = _candidate_set((10.0, 20.0))
    malformed = replace(
        candidate_set,
        candidates=tuple(
            replace(candidate, quality=QualityGrade.D) for candidate in candidate_set.candidates
        ),
    )

    with pytest.raises(CandidateSetInvariantError):
        aggregate_capacity_candidates(malformed)


def test_result_is_immutable() -> None:
    result = aggregate_capacity_candidates(_candidate_set((10.0, 20.0)))

    with pytest.raises(AttributeError):
        result.p50_raw_tokens = 99.0


def test_result_type_is_stable() -> None:
    result = aggregate_capacity_candidates(_candidate_set((10.0, 20.0)))

    assert isinstance(result, RobustCapacityAggregation)
