"""P026 exploratory calibration, integrity, estimator compatibility, and privacy."""

import math
import socket
import sqlite3
import subprocess
from dataclasses import FrozenInstanceError, fields, replace

import pytest

from codex_meter.analysis import (
    BENCHMARK_ANALYSIS_METHOD_VERSION,
    BenchmarkAnalysisPolicy,
    BenchmarkDatasetError,
    BenchmarkEvidenceDataset,
    BenchmarkExperimentEvidence,
    BenchmarkNumericalError,
    BenchmarkPolicyError,
    BenchmarkRegimeBoundary,
    BenchmarkRunEvidence,
    analyze_benchmark_calibration,
)
from codex_meter.analysis import (
    BenchmarkAnalysisWarning as Warning,
)
from codex_meter.analysis import (
    BenchmarkCalibrationStatus as Status,
)
from codex_meter.analysis import (
    BenchmarkEvidenceProvenance as Provenance,
)
from codex_meter.analysis import (
    BenchmarkIsolationStatus as Isolation,
)
from codex_meter.analysis import (
    BenchmarkMeasurementStatus as Measurement,
)
from codex_meter.analysis import (
    BenchmarkRecommendation as Recommendation,
)
from codex_meter.estimation import (
    ESTIMATOR_METHOD_VERSION,
    Confidence,
    aggregate_capacity_candidates,
    assess_capacity_evidence,
    derive_capacity_candidates,
)
from codex_meter.estimation import (
    CandidateExclusionReason as Exclusion,
)
from codex_meter.history import (
    ConfigurationKey,
    ObservationDataset,
    ObservationRecord,
    QuotaEvidenceRecord,
    QuotaMeterType,
)
from codex_meter.history import (
    EvidenceValidity as Validity,
)
from codex_meter.history import (
    ObservationLifecycle as Lifecycle,
)
from codex_meter.history import (
    QualityGrade as Quality,
)
from codex_meter.history import (
    ResetStatus as Reset,
)

FIVE, WEEK = QuotaMeterType.FIVE_HOUR, QuotaMeterType.WEEKLY
CONFIG = ConfigurationKey("plus", "model", "high", "standard", "1.0")
TIME = "2026-01-01T00:00:00Z"


def quota(meter=FIVE, **changes):
    return replace(
        QuotaEvidenceRecord(meter, Validity.VALID, Quality.A, 1.0, Reset.NOT_DETECTED), **changes
    )


def record(index, capacity=20_000_000, **changes):
    return replace(
        ObservationRecord(
            f"obs:{index:064x}",
            Lifecycle.FINALIZED,
            None,
            None,
            TIME,
            CONFIG,
            Quality.A,
            Validity.VALID,
            Quality.A,
            capacity // 100,
            quota(),
            quota(WEEK),
        ),
        **changes,
    )


def run(index, **changes):
    return replace(
        BenchmarkRunEvidence(
            "experiment",
            f"run-{index}",
            index,
            "workload",
            f"obs:{index:064x}",
            Measurement.COMPLETE,
            Isolation.VERIFIED_ISOLATED,
        ),
        **changes,
    )


def evidence(runs=(), *, experiments=None, provenance=Provenance.SYNTHETIC_FIXTURE, boundaries=()):
    return BenchmarkEvidenceDataset(
        provenance,
        (BenchmarkExperimentEvidence("experiment", "workload", CONFIG),)
        if experiments is None
        else experiments,
        tuple(runs),
        boundaries,
    )


def analyze(rows=(), *, links=None, reference=None, **kwargs):
    return analyze_benchmark_calibration(
        benchmark_dataset=ObservationDataset(tuple(rows)),
        benchmark_links=evidence(tuple(run(i) for i in range(len(rows))))
        if links is None
        else links,
        reference_dataset=None if reference is None else ObservationDataset(tuple(reference)),
        **kwargs,
    )


def comparison(b=(20_000_000,) * 4, r=(20_000_000,) * 4, **kwargs):
    return analyze(
        tuple(record(i, value) for i, value in enumerate(b)),
        reference=tuple(record(i + 1000, value) for i, value in enumerate(r)),
        **kwargs,
    ).comparisons[0]


