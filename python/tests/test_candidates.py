from dataclasses import replace

import pytest

from codex_meter.estimation import (
    CandidateExclusionReason,
    DatasetConfigurationMismatchError,
    MixedConfigurationDatasetError,
    SampleAccounting,
    derive_capacity_candidates,
)
from codex_meter.history import (
    ConfigurationKey,
    EvidenceValidity,
    ObservationDataset,
    ObservationLifecycle,
    ObservationRecord,
    QualityGrade,
    QuotaEvidenceRecord,
    QuotaMeterType,
    ResetStatus,
)

CONFIGURATION = ConfigurationKey("pro", "gpt-5", "medium", "standard", "1.0")
OTHER_CONFIGURATION = ConfigurationKey("pro", "gpt-5", "high", "standard", "1.0")


def _quota(
    meter_type: QuotaMeterType,
    *,
    validity: EvidenceValidity = EvidenceValidity.VALID,
    quality: QualityGrade = QualityGrade.B,
    delta: float | None = 5.0,
    reset_status: ResetStatus = ResetStatus.NOT_DETECTED,
) -> QuotaEvidenceRecord:
    return QuotaEvidenceRecord(meter_type, validity, quality, delta, reset_status)


def _record(
    observation_id: str = "observation-1",
    *,
    configuration: ConfigurationKey = CONFIGURATION,
    token_validity: EvidenceValidity = EvidenceValidity.VALID,
    token_quality: QualityGrade = QualityGrade.B,
    raw_total: int | None = 1_000_000,
    five_hour: QuotaEvidenceRecord | None = None,
    weekly: QuotaEvidenceRecord | None = None,
    summary_quality: QualityGrade = QualityGrade.B,
) -> ObservationRecord:
    return ObservationRecord(
        observation_id=observation_id,
        lifecycle=ObservationLifecycle.FINALIZED,
        started_at="2026-01-01T00:00:00.000000000Z",
        ended_at="2026-01-01T00:01:00.000000000Z",
        finalized_at="2026-01-01T00:02:00.000000000Z",
        configuration=configuration,
        summary_quality=summary_quality,
        token_validity=token_validity,
        token_quality=token_quality,
        raw_total=raw_total,
        five_hour=five_hour or _quota(QuotaMeterType.FIVE_HOUR),
        weekly=weekly or _quota(QuotaMeterType.WEEKLY),
    )


def _dataset(*records: ObservationRecord, selected: ConfigurationKey | None = None):
    return ObservationDataset(tuple(records), selected_configuration=selected)


def test_canonical_capacity_equation() -> None:
    record = _record(
        raw_total=4_500_000,
        five_hour=_quota(QuotaMeterType.FIVE_HOUR, delta=20.0),
    )

    result = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR)

    candidate = result.candidates[0]
    assert candidate.raw_total == 4_500_000
    assert candidate.raw_tokens_per_percentage_point == 225_000
    assert candidate.full_capacity_raw_tokens == 22_500_000


def test_simple_capacity_equation() -> None:
    record = _record(
        raw_total=100,
        five_hour=_quota(QuotaMeterType.FIVE_HOUR, delta=25.0),
    )

    candidate = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR).candidates[0]

    assert candidate.raw_tokens_per_percentage_point == 4
    assert candidate.full_capacity_raw_tokens == 400


def test_meter_branches_are_independent() -> None:
    record = _record(
        raw_total=1_000_000,
        five_hour=_quota(QuotaMeterType.FIVE_HOUR, delta=5.0),
        weekly=_quota(
            QuotaMeterType.WEEKLY,
            validity=EvidenceValidity.INVALID,
            quality=QualityGrade.X,
            delta=None,
            reset_status=ResetStatus.DETECTED,
        ),
        summary_quality=QualityGrade.X,
    )

    five_hour = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR)
    weekly = derive_capacity_candidates(_dataset(record), QuotaMeterType.WEEKLY)

    assert len(five_hour.candidates) == 1
    assert five_hour.candidates[0].quality is QualityGrade.B
    assert five_hour.candidates[0].full_capacity_raw_tokens == 20_000_000
    assert weekly.candidates == ()
    assert weekly.decisions[0].exclusion_reasons == (
        CandidateExclusionReason.METER_NOT_VALID,
        CandidateExclusionReason.METER_QUALITY_INELIGIBLE,
        CandidateExclusionReason.DELTA_UNAVAILABLE,
        CandidateExclusionReason.RESET_CROSSED,
    )


