import math
from dataclasses import FrozenInstanceError, replace
from datetime import UTC, datetime, timedelta

import pytest

from codex_meter.analysis import (
    MAXIMUM_REGIME_INPUT_OBSERVATIONS,
    REGIME_SIGNAL_METHOD_VERSION,
    RegimeDatasetError,
    RegimeDetectionPolicy,
    RegimeNumericalError,
    RegimePolicyError,
    RegimeSignalReason,
    RegimeSignalStatus,
    TrendDirection,
    detect_metering_regime_signals,
)
from codex_meter.estimation import (
    Confidence,
    ReasonCode,
    aggregate_capacity_candidates,
    assess_capacity_evidence,
    derive_capacity_candidates,
)
from codex_meter.history import (
    ConfigurationKey,
    EvidenceValidity,
    ObservationDatabase,
    ObservationDataset,
    ObservationLifecycle,
    ObservationRecord,
    QualityGrade,
    QuotaEvidenceRecord,
    QuotaMeterType,
    ResetStatus,
)

CONFIG = ConfigurationKey("plus", "model", "high", "standard", "1.0")
UNKNOWN_CONFIG = ConfigurationKey(None, None, None, None, None)
FIVE = QuotaMeterType.FIVE_HOUR
WEEK = QuotaMeterType.WEEKLY
RAW_TOTAL = 200_000


def _quota(
    meter: QuotaMeterType,
    *,
    delta: float | None = 1.0,
    validity: EvidenceValidity = EvidenceValidity.VALID,
    quality: QualityGrade = QualityGrade.A,
    reset: ResetStatus = ResetStatus.NOT_DETECTED,
) -> QuotaEvidenceRecord:
    return QuotaEvidenceRecord(meter, validity, quality, delta, reset)


def _record(
    index: int,
    five_capacity: int = 20_000_000,
    weekly_capacity: int | None = None,
    **changes,
) -> ObservationRecord:
    raw_total = five_capacity // 100
    weekly_capacity = five_capacity if weekly_capacity is None else weekly_capacity
    weekly_delta = raw_total * 100 / weekly_capacity
    timestamp = datetime(2026, 1, 1, tzinfo=UTC) + timedelta(seconds=index)
    finalized_at = timestamp.strftime("%Y-%m-%dT%H:%M:%S") + "Z"
    base = ObservationRecord(
        observation_id=f"observation-{index:04}",
        lifecycle=ObservationLifecycle.FINALIZED,
        started_at=None,
        ended_at=None,
        finalized_at=finalized_at,
        configuration=CONFIG,
        summary_quality=QualityGrade.A,
        token_validity=EvidenceValidity.VALID,
        token_quality=QualityGrade.A,
        raw_total=raw_total,
        five_hour=_quota(FIVE),
        weekly=_quota(WEEK, delta=weekly_delta),
    )
    return replace(base, **changes)


def _dataset(*records: ObservationRecord, selected=None) -> ObservationDataset:
    return ObservationDataset(tuple(records), selected_configuration=selected)


def _analyze(
    records: tuple[ObservationRecord, ...] = (),
    *,
    policy: RegimeDetectionPolicy | None = None,
    meters: tuple[QuotaMeterType, ...] = (FIVE, WEEK),
    selected=None,
):
    return detect_metering_regime_signals(
        dataset=ObservationDataset(records, selected_configuration=selected),
        target_meters=meters,
        policy=policy or RegimeDetectionPolicy(),
    )


def _two_cohorts(
    previous: tuple[int, ...] | int = 20_000_000,
    current: tuple[int, ...] | int = 25_000_000,
    *,
    policy: RegimeDetectionPolicy | None = None,
    meters: tuple[QuotaMeterType, ...] = (FIVE, WEEK),
    changes: dict[int, dict[str, object]] | None = None,
):
    before_values = (previous,) * 20 if isinstance(previous, int) else previous
    after_values = (current,) * 20 if isinstance(current, int) else current
    values = before_values + after_values
    changes = changes or {}
    records = tuple(
        _record(index, capacity, **changes.get(index, {})) for index, capacity in enumerate(values)
    )
    return _analyze(records, policy=policy, meters=meters)


