use serde_json::{json, Value};

use crate::benchmark::{
    AuthorizationId, BenchmarkDiagnostic, BenchmarkError, BenchmarkExperiment,
    BenchmarkObservationClaims, BenchmarkPlan, BenchmarkPreflight, BenchmarkRunState,
    EvidenceBasis, ExplicitRunAuthorization, GateBlockReason, GateEvidence,
    IdentityValidationError, IsolationClassification, MeasurementCompletionStatus,
    MeterPreflightEvidence, ObservationId, PlanValidationError, PreflightOutcome, RunId,
    TrustedRuntimeBinding, UnknownEvidenceReason, WorkloadKey, BENCHMARK_FRAMEWORK_VERSION,
    MAX_PLANNED_RUNS,
};
use crate::telemetry::{
    ConfigurationIdentity, ConfigurationValue, EvidenceStatus, EvidenceValidity,
    MetricAvailability, MetricProvenance, NormalizedObservation, ObservationLifecycle,
    ObservationQuotaEvidence, ObservationQuotaEvidenceSet, ObservationTiming,
    ObservationTokenEvidence, QualityGrade, QuotaMeterType, QuotaWindowIdentity, ReasonCode,
    ResetStatus, SchemaVersion, TokenCounters, TokenMetric,
};

const CREATED: &str = "2026-10-16T10:00:00Z";
const PREFLIGHT_STARTED: &str = "2026-10-16T10:01:00Z";
const PREFLIGHT_EVALUATED: &str = "2026-10-16T10:02:00Z";
const AUTHORIZED: &str = "2026-10-16T10:03:00Z";
const STARTED: &str = "2026-10-16T10:04:00Z";
const ENDED: &str = "2026-10-16T10:05:00Z";
const FINALIZED: &str = "2026-10-16T10:06:00Z";
const LINKED: &str = "2026-10-16T10:07:00Z";

fn available(value: &str) -> ConfigurationValue {
    ConfigurationValue {
        availability: MetricAvailability::Available,
        value: Some(value.to_owned()),
        provenance: MetricProvenance::Observed,
    }
}

fn unknown() -> ConfigurationValue {
    ConfigurationValue {
        availability: MetricAvailability::Unavailable,
        value: None,
        provenance: MetricProvenance::Unavailable,
    }
}

fn configuration() -> ConfigurationIdentity {
    ConfigurationIdentity {
        plan: available("plus"),
        model: available("codex-test-model"),
        reasoning_level: available("medium"),
        speed_mode: unknown(),
        codex_version: available("1.2.3"),
    }
}

fn experiment_id(value: &str) -> crate::benchmark::ExperimentId {
    crate::benchmark::ExperimentId::parse(value).unwrap()
}

fn run_id(value: &str) -> RunId {
    RunId::parse(value).unwrap()
}

fn authorization_id(value: &str) -> AuthorizationId {
    AuthorizationId::parse(value).unwrap()
}

fn plan_with(meters: Vec<QuotaMeterType>, count: u16) -> BenchmarkPlan {
    plan_for_experiment("experiment_a", meters, count)
}

fn plan_for_experiment(experiment: &str, meters: Vec<QuotaMeterType>, count: u16) -> BenchmarkPlan {
    BenchmarkPlan::new(
        BENCHMARK_FRAMEWORK_VERSION,
        experiment_id(experiment),
        configuration(),
        WorkloadKey::parse("workload_a").unwrap(),
        count,
        meters,
        CREATED,
    )
    .unwrap()
}

fn both_meters() -> Vec<QuotaMeterType> {
    vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly]
}

fn ready_preflight(
    plan: &BenchmarkPlan,
    at: &str,
    isolation: IsolationClassification,
) -> BenchmarkPreflight {
    let basis = EvidenceBasis::AuthorizedTestFixture;
    let availability = plan
        .target_quota_meters()
        .iter()
        .copied()
        .map(|meter| MeterPreflightEvidence::new(meter, GateEvidence::verified(basis)))
        .collect();
    let continuity = plan
        .target_quota_meters()
        .iter()
        .copied()
        .map(|meter| MeterPreflightEvidence::new(meter, GateEvidence::verified(basis)))
        .collect();
    BenchmarkPreflight::new(
        plan,
        GateEvidence::verified(basis),
        GateEvidence::verified(basis),
        availability,
        continuity,
        isolation,
        GateEvidence::verified(basis),
        at,
    )
    .unwrap()
}

fn preflight_unknown(plan: &BenchmarkPlan, at: &str) -> BenchmarkPreflight {
    BenchmarkPreflight::unknown_for_plan(plan, at).unwrap()
}

fn authorization(
    experiment: &crate::benchmark::ExperimentId,
    run: &RunId,
    repetition: u32,
    id: &str,
) -> ExplicitRunAuthorization {
    ExplicitRunAuthorization::new(
        experiment.clone(),
        run.clone(),
        repetition,
        authorization_id(id),
        AUTHORIZED,
    )
    .unwrap()
}

fn ready_experiment(
    run_name: &str,
    meters: Vec<QuotaMeterType>,
    count: u16,
) -> (BenchmarkExperiment, RunId) {
    let plan = plan_with(meters, count);
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let run = run_id(run_name);
    experiment.create_run(run.clone(), 0).unwrap();
    experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
    experiment
        .evaluate_preflight(
            &run,
            ready_preflight(
                &plan,
                PREFLIGHT_EVALUATED,
                IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
            ),
        )
        .unwrap();
    (experiment, run)
}

fn running_experiment(
    run_name: &str,
    meters: Vec<QuotaMeterType>,
    count: u16,
) -> (BenchmarkExperiment, RunId) {
    let (mut experiment, run) = ready_experiment(run_name, meters, count);
    let auth = authorization(experiment.plan().experiment_id(), &run, 0, "auth_event_a");
    experiment.authorize(&run, auth).unwrap();
    experiment.start_run(&run, STARTED).unwrap();
    (experiment, run)
}

fn awaiting_observation(
    run_name: &str,
    meters: Vec<QuotaMeterType>,
    count: u16,
) -> (BenchmarkExperiment, RunId) {
    let (mut experiment, run) = running_experiment(run_name, meters, count);
    experiment.mark_execution_ended(&run, ENDED).unwrap();
    (experiment, run)
}