def test_weekly_candidate_survives_five_hour_reset() -> None:
    record = _record(
        five_hour=_quota(
            QuotaMeterType.FIVE_HOUR,
            validity=EvidenceValidity.INVALID,
            quality=QualityGrade.X,
            delta=None,
            reset_status=ResetStatus.DETECTED,
        ),
        weekly=_quota(QuotaMeterType.WEEKLY, delta=10.0),
        summary_quality=QualityGrade.X,
    )

    weekly = derive_capacity_candidates(_dataset(record), QuotaMeterType.WEEKLY)

    assert len(weekly.candidates) == 1
    assert weekly.candidates[0].full_capacity_raw_tokens == 10_000_000


@pytest.mark.parametrize("quality", list(QualityGrade))
def test_quality_eligibility(quality: QualityGrade) -> None:
    record = _record(token_quality=quality)
    result = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR)

    if quality in (QualityGrade.A, QualityGrade.B, QualityGrade.C):
        assert len(result.candidates) == 1
        assert result.candidates[0].quality is max(
            (quality, QualityGrade.B), key=lambda grade: grade.severity
        )
    else:
        assert result.candidates == ()
        assert result.decisions[0].exclusion_reasons == (
            CandidateExclusionReason.TOKEN_QUALITY_INELIGIBLE,
        )


def test_candidate_quality_is_worst_relevant_evidence_only() -> None:
    record = _record(
        token_quality=QualityGrade.C,
        five_hour=_quota(
            QuotaMeterType.FIVE_HOUR,
            quality=QualityGrade.B,
            delta=5.0,
        ),
        weekly=_quota(
            QuotaMeterType.WEEKLY,
            validity=EvidenceValidity.INVALID,
            quality=QualityGrade.X,
            delta=None,
            reset_status=ResetStatus.DETECTED,
        ),
        summary_quality=QualityGrade.X,
    )

    result = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR)

    assert result.candidates[0].quality is QualityGrade.C


@pytest.mark.parametrize(
    ("validity", "quality", "reason"),
    [
        (
            EvidenceValidity.INVALID,
            QualityGrade.X,
            CandidateExclusionReason.METER_NOT_VALID,
        ),
        (
            EvidenceValidity.VALID,
            QualityGrade.D,
            CandidateExclusionReason.METER_QUALITY_INELIGIBLE,
        ),
    ],
)
def test_target_meter_validity_and_quality_are_independent(
    validity: EvidenceValidity,
    quality: QualityGrade,
    reason: CandidateExclusionReason,
) -> None:
    quota = _quota(
        QuotaMeterType.FIVE_HOUR,
        validity=validity,
        quality=quality,
        delta=5.0 if validity is EvidenceValidity.VALID else None,
    )

    result = derive_capacity_candidates(
        _dataset(_record(five_hour=quota)), QuotaMeterType.FIVE_HOUR
    )

    assert result.candidates == ()
    assert reason in result.decisions[0].exclusion_reasons


@pytest.mark.parametrize(
    ("field", "reason"),
    [
        ("token_validity", CandidateExclusionReason.TOKEN_NOT_VALID),
        ("raw_total", CandidateExclusionReason.RAW_TOTAL_UNAVAILABLE),
    ],
)
def test_token_evidence_exclusions(field: str, reason: CandidateExclusionReason) -> None:
    record = _record(
        **{field: EvidenceValidity.UNAVAILABLE} if field == "token_validity" else {field: None}
    )

    result = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR)

    assert result.candidates == ()
    assert reason in result.decisions[0].exclusion_reasons


def test_zero_raw_total_is_distinct_from_missing() -> None:
    result = derive_capacity_candidates(_dataset(_record(raw_total=0)), QuotaMeterType.FIVE_HOUR)

    assert result.decisions[0].exclusion_reasons == (CandidateExclusionReason.ZERO_RAW_TOTAL,)


@pytest.mark.parametrize(
    "quota, reason",
    [
        (
            _quota(QuotaMeterType.FIVE_HOUR, delta=None),
            CandidateExclusionReason.DELTA_UNAVAILABLE,
        ),
        (
            _quota(QuotaMeterType.FIVE_HOUR, delta=0.0),
            CandidateExclusionReason.ZERO_QUOTA_DELTA,
        ),
        (
            _quota(
                QuotaMeterType.FIVE_HOUR,
                validity=EvidenceValidity.INVALID,
                quality=QualityGrade.X,
                delta=None,
                reset_status=ResetStatus.DETECTED,
            ),
            CandidateExclusionReason.RESET_CROSSED,
        ),
    ],
)
def test_meter_exclusions(quota: QuotaEvidenceRecord, reason: CandidateExclusionReason) -> None:
    result = derive_capacity_candidates(
        _dataset(_record(five_hour=quota)), QuotaMeterType.FIVE_HOUR
    )

    assert result.candidates == ()
    assert reason in result.decisions[0].exclusion_reasons