def test_method_version_default_policy_and_empty_selected_configuration():
    policy = RegimeDetectionPolicy()
    assert REGIME_SIGNAL_METHOD_VERSION == "1.0.0"
    assert policy.cohort_observations == 20
    assert policy.minimum_used_observations == 8
    assert policy.minimum_relative_change == 0.25
    assert policy.maximum_input_observations == 500
    selected = _analyze((), selected=CONFIG)
    assert selected.configuration == CONFIG
    assert selected.unanchored_observations == selected.incomplete_tail_observations == 0
    assert len(selected.meter_analyses) == 2
    assert all(not item.cohorts and not item.comparisons for item in selected.meter_analyses)
    assert _analyze((), meters=()).meter_analyses == ()
    assert MAXIMUM_REGIME_INPUT_OBSERVATIONS == 100_000


@pytest.mark.parametrize(
    "kwargs",
    [
        {"cohort_observations": True},
        {"cohort_observations": 0},
        {"cohort_observations": 100_001},
        {"minimum_used_observations": True},
        {"minimum_used_observations": 1},
        {"minimum_used_observations": 21},
        {"maximum_input_observations": False},
        {"maximum_input_observations": 0},
        {"maximum_input_observations": 100_001},
        {"maximum_input_observations": 10},
        {"minimum_relative_change": True},
        {"minimum_relative_change": 0},
        {"minimum_relative_change": -0.1},
        {"minimum_relative_change": math.inf},
        {"minimum_relative_change": math.nan},
        {"minimum_relative_change": "0.25"},
    ],
)
def test_policy_rejects_invalid_types_and_bounds(kwargs):
    with pytest.raises(RegimePolicyError):
        RegimeDetectionPolicy(**kwargs)


def test_wrong_policy_target_and_dataset_types_are_rejected():
    with pytest.raises(RegimePolicyError):
        detect_metering_regime_signals(dataset=_dataset(), policy=object())
    with pytest.raises(RegimeDatasetError):
        detect_metering_regime_signals(dataset=object())
    with pytest.raises(RegimeDatasetError):
        detect_metering_regime_signals(dataset=_dataset(), target_meters=[FIVE])
    with pytest.raises(RegimeDatasetError):
        detect_metering_regime_signals(dataset=_dataset(), target_meters=(FIVE, FIVE))
    with pytest.raises(RegimeDatasetError):
        detect_metering_regime_signals(dataset=_dataset(), target_meters=("five_hour",))


@pytest.mark.parametrize("size", (1, 19, 20, 39, 40, 45, 60))
def test_small_and_partial_history_keeps_explicit_nonoverlapping_cohorts(size):
    records = tuple(_record(index) for index in range(size))
    result = _analyze(records, meters=(FIVE,))
    meter = result.meter_analyses[0]
    expected_cohorts = (size + 19) // 20
    assert len(meter.cohorts) == expected_cohorts
    assert [cohort.complete for cohort in meter.cohorts] == [
        index * 20 + 20 <= size for index in range(expected_cohorts)
    ]
    complete_count = size // 20
    assert len(meter.comparisons) == max(complete_count - 1, 0)
    assert result.incomplete_tail_observations == size % 20
    flattened = tuple(item for cohort in meter.cohorts for item in cohort.observation_ids)
    assert flattened == tuple(record.observation_id for record in records)
    assert len(set(flattened)) == len(flattened)
    if size % 20:
        partial = meter.cohorts[-1]
        assert partial.observation_count == size % 20
        assert partial.sample_accounting is None
        assert partial.estimated_full_capacity_raw_tokens is None
        assert partial.reason_codes == (RegimeSignalReason.INCOMPLETE_COHORT,)


def test_three_cohorts_compare_only_adjacent_pairs():
    values = (20_000_000,) * 20 + (25_000_000,) * 20 + (32_000_000,) * 20
    result = _analyze(tuple(_record(i, value) for i, value in enumerate(values)), meters=(FIVE,))
    meter = result.meter_analyses[0]
    assert [
        (item.previous_cohort_index, item.current_cohort_index) for item in meter.comparisons
    ] == [
        (1, 2),
        (2, 3),
    ]
    assert all(item.status is RegimeSignalStatus.SHIFT_SIGNAL for item in meter.comparisons)
    assert all(item.direction is TrendDirection.INCREASE for item in meter.comparisons)