@pytest.mark.parametrize("count", range(5))
def test_empty_through_four_runs_preserve_insufficiency(count):
    result = analyze(
        tuple(record(i) for i in range(count)), reference=tuple(record(i + 1000) for i in range(4))
    )
    assert result.method_version == BENCHMARK_ANALYSIS_METHOD_VERSION == "1.0.0"
    summary = result.benchmark_summaries[0]
    assert summary.sample_accounting.used_observations == count
    assert summary.confidence is (
        Confidence.INSUFFICIENT if count < 2 else Confidence.LOW if count < 4 else Confidence.MEDIUM
    )
    assert (summary.weighted_median_raw_tokens is None) == (count < 2)
    assert result.comparisons[0].status is (
        Status.INSUFFICIENT_EVIDENCE if count < 4 else Status.DESCRIPTIVELY_CONSISTENT
    )


def test_no_experiments_no_automatic_synthetic_measurements():
    result = analyze(links=evidence(experiments=()))
    assert result.benchmark_summaries == result.comparisons == ()
    assert result.configuration is None
    assert result.evidence_accounting.run_count == 0


@pytest.mark.parametrize(
    "change",
    [
        {"run_id": "run-0"},
        {"repetition_index": 0},
        {"observation_id": f"obs:{0:064x}"},
    ],
)
def test_duplicate_run_repetition_or_observation_association_rejected(change):
    with pytest.raises(BenchmarkDatasetError, match="duplicate"):
        analyze((record(0), record(1)), links=evidence((run(0), run(1, **change))))


def test_duplicate_observation_rows_rejected():
    with pytest.raises(BenchmarkDatasetError):
        analyze((record(0), record(0)))


def test_unresolved_links_and_unlinked_observations_accounted_separately():
    result = analyze((record(0),), links=evidence((run(1),)))
    counts = result.evidence_accounting
    assert counts.unresolved_links == counts.unlinked_observations == 1
    assert counts.resolved_links == counts.eligible_linked_runs == 0
    assert Warning.UNRESOLVED_LINKS in result.warnings
    assert Warning.UNLINKED_OBSERVATIONS in result.warnings
    assert result.benchmark_summaries[0].sample_accounting.candidate_observations == 0


@pytest.mark.parametrize(
    "change", [{"workload_key": "unknown"}, {"experiment_id": "unknown"}, {"repetition_index": 99}]
)
def test_run_must_match_declared_scope(change):
    exp = BenchmarkExperimentEvidence("experiment", "workload", CONFIG, planned_run_count=4)
    with pytest.raises(BenchmarkDatasetError):
        analyze((record(0),), links=evidence((run(0, **change),), experiments=(exp,)))


def test_mixed_workloads_and_experiments_are_never_pooled():
    experiments = tuple(
        BenchmarkExperimentEvidence(f"experiment-{i}", "same" if i < 2 else "other", CONFIG)
        for i in range(3)
    )
    runs = tuple(
        run(i, experiment_id=f"experiment-{i}", workload_key=experiments[i].workload_key)
        for i in range(3)
    )
    result = analyze(
        tuple(record(i, (i + 1) * 10_000_000) for i in range(3)),
        links=evidence(runs, experiments=experiments),
    )
    assert result.evidence_accounting.experiment_count == 3
    assert result.evidence_accounting.run_count == 3
    assert len(result.benchmark_summaries) == 6
    assert all(s.sample_accounting.used_observations == 1 for s in result.benchmark_summaries)
    assert Warning.SEPARATE_EXPERIMENTS in result.warnings


@pytest.mark.parametrize("dimension", [f.name for f in fields(CONFIG)])
@pytest.mark.parametrize("replacement", [None, "different"])
def test_each_configuration_dimension_is_exact(dimension, replacement):
    different = replace(CONFIG, **{dimension: replacement})
    with pytest.raises(BenchmarkDatasetError):
        analyze((record(0, configuration=different),))
    with pytest.raises(BenchmarkDatasetError):
        analyze((record(0), record(1, configuration=different)))
    with pytest.raises(BenchmarkDatasetError):
        analyze((record(0),), reference=(record(1000, configuration=different),))


def test_unknown_configuration_is_preserved_and_warned():
    config = ConfigurationKey(None, None, None, None, None)
    exp = BenchmarkExperimentEvidence("experiment", "workload", config)
    result = analyze(
        (record(0, configuration=config),), links=evidence((run(0),), experiments=(exp,))
    )
    assert result.configuration == config
    assert Warning.UNKNOWN_CONFIGURATION in result.warnings


@pytest.mark.parametrize("status", [Measurement.INCOMPLETE, Measurement.INVALID])
def test_incomplete_or_invalid_measurement_excluded_without_quality_promotion(status):
    result = analyze(
        (record(0, token_quality=Quality.B),), links=evidence((run(0, measurement_status=status),))
    )
    counts = result.evidence_accounting
    assert counts.resolved_links == 1
    assert counts.eligible_linked_runs == 0
    assert counts.incomplete_runs + counts.invalid_runs == 1
    assert Warning.MEASUREMENT_EXCLUDED in result.warnings