def test_multiple_exclusion_reasons_are_retained_in_stable_order() -> None:
    record = _record(
        token_validity=EvidenceValidity.UNAVAILABLE,
        token_quality=QualityGrade.X,
        raw_total=None,
        five_hour=_quota(
            QuotaMeterType.FIVE_HOUR,
            validity=EvidenceValidity.INVALID,
            quality=QualityGrade.X,
            delta=None,
            reset_status=ResetStatus.DETECTED,
        ),
    )

    decision = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR).decisions[0]

    assert decision.exclusion_reasons == (
        CandidateExclusionReason.TOKEN_NOT_VALID,
        CandidateExclusionReason.TOKEN_QUALITY_INELIGIBLE,
        CandidateExclusionReason.RAW_TOTAL_UNAVAILABLE,
        CandidateExclusionReason.METER_NOT_VALID,
        CandidateExclusionReason.METER_QUALITY_INELIGIBLE,
        CandidateExclusionReason.DELTA_UNAVAILABLE,
        CandidateExclusionReason.RESET_CROSSED,
    )


def test_tiny_positive_delta_is_not_capped_or_filtered() -> None:
    record = _record(
        raw_total=1_000,
        five_hour=_quota(QuotaMeterType.FIVE_HOUR, delta=0.000001),
    )

    candidate = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR).candidates[0]

    assert candidate.full_capacity_raw_tokens == 100_000_000_000


def test_configuration_guards() -> None:
    mixed = _dataset(_record("one"), _record("two", configuration=OTHER_CONFIGURATION))
    with pytest.raises(MixedConfigurationDatasetError):
        derive_capacity_candidates(mixed, QuotaMeterType.FIVE_HOUR)

    mismatched = _dataset(
        _record("one"),
        _record("two", configuration=OTHER_CONFIGURATION),
        selected=CONFIGURATION,
    )
    with pytest.raises(DatasetConfigurationMismatchError):
        derive_capacity_candidates(mismatched, QuotaMeterType.FIVE_HOUR)


def test_empty_dataset_preserves_selected_configuration() -> None:
    result = derive_capacity_candidates(_dataset(selected=CONFIGURATION), QuotaMeterType.WEEKLY)

    assert result.configuration == CONFIGURATION
    assert result.decisions == ()
    assert result.candidates == ()
    assert result.accounting == SampleAccounting(0, 0, 0, 0, 0)


def test_empty_unselected_dataset_has_no_configuration() -> None:
    result = derive_capacity_candidates(ObservationDataset(()), QuotaMeterType.WEEKLY)

    assert result.configuration is None
    assert result.accounting.candidate_observations == 0


def test_accounting_and_reason_counts_are_row_based() -> None:
    excluded = _record(
        "excluded",
        raw_total=None,
        five_hour=_quota(QuotaMeterType.FIVE_HOUR, delta=0.0),
    )
    included = _record("included")
    result = derive_capacity_candidates(_dataset(excluded, included), QuotaMeterType.FIVE_HOUR)

    assert result.accounting == SampleAccounting(2, 1, 1, 0, 1)
    assert result.excluded_by_reason(CandidateExclusionReason.RAW_TOTAL_UNAVAILABLE) == 1
    assert result.excluded_by_reason(CandidateExclusionReason.ZERO_QUOTA_DELTA) == 1
    assert result.accounting.excluded_observations == 1


def test_order_and_replay_are_deterministic() -> None:
    dataset = _dataset(
        _record("first", raw_total=100),
        _record("second", raw_total=200),
        _record("third", raw_total=0),
    )

    first = derive_capacity_candidates(dataset, QuotaMeterType.FIVE_HOUR)
    second = derive_capacity_candidates(dataset, QuotaMeterType.FIVE_HOUR)

    assert first == second
    assert [decision.observation_id for decision in first.decisions] == [
        "first",
        "second",
        "third",
    ]
    assert [candidate.observation_id for candidate in first.candidates] == [
        "first",
        "second",
    ]


def test_lifecycle_does_not_override_relevant_evidence() -> None:
    record = replace(_record(), lifecycle=ObservationLifecycle.INCOMPLETE)

    result = derive_capacity_candidates(_dataset(record), QuotaMeterType.FIVE_HOUR)

    assert len(result.candidates) == 1