fn awaiting_with_plan(plan: BenchmarkPlan, run_name: &str) -> (BenchmarkExperiment, RunId) {
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let run = run_id(run_name);
    experiment.create_run(run.clone(), 0).unwrap();
    experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
    experiment
        .evaluate_preflight(
            &run,
            ready_preflight(
                &plan,
                PREFLIGHT_EVALUATED,
                IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
            ),
        )
        .unwrap();
    experiment
        .authorize(
            &run,
            authorization(plan.experiment_id(), &run, 0, "auth_cross_experiment"),
        )
        .unwrap();
    experiment.start_run(&run, STARTED).unwrap();
    experiment.mark_execution_ended(&run, ENDED).unwrap();
    (experiment, run)
}

fn status(
    validity: EvidenceValidity,
    quality: QualityGrade,
    reason_codes: Vec<ReasonCode>,
) -> EvidenceStatus {
    EvidenceStatus {
        validity,
        quality,
        reason_codes,
    }
}

fn quota_evidence(
    meter_type: QuotaMeterType,
    validity: EvidenceValidity,
    reset_status: ResetStatus,
    reasons: Vec<ReasonCode>,
) -> ObservationQuotaEvidence {
    ObservationQuotaEvidence {
        meter_type,
        status: status(
            validity,
            if validity == EvidenceValidity::Valid {
                QualityGrade::B
            } else {
                QualityGrade::D
            },
            reasons,
        ),
        before_sample: None,
        after_sample: None,
        delta_percentage_points: Some(1.25),
        window_identity: QuotaWindowIdentity::Unavailable,
        reset_status,
    }
}

fn observation(
    id: &str,
    config: ConfigurationIdentity,
    lifecycle: ObservationLifecycle,
    five_hour: ObservationQuotaEvidence,
    weekly: ObservationQuotaEvidence,
    finalized_at: Option<&str>,
    extra_token_reasons: Vec<ReasonCode>,
) -> NormalizedObservation {
    let token_validity = if extra_token_reasons.contains(&ReasonCode::ProcessInterrupted)
        || extra_token_reasons.contains(&ReasonCode::TelemetryIncomplete)
    {
        EvidenceValidity::Incomplete
    } else {
        EvidenceValidity::Valid
    };
    NormalizedObservation {
        schema_version: SchemaVersion::V1,
        observation_id: id.to_owned(),
        task_id: Some("task:synthetic".to_owned()),
        session_id: Some("session:synthetic".to_owned()),
        source_instance_id: Some("source:synthetic".to_owned()),
        lifecycle_state: lifecycle,
        timing: ObservationTiming {
            started_at: Some(STARTED.to_owned()),
            ended_at: Some(ENDED.to_owned()),
            finalized_at: finalized_at.map(str::to_owned),
            duration_ms: Some(60_000),
        },
        configuration: config,
        summary_quality: QualityGrade::B,
        token_evidence: ObservationTokenEvidence {
            status: status(token_validity, QualityGrade::B, extra_token_reasons),
            raw_token_counters: TokenCounters {
                uncached_input: TokenMetric {
                    availability: MetricAvailability::Available,
                    value: Some(7),
                    provenance: MetricProvenance::Observed,
                },
                cached_input: TokenMetric {
                    availability: MetricAvailability::Available,
                    value: Some(11),
                    provenance: MetricProvenance::Derived,
                },
                output: TokenMetric {
                    availability: MetricAvailability::Available,
                    value: Some(13),
                    provenance: MetricProvenance::Observed,
                },
                reasoning_output: TokenMetric {
                    availability: MetricAvailability::Unavailable,
                    value: None,
                    provenance: MetricProvenance::Unavailable,
                },
                raw_total: TokenMetric {
                    availability: MetricAvailability::Available,
                    value: Some(31),
                    provenance: MetricProvenance::Derived,
                },
            },
        },
        quota_evidence: ObservationQuotaEvidenceSet { five_hour, weekly },
    }
}

fn valid_observation(id: &str, config: ConfigurationIdentity) -> NormalizedObservation {
    observation(
        id,
        config,
        ObservationLifecycle::Finalized,
        quota_evidence(
            QuotaMeterType::FiveHour,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![],
        ),
        quota_evidence(
            QuotaMeterType::Weekly,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![],
        ),
        Some(FINALIZED),
        vec![],
    )
}

fn observation_id(_seed: char) -> String {
    crate::telemetry::observation::observation_id_for_identity(
        "task:synthetic",
        Some("session:synthetic"),
        Some(ENDED),
    )
}

fn binding(
    experiment: &BenchmarkExperiment,
    run: &RunId,
    repetition: u32,
    observation_id: &str,
    isolation: IsolationClassification,
) -> TrustedRuntimeBinding {
    TrustedRuntimeBinding::from_trusted_runtime(
        experiment.plan().experiment_id().clone(),
        run.clone(),
        repetition,
        observation_id,
        isolation,
    )
    .unwrap()
}

fn complete_run(
    experiment: &mut BenchmarkExperiment,
    run: &RunId,
    link: crate::benchmark::VerifiedObservationLink,
) {
    let mut claims = BenchmarkObservationClaims::default();
    experiment.complete_run(run, link, &mut claims).unwrap();
}

#[test]
fn valid_plan_is_typed_versioned_and_keeps_unknown_configuration() {
    let plan = plan_with(both_meters(), 3);
    assert_eq!(plan.framework_version(), "1.0.0");
    assert_eq!(plan.configuration().speed_mode, unknown());
    let encoded = serde_json::to_string(&plan).unwrap();
    let decoded: BenchmarkPlan = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, plan);
    assert_eq!(decoded.configuration().speed_mode.value, None);
}

#[test]
fn opaque_ids_reject_empty_path_prompt_and_overlong_values() {
    assert_eq!(
        crate::benchmark::ExperimentId::parse(""),
        Err(IdentityValidationError::Empty)
    );
    assert!(crate::benchmark::ExperimentId::parse("../prompt").is_err());
    assert!(WorkloadKey::parse("/tmp/workload").is_err());
    assert!(RunId::parse("a".repeat(65)).is_err());
}