@pytest.mark.parametrize("lifecycle", [Lifecycle.INCOMPLETE, Lifecycle.INVALID])
def test_terminal_lifecycle_does_not_override_per_meter_p018_eligibility(lifecycle):
    result = analyze((record(0, lifecycle=lifecycle),))
    assert result.evidence_accounting.eligible_linked_runs == 1
    assert result.benchmark_summaries[0].sample_accounting.valid_observations == 1


@pytest.mark.parametrize("provenance", list(Provenance))
def test_provenance_retained_at_every_projection(provenance):
    rows = tuple(record(i) for i in range(4))
    result = analyze(
        rows,
        links=evidence(tuple(run(i) for i in range(4)), provenance=provenance),
        reference=tuple(record(i + 1000) for i in range(4)),
    )
    assert result.provenance is provenance
    assert all(s.provenance is provenance for s in result.benchmark_summaries)
    assert all(c.provenance is provenance for c in result.comparisons)
    assert all(c.reference.provenance is provenance for c in result.comparisons)
    assert Warning.NO_TRUSTED_PRODUCTION_EXPORT in result.warnings
    assert not hasattr(result, "verified")


@pytest.mark.parametrize("provenance", ["runtime_attested", "verified", True, "synthetic_fixture"])
def test_no_caller_claim_manufactures_trusted_provenance(provenance):
    with pytest.raises(BenchmarkDatasetError):
        evidence(provenance=provenance)


def test_arbitrary_payload_and_verified_fields_not_accepted():
    with pytest.raises(TypeError):
        BenchmarkEvidenceDataset(Provenance.SYNTHETIC_FIXTURE, (), (), verified=True)
    with pytest.raises(TypeError):
        BenchmarkRunEvidence(
            "experiment",
            "run",
            0,
            "workload",
            f"obs:{0:064x}",
            Measurement.COMPLETE,
            Isolation.UNKNOWN,
            metadata={},
        )


@pytest.mark.parametrize(
    "changes",
    [
        {"experiment_id": ""},
        {"run_id": "1run"},
        {"workload_key": "with spaces"},
        {"run_id": "r" * 65},
        {"run_id": "é"},
        {"observation_id": "observation-1"},
        {"observation_id": "obs:" + "A" * 64},
        {"repetition_index": -1},
        {"repetition_index": True},
        {"repetition_index": 100},
        {"repetition_index": 1.0},
        {"measurement_status": "complete"},
        {"measurement_status": "executed"},
        {"isolation_status": "verified_isolated"},
        {"started_at": "bad-time"},
        {"started_at": "2026-01-02T00:00:00Z", "ended_at": TIME},
    ],
)
def test_invalid_run_metadata_rejected_privately(changes):
    with pytest.raises(BenchmarkDatasetError) as error:
        run(0, **changes)
    assert "bad-time" not in str(error.value)
    assert "with spaces" not in str(error.value)


@pytest.mark.parametrize(
    "changes",
    [
        {"lifecycle": Lifecycle.ACTIVE},
        {"raw_total": -1},
        {"raw_total": True},
        {"raw_total": 1.5},
        {"token_quality": "A"},
        {"token_validity": "valid"},
        {"summary_quality": "A"},
        {"five_hour": quota(delta_percentage_points=math.nan)},
        {"five_hour": quota(delta_percentage_points=math.inf)},
        {"five_hour": quota(delta_percentage_points=-1)},
        {"five_hour": quota(delta_percentage_points=101)},
        {"five_hour": quota(delta_percentage_points=True)},
        {"five_hour": quota(meter_type=WEEK)},
        {"five_hour": quota(validity="valid")},
        {"five_hour": quota(quality="A")},
        {"five_hour": quota(reset_status="detected")},
        {"configuration": replace(CONFIG, model=123)},
        {"configuration": None},
        {"finalized_at": "bad"},
        {"started_at": "2026-01-02T00:00:00Z", "ended_at": TIME},
    ],
)
def test_malformed_observation_dataset_rejected(changes):
    with pytest.raises(BenchmarkDatasetError):
        analyze((record(0, **changes),))