def test_unordered_input_is_normalized_with_canonical_time_then_id_tie_break():
    values = (
        _record(3, 20_000_000, finalized_at="2026-01-01T00:00:02.1Z"),
        _record(1, 20_000_000, observation_id="z", finalized_at="2026-01-01T00:00:01Z"),
        _record(0, 20_000_000, observation_id="a", finalized_at="2026-01-01T00:00:01.000000000Z"),
        _record(2, 20_000_000, finalized_at="2026-01-01T00:00:02.100Z"),
    )
    result = _analyze(
        tuple(reversed(values)),
        policy=RegimeDetectionPolicy(
            cohort_observations=4, minimum_used_observations=2, maximum_input_observations=4
        ),
        meters=(FIVE,),
    )
    cohort = result.meter_analyses[0].cohorts[0]
    assert cohort.observation_ids == ("a", "z", "observation-0002", "observation-0003")
    assert cohort.started_at == "2026-01-01T00:00:01.000000000Z"
    assert cohort.ended_at == "2026-01-01T00:00:02.100000000Z"


def test_unanchored_rows_are_counted_but_never_assigned_to_cohorts():
    records = tuple(
        _record(i, finalized_at=None, started_at=None, ended_at=None) for i in range(20)
    )
    result = _analyze(records, meters=(FIVE,))
    assert result.unanchored_observations == 20
    assert result.incomplete_tail_observations == 0
    assert result.meter_analyses[0].cohorts == ()
    selected = _analyze(records[:1], meters=(FIVE,), selected=CONFIG)
    assert selected.configuration == CONFIG


@pytest.mark.parametrize(
    "bad_record",
    [
        _record(0, finalized_at="2026-01-01T00:00:00.1234567890Z"),
        _record(0, lifecycle=ObservationLifecycle.ACTIVE),
        _record(0, token_validity="valid"),
        _record(0, token_quality="A"),
        _record(0, summary_quality="A"),
        _record(0, raw_total=-1),
        _record(0, raw_total=True),
        _record(0, configuration=ConfigurationKey("plus", 5, "high", "standard", "1.0")),
        _record(0, five_hour=_quota("five_hour")),
        _record(0, five_hour=_quota(FIVE, delta=math.nan)),
        _record(0, five_hour=_quota(FIVE, delta=True)),
        _record(0, five_hour=_quota(FIVE, validity="valid")),
        _record(0, five_hour=_quota(FIVE, quality="A")),
        _record(0, five_hour=_quota(FIVE, reset="not_detected")),
    ],
)
def test_malformed_observation_evidence_is_rejected(bad_record):
    with pytest.raises(RegimeDatasetError):
        _analyze((bad_record,), meters=(FIVE,))


def test_duplicate_ids_nonterminal_rows_and_invalid_migration_version_fail_closed():
    duplicate = _record(0)
    with pytest.raises(RegimeDatasetError):
        _analyze((duplicate, duplicate), meters=(FIVE,))
    with pytest.raises(RegimeDatasetError):
        _analyze((_record(0, lifecycle=ObservationLifecycle.TASK_ENDED),), meters=(FIVE,))
    with pytest.raises(RegimeDatasetError):
        detect_metering_regime_signals(
            dataset=ObservationDataset((), migration_version=True), target_meters=(FIVE,)
        )


def test_mixed_and_selected_configurations_are_rejected_including_unknown_vs_known():
    unknown = replace(_record(0), configuration=UNKNOWN_CONFIG)
    known = _record(1)
    with pytest.raises(RegimeDatasetError):
        _analyze((unknown, known), meters=(FIVE,))
    with pytest.raises(RegimeDatasetError):
        _analyze((_record(0),), meters=(FIVE,), selected=UNKNOWN_CONFIG)
    selected = _analyze((), meters=(FIVE,), selected=UNKNOWN_CONFIG)
    assert selected.configuration == UNKNOWN_CONFIG