#[test]
fn plan_rejects_zero_and_excessive_run_counts() {
    let base = plan_with(both_meters(), 1);
    assert_eq!(
        BenchmarkPlan::new(
            BENCHMARK_FRAMEWORK_VERSION,
            experiment_id("experiment_b"),
            configuration(),
            WorkloadKey::parse("workload_b").unwrap(),
            0,
            both_meters(),
            CREATED,
        )
        .unwrap_err(),
        PlanValidationError::PlannedRunCountOutOfRange
    );
    assert_eq!(
        BenchmarkPlan::new(
            BENCHMARK_FRAMEWORK_VERSION,
            experiment_id("experiment_b"),
            configuration(),
            WorkloadKey::parse("workload_b").unwrap(),
            MAX_PLANNED_RUNS + 1,
            both_meters(),
            CREATED,
        )
        .unwrap_err(),
        PlanValidationError::PlannedRunCountOutOfRange
    );
    assert_eq!(base.planned_run_count(), 1);
}

#[test]
fn plan_rejects_duplicate_or_empty_meter_targets() {
    let duplicate = BenchmarkPlan::new(
        BENCHMARK_FRAMEWORK_VERSION,
        experiment_id("experiment_b"),
        configuration(),
        WorkloadKey::parse("workload_b").unwrap(),
        2,
        vec![QuotaMeterType::Weekly, QuotaMeterType::Weekly],
        CREATED,
    );
    assert_eq!(
        duplicate.unwrap_err(),
        PlanValidationError::DuplicateQuotaMeter
    );
    let empty = BenchmarkPlan::new(
        BENCHMARK_FRAMEWORK_VERSION,
        experiment_id("experiment_b"),
        configuration(),
        WorkloadKey::parse("workload_b").unwrap(),
        2,
        vec![],
        CREATED,
    );
    assert_eq!(empty.unwrap_err(), PlanValidationError::NoTargetQuotaMeters);
}

#[test]
fn plan_rejects_invalid_configuration_version_and_timestamp() {
    let mut invalid = configuration();
    invalid.speed_mode = ConfigurationValue {
        availability: MetricAvailability::Available,
        value: None,
        provenance: MetricProvenance::Observed,
    };
    assert_eq!(
        BenchmarkPlan::new(
            BENCHMARK_FRAMEWORK_VERSION,
            experiment_id("experiment_b"),
            invalid,
            WorkloadKey::parse("workload_b").unwrap(),
            1,
            both_meters(),
            CREATED,
        )
        .unwrap_err(),
        PlanValidationError::InvalidConfiguration
    );
    let mut path_in_configuration = configuration();
    path_in_configuration.model = available("/home/user/local-model");
    assert_eq!(
        BenchmarkPlan::new(
            BENCHMARK_FRAMEWORK_VERSION,
            experiment_id("experiment_b"),
            path_in_configuration,
            WorkloadKey::parse("workload_b").unwrap(),
            1,
            both_meters(),
            CREATED,
        )
        .unwrap_err(),
        PlanValidationError::InvalidConfiguration
    );
    for (version, timestamp, expected) in [
        (
            "99.0.0",
            CREATED,
            PlanValidationError::UnsupportedFrameworkVersion,
        ),
        (
            BENCHMARK_FRAMEWORK_VERSION,
            "2026-10-16T10:00:00+00:00",
            PlanValidationError::InvalidTimestamp,
        ),
        (
            BENCHMARK_FRAMEWORK_VERSION,
            "not-a-time",
            PlanValidationError::InvalidTimestamp,
        ),
    ] {
        assert_eq!(
            BenchmarkPlan::new(
                version,
                experiment_id("experiment_b"),
                configuration(),
                WorkloadKey::parse("workload_b").unwrap(),
                1,
                both_meters(),
                timestamp,
            )
            .unwrap_err(),
            expected
        );
    }
}

#[test]
fn plan_deserialization_rejects_missing_unknown_and_sensitive_fields() {
    let plan = plan_with(both_meters(), 2);
    let mut value = serde_json::to_value(&plan).unwrap();
    value.as_object_mut().unwrap().remove("experiment_id");
    assert!(serde_json::from_value::<BenchmarkPlan>(value).is_err());

    for key in ["prompt", "source_code", "api_key", "unrestricted_metadata"] {
        let mut value = serde_json::to_value(&plan).unwrap();
        value[key] = json!("must not be accepted");
        assert!(serde_json::from_value::<BenchmarkPlan>(value).is_err());
    }
}

#[test]
fn plan_json_rejects_boolean_negative_and_unknown_numeric_boundaries() {
    let plan = plan_with(both_meters(), 2);
    for invalid in [json!(true), json!(-1), json!(1.5)] {
        let mut value = serde_json::to_value(&plan).unwrap();
        value["planned_run_count"] = invalid;
        assert!(serde_json::from_value::<BenchmarkPlan>(value).is_err());
    }
}

#[test]
fn plan_json_rejects_unknown_enums_and_serializes_deterministically() {
    let plan = plan_with(both_meters(), 2);
    let first = serde_json::to_string(&plan).unwrap();
    let second = serde_json::to_string(&plan).unwrap();
    assert_eq!(first, second);
    let mut value = serde_json::to_value(&plan).unwrap();
    value["target_quota_meters"][0] = json!("monthly");
    assert!(serde_json::from_value::<BenchmarkPlan>(value).is_err());
}