@pytest.mark.parametrize("meter", [FIVE, WEEK])
@pytest.mark.parametrize("quality", list(Quality))
def test_quality_eligibility_never_promoted_by_benchmark_or_isolation(meter, quality):
    rows = tuple(record(i, token_quality=quality) for i in range(4))
    summary = analyze(rows, target_meters=(meter,)).benchmark_summaries[0]
    assert summary.sample_accounting.used_observations == (
        4 if quality in (Quality.A, Quality.B, Quality.C) else 0
    )
    assert summary.c_quality_used == (4 if quality is Quality.C else 0)
    assert all(d.candidate is None or d.candidate.quality is quality for d in summary.decisions)
    assert rows[0].token_quality is quality


@pytest.mark.parametrize("meter", [FIVE, WEEK])
@pytest.mark.parametrize(
    ("changes", "reason"),
    [
        ({"reset_status": Reset.DETECTED}, Exclusion.RESET_CROSSED),
        ({"delta_percentage_points": 0}, Exclusion.ZERO_QUOTA_DELTA),
        ({"delta_percentage_points": None}, Exclusion.DELTA_UNAVAILABLE),
        ({"validity": Validity.UNAVAILABLE}, Exclusion.METER_NOT_VALID),
        ({"quality": Quality.D}, Exclusion.METER_QUALITY_INELIGIBLE),
    ],
)
def test_meter_failures_are_independent_and_exclusions_detailed(meter, changes, reason):
    field = "five_hour" if meter is FIVE else "weekly"
    rows = tuple(record(i, **{field: quota(meter, **changes)}) for i in range(4))
    summaries = analyze(rows).benchmark_summaries
    affected = next(s for s in summaries if s.meter_type is meter)
    other = next(s for s in summaries if s.meter_type is not meter)
    assert affected.sample_accounting.excluded_observations == 4
    assert affected.sample_accounting.outliers_removed == 0
    assert all(reason in d.exclusion_reasons for d in affected.decisions)
    assert other.sample_accounting.used_observations == 4
    assert affected.reset_crossings == (4 if reason is Exclusion.RESET_CROSSED else 0)
    assert affected.missing_meter_evidence == (
        4 if reason in (Exclusion.DELTA_UNAVAILABLE, Exclusion.METER_NOT_VALID) else 0
    )


@pytest.mark.parametrize(
    ("value", "reason"), [(None, Exclusion.RAW_TOTAL_UNAVAILABLE), (0, Exclusion.ZERO_RAW_TOTAL)]
)
def test_missing_and_zero_tokens_distinct(value, reason):
    summary = analyze((record(0, raw_total=value),)).benchmark_summaries[0]
    assert reason in summary.decisions[0].exclusion_reasons
    assert summary.weighted_median_raw_tokens is None


def test_multiple_exclusion_reasons_count_one_row():
    result = analyze(
        (
            record(
                0,
                raw_total=None,
                five_hour=quota(
                    validity=Validity.UNAVAILABLE,
                    delta_percentage_points=None,
                    reset_status=Reset.DETECTED,
                ),
            ),
        )
    )
    summary = result.benchmark_summaries[0]
    assert summary.sample_accounting.excluded_observations == 1
    assert len(summary.decisions[0].exclusion_reasons) == 4


@pytest.mark.parametrize(
    "values",
    [
        (10_000_000, 20_000_000, 30_000_000, 40_000_000),
        (20_000_000,) * 8 + (100_000_000,),
        (18_000_000, 19_000_000, 20_000_000, 21_000_000, 22_000_000, 100_000_000),
    ],
)
def test_authoritative_p018_p019_p020_values_used_exactly(values):
    rows = tuple(
        record(i, value, token_quality=Quality.C if i == 0 else Quality.A)
        for i, value in enumerate(values)
    )
    summary = analyze(rows, target_meters=(FIVE,)).benchmark_summaries[0]
    candidates = derive_capacity_candidates(ObservationDataset(rows), FIVE)
    robust = aggregate_capacity_candidates(candidates)
    assessment = assess_capacity_evidence(candidates, robust)
    for name in (
        "weighted_median_raw_tokens",
        "p25_raw_tokens",
        "p50_raw_tokens",
        "p75_raw_tokens",
        "mad_raw_tokens",
        "used_observation_ids",
        "outlier_observation_ids",
    ):
        assert getattr(summary, name) == getattr(robust, name)
    assert summary.decisions == candidates.decisions
    assert summary.sample_accounting == robust.accounting
    assert summary.confidence is assessment.confidence
    assert summary.relative_iqr == assessment.relative_iqr
    assert summary.outlier_rate == assessment.outlier_rate
    assert summary.estimator_reason_codes == assessment.reason_codes
    assert ESTIMATOR_METHOD_VERSION == "1.0.0"


