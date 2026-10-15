from dataclasses import FrozenInstanceError, replace

import pytest

from codex_meter.analysis import (
    MAXIMUM_SAFE_TASK_COUNT,
    MAXIMUM_WORKLOAD_OBSERVATIONS,
    TaskSizeDistributionStatus,
    TaskSizeExclusion,
    TaskSizeExclusionReason,
    TaskSizeSample,
    WorkloadCapacityError,
    WorkloadConfigurationMismatchError,
    WorkloadDatasetError,
    WorkloadNumericalError,
    WorkloadPredictionStatus,
    analyze_historical_capacity,
    predict_remaining_workload,
)
from codex_meter.estimation import (
    PROVIDER_AUTHORITY,
    AnalyticsResultStatus,
    AnalyticsResultV1,
    CandidateDecision,
    Confidence,
    ConfigurationProvenance,
    CurrentCapacityResultContext,
    CurrentQuotaPosition,
    MeterCandidateSet,
    QuotaCapacityEstimate,
    RawCapacityCandidate,
    ReasonCode,
    ResultConfigurationIdentity,
    ResultConfigurationValue,
    SampleAccounting,
    build_current_capacity_result,
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
from codex_meter.history.sqlite import MAXIMUM_OBSERVATIONS_SAFETY_LIMIT

CONFIG = ConfigurationKey("plus", "gpt-5.6-sol", "high", "standard", None)
OTHER_CONFIG = ConfigurationKey("plus", "gpt-5.6-sol", "high", "standard", "1.2.3")
FIVE = QuotaMeterType.FIVE_HOUR
WEEKLY = QuotaMeterType.WEEKLY
GENERATED_AT = "2026-10-01T03:00:01.123456789Z"


def record(
    index=0,
    raw_total=100,
    *,
    token_validity=EvidenceValidity.VALID,
    token_quality=QualityGrade.A,
    summary_quality=QualityGrade.A,
    lifecycle=ObservationLifecycle.FINALIZED,
    configuration=CONFIG,
    five_hour=None,
    weekly=None,
    started_at=None,
    ended_at=None,
    finalized_at=None,
):
    return ObservationRecord(
        f"observation-{index:06}",
        lifecycle,
        started_at,
        ended_at,
        finalized_at,
        configuration,
        summary_quality,
        token_validity,
        token_quality,
        raw_total,
        five_hour
        or QuotaEvidenceRecord(
            FIVE,
            EvidenceValidity.VALID,
            QualityGrade.A,
            1.0,
            ResetStatus.NOT_DETECTED,
        ),
        weekly
        or QuotaEvidenceRecord(
            WEEKLY,
            EvidenceValidity.VALID,
            QualityGrade.A,
            1.0,
            ResetStatus.NOT_DETECTED,
        ),
    )


def configuration_identity(configuration=CONFIG):
    values = (
        configuration.plan,
        configuration.model,
        configuration.reasoning_level,
        configuration.speed_mode,
        configuration.codex_version,
    )
    fields = []
    for value in values:
        provenance = (
            ConfigurationProvenance.UNAVAILABLE
            if value is None
            else ConfigurationProvenance.OBSERVED
        )
        fields.append(ResultConfigurationValue(value, provenance))
    return ResultConfigurationIdentity(*fields)


def estimate(meter, confidence=Confidence.HIGH, remaining=1_000.0):
    if confidence is Confidence.INSUFFICIENT:
        accounting = SampleAccounting(0, 0, 0, 0, 0)
        reasons = (ReasonCode.UNKNOWN_REASON,)
        full = used = left = p25 = p50 = p75 = None
    else:
        accounting = SampleAccounting(2, 2, 0, 0, 2)
        reasons = ()
        full = max(remaining, 1.0)
        used = full - remaining
        left = remaining
        p25, p50, p75 = 100.0, 200.0, 300.0
    return QuotaCapacityEstimate(
        meter,
        confidence,
        accounting,
        reasons,
        full,
        used,
        left,
        p25,
        p50,
        p75,
    )


def capacity_result(*estimates, configuration=CONFIG):
    estimates = tuple(estimates)
    reasons = {reason for item in estimates for reason in item.reason_codes}
    has_sufficient = any(item.confidence is not Confidence.INSUFFICIENT for item in estimates)
    return AnalyticsResultV1(
        "request-022",
        AnalyticsResultStatus.SUCCEEDED
        if has_sufficient
        else AnalyticsResultStatus.INSUFFICIENT_EVIDENCE,
        configuration_identity(configuration),
        GENERATED_AT,
        "1.0.0",
        estimates,
        tuple(reason for reason in ReasonCode if reason in reasons),
    )


def predict(*records, result=None, selected_configuration=None):
    dataset = ObservationDataset(tuple(records), selected_configuration=selected_configuration)
    return predict_remaining_workload(
        dataset=dataset,
        capacity_result=result or capacity_result(estimate(FIVE)),
    )


def counts(prediction, index=0):
    item = prediction.meter_predictions[index]
    return item.light_task_count, item.normal_task_count, item.heavy_task_count


def test_empty_dataset_uses_capacity_configuration_but_has_insufficient_history():
    prediction = predict()
    distribution = prediction.task_size_distribution
    meter = prediction.meter_predictions[0]
    assert distribution.configuration == CONFIG
    assert distribution.status is TaskSizeDistributionStatus.INSUFFICIENT
    assert distribution.accounting.candidate_observations == 0
    assert (
        distribution.p25_raw_tokens,
        distribution.p50_raw_tokens,
        distribution.p75_raw_tokens,
    ) == (
        None,
        None,
        None,
    )
    assert meter.status is WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY
    assert meter.estimated_remaining_raw_tokens == 1_000.0
    assert counts(prediction) == (None, None, None)


@pytest.mark.parametrize("task_count", (1, 2))
def test_one_or_two_eligible_tasks_do_not_expose_quantiles(task_count):
    prediction = predict(*(record(i, i + 1) for i in range(task_count)))
    distribution = prediction.task_size_distribution
    assert distribution.status is TaskSizeDistributionStatus.INSUFFICIENT
    assert distribution.accounting.eligible_observations == task_count
    assert distribution.accounting.excluded_observations == 0
    assert (
        distribution.p25_raw_tokens
        is distribution.p50_raw_tokens
        is distribution.p75_raw_tokens
        is None
    )
    assert counts(prediction) == (None, None, None)


def test_three_eligible_tasks_are_sufficient_and_include_one_token_samples():
    prediction = predict(record(0, 1), record(1, 2), record(2, 3))
    distribution = prediction.task_size_distribution
    assert distribution.status is TaskSizeDistributionStatus.AVAILABLE
    assert tuple(sample.raw_total for sample in distribution.samples) == (1, 2, 3)
    assert (
        distribution.p25_raw_tokens,
        distribution.p50_raw_tokens,
        distribution.p75_raw_tokens,
    ) == (
        1.5,
        2.0,
        2.5,
    )


def test_four_task_type7_quantiles():
    prediction = predict(*(record(i, value) for i, value in enumerate((100, 200, 300, 400))))
    distribution = prediction.task_size_distribution
    assert (
        distribution.p25_raw_tokens,
        distribution.p50_raw_tokens,
        distribution.p75_raw_tokens,
    ) == (
        175.0,
        250.0,
        325.0,
    )


def test_light_normal_heavy_counts_match_scenario_example():
    task_sizes = (10_000, 30_000, 70_000, 190_000)
    result = capacity_result(estimate(FIVE, remaining=2_500_000.0))
    prediction = predict(
        *(record(i, size) for i, size in enumerate(task_sizes)),
        result=result,
    )
    distribution = prediction.task_size_distribution
    assert (
        distribution.p25_raw_tokens,
        distribution.p50_raw_tokens,
        distribution.p75_raw_tokens,
    ) == (
        25_000.0,
        50_000.0,
        100_000.0,
    )
    assert counts(prediction) == (100, 50, 25)


def test_zero_remaining_capacity_is_available_with_zero_scenario_counts():
    prediction = predict(
        record(0, 100),
        record(1, 200),
        record(2, 300),
        result=capacity_result(estimate(FIVE, remaining=0.0)),
    )
    assert counts(prediction) == (0, 0, 0)
    assert prediction.meter_predictions[0].estimated_remaining_raw_tokens == 0.0


def test_nonintegral_ratio_uses_floor():
    prediction = predict(
        *(record(i, 100) for i in range(3)),
        result=capacity_result(estimate(FIVE, remaining=299.0)),
    )
    assert counts(prediction) == (2, 2, 2)


def test_exactly_divisible_capacity_returns_exact_task_count():
    prediction = predict(
        *(record(i, 100) for i in range(3)),
        result=capacity_result(estimate(FIVE, remaining=1_000.0)),
    )
    assert counts(prediction) == (10, 10, 10)


def test_positive_but_insufficient_scenario_budget_returns_zero_tasks():
    prediction = predict(
        *(record(i, 100) for i in range(3)),
        result=capacity_result(estimate(FIVE, remaining=99.0)),
    )
    assert counts(prediction) == (0, 0, 0)
    assert prediction.meter_predictions[0].status is WorkloadPredictionStatus.AVAILABLE


def test_task_counts_are_monotone_across_quantiles():
    prediction = predict(
        *(record(i, value) for i, value in enumerate((100, 200, 300, 400))),
        result=capacity_result(estimate(FIVE, remaining=1_000.0)),
    )
    light, normal, heavy = counts(prediction)
    assert light >= normal >= heavy


def test_safe_integer_boundary_is_supported():
    prediction = predict(
        *(record(i, 1) for i in range(3)),
        result=capacity_result(estimate(FIVE, remaining=float(MAXIMUM_SAFE_TASK_COUNT))),
    )
    assert counts(prediction) == (MAXIMUM_SAFE_TASK_COUNT,) * 3


def test_task_count_over_safe_integer_boundary_fails_closed():
    prediction_capacity = capacity_result(estimate(FIVE, remaining=float(2**53)))
    with pytest.raises(WorkloadNumericalError):
        predict(*(record(i, 1) for i in range(3)), result=prediction_capacity)


@pytest.mark.parametrize("quality", (QualityGrade.A, QualityGrade.B, QualityGrade.C))
def test_token_valid_a_b_c_samples_are_eligible_without_quality_weighting(quality):
    prediction = predict(
        record(0, 10, token_quality=quality),
        record(1, 20, token_quality=quality),
        record(2, 30, token_quality=quality),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3
    assert tuple(sample.raw_total for sample in prediction.task_size_distribution.samples) == (
        10,
        20,
        30,
    )
    assert prediction.task_size_distribution.p50_raw_tokens == 20.0
    assert prediction.task_size_distribution.c_quality_observations == (
        3 if quality is QualityGrade.C else 0
    )


@pytest.mark.parametrize("quality", (QualityGrade.D, QualityGrade.X))
def test_token_d_and_x_quality_are_excluded(quality):
    prediction = predict(record(0, 100, token_quality=quality))
    (exclusion,) = prediction.task_size_distribution.exclusions
    assert exclusion.reasons == (TaskSizeExclusionReason.TOKEN_QUALITY_INELIGIBLE,)
    assert prediction.task_size_distribution.accounting.excluded_observations == 1


@pytest.mark.parametrize(
    "validity",
    (EvidenceValidity.INCOMPLETE, EvidenceValidity.INVALID, EvidenceValidity.UNAVAILABLE),
)
def test_nonvalid_token_evidence_is_excluded(validity):
    prediction = predict(record(0, 100, token_validity=validity))
    assert prediction.task_size_distribution.exclusions[0].reasons == (
        TaskSizeExclusionReason.TOKEN_NOT_VALID,
    )


@pytest.mark.parametrize(
    "lifecycle",
    (ObservationLifecycle.INCOMPLETE, ObservationLifecycle.INVALID),
)
def test_terminal_incomplete_or_invalid_lifecycle_does_not_discard_valid_token_evidence(
    lifecycle,
):
    prediction = predict(
        record(0, 100, lifecycle=lifecycle),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_unavailable_and_zero_raw_totals_have_separate_reasons():
    prediction = predict(record(0, None), record(1, 0))
    by_id = {item.observation_id: item for item in prediction.task_size_distribution.exclusions}
    assert by_id["observation-000000"].reasons == (TaskSizeExclusionReason.RAW_TOTAL_UNAVAILABLE,)
    assert by_id["observation-000001"].reasons == (TaskSizeExclusionReason.ZERO_RAW_TOTAL,)
    assert prediction.task_size_distribution.accounting.excluded_observations == 2


def test_multiple_exclusion_reasons_count_one_observation_once():
    prediction = predict(
        record(
            3,
            None,
            token_validity=EvidenceValidity.INVALID,
            token_quality=QualityGrade.X,
        )
    )
    exclusion = prediction.task_size_distribution.exclusions[0]
    assert exclusion.reasons == (
        TaskSizeExclusionReason.TOKEN_NOT_VALID,
        TaskSizeExclusionReason.TOKEN_QUALITY_INELIGIBLE,
        TaskSizeExclusionReason.RAW_TOTAL_UNAVAILABLE,
    )
    assert prediction.task_size_distribution.accounting.candidate_observations == 1
    assert prediction.task_size_distribution.accounting.excluded_observations == 1


def test_quota_reset_does_not_invalidate_valid_task_size():
    reset_quota = QuotaEvidenceRecord(
        FIVE, EvidenceValidity.INVALID, QualityGrade.X, None, ResetStatus.DETECTED
    )
    prediction = predict(
        record(0, 100, five_hour=reset_quota),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_zero_quota_delta_does_not_invalidate_valid_task_size():
    zero_delta = QuotaEvidenceRecord(
        FIVE, EvidenceValidity.VALID, QualityGrade.A, 0.0, ResetStatus.NOT_DETECTED
    )
    prediction = predict(
        record(0, 100, five_hour=zero_delta),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_weekly_quota_failure_does_not_invalidate_valid_task_size():
    failed_weekly = QuotaEvidenceRecord(
        WEEKLY, EvidenceValidity.UNAVAILABLE, QualityGrade.X, None, ResetStatus.UNAVAILABLE
    )
    prediction = predict(
        record(0, 100, weekly=failed_weekly),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_both_unavailable_quota_meters_do_not_invalidate_valid_task_size():
    unavailable_five_hour = QuotaEvidenceRecord(
        FIVE, EvidenceValidity.UNAVAILABLE, QualityGrade.X, None, ResetStatus.UNAVAILABLE
    )
    unavailable_weekly = QuotaEvidenceRecord(
        WEEKLY, EvidenceValidity.UNAVAILABLE, QualityGrade.X, None, ResetStatus.UNAVAILABLE
    )
    prediction = predict(
        record(0, 100, five_hour=unavailable_five_hour, weekly=unavailable_weekly),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_global_summary_quality_x_does_not_gate_valid_token_evidence():
    prediction = predict(
        record(0, 100, summary_quality=QualityGrade.X),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_legitimate_large_task_tail_is_not_removed_as_a_capacity_outlier():
    prediction = predict(*(record(i, value) for i, value in enumerate((100, 100, 100, 1_000_000))))
    distribution = prediction.task_size_distribution
    assert distribution.accounting.eligible_observations == 4
    assert distribution.samples[-1].raw_total == 1_000_000
    assert distribution.p75_raw_tokens == 250_075.0


def test_five_hour_and_weekly_capacity_predictions_are_independent_and_ordered():
    result = capacity_result(
        estimate(FIVE, remaining=500.0),
        estimate(WEEKLY, remaining=2_000.0),
    )
    prediction = predict(
        *(record(i, 100) for i in range(3)),
        result=result,
    )
    assert tuple(item.meter_type for item in prediction.meter_predictions) == (FIVE, WEEKLY)
    assert counts(prediction, 0) == (5, 5, 5)
    assert counts(prediction, 1) == (20, 20, 20)
    assert prediction.meter_predictions[0].estimated_remaining_raw_tokens == 500.0
    assert prediction.meter_predictions[1].estimated_remaining_raw_tokens == 2_000.0


def test_mixed_meter_confidence_is_preserved():
    result = capacity_result(
        estimate(FIVE, Confidence.HIGH, 500.0),
        estimate(WEEKLY, Confidence.INSUFFICIENT),
    )
    prediction = predict(*(record(i, 100) for i in range(3)), result=result)
    five, weekly = prediction.meter_predictions
    assert five.capacity_confidence is Confidence.HIGH
    assert five.status is WorkloadPredictionStatus.AVAILABLE
    assert counts(prediction, 0) == (5, 5, 5)
    assert weekly.capacity_confidence is Confidence.INSUFFICIENT
    assert weekly.status is WorkloadPredictionStatus.INSUFFICIENT_CAPACITY
    assert weekly.estimated_remaining_raw_tokens is None
    assert (weekly.light_task_count, weekly.normal_task_count, weekly.heavy_task_count) == (
        None,
        None,
        None,
    )


def test_insufficient_capacity_does_not_borrow_another_meter():
    result = capacity_result(
        estimate(FIVE, Confidence.INSUFFICIENT),
        estimate(WEEKLY, Confidence.HIGH, 500.0),
    )
    prediction = predict(*(record(i, 100) for i in range(3)), result=result)
    five, weekly = prediction.meter_predictions
    assert five.status is WorkloadPredictionStatus.INSUFFICIENT_CAPACITY
    assert five.estimated_remaining_raw_tokens is None
    assert counts(prediction, 0) == (None, None, None)
    assert counts(prediction, 1) == (5, 5, 5)


def test_task_history_insufficiency_does_not_discard_capacity():
    prediction = predict(record(0, 100))
    meter = prediction.meter_predictions[0]
    assert meter.status is WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY
    assert meter.estimated_remaining_raw_tokens == 1_000.0
    assert (meter.light_task_count, meter.normal_task_count, meter.heavy_task_count) == (
        None,
        None,
        None,
    )


def test_both_task_history_and_capacity_insufficiencies_are_distinct():
    prediction = predict(
        record(0, 100),
        result=capacity_result(estimate(FIVE, Confidence.INSUFFICIENT)),
    )
    meter = prediction.meter_predictions[0]
    assert meter.status is WorkloadPredictionStatus.INSUFFICIENT_TASK_HISTORY_AND_CAPACITY
    assert meter.estimated_remaining_raw_tokens is None
    assert meter.insufficiency_reasons
    assert counts(prediction) == (None, None, None)


def test_different_row_configuration_is_rejected():
    with pytest.raises(WorkloadConfigurationMismatchError):
        predict(record(0, 100), record(1, 200), record(2, 300, configuration=OTHER_CONFIG))


def test_selected_dataset_configuration_must_match_target():
    with pytest.raises(WorkloadConfigurationMismatchError):
        predict(
            record(0, 100),
            selected_configuration=OTHER_CONFIG,
        )


def test_unknown_configuration_value_differs_from_known_value():
    result = capacity_result(estimate(FIVE), configuration=OTHER_CONFIG)
    with pytest.raises(WorkloadConfigurationMismatchError):
        predict(record(0, 100, configuration=CONFIG), result=result)


def test_duplicate_observation_ids_are_rejected():
    duplicate = replace(record(0, 200), raw_total=200)
    with pytest.raises(WorkloadDatasetError, match="duplicate"):
        predict(record(0, 100), duplicate)


def test_nonterminal_lifecycle_is_rejected():
    with pytest.raises(WorkloadDatasetError, match="terminal"):
        predict(record(0, 100, lifecycle=ObservationLifecycle.ACTIVE))


@pytest.mark.parametrize("raw_total", (True, -1, 1.5, float("nan"), float("inf")))
def test_malformed_raw_totals_are_rejected(raw_total):
    with pytest.raises(WorkloadDatasetError):
        predict(record(0, raw_total))


def test_missing_timestamps_are_accepted():
    prediction = predict(
        record(0, 100, started_at=None, ended_at=None, finalized_at=None),
        record(1, 200),
        record(2, 300),
    )
    assert prediction.task_size_distribution.accounting.eligible_observations == 3


def test_oversized_dataset_fails_instead_of_silently_truncating():
    row = record(0, 1)
    dataset = ObservationDataset((row,) * (MAXIMUM_WORKLOAD_OBSERVATIONS + 1))
    with pytest.raises(WorkloadDatasetError, match="safety limit"):
        predict_remaining_workload(dataset=dataset, capacity_result=capacity_result(estimate(FIVE)))


def test_workload_input_bound_matches_the_p017_reader_limit():
    assert MAXIMUM_WORKLOAD_OBSERVATIONS == MAXIMUM_OBSERVATIONS_SAFETY_LIMIT


def test_input_and_output_models_remain_immutable():
    task = record(0, 123)
    dataset = ObservationDataset((task, record(1, 200), record(2, 300)))
    result = capacity_result(estimate(FIVE))
    prediction = predict_remaining_workload(dataset=dataset, capacity_result=result)
    (sample,) = (
        item
        for item in prediction.task_size_distribution.samples
        if item.observation_id == task.observation_id
    )
    with pytest.raises(FrozenInstanceError):
        sample.raw_total = 999
    with pytest.raises(FrozenInstanceError):
        prediction.meter_predictions[0].normal_task_count = 5
    assert dataset.observations[0].raw_total == 123
    assert result.quota_capacity_estimates[0].estimated_remaining_raw_tokens == 1_000.0


def test_deterministic_replay_and_sorted_decision_order():
    rows = (record(9, 900), record(1, 100), record(5, 0))
    first = predict(*rows)
    second = predict(*rows)
    assert first == second
    assert tuple(item.observation_id for item in first.task_size_distribution.decisions) == (
        "observation-000001",
        "observation-000005",
        "observation-000009",
    )


def test_p020_builder_serialization_is_unchanged_by_prediction():
    candidates = tuple(
        RawCapacityCandidate(
            f"capacity-{index}",
            FIVE,
            CONFIG,
            QualityGrade.A,
            100,
            0.1,
            1_000.0,
            10_000.0,
        )
        for index in range(2)
    )
    decisions = tuple(CandidateDecision(item.observation_id, FIVE, item, ()) for item in candidates)
    candidate_set = MeterCandidateSet(
        FIVE,
        CONFIG,
        decisions,
        candidates,
        SampleAccounting(2, 2, 0, 0, 2),
    )
    context = CurrentCapacityResultContext(
        "request-builder",
        configuration_identity(),
        (FIVE,),
        GENERATED_AT,
    )
    result = build_current_capacity_result(
        context=context,
        candidate_sets={FIVE: candidate_set},
        current_positions={FIVE: CurrentQuotaPosition(FIVE, 50.0)},
    )
    before = result.to_json()
    prediction = predict(
        record(0, 100),
        record(1, 200),
        record(2, 300),
        result=result,
    )
    assert prediction.meter_predictions[0].capacity_provider_authority == PROVIDER_AUTHORITY
    assert result.to_json() == before
    assert '"analysis_type":"current_capacity_estimate"' in before
    assert "workload" not in before


def test_p021_history_behavior_is_unchanged_by_prediction():
    rows = (
        record(0, 100, finalized_at="2026-10-01T00:00:00Z"),
        record(1, 200, finalized_at="2026-10-01T00:01:00Z"),
        record(2, 300, finalized_at="2026-10-01T00:02:00Z"),
    )
    dataset = ObservationDataset(rows)
    before = analyze_historical_capacity(dataset)
    predict_remaining_workload(dataset=dataset, capacity_result=capacity_result(estimate(FIVE)))
    assert analyze_historical_capacity(dataset) == before


def test_prediction_does_not_open_sqlite(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("prediction attempted SQLite access")

    monkeypatch.setattr(ObservationDatabase, "open", forbidden)
    prediction = predict(*(record(i, 100) for i in range(3)))
    assert prediction.meter_predictions[0].status is WorkloadPredictionStatus.AVAILABLE


def test_invalid_capacity_result_type_is_rejected():
    with pytest.raises(WorkloadCapacityError):
        predict_remaining_workload(dataset=ObservationDataset(()), capacity_result={})


def test_invalid_dataset_container_types_are_rejected():
    with pytest.raises(WorkloadDatasetError):
        predict_remaining_workload(dataset={}, capacity_result=capacity_result(estimate(FIVE)))
    malformed = replace(ObservationDataset(()), observations=[])
    with pytest.raises(WorkloadDatasetError):
        predict_remaining_workload(
            dataset=malformed,
            capacity_result=capacity_result(estimate(FIVE)),
        )


def test_result_configuration_provenance_and_capacity_timestamp_are_preserved():
    result = capacity_result(estimate(FIVE))
    prediction = predict_remaining_workload(dataset=ObservationDataset(()), capacity_result=result)
    assert prediction.capacity_request_id == "request-022"
    assert prediction.capacity_generated_at == GENERATED_AT
    assert prediction.capacity_estimator_method_version == "1.0.0"
    assert prediction.configuration == result.dataset_configuration.configuration_key
    assert prediction.meter_predictions[0].capacity_provider_authority == PROVIDER_AUTHORITY


def test_task_size_sample_keeps_integer_raw_total_exactly():
    sample = TaskSizeSample("opaque-id", CONFIG, 9_007_199_254_740_993, QualityGrade.A)
    assert sample.raw_total == 9_007_199_254_740_993


def test_task_exclusion_rejects_empty_and_duplicate_reasons():
    with pytest.raises(WorkloadDatasetError):
        TaskSizeExclusion("obs", ())
    with pytest.raises(WorkloadDatasetError):
        TaskSizeExclusion(
            "obs",
            (TaskSizeExclusionReason.ZERO_RAW_TOTAL, TaskSizeExclusionReason.ZERO_RAW_TOTAL),
        )


def test_capacity_estimates_retain_confidence_and_reason_codes_in_result_meter_order():
    result = capacity_result(
        estimate(FIVE, Confidence.LOW, 400.0),
        estimate(WEEKLY, Confidence.MEDIUM, 900.0),
    )
    prediction = predict(*(record(i, 100) for i in range(3)), result=result)
    assert tuple(item.capacity_confidence for item in prediction.meter_predictions) == (
        Confidence.LOW,
        Confidence.MEDIUM,
    )
    assert tuple(item.meter_type for item in prediction.meter_predictions) == (FIVE, WEEKLY)


def test_zero_raw_total_never_becomes_a_one_token_sample():
    prediction = predict(record(0, 0), record(1, 10), record(2, 20))
    assert prediction.task_size_distribution.accounting.eligible_observations == 2
    assert prediction.task_size_distribution.status is TaskSizeDistributionStatus.INSUFFICIENT
    assert prediction.task_size_distribution.exclusions[0].reasons == (
        TaskSizeExclusionReason.ZERO_RAW_TOTAL,
    )