#[test]
fn all_verified_preflight_gates_produce_ready() {
    let plan = plan_with(both_meters(), 2);
    let preflight = ready_preflight(
        &plan,
        PREFLIGHT_EVALUATED,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    assert_eq!(preflight.outcome(), PreflightOutcome::Ready);
    assert_eq!(preflight.quota_meter_availability().len(), 2);
    assert_eq!(preflight.quota_window_continuity().len(), 2);
}

#[test]
fn missing_isolation_and_unknown_meter_leave_preflight_unknown() {
    let plan = plan_with(both_meters(), 2);
    let unknown = preflight_unknown(&plan, PREFLIGHT_EVALUATED);
    assert_eq!(unknown.outcome(), PreflightOutcome::Unknown);

    let mut availability = plan
        .target_quota_meters()
        .iter()
        .copied()
        .map(|meter| {
            MeterPreflightEvidence::new(
                meter,
                GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
            )
        })
        .collect::<Vec<_>>();
    availability[0] = MeterPreflightEvidence::new(
        availability[0].meter(),
        GateEvidence::unknown(UnknownEvidenceReason::QuotaMeterNotObserved),
    );
    let continuity = plan
        .target_quota_meters()
        .iter()
        .copied()
        .map(|meter| {
            MeterPreflightEvidence::new(
                meter,
                GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
            )
        })
        .collect();
    let value = BenchmarkPreflight::new(
        &plan,
        GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
        GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
        availability,
        continuity,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
        GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
        PREFLIGHT_EVALUATED,
    )
    .unwrap();
    assert_eq!(value.outcome(), PreflightOutcome::Unknown);
}

#[test]
fn blocked_telemetry_source_blocks_preflight() {
    let plan = plan_with(both_meters(), 2);
    let meters: Vec<MeterPreflightEvidence> = plan
        .target_quota_meters()
        .iter()
        .copied()
        .map(|meter| {
            MeterPreflightEvidence::new(
                meter,
                GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
            )
        })
        .collect();
    let preflight = BenchmarkPreflight::new(
        &plan,
        GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
        GateEvidence::blocked(GateBlockReason::TelemetrySourceUnavailable),
        meters.clone(),
        meters,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
        GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
        PREFLIGHT_EVALUATED,
    )
    .unwrap();
    assert_eq!(preflight.outcome(), PreflightOutcome::Blocked);
}

#[test]
fn preflight_requires_exact_meter_coverage_and_explicit_utc_time() {
    let plan = plan_with(both_meters(), 2);
    let one_meter = vec![MeterPreflightEvidence::new(
        QuotaMeterType::FiveHour,
        GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
    )];
    assert_eq!(
        BenchmarkPreflight::new(
            &plan,
            GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
            GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
            one_meter.clone(),
            one_meter,
            IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
            GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture),
            PREFLIGHT_EVALUATED,
        )
        .unwrap_err(),
        BenchmarkError::InvalidPreflight
    );
    assert_eq!(
        BenchmarkPreflight::unknown_for_plan(&plan, "2026-10-16T10:02:00+00:00").unwrap_err(),
        BenchmarkError::InvalidTimestamp
    );
}

#[test]
fn ready_preflight_does_not_authorize_or_start_a_run() {
    let (mut experiment, run) = ready_experiment("run_a", both_meters(), 2);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Ready
    );
    assert_eq!(
        experiment.start_run(&run, STARTED).unwrap_err(),
        BenchmarkError::AuthorizationRequired
    );
}

#[test]
fn unknown_or_blocked_preflight_cannot_be_authorized() {
    let plan = plan_with(both_meters(), 2);
    let mut unknown_exp = BenchmarkExperiment::new(plan.clone()).unwrap();
    let unknown_run = run_id("run_unknown");
    unknown_exp.create_run(unknown_run.clone(), 0).unwrap();
    unknown_exp
        .begin_preflight(&unknown_run, PREFLIGHT_STARTED)
        .unwrap();
    unknown_exp
        .evaluate_preflight(&unknown_run, preflight_unknown(&plan, PREFLIGHT_EVALUATED))
        .unwrap();
    assert_eq!(
        unknown_exp
            .authorize(
                &unknown_run,
                authorization(plan.experiment_id(), &unknown_run, 0, "auth_unknown"),
            )
            .unwrap_err(),
        BenchmarkError::PreflightUnknown
    );

    let mut blocked_exp = BenchmarkExperiment::new(plan.clone()).unwrap();
    let blocked_run = run_id("run_blocked");
    blocked_exp.create_run(blocked_run.clone(), 0).unwrap();
    blocked_exp
        .begin_preflight(&blocked_run, PREFLIGHT_STARTED)
        .unwrap();
    let blocked = ready_preflight(
        &plan,
        PREFLIGHT_EVALUATED,
        IsolationClassification::possible_concurrent_usage(EvidenceBasis::AuthorizedTestFixture),
    );
    assert_eq!(blocked.outcome(), PreflightOutcome::Blocked);
    blocked_exp
        .evaluate_preflight(&blocked_run, blocked)
        .unwrap();
    assert_eq!(
        blocked_exp
            .authorize(
                &blocked_run,
                authorization(plan.experiment_id(), &blocked_run, 0, "auth_blocked"),
            )
            .unwrap_err(),
        BenchmarkError::PreflightBlocked
    );
}

#[test]
fn explicit_scoped_authorization_permits_only_its_run() {
    let plan = plan_with(both_meters(), 2);
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let first = run_id("run_first");
    let second = run_id("run_second");
    for (index, run) in [first.clone(), second.clone()].into_iter().enumerate() {
        experiment.create_run(run.clone(), index as u32).unwrap();
        experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
        experiment
            .evaluate_preflight(
                &run,
                ready_preflight(
                    &plan,
                    PREFLIGHT_EVALUATED,
                    IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
                ),
            )
            .unwrap();
    }
    let event = authorization(plan.experiment_id(), &first, 0, "auth_single");
    experiment.authorize(&first, event.clone()).unwrap();
    assert_eq!(
        experiment.authorize(&second, event).unwrap_err(),
        BenchmarkError::AuthorizationScopeMismatch
    );
    let second_event = authorization(plan.experiment_id(), &second, 1, "auth_single");
    assert_eq!(
        experiment.authorize(&second, second_event).unwrap_err(),
        BenchmarkError::AuthorizationScopeMismatch
    );
    assert_eq!(
        experiment.run(&first).unwrap().state(),
        BenchmarkRunState::Authorized
    );
    assert_eq!(
        experiment.run(&second).unwrap().state(),
        BenchmarkRunState::Ready
    );
}

#[test]
fn valid_lifecycle_waits_for_observation_after_execution_end() {
    let (experiment, run) = awaiting_observation("run_lifecycle", both_meters(), 1);
    let result = experiment.run(&run).unwrap();
    assert_eq!(result.state(), BenchmarkRunState::AwaitingObservation);
    assert_eq!(result.started_at(), Some(STARTED));
    assert_eq!(result.ended_at(), Some(ENDED));
    assert_eq!(result.finalized_at(), None);
    assert_eq!(result.observation_id(), None);
    assert_eq!(result.completion_status(), None);
}

#[test]
fn invalid_transitions_and_unknown_runs_fail_closed() {
    let plan = plan_with(both_meters(), 1);
    let mut experiment = BenchmarkExperiment::new(plan).unwrap();
    let run = run_id("run_invalid");
    assert_eq!(
        experiment.start_run(&run, STARTED).unwrap_err(),
        BenchmarkError::RunNotFound
    );
    experiment.create_run(run.clone(), 0).unwrap();
    assert_eq!(
        experiment.mark_execution_ended(&run, ENDED).unwrap_err(),
        BenchmarkError::InvalidTransition
    );
    assert_eq!(
        experiment.start_run(&run, STARTED).unwrap_err(),
        BenchmarkError::AuthorizationRequired
    );
}