def test_type7_percentiles_and_weighted_center_are_distinct():
    rows = tuple(
        record(i, value * 1_000_000, token_quality=Quality.C if i > 0 else Quality.A)
        for i, value in enumerate((10, 20, 30, 40))
    )
    summary = analyze(rows).benchmark_summaries[0]
    assert (summary.p25_raw_tokens, summary.p50_raw_tokens, summary.p75_raw_tokens) == (
        17_500_000,
        25_000_000,
        32_500_000,
    )
    assert summary.weighted_median_raw_tokens == 10_000_000
    assert summary.mad_raw_tokens == 10_000_000


@pytest.mark.parametrize(
    ("b", "expected", "percent", "ratio"),
    [
        (20_000_000, Status.DESCRIPTIVELY_CONSISTENT, 0, 1),
        (25_000_000, Status.MATERIAL_DIFFERENCE_SEPARATED, 25, 1.25),
        (15_000_000, Status.MATERIAL_DIFFERENCE_SEPARATED, -25, 0.75),
        (24_000_000, Status.MATERIAL_DIFFERENCE_SEPARATED, 20, 1.2),
        (16_000_000, Status.MATERIAL_DIFFERENCE_SEPARATED, -20, 0.8),
        (23_999_900, Status.DESCRIPTIVELY_CONSISTENT, 19.9995, 1.199995),
        (24_000_100, Status.MATERIAL_DIFFERENCE_SEPARATED, 20.0005, 1.200005),
    ],
)
def test_comparison_formulas_and_inclusive_threshold(b, expected, percent, ratio):
    result = comparison(b=(b,) * 4)
    assert result.status is expected
    assert result.absolute_difference_raw_tokens == b - 20_000_000
    assert result.relative_difference_percent == pytest.approx(percent)
    assert result.relative_difference == pytest.approx(percent / 100)
    assert result.benchmark_to_reference_ratio == ratio
    assert (Recommendation.REVIEW_ESTIMATOR_CALIBRATION in result.recommendations) == (
        expected is not Status.DESCRIPTIVELY_CONSISTENT
    )


@pytest.mark.parametrize("touching", [False, True])
def test_iqr_overlap_including_touching_endpoints(touching):
    # IQR reference [18, 22], benchmark [22, 26] (touch) or [21, 25] (overlap).
    reference = (16, 18, 18, 20, 20, 22, 22, 24)
    benchmark = tuple(v + (4 if touching else 3) for v in reference)
    result = comparison(
        b=tuple(v * 1_000_000 for v in benchmark),
        r=tuple(v * 1_000_000 for v in reference),
        policy=BenchmarkAnalysisPolicy(material_relative_difference=0.15),
    )
    assert result.iqr_overlap is True
    assert result.status is Status.MATERIAL_DIFFERENCE_OVERLAPPING
    if touching:
        assert result.benchmark.p25_raw_tokens == result.reference.p75_raw_tokens


@pytest.mark.parametrize("side", ["benchmark", "reference"])
@pytest.mark.parametrize("reason", ["count", "quality", "dispersion"])
def test_each_side_requires_minimum_retained_count_and_medium_confidence(side, reason):
    b = tuple(record(i) for i in range(4))
    r = tuple(record(i + 1000) for i in range(4))
    selected = b if side == "benchmark" else r
    if reason == "count":
        selected = selected[:3]
    elif reason == "quality":
        selected = tuple(replace(row, token_quality=Quality.C) for row in selected)
    else:
        selected = tuple(
            replace(row, raw_total=value)
            for row, value in zip(selected, (10_000, 20_000, 30_000, 40_000), strict=True)
        )
    result = analyze(
        selected if side == "benchmark" else b, reference=selected if side == "reference" else r
    ).comparisons[0]
    assert result.status is Status.INSUFFICIENT_EVIDENCE
    assert result.relative_difference is result.benchmark_to_reference_ratio is None
    assert Recommendation.COLLECT_MORE_SAMPLES in result.recommendations


def test_post_outlier_count_controls_comparison():
    result = comparison(b=(20_000_000,) * 3 + (100_000_000,))
    assert result.benchmark.sample_accounting.outliers_removed == 1
    assert result.benchmark.sample_accounting.used_observations == 3
    assert result.status is Status.INSUFFICIENT_EVIDENCE