def test_input_limit_rejects_without_silent_truncation():
    policy = RegimeDetectionPolicy(maximum_input_observations=20)
    with pytest.raises(RegimeDatasetError, match="maximum_input_observations"):
        _analyze(tuple(_record(i) for i in range(21)), policy=policy, meters=(FIVE,))


def test_all_rows_count_toward_cohort_even_when_candidate_evidence_is_excluded():
    records = tuple(
        _record(
            index,
            five_capacity=20_000_000,
            token_validity=(EvidenceValidity.INVALID if index < 4 else EvidenceValidity.VALID),
        )
        for index in range(20)
    )
    cohort = _analyze(records, meters=(FIVE,)).meter_analyses[0].cohorts[0]
    assert cohort.observation_count == 20
    assert cohort.sample_accounting.candidate_observations == 20
    assert cohort.sample_accounting.valid_observations == 16
    assert cohort.sample_accounting.excluded_observations == 4
    assert cohort.sample_accounting.candidate_observations == (
        cohort.sample_accounting.valid_observations + cohort.sample_accounting.excluded_observations
    )


def test_p018_p019_and_p020_are_recomputed_for_each_complete_cohort():
    capacities = tuple(20_000_000 + index * 100_000 for index in range(20))
    records = tuple(_record(index, capacity) for index, capacity in enumerate(capacities))
    cohort = _analyze(records, meters=(FIVE,)).meter_analyses[0].cohorts[0]
    candidates = derive_capacity_candidates(_dataset(*records), FIVE)
    aggregation = aggregate_capacity_candidates(candidates)
    assessment = assess_capacity_evidence(candidates, aggregation)
    assert cohort.estimated_full_capacity_raw_tokens == aggregation.weighted_median_raw_tokens
    assert cohort.p25_raw_tokens == aggregation.p25_raw_tokens
    assert cohort.p50_raw_tokens == aggregation.p50_raw_tokens
    assert cohort.p75_raw_tokens == aggregation.p75_raw_tokens
    assert cohort.confidence is assessment.confidence
    assert cohort.estimator_method_version == "1.0.0"


def test_prefix_stability_and_stable_cohort_identity_when_later_rows_append():
    records = tuple(
        _record(i, 20_000_000 if i < 20 else 25_000_000 if i < 40 else 30_000_000)
        for i in range(60)
    )
    prefix = _analyze(records[:40], meters=(FIVE,)).meter_analyses[0]
    extended = _analyze(records, meters=(FIVE,)).meter_analyses[0]
    assert prefix.cohorts == extended.cohorts[:2]
    assert [item.cohort_id for item in prefix.cohorts] == [
        item.cohort_id for item in extended.cohorts[:2]
    ]
    assert len(set(item.cohort_id for item in extended.cohorts)) == 3


