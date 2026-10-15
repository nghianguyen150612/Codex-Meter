import json
import math
import sqlite3
from dataclasses import FrozenInstanceError, asdict, fields, replace

import pytest

from codex_meter.analysis import (
    ComparisonUnavailableReason,
    HistoricalAnalysisError,
    HistoricalAnalysisPolicy,
    HistoricalDatasetError,
    HistoricalPolicyError,
    TrendDirection,
    analyze_historical_capacity,
    compare_historical_snapshots,
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

CONFIG = ConfigurationKey("plus", "model", "high", "standard", None)
FIVE = QuotaMeterType.FIVE_HOUR
WEEK = QuotaMeterType.WEEKLY


def record(index=0, raw_total=100, **changes):
    base = ObservationRecord(
        f"observation-{index:03}",
        ObservationLifecycle.FINALIZED,
        None,
        None,
        f"2026-01-01T00:{index // 60:02}:{index % 60:02}Z",
        CONFIG,
        QualityGrade.A,
        EvidenceValidity.VALID,
        QualityGrade.A,
        raw_total,
        QuotaEvidenceRecord(
            FIVE, EvidenceValidity.VALID, QualityGrade.A, 100.0, ResetStatus.NOT_DETECTED
        ),
        QuotaEvidenceRecord(
            WEEK, EvidenceValidity.VALID, QualityGrade.A, 50.0, ResetStatus.NOT_DETECTED
        ),
    )
    return replace(base, **changes)


def analyze(*records, window=50, meters=(FIVE, WEEK)):
    return analyze_historical_capacity(
        ObservationDataset(records), meters, HistoricalAnalysisPolicy(window_observations=window)
    )


def series(*records, window=50):
    return analyze(*records, window=window).series[0]


def test_empty_dataset_and_selected_configuration():
    result = analyze()
    assert result.unanchored_observations == 0
    assert len(result.series) == 2
    assert all(
        s.configuration is None and s.snapshots == s.comparisons == () for s in result.series
    )
    selected = analyze_historical_capacity(ObservationDataset((), selected_configuration=CONFIG))
    assert all(s.configuration == CONFIG for s in selected.series)
    assert analyze(meters=()).series == ()


def test_one_observation_remains_visible_and_insufficient():
    s = series(record())
    (snapshot,) = s.snapshots
    assert snapshot.observation_id == "observation-000"
    assert snapshot.confidence is Confidence.INSUFFICIENT
    assert snapshot.sample_accounting.used_observations == 1
    assert snapshot.estimated_full_capacity_raw_tokens is None
    assert snapshot.p25_raw_tokens is snapshot.p50_raw_tokens is snapshot.p75_raw_tokens is None
    assert snapshot.reason_codes == (ReasonCode.UNKNOWN_REASON,)
    assert s.comparisons == ()
    assert not any("used_raw" in f.name or "remaining_raw" in f.name for f in fields(snapshot))
    with pytest.raises(FrozenInstanceError):
        snapshot.observed_at = "changed"


@pytest.mark.parametrize(
    ("values", "direction", "expected"),
    [
        ((100, 200, 300), TrendDirection.INCREASE, 50.0),
        ((300, 200, 100), TrendDirection.DECREASE, -50.0),
        ((100, 100, 100), TrendDirection.UNCHANGED, 0.0),
    ],
)
def test_adjacent_trends(values, direction, expected):
    s = series(*(record(i, value) for i, value in enumerate(values)))
    assert s.snapshots[1].confidence is Confidence.LOW
    assert s.comparisons[0].direction is TrendDirection.UNAVAILABLE
    comparison = s.comparisons[1]
    assert comparison.direction is direction
    assert comparison.absolute_change_raw_tokens == expected
    before = s.snapshots[1].estimated_full_capacity_raw_tokens
    assert comparison.relative_change_percent == 100 * expected / before


def test_two_observations_and_meter_independence():
    result = analyze(record(), record(1, 200))
    five, weekly = result.series
    assert five.snapshots[-1].estimated_full_capacity_raw_tokens == 150.0
    assert weekly.snapshots[-1].estimated_full_capacity_raw_tokens == 300.0
    assert five.snapshots[-1].p50_raw_tokens == 150.0
    assert five.snapshots[-1].sample_accounting.used_observations == 2
    assert weekly.snapshots[-1].sample_accounting.used_observations == 2
    assert analyze(record(), meters=(WEEK, FIVE)).series[0].meter_type is WEEK


def test_mixed_fractional_precision_ties_and_unordered_input():
    times = (
        "2026-01-01T00:00:00Z",
        "2026-01-01T00:00:00.000000001Z",
        "2026-01-01T00:00:00.1Z",
        "2026-01-01T00:00:00.100000000Z",
    )
    records = tuple(record(i, finalized_at=t) for i, t in enumerate(times))
    dataset = ObservationDataset(tuple(reversed(records)))
    s = analyze_historical_capacity(dataset).series[0]
    assert tuple(p.observation_id for p in s.snapshots) == tuple(r.observation_id for r in records)
    assert all(len(p.observed_at) == 30 for p in s.snapshots)
    assert s.snapshots[2].observed_at == s.snapshots[3].observed_at
    assert dataset.observations == tuple(reversed(records))


def test_prefix_invariance_for_every_prefix_and_meter():
    records = tuple(record(i, 100 + i * 20) for i in range(10))
    full = analyze(*records, window=4)
    for size in range(1, len(records) + 1):
        prefix = analyze(*reversed(records[:size]), window=4)
        for small, large in zip(prefix.series, full.series, strict=True):
            assert small.snapshots == large.snapshots[:size]
            assert small.comparisons == large.comparisons[: size - 1]


def test_rolling_eviction_counts_all_terminal_rows_and_recomputes():
    records = (
        record(),
        record(1),
        record(2, raw_total=None),
        record(3, 200, lifecycle=ObservationLifecycle.INCOMPLETE),
        record(4, 300, lifecycle=ObservationLifecycle.INVALID),
    )
    s = series(*records, window=2)
    assert [p.sample_accounting.candidate_observations for p in s.snapshots] == [1, 2, 2, 2, 2]
    assert [p.sample_accounting.used_observations for p in s.snapshots] == [1, 2, 1, 1, 2]
    assert s.snapshots[-1].estimated_full_capacity_raw_tokens == 250.0
    assert s.snapshots[2].sample_accounting.excluded_observations == 1
    assert s.comparisons[-1].direction is TrendDirection.UNAVAILABLE
    assert s.comparisons[-1].unavailable_reason is ComparisonUnavailableReason.INSUFFICIENT_EVIDENCE
    for i, p in enumerate(s.snapshots):
        a = p.sample_accounting
        assert a.candidate_observations <= min(i + 1, 2)
        assert a.candidate_observations == a.valid_observations + a.excluded_observations
        assert a.used_observations == a.valid_observations - a.outliers_removed


@pytest.mark.parametrize("reset_meter", (FIVE, WEEK))
def test_reset_affects_only_its_meter(reset_meter):
    quota = QuotaEvidenceRecord(
        reset_meter, EvidenceValidity.INVALID, QualityGrade.X, None, ResetStatus.DETECTED
    )
    attr = "five_hour" if reset_meter is FIVE else "weekly"
    result = analyze(
        *(record(i, summary_quality=QualityGrade.X, **{attr: quota}) for i in range(3))
    )
    affected = next(s for s in result.series if s.meter_type is reset_meter)
    other = next(s for s in result.series if s.meter_type is not reset_meter)
    assert affected.snapshots[-1].confidence is Confidence.INSUFFICIENT
    assert ReasonCode.QUOTA_RESET_CROSSED in affected.snapshots[-1].reason_codes
    assert affected.snapshots[-1].sample_accounting.excluded_observations == 3
    assert other.snapshots[-1].confidence is Confidence.LOW
    assert other.snapshots[-1].sample_accounting.used_observations == 3


def test_confidence_evolves_and_quality_c_uses_p020_policy():
    records = tuple(record(i) for i in range(8))
    s = series(*records)
    assert [p.confidence for p in s.snapshots] == [
        Confidence.INSUFFICIENT,
        Confidence.LOW,
        Confidence.LOW,
        Confidence.MEDIUM,
        Confidence.MEDIUM,
        Confidence.MEDIUM,
        Confidence.MEDIUM,
        Confidence.HIGH,
    ]
    c_records = records[:-1] + (replace(records[-1], token_quality=QualityGrade.C),)
    c_snapshot = series(*c_records).snapshots[-1]
    assert c_snapshot.confidence is Confidence.MEDIUM
    assert ReasonCode.CONCURRENT_USAGE_POSSIBLE in c_snapshot.reason_codes
    candidates = derive_capacity_candidates(ObservationDataset(c_records), FIVE)
    assessment = assess_capacity_evidence(candidates, aggregate_capacity_candidates(candidates))
    assert c_snapshot.confidence == assessment.confidence
    assert c_snapshot.reason_codes == assessment.reason_codes
    assert c_snapshot.relative_iqr == assessment.relative_iqr == 0.0
    assert (
        series(*(replace(r, token_quality=QualityGrade.C) for r in records))
        .snapshots[-1]
        .confidence
        is Confidence.LOW
    )


@pytest.mark.parametrize("quality", (QualityGrade.D, QualityGrade.X))
def test_quality_exclusions(quality):
    s = series(record(), record(1, token_quality=quality))
    assert s.snapshots[-1].sample_accounting.excluded_observations == 1
    assert s.snapshots[-1].confidence is Confidence.INSUFFICIENT
    assert ReasonCode.TELEMETRY_INCOMPLETE in s.snapshots[-1].reason_codes


def test_zero_quota_delta_and_missing_raw_evidence():
    row = record(1)
    row = replace(row, five_hour=replace(row.five_hour, delta_percentage_points=0.0))
    five, weekly = analyze(record(), row, record(2, raw_total=None)).series
    assert five.snapshots[-1].sample_accounting.valid_observations == 1
    assert weekly.snapshots[-1].sample_accounting.valid_observations == 2
    assert ReasonCode.UNKNOWN_REASON in five.snapshots[-1].reason_codes
    assert ReasonCode.TELEMETRY_INCOMPLETE in five.snapshots[-1].reason_codes


def test_outliers_and_dispersion_are_retained_in_accounting():
    s = series(*(record(i, value) for i, value in enumerate((100, 110, 120, 130, 1000))))
    p = s.snapshots[-1]
    assert p.sample_accounting.valid_observations == 5
    assert p.sample_accounting.outliers_removed == 1
    assert p.sample_accounting.used_observations == 4
    assert p.mad_raw_tokens == 10.0
    assert p.outlier_rate == 0.2
    assert p.p25_raw_tokens == pytest.approx(107.5)
    assert p.p50_raw_tokens == 115.0
    assert p.p75_raw_tokens == 122.5
    assert p.relative_iqr == pytest.approx(15 / 115)
    assert p.estimated_full_capacity_raw_tokens == 115.0


def test_weighted_center_differs_from_p50():
    rows = tuple(
        record(i, value, token_quality=q)
        for i, (value, q) in enumerate(
            ((10, QualityGrade.A), (20, QualityGrade.A), (30, QualityGrade.B), (40, QualityGrade.C))
        )
    )
    p = series(*rows).snapshots[-1]
    assert p.estimated_full_capacity_raw_tokens == 20.0
    assert p.p50_raw_tokens == 25.0


def test_unanchored_accounting_and_fallback_without_fabrication():
    rows = (
        record(0, finalized_at=None),
        record(1, finalized_at=None, ended_at="2026-01-01T00:00:01.1Z"),
        record(2, finalized_at=None, started_at="2026-01-01T00:00:02Z"),
        record(3),
    )
    result = analyze(*rows, window=2)
    assert result.unanchored_observations == 1
    for s in result.series:
        assert [p.observation_id for p in s.snapshots] == [r.observation_id for r in rows[1:]]
        assert s.snapshots[0].observed_at == "2026-01-01T00:00:01.100000000Z"
        assert s.snapshots[-1].sample_accounting.candidate_observations == 2
    only_unanchored = analyze(rows[0])
    assert only_unanchored.unanchored_observations == 1
    assert only_unanchored.series[0].configuration == CONFIG
    assert only_unanchored.series[0].snapshots == ()
    # P018's nonhistorical treatment of missing timing evidence remains unchanged.
    assert len(derive_capacity_candidates(ObservationDataset((rows[0],)), FIVE).candidates) == 1


@pytest.mark.parametrize("field", [f.name for f in fields(CONFIG)])
def test_all_configuration_dimensions_isolated_including_unknown(field):
    unknown = replace(CONFIG, **{field: None})
    known = replace(CONFIG, **{field: "known"})
    with pytest.raises(HistoricalDatasetError, match="configurations"):
        analyze(record(configuration=unknown), record(1, configuration=known))


def test_selected_configuration_must_match_even_unanchored_rows():
    with pytest.raises(HistoricalDatasetError):
        analyze_historical_capacity(
            ObservationDataset(
                (record(finalized_at=None),),
                selected_configuration=replace(CONFIG, codex_version="known"),
            )
        )


@pytest.mark.parametrize("value", (True, False, 0, -1, 501, 1.5, "50", None, math.inf))
@pytest.mark.parametrize("field", ("window_observations", "maximum_snapshots"))
def test_invalid_policy_limits(field, value):
    with pytest.raises(HistoricalPolicyError):
        HistoricalAnalysisPolicy(**{field: value})


def test_dataset_limit_is_not_silent_truncation():
    rows = tuple(record(i) for i in range(501))
    with pytest.raises(HistoricalDatasetError, match="P017"):
        analyze_historical_capacity(ObservationDataset(rows))
    with pytest.raises(HistoricalDatasetError, match="P017"):
        analyze_historical_capacity(
            ObservationDataset(rows[:2]), policy=HistoricalAnalysisPolicy(maximum_snapshots=1)
        )
    assert (
        len(
            analyze_historical_capacity(
                ObservationDataset(rows[:500]),
                (FIVE,),
                HistoricalAnalysisPolicy(window_observations=1),
            )
            .series[0]
            .snapshots
        )
        == 500
    )


@pytest.mark.parametrize("targets", ((FIVE, FIVE), ("five_hour",), [FIVE], (object(),)))
def test_invalid_meter_targets(targets):
    with pytest.raises(HistoricalAnalysisError):
        analyze_historical_capacity(ObservationDataset(()), targets)


@pytest.mark.parametrize(
    "dataset", (None, (), {}, ObservationDataset([]), ObservationDataset((None,)))
)
def test_invalid_dataset_types(dataset):
    with pytest.raises(HistoricalDatasetError):
        analyze_historical_capacity(dataset)


def test_duplicate_ids_rejected_even_without_targets_or_timing():
    with pytest.raises(HistoricalDatasetError, match="duplicate"):
        analyze(record(), record(finalized_at=None), meters=())


@pytest.mark.parametrize(
    "lifecycle",
    [
        s
        for s in ObservationLifecycle
        if s
        not in (
            ObservationLifecycle.FINALIZED,
            ObservationLifecycle.INCOMPLETE,
            ObservationLifecycle.INVALID,
        )
    ],
)
def test_nonterminal_rows_rejected(lifecycle):
    with pytest.raises(HistoricalDatasetError, match="terminal"):
        analyze(record(lifecycle=lifecycle))


@pytest.mark.parametrize(
    "timestamp",
    (
        "",
        "bad",
        "2026-02-30T00:00:00Z",
        "2026-01-01T00:00:00+00:00",
        "2026-01-01T00:00:00.1234567890Z",
        123,
    ),
)
@pytest.mark.parametrize("field", ("started_at", "ended_at", "finalized_at"))
def test_invalid_timestamps_rejected_including_non_anchor(field, timestamp):
    with pytest.raises(HistoricalDatasetError, match="timestamp"):
        analyze(record(**{field: timestamp}))


def test_unsupported_evidence_meter_and_policy_type():
    row = record()
    with pytest.raises(HistoricalDatasetError, match="meter"):
        analyze(replace(row, five_hour=replace(row.five_hour, meter_type="five_hour")))
    with pytest.raises(HistoricalAnalysisError, match="policy"):
        analyze_historical_capacity(ObservationDataset(()), policy=None)


def test_nonfinite_trend_fails_closed_and_serializes_without_nan():
    s = series(record(), record(1), record(2))
    previous, current = s.snapshots[1:]
    previous = replace(previous, estimated_full_capacity_raw_tokens=1e-300)
    current = replace(current, estimated_full_capacity_raw_tokens=1e300)
    comparison = compare_historical_snapshots(previous, current)
    assert comparison.direction is TrendDirection.UNAVAILABLE
    assert comparison.unavailable_reason is ComparisonUnavailableReason.NONFINITE_CHANGE
    assert comparison.absolute_change_raw_tokens is comparison.relative_change_percent is None
    json.dumps(asdict(comparison), allow_nan=False)


@pytest.mark.parametrize(
    "changes",
    (
        {"meter_type": WEEK},
        {"configuration": replace(CONFIG, plan="pro")},
        {"estimator_method_version": "2.0.0"},
    ),
)
def test_comparisons_reject_incompatible_identity(changes):
    s = series(record(), record(1), record(2))
    with pytest.raises(HistoricalAnalysisError, match="matching"):
        compare_historical_snapshots(s.snapshots[1], replace(s.snapshots[2], **changes))


def test_comparisons_reject_reverse_order():
    s = series(record(), record(1), record(2))
    with pytest.raises(HistoricalAnalysisError, match="ordered"):
        compare_historical_snapshots(s.snapshots[2], s.snapshots[1])


def test_deterministic_replay_and_no_storage_access(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("historical analyzer must not open storage")

    monkeypatch.setattr(sqlite3, "connect", forbidden)
    monkeypatch.setattr(ObservationDatabase, "open", forbidden)
    rows = tuple(record(i, 100 + i) for i in range(8))
    a = analyze(*rows, window=4)
    b = analyze(*reversed(rows), window=4)
    assert a == b
    assert json.dumps(asdict(a), allow_nan=False, sort_keys=True) == json.dumps(
        asdict(b), allow_nan=False, sort_keys=True
    )


@pytest.mark.parametrize(
    "changes",
    (
        {"token_quality": "A"},
        {"token_validity": "valid"},
        {"raw_total": True},
        {"raw_total": -1},
        {"configuration": "plus"},
    ),
)
def test_malformed_manual_projections_fail_with_typed_error(changes):
    with pytest.raises(HistoricalDatasetError):
        analyze(record(**changes))


@pytest.mark.parametrize(
    "field",
    ("estimated_full_capacity_raw_tokens", "p25_raw_tokens", "p50_raw_tokens", "p75_raw_tokens"),
)
@pytest.mark.parametrize("value", (math.nan, math.inf, 0.0, -1.0, True))
def test_snapshot_rejects_nonfinite_or_nonpositive_capacities(field, value):
    snapshot = series(record(), record(1)).snapshots[-1]
    with pytest.raises(HistoricalAnalysisError):
        replace(snapshot, **{field: value})


def test_equal_time_prefix_uses_only_ids_already_visited():
    rows = tuple(record(i, 100 * (i + 1), finalized_at="2026-01-01T00:00:00Z") for i in range(3))
    full = series(*reversed(rows))
    assert full.snapshots[:2] == series(*reversed(rows[:2])).snapshots
    assert full.snapshots[1].estimated_full_capacity_raw_tokens == 150.0


def test_unanchored_rows_still_count_toward_dataset_bound():
    rows = tuple(record(i, finalized_at=None) for i in range(2))
    with pytest.raises(HistoricalDatasetError, match="P017"):
        analyze_historical_capacity(
            ObservationDataset(rows), policy=HistoricalAnalysisPolicy(maximum_snapshots=1)
        )


def test_window_of_one_keeps_all_snapshots_insufficient():
    s = series(record(), record(1), record(2), window=1)
    assert all(p.confidence is Confidence.INSUFFICIENT for p in s.snapshots)
    assert all(c.direction is TrendDirection.UNAVAILABLE for c in s.comparisons)


def test_snapshot_ceiling_applies_across_requested_meters():
    rows = tuple(record(i) for i in range(251))
    with pytest.raises(HistoricalDatasetError, match="total"):
        analyze_historical_capacity(ObservationDataset(rows))
    bounded = analyze_historical_capacity(
        ObservationDataset(rows[:250]), policy=HistoricalAnalysisPolicy(window_observations=1)
    )
    assert sum(len(s.snapshots) for s in bounded.series) == 500