@pytest.mark.parametrize("resolved", [True, False])
def test_reference_overlap_rejected_even_for_unresolved_claims(resolved):
    with pytest.raises(BenchmarkDatasetError, match="overlap"):
        analyze((record(0),) if resolved else (), links=evidence((run(0),)), reference=(record(0),))


def test_no_reference_is_typed_unavailable_and_different_meters_never_substituted():
    result = analyze(tuple(record(i) for i in range(4)))
    assert all(c.status is Status.COMPARISON_UNAVAILABLE for c in result.comparisons)
    ref = tuple(
        record(i + 1000, weekly=quota(WEEK, validity=Validity.UNAVAILABLE)) for i in range(4)
    )
    result = analyze(tuple(record(i) for i in range(4)), reference=ref)
    assert result.comparisons[0].status is Status.DESCRIPTIVELY_CONSISTENT
    assert result.comparisons[1].status is Status.INSUFFICIENT_EVIDENCE


@pytest.mark.parametrize("isolation", list(Isolation))
def test_declared_isolation_remains_a_claim(isolation):
    result = analyze(
        (record(0, token_quality=Quality.C),), links=evidence((run(0, isolation_status=isolation),))
    )
    assert result.benchmark_summaries[0].c_quality_used == 1
    assert result.evidence_accounting.unknown_isolation == (isolation is Isolation.UNKNOWN)
    assert result.evidence_accounting.possible_concurrent_isolation == (
        isolation is Isolation.POSSIBLE_CONCURRENT_USAGE
    )
    assert Warning.NO_TRUSTED_PRODUCTION_EXPORT in result.warnings


def test_regime_boundary_between_cohorts_blocks_only_its_meter():
    b = tuple(record(i, finalized_at="2026-01-03T00:00:00Z") for i in range(4))
    r = tuple(record(i + 1000) for i in range(4))
    links = evidence(
        tuple(run(i) for i in range(4)),
        boundaries=(BenchmarkRegimeBoundary("2026-01-02T00:00:00Z", FIVE),),
    )
    result = analyze(b, links=links, reference=r)
    assert result.comparisons[0].status is Status.COMPARISON_UNAVAILABLE
    assert Warning.REGIME_BOUNDARY in result.comparisons[0].warnings
    assert result.comparisons[1].status is Status.DESCRIPTIVELY_CONSISTENT


def test_spanning_boundary_retains_descriptive_summary_but_blocks_calibration():
    rows = tuple(
        record(i, finalized_at=TIME if i < 2 else "2026-01-03T00:00:00Z") for i in range(4)
    )
    links = evidence(
        tuple(run(i) for i in range(4)),
        boundaries=(BenchmarkRegimeBoundary("2026-01-02T00:00:00Z", FIVE),),
    )
    result = analyze(rows, links=links, reference=tuple(record(i + 1000) for i in range(4)))
    assert result.benchmark_summaries[0].spans_regime_boundary is True
    assert result.comparisons[0].status is Status.COMPARISON_UNAVAILABLE
    assert Recommendation.INVESTIGATE_POSSIBLE_REGIME_SHIFT in result.comparisons[0].recommendations


def test_insufficiency_precedes_temporal_unavailability():
    links = evidence((run(0),), boundaries=(BenchmarkRegimeBoundary(TIME, FIVE),))
    result = analyze((record(0),), links=links, reference=(record(1000),))
    assert result.comparisons[0].status is Status.INSUFFICIENT_EVIDENCE
    assert Warning.REGIME_BOUNDARY in result.comparisons[0].warnings


def test_missing_times_are_explicit_and_fail_closed_with_supplied_boundary():
    rows = tuple(record(i, finalized_at=None) for i in range(4))
    links = evidence(
        tuple(run(i) for i in range(4)), boundaries=(BenchmarkRegimeBoundary(TIME, FIVE),)
    )
    result = analyze(rows, links=links, reference=tuple(record(i + 1000) for i in range(4)))
    assert result.benchmark_summaries[0].observations_without_time == 4
    assert Warning.TEMPORAL_EVIDENCE_INCOMPLETE in result.warnings
    assert result.comparisons[0].status is Status.COMPARISON_UNAVAILABLE


def test_available_run_observation_timing_consistency():
    with pytest.raises(BenchmarkDatasetError, match="timing"):
        analyze(
            (record(0, started_at=TIME),),
            links=evidence((run(0, started_at="2026-01-02T00:00:00Z"),)),
        )