#[test]
fn run_ids_repetitions_and_planned_count_are_unique_and_bounded() {
    let plan = plan_with(both_meters(), 1);
    let mut experiment = BenchmarkExperiment::new(plan).unwrap();
    let first = run_id("run_one");
    experiment.create_run(first.clone(), 4).unwrap();
    assert_eq!(
        experiment.create_run(first, 8).unwrap_err(),
        BenchmarkError::DuplicateRun
    );
    assert_eq!(
        experiment.create_run(run_id("run_two"), 4).unwrap_err(),
        BenchmarkError::DuplicateRepetition
    );
    assert_eq!(
        experiment.create_run(run_id("run_three"), 5).unwrap_err(),
        BenchmarkError::RunLimitExceeded
    );
}

#[test]
fn exact_transition_replays_are_idempotent_and_conflicts_fail() {
    let (mut experiment, run) = ready_experiment("run_replay", both_meters(), 1);
    experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
    let preflight = experiment.run(&run).unwrap().preflight_status().unwrap();
    assert_eq!(preflight, PreflightOutcome::Ready);
    let auth = authorization(experiment.plan().experiment_id(), &run, 0, "auth_replay");
    experiment.authorize(&run, auth.clone()).unwrap();
    experiment.authorize(&run, auth).unwrap();
    experiment.start_run(&run, STARTED).unwrap();
    experiment.start_run(&run, STARTED).unwrap();
    assert_eq!(
        experiment
            .start_run(&run, "2026-10-16T10:04:01Z")
            .unwrap_err(),
        BenchmarkError::ConflictingReplay
    );
    experiment.mark_execution_ended(&run, ENDED).unwrap();
    experiment.mark_execution_ended(&run, ENDED).unwrap();
    assert_eq!(
        experiment
            .mark_execution_ended(&run, "2026-10-16T10:05:01Z")
            .unwrap_err(),
        BenchmarkError::ConflictingReplay
    );
}