def test_identical_cohorts_are_no_detected_shift_with_no_false_change_time():
    comparison = (
        _two_cohorts(20_000_000, 20_000_000, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    )
    assert comparison.status is RegimeSignalStatus.NO_DETECTED_SHIFT
    assert comparison.direction is TrendDirection.UNCHANGED
    assert comparison.absolute_change_raw_tokens == 0.0
    assert comparison.relative_change_percent == 0.0


def test_exactly_eight_retained_samples_pass_the_method_v1_minimum():
    changes = {index: {"token_validity": EvidenceValidity.INVALID} for index in range(8, 20)}
    comparison = _two_cohorts(changes=changes, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    assert comparison.previous_used_observations == 8
    assert comparison.previous_confidence is Confidence.HIGH
    assert comparison.status is RegimeSignalStatus.SHIFT_SIGNAL


def test_requested_meter_order_is_preserved():
    result = _analyze(tuple(_record(index) for index in range(20)), meters=(WEEK, FIVE))
    assert tuple(item.meter_type for item in result.meter_analyses) == (WEEK, FIVE)


@pytest.mark.parametrize(
    ("before", "after", "status", "direction"),
    [
        (20_000_000, 24_000_000, RegimeSignalStatus.NO_DETECTED_SHIFT, TrendDirection.INCREASE),
        (20_000_000, 25_000_000, RegimeSignalStatus.SHIFT_SIGNAL, TrendDirection.INCREASE),
        (30_000_000, 22_500_000, RegimeSignalStatus.SHIFT_SIGNAL, TrendDirection.DECREASE),
    ],
)
def test_effect_threshold_is_inclusive_and_shift_direction_is_descriptive(
    before, after, status, direction
):
    comparison = _two_cohorts(before, after, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    assert comparison.status is status
    assert comparison.direction is direction
    assert comparison.relative_change_percent == pytest.approx(100 * (after - before) / before)
    assert comparison.absolute_change_raw_tokens == float(after - before)


def test_large_median_change_with_overlapping_iqrs_is_ambiguous():
    before = tuple(10_000_000 + index * 1_000_000 for index in range(20))
    after = tuple(16_000_000 + index * 1_000_000 for index in range(20))
    comparison = _two_cohorts(before, after, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    assert comparison.relative_change_percent >= 25
    assert comparison.previous_p75_raw_tokens >= comparison.current_p25_raw_tokens
    assert comparison.status is RegimeSignalStatus.AMBIGUOUS_VARIATION
    assert RegimeSignalReason.DISTRIBUTION_OVERLAP in comparison.reason_codes


def test_iqr_intervals_touching_at_the_boundary_are_ambiguous():
    before = tuple(
        value * 1_000_000
        for value in (
            10,
            11,
            12,
            13,
            14,
            15,
            16,
            17,
            18,
            19,
            19,
            19,
            19,
            19,
            19,
            23,
            24,
            25,
            26,
            27,
        )
    )
    after = tuple(
        value * 1_000_000
        for value in (
            14,
            15,
            16,
            17,
            17,
            21,
            22,
            23,
            24,
            25,
            26,
            27,
            28,
            29,
            30,
            31,
            32,
            33,
            34,
            35,
        )
    )
    comparison = _two_cohorts(before, after, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    assert comparison.previous_p75_raw_tokens == comparison.current_p25_raw_tokens == 20_000_000.0
    assert comparison.status is RegimeSignalStatus.AMBIGUOUS_VARIATION


@pytest.mark.parametrize("low_side", ("previous", "current"))
def test_low_confidence_cohort_cannot_support_a_shift(low_side):
    changes = {}
    indices = range(20) if low_side == "previous" else range(20, 40)
    for index in indices:
        changes[index] = {"token_quality": QualityGrade.C}
    result = _two_cohorts(changes=changes, meters=(FIVE,))
    comparison = result.meter_analyses[0].comparisons[0]
    assert (comparison.previous_confidence is Confidence.LOW) == (low_side == "previous")
    assert (comparison.current_confidence is Confidence.LOW) == (low_side == "current")
    assert comparison.status is RegimeSignalStatus.INSUFFICIENT_EVIDENCE
    assert RegimeSignalReason.LOW_CAPACITY_CONFIDENCE in comparison.reason_codes


@pytest.mark.parametrize("low_side", ("previous", "current"))
def test_retained_sample_minimum_is_checked_independently_for_each_cohort(low_side):
    changes = {}
    indices = range(7, 20) if low_side == "previous" else range(27, 40)
    for index in indices:
        changes[index] = {"token_validity": EvidenceValidity.INVALID}
    comparison = _two_cohorts(changes=changes, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    assert comparison.status is RegimeSignalStatus.INSUFFICIENT_EVIDENCE
    assert comparison.previous_used_observations == (7 if low_side == "previous" else 20)
    assert comparison.current_used_observations == (7 if low_side == "current" else 20)
    assert RegimeSignalReason.INSUFFICIENT_SAMPLES in comparison.reason_codes


def test_quality_c_is_preserved_and_confidence_comes_from_p020_unchanged():
    changes = {19: {"token_quality": QualityGrade.C}}
    result = _two_cohorts(changes=changes, meters=(FIVE,))
    first = result.meter_analyses[0].cohorts[0]
    assert first.confidence is Confidence.MEDIUM
    assert ReasonCode.CONCURRENT_USAGE_POSSIBLE in first.estimator_reason_codes
    assert RegimeSignalReason.QUALITY_DEGRADATION in first.reason_codes
    assert result.meter_analyses[0].comparisons[0].status is RegimeSignalStatus.SHIFT_SIGNAL


def test_outlier_removal_accounting_and_diagnostic_are_preserved():
    records = tuple(
        _record(index, 100_000_000 if index == 0 else 20_000_000) for index in range(20)
    )
    cohort = _analyze(records, meters=(FIVE,)).meter_analyses[0].cohorts[0]
    assert cohort.sample_accounting.valid_observations == 20
    assert cohort.sample_accounting.outliers_removed == 1
    assert cohort.sample_accounting.used_observations == 19
    assert RegimeSignalReason.OUTLIERS_REMOVED in cohort.reason_codes


def test_reset_crossing_invalidates_only_its_meter_and_does_not_itself_signal_a_change():
    reset = _quota(FIVE, reset=ResetStatus.DETECTED)
    records = tuple(
        _record(index, five_hour=reset) if index == 0 else _record(index) for index in range(40)
    )
    result = _analyze(records)
    five, weekly = result.meter_analyses
    assert RegimeSignalReason.RESET_CROSSED in five.cohorts[0].reason_codes
    assert five.comparisons[0].status is RegimeSignalStatus.NO_DETECTED_SHIFT
    assert weekly.comparisons[0].status is RegimeSignalStatus.NO_DETECTED_SHIFT
    assert weekly.cohorts[0].sample_accounting.used_observations == 20


def test_five_hour_and_weekly_capacity_analyses_are_independent():
    records = tuple(
        _record(
            index,
            20_000_000,
            weekly_capacity=20_000_000 if index < 20 else 25_000_000,
            five_hour=(
                _quota(FIVE, validity=EvidenceValidity.INVALID, reset=ResetStatus.DETECTED)
                if index < 20
                else _quota(FIVE)
            ),
        )
        for index in range(40)
    )
    result = _analyze(records)
    five, weekly = result.meter_analyses
    assert five.comparisons[0].status is RegimeSignalStatus.INSUFFICIENT_EVIDENCE
    assert weekly.comparisons[0].status is RegimeSignalStatus.SHIFT_SIGNAL


def test_nonfinite_projection_fails_with_typed_numerical_error():
    records = tuple(_record(index, raw_total=10**309) for index in range(20))
    with pytest.raises(RegimeNumericalError):
        _analyze(records, meters=(FIVE,))


def test_nonfinite_relative_percentage_returns_explicit_unavailable_comparison():
    records = tuple(
        _record(index, raw_total=1, five_hour=_quota(FIVE, delta=100.0)) for index in range(20)
    ) + tuple(
        _record(index + 20, raw_total=19 * 10**303, five_hour=_quota(FIVE, delta=1.0))
        for index in range(20)
    )
    comparison = _analyze(records, meters=(FIVE,)).meter_analyses[0].comparisons[0]
    assert comparison.status is RegimeSignalStatus.INSUFFICIENT_EVIDENCE
    assert comparison.direction is TrendDirection.UNAVAILABLE
    assert comparison.absolute_change_raw_tokens is None
    assert comparison.relative_change_percent is None
    assert RegimeSignalReason.NONFINITE_COMPARISON in comparison.reason_codes


def test_result_is_deterministic_immutable_and_does_not_open_sqlite(tmp_path, monkeypatch):
    records = tuple(_record(index) for index in range(20))
    monkeypatch.setattr(
        ObservationDatabase,
        "open",
        classmethod(lambda cls, *args, **kwargs: pytest.fail("SQLite must not be opened")),
    )
    first = _analyze(records)
    second = _analyze(records)
    assert first == second
    assert first.method_version == REGIME_SIGNAL_METHOD_VERSION
    with pytest.raises(FrozenInstanceError):
        first.unanchored_observations = 3
    assert list(tmp_path.iterdir()) == []