def test_nonfinite_candidate_is_p018_exclusion_not_nan():
    result = analyze((record(0, raw_total=10**309),))
    summary = result.benchmark_summaries[0]
    assert summary.weighted_median_raw_tokens is None
    assert Exclusion.NONFINITE_CANDIDATE in summary.decisions[0].exclusion_reasons


def test_aggregation_overflow_raises_privacy_safe_typed_error():
    with pytest.raises(BenchmarkNumericalError):
        analyze(tuple(record(i, raw_total=10**306) for i in range(4)))


def test_nonfinite_comparison_returns_unavailable_without_clamping():
    b = tuple(record(i, raw_total=19 * 10**303) for i in range(4))
    r = tuple(
        record(i + 1000, raw_total=1, five_hour=quota(delta_percentage_points=100))
        for i in range(4)
    )
    result = analyze(b, reference=r).comparisons[0]
    assert result.status is Status.COMPARISON_UNAVAILABLE
    assert Warning.NONFINITE_COMPARISON in result.warnings
    assert result.absolute_difference_raw_tokens is result.relative_difference_percent is None
    assert result.benchmark_to_reference_ratio is result.iqr_overlap is None


@pytest.mark.parametrize(
    "changes",
    [
        {"minimum_used_observations": True},
        {"minimum_used_observations": 3},
        {"minimum_used_observations": 501},
        {"maximum_input_observations": False},
        {"maximum_input_observations": 3},
        {"maximum_input_observations": 100_001},
        {"material_relative_difference": True},
        {"material_relative_difference": 0},
        {"material_relative_difference": -1},
        {"material_relative_difference": math.nan},
        {"material_relative_difference": math.inf},
        {"material_relative_difference": "0.2"},
        {"material_relative_difference": 10**1000},
    ],
)
def test_policy_validated(changes):
    with pytest.raises(BenchmarkPolicyError):
        BenchmarkAnalysisPolicy(**changes)


def test_oversized_datasets_and_links_rejected_without_truncation():
    policy = BenchmarkAnalysisPolicy(maximum_input_observations=4)
    with pytest.raises(BenchmarkDatasetError):
        analyze(tuple(record(i) for i in range(5)), policy=policy)
    with pytest.raises(BenchmarkDatasetError):
        analyze(links=evidence(tuple(run(i) for i in range(5))), policy=policy)
    with pytest.raises(BenchmarkDatasetError):
        analyze(reference=tuple(record(i + 1000) for i in range(5)), policy=policy)


@pytest.mark.parametrize(
    "changes",
    [
        {"target_meters": [FIVE]},
        {"target_meters": ()},
        {"target_meters": (FIVE, FIVE)},
        {"target_meters": ("five_hour",)},
        {"benchmark_dataset": object()},
        {"benchmark_links": object()},
        {"benchmark_dataset": ObservationDataset((), migration_version=True)},
        {"benchmark_dataset": ObservationDataset([])},
        {"reference_dataset": object()},
    ],
)
def test_public_input_integrity(changes):
    args = {"benchmark_dataset": ObservationDataset(()), "benchmark_links": evidence()}
    args.update(changes)
    with pytest.raises(BenchmarkDatasetError):
        analyze_benchmark_calibration(**args)


def test_wrong_policy_type():
    with pytest.raises(BenchmarkPolicyError):
        analyze(policy=object())


def test_pure_replay_no_sqlite_network_provider_file_or_clock_dependency(monkeypatch, capsys):
    def forbidden(*args, **kwargs):
        pytest.fail("analyzer attempted external I/O")

    rows = tuple(record(i) for i in range(8))
    refs = tuple(record(i + 1000, 16_000_000) for i in range(8))
    original = (rows, refs)
    with monkeypatch.context() as context:
        context.setattr(sqlite3, "connect", forbidden)
        context.setattr(socket, "create_connection", forbidden)
        context.setattr(subprocess, "Popen", forbidden)
        context.setattr("builtins.open", forbidden)
        first = analyze(rows, reference=refs)
        second = analyze(rows, reference=refs)
    assert first == second
    assert original == (rows, refs)
    assert capsys.readouterr() == ("", "")
    for value, field in (
        (first, "provenance"),
        (first.comparisons[0], "status"),
        (first.benchmark_summaries[0], "confidence"),
        (first.evidence_accounting, "run_count"),
    ):
        with pytest.raises(FrozenInstanceError):
            setattr(value, field, None)
    assert not hasattr(first, "estimated_remaining_raw_tokens")
    assert not hasattr(first, "provider_change")
    assert all(c.status is Status.MATERIAL_DIFFERENCE_SEPARATED for c in first.comparisons)