#[test]
fn preflight_transition_replay_requires_identical_timestamp_and_evidence() {
    let plan = plan_with(both_meters(), 1);
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let run = run_id("run_preflight_replay");
    experiment.create_run(run.clone(), 0).unwrap();
    experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
    experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
    assert_eq!(
        experiment
            .begin_preflight(&run, "2026-10-16T10:01:01Z")
            .unwrap_err(),
        BenchmarkError::ConflictingReplay
    );
    let preflight = ready_preflight(
        &plan,
        PREFLIGHT_EVALUATED,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    experiment
        .evaluate_preflight(&run, preflight.clone())
        .unwrap();
    experiment.evaluate_preflight(&run, preflight).unwrap();
    let conflicting = ready_preflight(
        &plan,
        "2026-10-16T10:02:01Z",
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    assert_eq!(
        experiment
            .evaluate_preflight(&run, conflicting)
            .unwrap_err(),
        BenchmarkError::ConflictingReplay
    );
}

#[test]
fn preflight_for_a_different_plan_cannot_be_attached_to_a_run() {
    let plan = plan_with(both_meters(), 1);
    let other_plan = BenchmarkPlan::new(
        BENCHMARK_FRAMEWORK_VERSION,
        plan.experiment_id().clone(),
        ConfigurationIdentity {
            model: available("different-model"),
            ..configuration()
        },
        WorkloadKey::parse("different_workload").unwrap(),
        plan.planned_run_count(),
        plan.target_quota_meters().to_vec(),
        CREATED,
    )
    .unwrap();
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let run = run_id("run_wrong_preflight_scope");
    experiment.create_run(run.clone(), 0).unwrap();
    experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();

    assert_eq!(
        experiment
            .evaluate_preflight(
                &run,
                ready_preflight(
                    &other_plan,
                    PREFLIGHT_EVALUATED,
                    IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
                ),
            )
            .unwrap_err(),
        BenchmarkError::InvalidPreflight
    );
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::PreflightPending
    );
}

#[test]
fn abort_is_explicit_terminal_and_running_abort_records_interruption() {
    let mut planned = BenchmarkExperiment::new(plan_with(both_meters(), 2)).unwrap();
    let planned_id = run_id("run_abort_planned");
    planned.create_run(planned_id.clone(), 0).unwrap();
    assert_eq!(
        planned
            .abort_run(&planned_id, "2026-10-16T09:59:59Z")
            .unwrap_err(),
        BenchmarkError::InvalidTimeOrder
    );
    planned.abort_run(&planned_id, PREFLIGHT_STARTED).unwrap();
    planned.abort_run(&planned_id, PREFLIGHT_STARTED).unwrap();
    assert_eq!(
        planned.run(&planned_id).unwrap().state(),
        BenchmarkRunState::Aborted
    );
    assert_eq!(
        planned.start_run(&planned_id, STARTED).unwrap_err(),
        BenchmarkError::AuthorizationRequired
    );

    let (mut running, running_id) = running_experiment("run_abort_running", both_meters(), 1);
    running.abort_run(&running_id, ENDED).unwrap();
    assert!(running
        .run(&running_id)
        .unwrap()
        .diagnostics()
        .contains(&BenchmarkDiagnostic::InterruptedExecution));
    assert_eq!(running.run(&running_id).unwrap().ended_at(), None);
    assert_eq!(
        running.abort_run(&running_id, FINALIZED).unwrap_err(),
        BenchmarkError::ConflictingReplay
    );
    planned.create_run(run_id("run_after_abort"), 1).unwrap();
    assert_eq!(
        planned.run(&planned_id).unwrap().state(),
        BenchmarkRunState::Aborted
    );
    assert_eq!(
        planned.run(&run_id("run_after_abort")).unwrap().state(),
        BenchmarkRunState::Planned
    );
}

#[test]
fn terminal_run_is_immutable_and_creating_another_run_does_not_change_it() {
    let (mut experiment, run) = awaiting_observation("run_terminal", both_meters(), 2);
    let observation = valid_observation(
        &observation_id('a'),
        experiment.plan().configuration().clone(),
    );
    let binding = binding(
        &experiment,
        &run,
        0,
        &observation.observation_id,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    let link = experiment
        .link_observation(&run, &binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    let before_state = experiment.run(&run).unwrap().state();
    let before_observation_id = experiment
        .run(&run)
        .unwrap()
        .observation_id()
        .unwrap()
        .as_str()
        .to_owned();
    experiment.create_run(run_id("run_second"), 1).unwrap();
    assert_eq!(experiment.run(&run).unwrap().state(), before_state);
    assert_eq!(
        experiment
            .run(&run)
            .unwrap()
            .observation_id()
            .unwrap()
            .as_str(),
        before_observation_id
    );
    assert_eq!(
        experiment
            .abort_run(&run, "2026-10-16T10:08:00Z")
            .unwrap_err(),
        BenchmarkError::InvalidTransition
    );
}

#[test]
fn caller_supplied_observation_id_without_matching_trusted_binding_is_unresolved() {
    let (experiment, run) = awaiting_observation("run_binding", both_meters(), 1);
    let observation = valid_observation(
        &observation_id('b'),
        experiment.plan().configuration().clone(),
    );
    let wrong_binding = TrustedRuntimeBinding::from_trusted_runtime(
        experiment.plan().experiment_id().clone(),
        run_id("another_run"),
        0,
        observation.observation_id.clone(),
        IsolationClassification::Unknown,
    )
    .unwrap();
    assert_eq!(
        experiment
            .link_observation(&run, &wrong_binding, &observation, LINKED)
            .unwrap_err(),
        BenchmarkError::ObservationUnresolved
    );
    assert_eq!(
        ObservationId::parse("arbitrary-observation-id").unwrap_err(),
        BenchmarkError::ObservationMismatch
    );
}

#[test]
fn observation_link_requires_terminal_evidence_and_finalization_time() {
    let (experiment, run) = awaiting_observation("run_nonterminal", both_meters(), 1);
    let id = observation_id('c');
    let initial_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let nonterminal = observation(
        &id,
        experiment.plan().configuration().clone(),
        ObservationLifecycle::Reconciling,
        quota_evidence(
            QuotaMeterType::FiveHour,
            EvidenceValidity::Incomplete,
            ResetStatus::Unavailable,
            vec![],
        ),
        quota_evidence(
            QuotaMeterType::Weekly,
            EvidenceValidity::Incomplete,
            ResetStatus::Unavailable,
            vec![],
        ),
        None,
        vec![],
    );
    assert_eq!(
        experiment
            .link_observation(&run, &initial_binding, &nonterminal, LINKED)
            .unwrap_err(),
        BenchmarkError::ObservationUnresolved
    );
    let missing_finalization = valid_observation(&id, experiment.plan().configuration().clone());
    let mut missing_finalization = missing_finalization;
    missing_finalization.timing.finalized_at = None;
    assert_eq!(
        experiment
            .link_observation(&run, &initial_binding, &missing_finalization, LINKED)
            .unwrap_err(),
        BenchmarkError::ObservationUnresolved
    );
}

#[test]
fn observation_link_checks_configuration_and_temporal_consistency() {
    let (experiment, run) = awaiting_observation("run_config_mismatch", both_meters(), 1);
    let id = observation_id('d');
    let initial_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let mut mismatch = configuration();
    mismatch.model = available("different-model");
    let observation = valid_observation(&id, mismatch);
    assert_eq!(
        experiment
            .link_observation(&run, &initial_binding, &observation, LINKED)
            .unwrap_err(),
        BenchmarkError::ConfigurationMismatch
    );

    let mut bad_time = valid_observation(&id, experiment.plan().configuration().clone());
    bad_time.timing.ended_at = Some("2026-10-16T10:06:30Z".to_owned());
    bad_time.observation_id = crate::telemetry::observation::observation_id_for_identity(
        "task:synthetic",
        Some("session:synthetic"),
        bad_time.timing.ended_at.as_deref(),
    );
    let bad_time_binding = binding(
        &experiment,
        &run,
        0,
        &bad_time.observation_id,
        IsolationClassification::Unknown,
    );
    assert_eq!(
        experiment
            .link_observation(&run, &bad_time_binding, &bad_time, LINKED)
            .unwrap_err(),
        BenchmarkError::InvalidTimeOrder
    );

    let mut early_finalization = valid_observation(&id, experiment.plan().configuration().clone());
    early_finalization.timing.ended_at = None;
    early_finalization.timing.finalized_at = Some("2026-10-16T10:04:30Z".to_owned());
    early_finalization.observation_id = crate::telemetry::observation::observation_id_for_identity(
        "task:synthetic",
        Some("session:synthetic"),
        None,
    );
    let early_finalization_binding = binding(
        &experiment,
        &run,
        0,
        &early_finalization.observation_id,
        IsolationClassification::Unknown,
    );
    assert_eq!(
        experiment
            .link_observation(
                &run,
                &early_finalization_binding,
                &early_finalization,
                LINKED
            )
            .unwrap_err(),
        BenchmarkError::InvalidTimeOrder
    );

    let mut bad_identity = valid_observation(&id, experiment.plan().configuration().clone());
    bad_identity.task_id = Some("task:other".to_owned());
    assert_eq!(
        experiment
            .link_observation(&run, &initial_binding, &bad_identity, LINKED)
            .unwrap_err(),
        BenchmarkError::ObservationMismatch
    );
}

#[test]
fn duplicate_observation_association_is_rejected() {
    let plan = plan_with(both_meters(), 2);
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let first = run_id("run_obs_first");
    let second = run_id("run_obs_second");
    for (index, run) in [first.clone(), second.clone()].into_iter().enumerate() {
        experiment.create_run(run.clone(), index as u32).unwrap();
        experiment.begin_preflight(&run, PREFLIGHT_STARTED).unwrap();
        experiment
            .evaluate_preflight(
                &run,
                ready_preflight(
                    &plan,
                    PREFLIGHT_EVALUATED,
                    IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
                ),
            )
            .unwrap();
        let auth_id = if index == 0 { "auth_one" } else { "auth_two" };
        experiment
            .authorize(
                &run,
                authorization(plan.experiment_id(), &run, index as u32, auth_id),
            )
            .unwrap();
        experiment.start_run(&run, STARTED).unwrap();
        experiment.mark_execution_ended(&run, ENDED).unwrap();
    }
    let id = observation_id('e');
    let observation = valid_observation(&id, plan.configuration().clone());
    let isolation = IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture);
    let first_binding = binding(&experiment, &first, 0, &id, isolation);
    let first_link = experiment
        .link_observation(&first, &first_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &first, first_link);
    let second_binding = binding(&experiment, &second, 1, &id, isolation);
    assert_eq!(
        experiment
            .link_observation(&second, &second_binding, &observation, LINKED)
            .unwrap_err(),
        BenchmarkError::DuplicateObservation
    );
}

#[test]
fn shared_claim_ledger_rejects_observation_reuse_across_experiments() {
    let plan_a = plan_for_experiment("experiment_a", both_meters(), 1);
    let plan_b = plan_for_experiment("experiment_b", both_meters(), 1);
    let (mut experiment_a, run_a) = awaiting_with_plan(plan_a.clone(), "run_claim_a");
    let (mut experiment_b, run_b) = awaiting_with_plan(plan_b.clone(), "run_claim_b");
    let id = observation_id('e');
    let observation = valid_observation(&id, plan_a.configuration().clone());
    let isolation = IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture);
    let link_a = experiment_a
        .link_observation(
            &run_a,
            &binding(&experiment_a, &run_a, 0, &id, isolation),
            &observation,
            LINKED,
        )
        .unwrap();
    let link_b = experiment_b
        .link_observation(
            &run_b,
            &binding(&experiment_b, &run_b, 0, &id, isolation),
            &observation,
            LINKED,
        )
        .unwrap();
    let mut claims = BenchmarkObservationClaims::default();
    experiment_a
        .complete_run(&run_a, link_a, &mut claims)
        .unwrap();
    assert_eq!(
        experiment_b
            .complete_run(&run_b, link_b, &mut claims)
            .unwrap_err(),
        BenchmarkError::DuplicateObservation
    );
}

#[test]
fn matching_verified_terminal_observation_completes_without_mutating_it() {
    let (mut experiment, run) = awaiting_observation("run_match", both_meters(), 1);
    let id = observation_id('f');
    let observation = valid_observation(&id, experiment.plan().configuration().clone());
    let preserved = observation.clone();
    let link_binding = binding(
        &experiment,
        &run,
        0,
        &id,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(observation, preserved);
    let completed = experiment.run(&run).unwrap();
    assert_eq!(completed.state(), BenchmarkRunState::Completed);
    assert_eq!(
        completed.completion_status(),
        Some(MeasurementCompletionStatus::MeasurementComplete)
    );
    assert_eq!(completed.observation_id().unwrap().as_str(), id);
    assert_eq!(completed.finalized_at(), Some(FINALIZED));
}

#[test]
fn incomplete_and_invalid_observations_remain_incomplete() {
    for (hex, lifecycle, validity) in [
        (
            '1',
            ObservationLifecycle::Incomplete,
            EvidenceValidity::Incomplete,
        ),
        (
            '2',
            ObservationLifecycle::Invalid,
            EvidenceValidity::Invalid,
        ),
    ] {
        let (mut experiment, run) = awaiting_observation("run_bad_obs", both_meters(), 1);
        let id = observation_id(hex);
        let observation = observation(
            &id,
            experiment.plan().configuration().clone(),
            lifecycle,
            quota_evidence(
                QuotaMeterType::FiveHour,
                validity,
                ResetStatus::Unavailable,
                vec![],
            ),
            quota_evidence(
                QuotaMeterType::Weekly,
                validity,
                ResetStatus::Unavailable,
                vec![],
            ),
            Some(FINALIZED),
            vec![],
        );
        let link_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
        let link = experiment
            .link_observation(&run, &link_binding, &observation, LINKED)
            .unwrap();
        complete_run(&mut experiment, &run, link);
        let completed = experiment.run(&run).unwrap();
        assert_eq!(completed.state(), BenchmarkRunState::Incomplete);
        assert_eq!(
            completed.completion_status(),
            Some(MeasurementCompletionStatus::MeasurementIncomplete)
        );
        assert_eq!(observation.lifecycle_state, lifecycle);
        assert_eq!(
            observation.quota_evidence.five_hour.status.validity,
            validity
        );
    }
}

#[test]
fn per_meter_reset_evidence_stays_independent() {
    let (mut experiment, run) = awaiting_observation("run_meter_independent", both_meters(), 1);
    let id = observation_id('3');
    let observation = observation(
        &id,
        experiment.plan().configuration().clone(),
        ObservationLifecycle::Incomplete,
        quota_evidence(
            QuotaMeterType::FiveHour,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![],
        ),
        quota_evidence(
            QuotaMeterType::Weekly,
            EvidenceValidity::Invalid,
            ResetStatus::Detected,
            vec![ReasonCode::QuotaResetCrossed],
        ),
        Some(FINALIZED),
        vec![],
    );
    let link_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(
        observation.quota_evidence.five_hour.status.validity,
        EvidenceValidity::Valid
    );
    assert_eq!(
        observation.quota_evidence.weekly.status.validity,
        EvidenceValidity::Invalid
    );
    assert!(experiment
        .run(&run)
        .unwrap()
        .diagnostics()
        .contains(&BenchmarkDiagnostic::QuotaResetCrossing));
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Incomplete
    );
}

#[test]
fn weekly_evidence_survives_a_five_hour_reset() {
    let (mut experiment, run) = awaiting_observation("run_weekly_survives", both_meters(), 1);
    let id = observation_id('9');
    let observation = observation(
        &id,
        experiment.plan().configuration().clone(),
        ObservationLifecycle::Incomplete,
        quota_evidence(
            QuotaMeterType::FiveHour,
            EvidenceValidity::Invalid,
            ResetStatus::Detected,
            vec![ReasonCode::QuotaResetCrossed],
        ),
        quota_evidence(
            QuotaMeterType::Weekly,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![],
        ),
        Some(FINALIZED),
        vec![],
    );
    let link_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(
        observation.quota_evidence.five_hour.status.validity,
        EvidenceValidity::Invalid
    );
    assert_eq!(
        observation.quota_evidence.weekly.status.validity,
        EvidenceValidity::Valid
    );
    assert!(experiment
        .run(&run)
        .unwrap()
        .diagnostics()
        .contains(&BenchmarkDiagnostic::QuotaResetCrossing));
}

#[test]
fn unknown_runtime_isolation_does_not_become_a_clean_controlled_claim() {
    let (mut experiment, run) = awaiting_observation("run_unknown_isolation", both_meters(), 1);
    let id = observation_id('4');
    let observation = valid_observation(&id, experiment.plan().configuration().clone());
    let link_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Completed
    );
    assert_eq!(
        experiment.run(&run).unwrap().isolation_status(),
        IsolationClassification::Unknown
    );
    assert_eq!(observation.summary_quality, QualityGrade::B);
}