def test_canonical_time_ranges_order_and_replay_under_input_permutation():
    rows = tuple(record(i, finalized_at=f"2026-01-01T00:00:0{i}.0Z") for i in range(4))
    runs = tuple(run(i) for i in range(4))
    first = analyze(rows, links=evidence(runs), target_meters=(WEEK, FIVE))
    second = analyze(
        tuple(reversed(rows)), links=evidence(tuple(reversed(runs))), target_meters=(WEEK, FIVE)
    )
    assert first == second
    assert first.benchmark_summaries[0].meter_type is WEEK
    assert first.benchmark_summaries[0].observed_from == "2026-01-01T00:00:00.000000000Z"
    assert first.benchmark_summaries[0].observed_through == "2026-01-01T00:00:03.000000000Z"


def test_duplicate_experiment_and_invalid_experiment_metadata():
    exp = BenchmarkExperimentEvidence("experiment", "workload", CONFIG)
    with pytest.raises(BenchmarkDatasetError):
        analyze(links=evidence(experiments=(exp, exp)))
    for change in (
        {"planned_run_count": True},
        {"planned_run_count": 0},
        {"planned_run_count": 101},
        {"target_meters": ()},
        {"configuration": replace(CONFIG, model="m" * 129)},
    ):
        with pytest.raises(BenchmarkDatasetError):
            replace(exp, **change)


def test_nonmatching_experiment_meter_is_not_substituted():
    exp = BenchmarkExperimentEvidence("experiment", "workload", CONFIG, target_meters=(WEEK,))
    result = analyze(
        (record(0),), links=evidence((run(0),), experiments=(exp,)), target_meters=(FIVE,)
    )
    assert result.benchmark_summaries == result.comparisons == ()


def test_reference_without_benchmark_scope_retains_ordinary_configuration():
    result = analyze(links=evidence(experiments=()), reference=(record(1000),))
    reference = result.reference_summaries[0]
    assert reference.configuration == CONFIG
    assert reference.cohort_kind.value == "observational_reference"
    assert reference.experiment_id is reference.workload_key is None
    assert result.benchmark_summaries == ()


def test_explicit_selected_configuration_must_match_all_rows():
    with pytest.raises(BenchmarkDatasetError):
        analyze_benchmark_calibration(
            benchmark_dataset=ObservationDataset(
                (record(0),), selected_configuration=replace(CONFIG, speed_mode=None)
            ),
            benchmark_links=evidence((run(0),)),
        )


def test_policy_can_raise_minimum_but_never_override_p020_confidence():
    result = comparison(policy=BenchmarkAnalysisPolicy(minimum_used_observations=8))
    assert result.benchmark.confidence is Confidence.MEDIUM
    assert result.status is Status.INSUFFICIENT_EVIDENCE
    result = comparison(
        b=(24_000_000,) * 4, policy=BenchmarkAnalysisPolicy(material_relative_difference=0.25)
    )
    assert result.status is Status.DESCRIPTIVELY_CONSISTENT


@pytest.mark.parametrize(
    "changes",
    [
        {"provenance": object()},
        {"experiments": []},
        {"runs": []},
        {"regime_boundaries": []},
        {"runs": (object(),)},
        {"experiments": (object(),)},
        {"regime_boundaries": (object(),)},
    ],
)
def test_evidence_dataset_requires_immutable_typed_collections(changes):
    with pytest.raises(BenchmarkDatasetError):
        replace(evidence(), **changes)


@pytest.mark.parametrize(
    "changes", [{"observed_at": "invalid"}, {"observed_at": None}, {"meter_type": "five_hour"}]
)
def test_boundary_input_validation(changes):
    with pytest.raises(BenchmarkDatasetError):
        replace(BenchmarkRegimeBoundary(TIME, FIVE), **changes)


def test_unresolved_incomplete_links_are_not_double_counted_as_statistical_exclusions():
    result = analyze(
        (record(0), record(1)),
        links=evidence(
            (
                run(0),
                run(1, measurement_status=Measurement.INCOMPLETE),
                run(2, measurement_status=Measurement.INVALID),
            )
        ),
    )
    accounting = result.evidence_accounting
    assert accounting.run_count == 3
    assert accounting.resolved_links == 2
    assert accounting.unresolved_links == 1
    assert accounting.eligible_linked_runs == 1
    assert accounting.incomplete_runs == accounting.invalid_runs == 1
    assert result.benchmark_summaries[0].sample_accounting.candidate_observations == 1