#[test]
fn observation_quality_raw_tokens_and_meter_deltas_are_never_rewritten() {
    let (mut experiment, run) = awaiting_observation("run_no_mutation", both_meters(), 1);
    let id = observation_id('5');
    let mut observation = valid_observation(&id, experiment.plan().configuration().clone());
    observation.summary_quality = QualityGrade::C;
    observation.quota_evidence.five_hour.status.quality = QualityGrade::C;
    let original_quality = observation.summary_quality;
    let original_raw_total = observation
        .token_evidence
        .raw_token_counters
        .raw_total
        .clone();
    let original_five_delta = observation.quota_evidence.five_hour.delta_percentage_points;
    let original_weekly_delta = observation.quota_evidence.weekly.delta_percentage_points;
    let link_binding = binding(
        &experiment,
        &run,
        0,
        &id,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(observation.summary_quality, original_quality);
    assert_eq!(
        observation.token_evidence.raw_token_counters.raw_total,
        original_raw_total
    );
    assert_eq!(
        observation.quota_evidence.five_hour.delta_percentage_points,
        original_five_delta
    );
    assert_eq!(
        observation.quota_evidence.weekly.delta_percentage_points,
        original_weekly_delta
    );
    assert_eq!(observation.summary_quality, QualityGrade::C);
}

#[test]
fn observation_concurrency_reason_is_retained_as_contamination_evidence() {
    let (mut experiment, run) = awaiting_observation("run_concurrent", both_meters(), 1);
    let id = observation_id('6');
    let observation = observation(
        &id,
        experiment.plan().configuration().clone(),
        ObservationLifecycle::Incomplete,
        quota_evidence(
            QuotaMeterType::FiveHour,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![ReasonCode::ConcurrentUsagePossible],
        ),
        quota_evidence(
            QuotaMeterType::Weekly,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![],
        ),
        Some(FINALIZED),
        vec![],
    );
    let link_binding = binding(
        &experiment,
        &run,
        0,
        &id,
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
    );
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(
        experiment.run(&run).unwrap().isolation_status(),
        IsolationClassification::possible_concurrent_usage(EvidenceBasis::ObservationPipeline)
    );
    assert!(experiment
        .run(&run)
        .unwrap()
        .diagnostics()
        .contains(&BenchmarkDiagnostic::ConcurrentCodexOrWorkUsage));
}

#[test]
fn runtime_reported_concurrent_usage_is_retained_without_an_observation_reason() {
    let (mut experiment, run) = awaiting_observation("run_runtime_concurrent", both_meters(), 1);
    let id = observation_id('6');
    let observation = valid_observation(&id, experiment.plan().configuration().clone());
    let link_binding = binding(
        &experiment,
        &run,
        0,
        &id,
        IsolationClassification::possible_concurrent_usage(EvidenceBasis::ObservedRuntime),
    );
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    assert_eq!(
        experiment.run(&run).unwrap().isolation_status(),
        IsolationClassification::possible_concurrent_usage(EvidenceBasis::ObservedRuntime)
    );
    assert!(experiment
        .run(&run)
        .unwrap()
        .diagnostics()
        .contains(&BenchmarkDiagnostic::ConcurrentCodexOrWorkUsage));
}

#[test]
fn interrupted_and_missing_meter_diagnostics_remain_separate() {
    let (mut experiment, run) = awaiting_observation("run_diagnostics", both_meters(), 1);
    let id = observation_id('7');
    let observation = observation(
        &id,
        experiment.plan().configuration().clone(),
        ObservationLifecycle::Incomplete,
        quota_evidence(
            QuotaMeterType::FiveHour,
            EvidenceValidity::Unavailable,
            ResetStatus::Unavailable,
            vec![ReasonCode::MeterUnavailable, ReasonCode::MeterUnstable],
        ),
        quota_evidence(
            QuotaMeterType::Weekly,
            EvidenceValidity::Valid,
            ResetStatus::NotDetected,
            vec![],
        ),
        Some(FINALIZED),
        vec![ReasonCode::ProcessInterrupted],
    );
    let link_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    let diagnostics = experiment.run(&run).unwrap().diagnostics();
    assert!(diagnostics.contains(&BenchmarkDiagnostic::MissingMeterEvidence));
    assert!(diagnostics.contains(&BenchmarkDiagnostic::DelayedMeterStabilization));
    assert!(diagnostics.contains(&BenchmarkDiagnostic::InterruptedExecution));
    assert!(diagnostics.contains(&BenchmarkDiagnostic::IncompleteTokenTelemetry));
}

#[test]
fn exact_observation_link_replay_is_idempotent_and_conflicting_time_fails() {
    let (mut experiment, run) = awaiting_observation("run_link_replay", both_meters(), 1);
    let id = observation_id('8');
    let observation = valid_observation(&id, experiment.plan().configuration().clone());
    let link_binding = binding(&experiment, &run, 0, &id, IsolationClassification::Unknown);
    let link = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, link);
    let replay = experiment
        .link_observation(&run, &link_binding, &observation, LINKED)
        .unwrap();
    complete_run(&mut experiment, &run, replay);
    assert_eq!(
        experiment
            .link_observation(&run, &link_binding, &observation, "2026-10-16T10:07:01Z")
            .unwrap_err(),
        BenchmarkError::ConflictingReplay
    );
}

#[test]
fn benchmark_plan_serialization_contains_no_execution_payload_fields() {
    let value: Value = serde_json::to_value(plan_with(both_meters(), 1)).unwrap();
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 7);
    for forbidden in [
        "prompt",
        "response",
        "reasoning_trace",
        "tool_output",
        "source_files",
        "repository_contents",
        "git_remote",
        "user_email",
        "access_token",
        "api_key",
        "credentials",
    ] {
        assert!(!object.contains_key(forbidden));
    }
}

#[test]
fn unknown_optional_configuration_stays_unknown_through_deterministic_replay() {
    let plan = plan_with(both_meters(), 2);
    let serialized = serde_json::to_string(&plan).unwrap();
    let replay: BenchmarkPlan = serde_json::from_str(&serialized).unwrap();
    assert_eq!(
        replay.configuration().speed_mode.availability,
        MetricAvailability::Unavailable
    );
    assert_eq!(
        replay.configuration().speed_mode.provenance,
        MetricProvenance::Unavailable
    );
    assert_eq!(replay.configuration().speed_mode.value, None);
}
