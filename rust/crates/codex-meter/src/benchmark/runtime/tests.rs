use std::collections::VecDeque;
use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;
use crate::benchmark::adapter::{ResolvedBenchmarkWorkload, RuntimeAdapterError};
use crate::benchmark::dispatch::{
    DispatchExecutionLease, DispatchReservation, DispatchTerminalOutcome,
    FileBenchmarkDispatchGuard,
};
use crate::benchmark::model::{AuthorizationId, BenchmarkPlan, WorkloadKey};
use crate::benchmark::validation::{EvidenceBasis, GateEvidence, MeterPreflightEvidence};
use crate::benchmark::{CancellationPolicy, BENCHMARK_FRAMEWORK_VERSION};
use crate::telemetry::{
    ConfigurationValue, EvidenceStatus, EvidenceValidity, MetricAvailability, MetricProvenance,
    ObservationLifecycle, ObservationQuotaEvidence, ObservationQuotaEvidenceSet, ObservationTiming,
    ObservationTokenEvidence, QualityGrade, QuotaMeterType, QuotaWindowIdentity, ReasonCode,
    ResetStatus, SchemaVersion, TokenCounters, TokenMetric,
};

const CREATED: &str = "2026-10-16T10:00:00Z";
const PREFLIGHT_STARTED: &str = "2026-10-16T10:01:00Z";
const PREFLIGHT_EVALUATED: &str = "2026-10-16T10:02:00Z";
const AUTHORIZED: &str = "2026-10-16T10:03:00Z";
const RESERVED: &str = "2026-10-16T10:03:30Z";
const STARTED: &str = "2026-10-16T10:04:00Z";
const ENDED: &str = "2026-10-16T10:05:00Z";
const RESULT_OBSERVED: &str = "2026-10-16T10:05:30Z";
const FINALIZED: &str = "2026-10-16T10:06:00Z";
const LINKED: &str = "2026-10-16T10:07:00Z";

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct TestDirectory(std::path::PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-meter-p025-runtime-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn guard(&self) -> Arc<FileBenchmarkDispatchGuard> {
        Arc::new(FileBenchmarkDispatchGuard::open(&self.0).unwrap())
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config_value(value: &str) -> ConfigurationValue {
    ConfigurationValue {
        availability: MetricAvailability::Available,
        value: Some(value.to_owned()),
        provenance: MetricProvenance::Observed,
    }
}

fn unavailable() -> ConfigurationValue {
    ConfigurationValue {
        availability: MetricAvailability::Unavailable,
        value: None,
        provenance: MetricProvenance::Unavailable,
    }
}

fn configuration() -> ConfigurationIdentity {
    ConfigurationIdentity {
        plan: config_value("plus"),
        model: config_value("codex-test-model"),
        reasoning_level: config_value("medium"),
        speed_mode: config_value("fast"),
        codex_version: config_value("1.2.3"),
    }
}

fn plan(experiment: &str, count: u16, meters: Vec<QuotaMeterType>) -> BenchmarkPlan {
    BenchmarkPlan::new(
        BENCHMARK_FRAMEWORK_VERSION,
        ExperimentId::parse(experiment).unwrap(),
        configuration(),
        WorkloadKey::parse("workload_a").unwrap(),
        count,
        meters,
        CREATED,
    )
    .unwrap()
}

fn meter_evidence(
    meters: &[QuotaMeterType],
    evidence: GateEvidence,
) -> Vec<MeterPreflightEvidence> {
    meters
        .iter()
        .copied()
        .map(|meter| MeterPreflightEvidence::new(meter, evidence))
        .collect()
}

fn ready_snapshot(plan: &BenchmarkPlan) -> RuntimeEvidenceSnapshot {
    let meters = plan.target_quota_meters();
    let verified = GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture);
    RuntimeEvidenceSnapshot::new(
        Some(plan.configuration().clone()),
        verified,
        meter_evidence(meters, verified),
        meter_evidence(meters, verified),
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
        verified,
        PREFLIGHT_EVALUATED,
    )
}

#[derive(Clone)]
struct FixedEvidenceSource {
    snapshot: RuntimeEvidenceSnapshot,
}

impl BenchmarkRuntimeEvidenceSource for FixedEvidenceSource {
    fn collect(
        &self,
        _plan: &BenchmarkPlan,
        _run_id: &RunId,
    ) -> Result<RuntimeEvidenceSnapshot, RuntimeError> {
        Ok(self.snapshot.clone())
    }
}

struct FixedClock {
    timestamps: Mutex<VecDeque<&'static str>>,
}

impl FixedClock {
    fn standard() -> Self {
        Self {
            timestamps: Mutex::new(VecDeque::from([RESERVED, STARTED, RESULT_OBSERVED])),
        }
    }
}

impl BenchmarkRuntimeClock for FixedClock {
    fn now_utc(&self) -> Result<String, RuntimeError> {
        Ok(self
            .timestamps
            .lock()
            .map_err(|_| RuntimeError::RecoveryEvidenceIncomplete)?
            .pop_front()
            .unwrap_or(RESULT_OBSERVED)
            .to_owned())
    }
}

struct CancellingClock {
    calls: AtomicUsize,
    cancellation: CancellationToken,
}

impl BenchmarkRuntimeClock for CancellingClock {
    fn now_utc(&self) -> Result<String, RuntimeError> {
        match self.calls.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(RESERVED.to_owned()),
            1 => {
                // Cancellation lands after the coordinator's last preflight
                // check and immediately before the adapter worker can start.
                self.cancellation.cancel();
                Ok(STARTED.to_owned())
            }
            _ => Ok(RESULT_OBSERVED.to_owned()),
        }
    }
}

#[derive(Default)]
struct FakeIngress {
    authorization: Option<ExplicitRunAuthorization>,
    calls: usize,
}

impl BenchmarkAuthorizationIngress for FakeIngress {
    fn consume(
        &mut self,
        request: &RunAuthorizationRequest,
    ) -> Result<TrustedRunAuthorization, RuntimeError> {
        self.calls += 1;
        let authorization = self
            .authorization
            .clone()
            .ok_or(RuntimeError::AuthorizationMissing)?;
        Ok(TrustedRunAuthorization::new(authorization, request.clone()))
    }
}

struct FakeWorkloadResolver {
    accepts_key: bool,
}

impl BenchmarkWorkloadResolver for FakeWorkloadResolver {
    fn resolve(&self, key: &WorkloadKey) -> Result<ResolvedBenchmarkWorkload, RuntimeAdapterError> {
        if self.accepts_key && key.as_str() == "workload_a" {
            Ok(ResolvedBenchmarkWorkload::for_test(key.clone()))
        } else {
            Err(RuntimeAdapterError::UnknownWorkloadKey)
        }
    }
}

#[derive(Clone)]
struct FakeAdapter {
    calls: Arc<AtomicUsize>,
    status: AdapterExecutionStatus,
    started_at: Option<String>,
    ended_at: Option<String>,
    delay: Duration,
    cancel_during_call: Option<CancellationToken>,
    correlation: Option<ExecutionCorrelationEvidence>,
    fail_validation: bool,
    fail_execution: bool,
}

impl FakeAdapter {
    fn ended(correlation: Option<ExecutionCorrelationEvidence>) -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
            status: AdapterExecutionStatus::ExecutionEnded,
            started_at: Some(STARTED.to_owned()),
            ended_at: Some(ENDED.to_owned()),
            delay: Duration::ZERO,
            cancel_during_call: None,
            correlation,
            fail_validation: false,
            fail_execution: false,
        }
    }
}

impl BenchmarkExecutionAdapter for FakeAdapter {
    fn validate(
        &self,
        workload: &ResolvedBenchmarkWorkload,
        _policy: RuntimeExecutionPolicy,
    ) -> Result<(), RuntimeAdapterError> {
        if self.fail_validation || workload.key().as_str() != "workload_a" {
            Err(RuntimeAdapterError::AdapterUnavailable)
        } else {
            Ok(())
        }
    }

    fn execute(
        &self,
        _workload: &ResolvedBenchmarkWorkload,
        control: &ExecutionControl,
    ) -> Result<AdapterExecutionResult, RuntimeAdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert!(control.remaining() <= Duration::from_secs(10));
        if let Some(token) = &self.cancel_during_call {
            token.cancel();
        }
        if self.fail_execution {
            return Err(RuntimeAdapterError::ExecutionOutcomeUnknown);
        }
        if !self.delay.is_zero() {
            thread::sleep(self.delay);
        }
        let mut result = AdapterExecutionResult::new(
            self.status,
            self.started_at.clone(),
            self.ended_at.clone(),
        );
        if let Some(correlation) = &self.correlation {
            result = result.with_correlation(correlation.clone());
        }
        Ok(result)
    }
}

fn policy(maximum_elapsed: Duration) -> RuntimeExecutionPolicy {
    RuntimeExecutionPolicy::new(
        maximum_elapsed,
        Duration::from_secs(30),
        CancellationPolicy::RequestWhenSupported,
    )
    .unwrap()
}

fn auth(experiment: &str, run: &str, repetition: u32, auth_id: &str) -> ExplicitRunAuthorization {
    ExplicitRunAuthorization::new(
        ExperimentId::parse(experiment).unwrap(),
        RunId::parse(run).unwrap(),
        repetition,
        AuthorizationId::parse(auth_id).unwrap(),
        AUTHORIZED,
    )
    .unwrap()
}

fn prepared(
    coordinator: &BenchmarkRuntimeCoordinator,
    experiment_name: &str,
    run_name: &str,
    count: u16,
    snapshot: RuntimeEvidenceSnapshot,
) -> (BenchmarkExperiment, RunId) {
    let plan = plan(
        experiment_name,
        count,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    let mut experiment = BenchmarkExperiment::new(plan).unwrap();
    let run = RunId::parse(run_name).unwrap();
    experiment.create_run(run.clone(), 0).unwrap();
    coordinator
        .evaluate_preflight(
            &mut experiment,
            &run,
            PREFLIGHT_STARTED,
            &FixedEvidenceSource { snapshot },
        )
        .unwrap();
    (experiment, run)
}

fn prepared_ready(
    coordinator: &BenchmarkRuntimeCoordinator,
    experiment_name: &str,
    run_name: &str,
) -> (BenchmarkExperiment, RunId) {
    let plan = plan(
        experiment_name,
        1,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    prepared(
        coordinator,
        experiment_name,
        run_name,
        1,
        ready_snapshot(&plan),
    )
}

fn ingress(experiment: &BenchmarkExperiment, run: &RunId, auth_id: &str) -> FakeIngress {
    FakeIngress {
        authorization: Some(auth(
            experiment.plan().experiment_id().as_str(),
            run.as_str(),
            0,
            auth_id,
        )),
        calls: 0,
    }
}

fn scope(experiment: &BenchmarkExperiment, run: &RunId, auth_id: &str) -> DispatchScope {
    DispatchScope::new(
        experiment.plan().experiment_id().clone(),
        run.clone(),
        0,
        AuthorizationId::parse(auth_id).unwrap(),
    )
}

fn observation_id(task_id: &str, session_id: &str) -> String {
    crate::telemetry::observation::observation_id_for_identity(
        task_id,
        Some(session_id),
        Some(ENDED),
    )
}

fn correlation(
    experiment: &BenchmarkExperiment,
    run: &RunId,
    task_id: &str,
    session_id: &str,
    source_id: &str,
    isolation: IsolationClassification,
) -> ExecutionCorrelationEvidence {
    ExecutionCorrelationEvidence::from_verified_adapter(
        experiment.plan().experiment_id().clone(),
        run.clone(),
        0,
        observation_id(task_id, session_id),
        task_id,
        session_id,
        source_id,
        isolation,
    )
    .unwrap()
}

fn status(validity: EvidenceValidity, quality: QualityGrade) -> EvidenceStatus {
    EvidenceStatus {
        validity,
        quality,
        reason_codes: Vec::<ReasonCode>::new(),
    }
}

fn valid_meter(meter_type: QuotaMeterType) -> ObservationQuotaEvidence {
    ObservationQuotaEvidence {
        meter_type,
        status: status(EvidenceValidity::Valid, QualityGrade::B),
        before_sample: None,
        after_sample: None,
        delta_percentage_points: Some(1.25),
        window_identity: QuotaWindowIdentity::Unavailable,
        reset_status: ResetStatus::NotDetected,
    }
}

fn observation(
    plan: &BenchmarkPlan,
    lifecycle: ObservationLifecycle,
    task_id: &str,
    session_id: &str,
    source_id: &str,
) -> NormalizedObservation {
    let id = observation_id(task_id, session_id);
    NormalizedObservation {
        schema_version: SchemaVersion::V1,
        observation_id: id,
        task_id: Some(task_id.to_owned()),
        session_id: Some(session_id.to_owned()),
        source_instance_id: Some(source_id.to_owned()),
        lifecycle_state: lifecycle,
        timing: ObservationTiming {
            started_at: Some(STARTED.to_owned()),
            ended_at: Some(ENDED.to_owned()),
            finalized_at: matches!(
                lifecycle,
                ObservationLifecycle::Finalized
                    | ObservationLifecycle::Incomplete
                    | ObservationLifecycle::Invalid
            )
            .then(|| FINALIZED.to_owned()),
            duration_ms: Some(60_000),
        },
        configuration: plan.configuration().clone(),
        summary_quality: QualityGrade::B,
        token_evidence: ObservationTokenEvidence {
            status: status(
                if lifecycle == ObservationLifecycle::Finalized {
                    EvidenceValidity::Valid
                } else {
                    EvidenceValidity::Incomplete
                },
                QualityGrade::B,
            ),
            raw_token_counters: TokenCounters {
                uncached_input: TokenMetric::unavailable(),
                cached_input: TokenMetric::observed(7),
                output: TokenMetric::observed(11),
                reasoning_output: TokenMetric::unavailable(),
                raw_total: TokenMetric::observed(18),
            },
        },
        quota_evidence: ObservationQuotaEvidenceSet {
            five_hour: valid_meter(QuotaMeterType::FiveHour),
            weekly: valid_meter(QuotaMeterType::Weekly),
        },
    }
}

#[derive(Clone)]
struct FakeObservationResolver {
    stored: Option<ResolvedAuthoritativeObservation>,
}

impl BenchmarkObservationResolver for FakeObservationResolver {
    fn resolve(
        &self,
        _correlation: &ExecutionCorrelationEvidence,
    ) -> Result<Option<ResolvedAuthoritativeObservation>, RuntimeError> {
        Ok(self.stored.clone())
    }
}

fn stored(observation: NormalizedObservation) -> ResolvedAuthoritativeObservation {
    ResolvedAuthoritativeObservation::for_test(StoredObservation {
        observation,
        storage_revision: 1,
        payload_sha256: "synthetic-test-evidence".to_owned(),
    })
}

fn wait_for_dispatch_slot_release(directory: &TestDirectory) {
    let path = directory
        .0
        .join(crate::benchmark::dispatch::ACTIVE_DISPATCH_LOCK);
    let deadline = Instant::now() + Duration::from_secs(1);
    while path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    assert!(!path.exists());
}

#[test]
fn disabled_is_the_only_production_execution_mode_and_never_consumes_authorization() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_disabled(directory.guard());
    assert_eq!(coordinator.execution_mode(), ExecutionMode::Disabled);
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_disabled", "run_disabled");
    let mut ingress = ingress(&experiment, &run, "auth_disabled");
    let adapter = FakeAdapter::ended(None);
    let result = coordinator.execute(
        &mut experiment,
        &run,
        &mut ingress,
        &FakeWorkloadResolver { accepts_key: true },
        Some(Arc::new(adapter.clone())),
        policy(Duration::from_secs(2)),
        CancellationToken::default(),
        &FixedClock::standard(),
    );
    assert_eq!(result.unwrap_err(), RuntimeError::ExecutionDisabled);
    assert_eq!(ingress.calls, 0);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        coordinator
            .recover_dispatch_state(&scope(&experiment, &run, "auth_disabled"))
            .unwrap(),
        DispatchJournalState::NotReserved
    );
}

#[test]
fn ready_preflight_is_not_authorization_and_missing_action_fails_closed() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_no_auth", "run_no_auth");
    let mut ingress = FakeIngress::default();
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::AuthorizationMissing
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Ready
    );
}

#[test]
fn unknown_preflight_and_blocked_configuration_never_consume_authorization() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let unknown_plan = plan(
        "experiment_unknown",
        1,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    let (mut unknown, unknown_run) = prepared(
        &coordinator,
        "experiment_unknown",
        "run_unknown",
        1,
        RuntimeEvidenceSnapshot::unknown_for_plan(&unknown_plan, PREFLIGHT_EVALUATED),
    );
    assert_eq!(
        unknown.run(&unknown_run).unwrap().preflight_status(),
        Some(PreflightOutcome::Unknown)
    );
    let mut unknown_ingress = ingress(&unknown, &unknown_run, "auth_unknown");
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut unknown,
                &unknown_run,
                &mut unknown_ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::PreflightNotReady
    );
    assert_eq!(unknown_ingress.calls, 0);

    let mut mismatch = configuration();
    mismatch.model = config_value("different-model");
    let blocked_plan = plan(
        "experiment_blocked",
        1,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    let verified = GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture);
    let blocked_snapshot = RuntimeEvidenceSnapshot::new(
        Some(mismatch),
        verified,
        meter_evidence(blocked_plan.target_quota_meters(), verified),
        meter_evidence(blocked_plan.target_quota_meters(), verified),
        IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
        verified,
        PREFLIGHT_EVALUATED,
    );
    let (mut blocked, blocked_run) = prepared(
        &coordinator,
        "experiment_blocked",
        "run_blocked",
        1,
        blocked_snapshot,
    );
    assert_eq!(
        blocked.run(&blocked_run).unwrap().preflight_status(),
        Some(PreflightOutcome::Blocked)
    );
    let mut blocked_ingress = ingress(&blocked, &blocked_run, "auth_blocked");
    assert_eq!(
        coordinator
            .execute(
                &mut blocked,
                &blocked_run,
                &mut blocked_ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::PreflightNotReady
    );
    assert_eq!(blocked_ingress.calls, 0);
}

#[test]
fn unavailable_configuration_dimension_remains_unknown() {
    let mut planned = configuration();
    planned.speed_mode = unavailable();
    let observed = configuration();
    assert_eq!(
        configuration_readiness(&planned, Some(&observed)),
        GateEvidence::unknown(UnknownEvidenceReason::ConfigurationNotObserved)
    );
}

#[test]
fn test_fixture_preflight_cannot_cross_a_live_dispatch_boundary() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_live_for_test(directory.guard());
    let plan = plan(
        "experiment_fixture",
        1,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    let fixture = GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture);
    let (mut experiment, run) = prepared(
        &coordinator,
        "experiment_fixture",
        "run_fixture",
        1,
        RuntimeEvidenceSnapshot::new(
            Some(configuration()),
            fixture,
            meter_evidence(plan.target_quota_meters(), fixture),
            meter_evidence(plan.target_quota_meters(), fixture),
            IsolationClassification::verified(EvidenceBasis::AuthorizedTestFixture),
            fixture,
            PREFLIGHT_EVALUATED,
        ),
    );
    let mut ingress = ingress(&experiment, &run, "auth_fixture");
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::TestFixtureEvidenceRejected
    );
    assert_eq!(ingress.calls, 0);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn exactly_one_simulated_dispatch_yields_execution_ended_not_measurement_complete() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_success", "run_success");
    let evidence = correlation(
        &experiment,
        &run,
        "task:runtime",
        "session:runtime",
        "source:runtime",
        IsolationClassification::Unknown,
    );
    let adapter = FakeAdapter::ended(Some(evidence));
    let mut ingress = ingress(&experiment, &run, "auth_success");
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::ExecutionEnded);
    assert_eq!(
        receipt.correlation_status(),
        RuntimeCorrelationStatus::AwaitingObservation
    );
    assert!(receipt.is_synthetic());
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ingress.calls, 1);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::AwaitingObservation
    );
}

#[test]
fn public_p024_authorization_value_alone_is_not_an_ingress() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_disabled(directory.guard());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_public_auth", "run_public_auth");
    let _ordinary_value = auth(
        "experiment_public_auth",
        "run_public_auth",
        0,
        "auth_public",
    );
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut FakeIngress::default(),
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::ExecutionDisabled
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn wrong_experiment_run_or_repetition_authorization_is_rejected_before_dispatch() {
    for (experiment_name, run_name, wrong) in [
        (
            "experiment_wrong_exp",
            "run_wrong_exp",
            auth("experiment_other", "run_wrong_exp", 0, "auth_wrong_exp"),
        ),
        (
            "experiment_wrong_run",
            "run_wrong_run",
            auth("experiment_wrong_run", "run_other", 0, "auth_wrong_run"),
        ),
        (
            "experiment_wrong_rep",
            "run_wrong_rep",
            auth("experiment_wrong_rep", "run_wrong_rep", 1, "auth_wrong_rep"),
        ),
    ] {
        let directory = TestDirectory::new();
        let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
        let (mut experiment, run) = prepared_ready(&coordinator, experiment_name, run_name);
        let mut ingress = FakeIngress {
            authorization: Some(wrong),
            calls: 0,
        };
        let adapter = FakeAdapter::ended(None);
        let error = coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            RuntimeError::Benchmark(BenchmarkError::AuthorizationScopeMismatch)
        ));
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            coordinator
                .recover_dispatch_state(&DispatchScope::new(
                    experiment.plan().experiment_id().clone(),
                    run.clone(),
                    0,
                    AuthorizationId::parse("auth_never_used").unwrap(),
                ))
                .unwrap(),
            DispatchJournalState::NotReserved
        );
    }
}

#[test]
fn duplicate_authorization_id_cannot_authorize_another_run() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let plan = plan(
        "experiment_duplicate_auth",
        2,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    let mut experiment = BenchmarkExperiment::new(plan.clone()).unwrap();
    let run_a = RunId::parse("run_duplicate_auth_a").unwrap();
    let run_b = RunId::parse("run_duplicate_auth_b").unwrap();
    experiment.create_run(run_a.clone(), 0).unwrap();
    experiment.create_run(run_b.clone(), 1).unwrap();
    let source = FixedEvidenceSource {
        snapshot: ready_snapshot(&plan),
    };
    coordinator
        .evaluate_preflight(&mut experiment, &run_a, PREFLIGHT_STARTED, &source)
        .unwrap();
    coordinator
        .evaluate_preflight(&mut experiment, &run_b, PREFLIGHT_STARTED, &source)
        .unwrap();

    let mut ingress_a = FakeIngress {
        authorization: Some(auth(
            "experiment_duplicate_auth",
            run_a.as_str(),
            0,
            "auth_shared",
        )),
        calls: 0,
    };
    let adapter_a = FakeAdapter::ended(None);
    coordinator
        .execute(
            &mut experiment,
            &run_a,
            &mut ingress_a,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter_a.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();

    let mut ingress_b = FakeIngress {
        authorization: Some(auth(
            "experiment_duplicate_auth",
            run_b.as_str(),
            1,
            "auth_shared",
        )),
        calls: 0,
    };
    let adapter_b = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run_b,
                &mut ingress_b,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter_b.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::Benchmark(BenchmarkError::AuthorizationScopeMismatch)
    );
    assert_eq!(adapter_a.calls.load(Ordering::SeqCst), 1);
    assert_eq!(adapter_b.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn two_coordinator_instances_sharing_one_journal_cannot_dispatch_the_same_run_twice() {
    let directory = TestDirectory::new();
    let shared_guard = directory.guard();
    let first = BenchmarkRuntimeCoordinator::new_simulation(shared_guard.clone());
    let second = BenchmarkRuntimeCoordinator::new_simulation(shared_guard);
    let (mut experiment_a, run_a) = prepared_ready(
        &first,
        "experiment_shared_coordinator",
        "run_shared_coordinator",
    );
    let (mut experiment_b, run_b) = prepared_ready(
        &second,
        "experiment_shared_coordinator",
        "run_shared_coordinator",
    );
    let mut ingress_a = ingress(&experiment_a, &run_a, "auth_shared_coordinator");
    let mut ingress_b = ingress(&experiment_b, &run_b, "auth_shared_coordinator");
    let adapter_a = FakeAdapter::ended(None);
    let adapter_b = FakeAdapter::ended(None);
    first
        .execute(
            &mut experiment_a,
            &run_a,
            &mut ingress_a,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter_a.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(
        second
            .execute(
                &mut experiment_b,
                &run_b,
                &mut ingress_b,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter_b.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::DispatchReservationConflict
    );
    assert_eq!(adapter_a.calls.load(Ordering::SeqCst), 1);
    assert_eq!(adapter_b.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn adapter_unavailable_and_unknown_workload_fail_before_reservation() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_adapter_missing",
        "run_adapter_missing",
    );
    let mut missing_adapter_auth = ingress(&experiment, &run, "auth_adapter_missing");
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut missing_adapter_auth,
                &FakeWorkloadResolver { accepts_key: true },
                None,
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::AdapterUnavailable
    );

    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_unknown_workload",
        "run_unknown_workload",
    );
    let mut workload_auth = ingress(&experiment, &run, "auth_unknown_workload");
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut workload_auth,
                &FakeWorkloadResolver { accepts_key: false },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::Adapter(RuntimeAdapterError::UnknownWorkloadKey)
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn adapter_validation_failure_consumes_authorization_without_reserving_or_invoking() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_bad_adapter", "run_bad_adapter");
    let mut ingress = ingress(&experiment, &run, "auth_bad_adapter");
    let mut adapter = FakeAdapter::ended(None);
    adapter.fail_validation = true;
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::AdapterUnavailable
    );
    assert_eq!(ingress.calls, 1);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Authorized
    );
}

struct FaultingGuard {
    inner: Arc<FileBenchmarkDispatchGuard>,
    fail_reservation: bool,
    fail_acknowledgement: bool,
}

impl crate::benchmark::dispatch::sealed::Sealed for FaultingGuard {}

impl BenchmarkDispatchGuard for FaultingGuard {
    fn reserve(
        &self,
        scope: &DispatchScope,
        reserved_at: &str,
    ) -> Result<DispatchReservation, DispatchJournalError> {
        if self.fail_reservation {
            Err(DispatchJournalError::Io)
        } else {
            self.inner.reserve(scope, reserved_at)
        }
    }

    fn acknowledge(
        &self,
        reservation: &DispatchReservation,
        at: &str,
    ) -> Result<(), DispatchJournalError> {
        if self.fail_acknowledgement {
            Err(DispatchJournalError::Io)
        } else {
            self.inner.acknowledge(reservation, at)
        }
    }

    fn finish(
        &self,
        reservation: &DispatchReservation,
        outcome: DispatchTerminalOutcome,
        at: &str,
    ) -> Result<(), DispatchJournalError> {
        self.inner.finish(reservation, outcome, at)
    }

    fn state(&self, scope: &DispatchScope) -> Result<DispatchJournalState, DispatchJournalError> {
        self.inner.state(scope)
    }

    fn claim_observation(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
        at: &str,
    ) -> Result<(), DispatchJournalError> {
        self.inner
            .claim_observation(observation_id, experiment_id, run_id, repetition_index, at)
    }

    fn acquire_execution_slot(
        &self,
        reservation: &DispatchReservation,
    ) -> Result<DispatchExecutionLease, DispatchJournalError> {
        self.inner.acquire_execution_slot(reservation)
    }
}

#[test]
fn reservation_persistence_failure_prevents_adapter_invocation() {
    let directory = TestDirectory::new();
    let base = directory.guard();
    let guard = Arc::new(FaultingGuard {
        inner: Arc::clone(&base),
        fail_reservation: true,
        fail_acknowledgement: false,
    });
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(guard);
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_reservation_failure",
        "run_reservation_failure",
    );
    let mut ingress = ingress(&experiment, &run, "auth_reservation_failure");
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::DispatchPersistenceFailure
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        base.state(&scope(&experiment, &run, "auth_reservation_failure"))
            .unwrap(),
        DispatchJournalState::NotReserved
    );
}

#[test]
fn adapter_call_before_acknowledgement_failure_recovers_as_uncertain() {
    let directory = TestDirectory::new();
    let base = directory.guard();
    let guard = Arc::new(FaultingGuard {
        inner: Arc::clone(&base),
        fail_reservation: false,
        fail_acknowledgement: true,
    });
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(guard);
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_ack_failure", "run_ack_failure");
    let mut ingress = ingress(&experiment, &run, "auth_ack_failure");
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::DispatchPersistenceFailure
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        FileBenchmarkDispatchGuard::open(&directory.0)
            .unwrap()
            .state(&scope(&experiment, &run, "auth_ack_failure"))
            .unwrap(),
        DispatchJournalState::ReservedOutcomeUnknown
    );
}

#[test]
fn cancellation_before_authorization_prevents_ingress_and_dispatch() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_cancel_before",
        "run_cancel_before",
    );
    let mut ingress = ingress(&experiment, &run, "auth_cancel_before");
    let adapter = FakeAdapter::ended(None);
    let token = CancellationToken::default();
    token.cancel();
    assert_eq!(
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                token,
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::ExecutionCancelled
    );
    assert_eq!(ingress.calls, 0);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

struct CancelAfterReserveGuard {
    inner: Arc<FileBenchmarkDispatchGuard>,
    cancellation: CancellationToken,
}

impl crate::benchmark::dispatch::sealed::Sealed for CancelAfterReserveGuard {}

impl BenchmarkDispatchGuard for CancelAfterReserveGuard {
    fn reserve(
        &self,
        scope: &DispatchScope,
        reserved_at: &str,
    ) -> Result<DispatchReservation, DispatchJournalError> {
        let reservation = self.inner.reserve(scope, reserved_at)?;
        self.cancellation.cancel();
        Ok(reservation)
    }

    fn acknowledge(
        &self,
        reservation: &DispatchReservation,
        at: &str,
    ) -> Result<(), DispatchJournalError> {
        self.inner.acknowledge(reservation, at)
    }

    fn finish(
        &self,
        reservation: &DispatchReservation,
        outcome: DispatchTerminalOutcome,
        at: &str,
    ) -> Result<(), DispatchJournalError> {
        self.inner.finish(reservation, outcome, at)
    }

    fn state(&self, scope: &DispatchScope) -> Result<DispatchJournalState, DispatchJournalError> {
        self.inner.state(scope)
    }

    fn claim_observation(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
        at: &str,
    ) -> Result<(), DispatchJournalError> {
        self.inner
            .claim_observation(observation_id, experiment_id, run_id, repetition_index, at)
    }

    fn acquire_execution_slot(
        &self,
        reservation: &DispatchReservation,
    ) -> Result<DispatchExecutionLease, DispatchJournalError> {
        self.inner.acquire_execution_slot(reservation)
    }
}

#[test]
fn cancellation_after_reservation_prevents_adapter_invocation_and_stays_consumed() {
    let directory = TestDirectory::new();
    let cancellation = CancellationToken::default();
    let guard = Arc::new(CancelAfterReserveGuard {
        inner: directory.guard(),
        cancellation: cancellation.clone(),
    });
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(guard.clone());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_cancel_reserved",
        "run_cancel_reserved",
    );
    let mut ingress = ingress(&experiment, &run, "auth_cancel_reserved");
    let adapter = FakeAdapter::ended(None);
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            cancellation,
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::Cancelled);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        guard
            .state(&scope(&experiment, &run, "auth_cancel_reserved"))
            .unwrap(),
        DispatchJournalState::Terminal(DispatchTerminalOutcome::CancelledBeforeDispatch)
    );
}

#[test]
fn cancellation_at_adapter_boundary_prevents_new_dispatch() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_cancel_boundary",
        "run_cancel_boundary",
    );
    let mut ingress = ingress(&experiment, &run, "auth_cancel_boundary");
    let cancellation = CancellationToken::default();
    let adapter = FakeAdapter::ended(None);
    let clock = CancellingClock {
        calls: AtomicUsize::new(0),
        cancellation: cancellation.clone(),
    };
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            cancellation,
            &clock,
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::Cancelled);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        coordinator
            .recover_dispatch_state(&scope(&experiment, &run, "auth_cancel_boundary"))
            .unwrap(),
        DispatchJournalState::Terminal(DispatchTerminalOutcome::CancelledBeforeDispatch)
    );
}

#[test]
fn cancellation_during_execution_preserves_uncertain_status_without_retry() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_cancel_during",
        "run_cancel_during",
    );
    let mut ingress = ingress(&experiment, &run, "auth_cancel_during");
    let token = CancellationToken::default();
    let mut adapter = FakeAdapter::ended(None);
    adapter.status = AdapterExecutionStatus::Cancelled { confirmed: false };
    adapter.cancel_during_call = Some(token.clone());
    adapter.ended_at = None;
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            token,
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::OutcomeUnknown);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Aborted
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        coordinator
            .recover_dispatch_state(&scope(&experiment, &run, "auth_cancel_during"))
            .unwrap(),
        DispatchJournalState::Terminal(DispatchTerminalOutcome::OutcomeUnknown)
    );
    assert!(coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .is_err());
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn adapter_confirmed_cancellation_records_execution_end() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_cancel_confirmed",
        "run_cancel_confirmed",
    );
    let mut ingress = ingress(&experiment, &run, "auth_cancel_confirmed");
    let token = CancellationToken::default();
    let mut adapter = FakeAdapter::ended(None);
    adapter.status = AdapterExecutionStatus::Cancelled { confirmed: true };
    adapter.cancel_during_call = Some(token.clone());
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            token,
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::Cancelled);
    assert_eq!(receipt.ended_at(), Some(ENDED));
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::AwaitingObservation
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn timeout_is_unknown_and_exact_replay_cannot_launch_again() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_timeout", "run_timeout");
    let mut ingress = ingress(&experiment, &run, "auth_timeout");
    let mut adapter = FakeAdapter::ended(None);
    adapter.delay = Duration::from_millis(15);
    let cancellation = CancellationToken::default();
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_millis(1)),
            cancellation.clone(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::OutcomeUnknown);
    assert!(cancellation.is_cancelled());
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Aborted
    );
    wait_for_dispatch_slot_release(&directory);
    assert!(adapter.calls.load(Ordering::SeqCst) <= 1);
    assert_eq!(
        coordinator
            .recover_dispatch_state(&scope(&experiment, &run, "auth_timeout"))
            .unwrap(),
        DispatchJournalState::Terminal(DispatchTerminalOutcome::TimedOut)
    );
    assert!(matches!(
        coordinator.execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        ),
        Err(RuntimeError::Benchmark(BenchmarkError::ConflictingReplay))
            | Err(RuntimeError::DispatchReservationConflict)
    ));
    assert!(adapter.calls.load(Ordering::SeqCst) <= 1);
}

#[test]
fn adapter_failure_is_terminal_and_does_not_trigger_a_retry() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_failure", "run_failure");
    let mut ingress = ingress(&experiment, &run, "auth_failure");
    let mut adapter = FakeAdapter::ended(None);
    adapter.status = AdapterExecutionStatus::Failed;
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::Failed);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::AwaitingObservation
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn timeout_stop_waiting_policy_does_not_claim_cancellation_succeeded() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_stop_waiting", "run_stop_waiting");
    let mut ingress = ingress(&experiment, &run, "auth_stop_waiting");
    let mut adapter = FakeAdapter::ended(None);
    adapter.delay = Duration::from_millis(15);
    let token = CancellationToken::default();
    let stop_waiting_policy = RuntimeExecutionPolicy::new(
        Duration::from_millis(1),
        Duration::from_secs(5),
        CancellationPolicy::StopWaitingOnly,
    )
    .unwrap();
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            stop_waiting_policy,
            token.clone(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::OutcomeUnknown);
    assert!(!token.is_cancelled());
    wait_for_dispatch_slot_release(&directory);
    assert!(adapter.calls.load(Ordering::SeqCst) <= 1);
}

#[test]
fn adapter_unknown_return_is_preserved_without_retry() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_unknown_result",
        "run_unknown_result",
    );
    let mut ingress = ingress(&experiment, &run, "auth_unknown_result");
    let mut adapter = FakeAdapter::ended(None);
    adapter.fail_execution = true;
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.status(), RuntimeExecutionStatus::OutcomeUnknown);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Aborted
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn deterministic_receipt_has_only_opaque_scope_status_and_times() {
    let make_receipt = || {
        let directory = TestDirectory::new();
        let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
        let (mut experiment, run) =
            prepared_ready(&coordinator, "experiment_receipt", "run_receipt");
        let mut ingress = ingress(&experiment, &run, "auth_receipt");
        let adapter = FakeAdapter::ended(None);
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap()
    };
    let first = make_receipt();
    let second = make_receipt();
    assert_eq!(first, second);
    assert!(first.is_synthetic());
    let serialized = format!("{first:?}");
    assert!(!serialized.contains("prompt"));
    assert!(!serialized.contains("response"));
    assert!(!serialized.contains("command"));
}

#[test]
fn exact_scope_binds_authorization_and_no_batch_is_dispatched() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_one_run", "run_one_run");
    let mut ingress = ingress(&experiment, &run, "auth_one_run");
    let adapter = FakeAdapter::ended(None);
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(receipt.repetition_index(), 0);
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(ingress.calls, 1);
}

#[test]
fn sqlite_resolver_returns_only_existing_observations() {
    let store = SqliteStore::open_in_memory().unwrap();
    let plan = plan(
        "experiment_sqlite_resolver",
        1,
        vec![QuotaMeterType::FiveHour],
    );
    let mut experiment = BenchmarkExperiment::new(plan).unwrap();
    let run = RunId::parse("run_sqlite_resolver").unwrap();
    experiment.create_run(run.clone(), 0).unwrap();
    let correlation = correlation(
        &experiment,
        &run,
        "task:runtime",
        "session:runtime",
        "source:runtime",
        IsolationClassification::Unknown,
    );
    let resolver = SqliteBenchmarkObservationResolver::new(&store);
    assert!(resolver.resolve(&correlation).unwrap().is_none());
    let _ = store.foreign_keys_enabled().unwrap();
}

#[test]
fn caller_observation_id_alone_cannot_construct_runtime_correlation() {
    let arbitrary = format!("obs:{}", "a".repeat(64));
    assert!(ExecutionCorrelationEvidence::from_verified_adapter(
        ExperimentId::parse("experiment_no_corr").unwrap(),
        RunId::parse("run_no_corr").unwrap(),
        0,
        arbitrary,
        "",
        "session:runtime",
        "source:runtime",
        IsolationClassification::Unknown,
    )
    .is_err());
}

#[test]
fn matching_task_session_source_and_terminal_observation_complete_run() {
    let directory = TestDirectory::new();
    let guard = directory.guard();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(guard.clone());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_correlation", "run_correlation");
    let correlation = correlation(
        &experiment,
        &run,
        "task:runtime",
        "session:runtime",
        "source:runtime",
        IsolationClassification::Unknown,
    );
    let mut ingress = ingress(&experiment, &run, "auth_correlation");
    let adapter = FakeAdapter::ended(Some(correlation.clone()));
    let receipt = coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert!(receipt.correlation_evidence().is_some());
    let observation = observation(
        experiment.plan(),
        ObservationLifecycle::Finalized,
        "task:runtime",
        "session:runtime",
        "source:runtime",
    );
    let resolver = FakeObservationResolver {
        stored: Some(stored(observation.clone())),
    };
    assert_eq!(
        coordinator
            .reconcile_observation(&mut experiment, &run, &correlation, &resolver, LINKED)
            .unwrap(),
        BenchmarkRunState::Completed
    );
    assert_eq!(
        experiment.run(&run).unwrap().isolation_status(),
        IsolationClassification::Unknown
    );
    assert_eq!(observation.summary_quality, QualityGrade::B);
    assert_eq!(
        observation
            .token_evidence
            .raw_token_counters
            .raw_total
            .value,
        Some(18)
    );
    assert_eq!(
        guard
            .observation_claim_owner(correlation.observation_id())
            .unwrap()
            .unwrap()
            .2,
        0
    );
}

#[test]
fn nonterminal_observation_stays_awaiting_and_is_not_claimed() {
    let directory = TestDirectory::new();
    let guard = directory.guard();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(guard.clone());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_nonterminal", "run_nonterminal");
    let correlation = correlation(
        &experiment,
        &run,
        "task:pending",
        "session:pending",
        "source:pending",
        IsolationClassification::Unknown,
    );
    let mut ingress = ingress(&experiment, &run, "auth_nonterminal");
    let adapter = FakeAdapter::ended(Some(correlation.clone()));
    coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    let resolver = FakeObservationResolver {
        stored: Some(stored(observation(
            experiment.plan(),
            ObservationLifecycle::Reconciling,
            "task:pending",
            "session:pending",
            "source:pending",
        ))),
    };
    assert_eq!(
        coordinator
            .reconcile_observation(&mut experiment, &run, &correlation, &resolver, LINKED)
            .unwrap_err(),
        RuntimeError::Benchmark(BenchmarkError::ObservationUnresolved)
    );
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::AwaitingObservation
    );
    assert!(guard
        .observation_claim_owner(correlation.observation_id())
        .unwrap()
        .is_none());
}

#[test]
fn mismatched_experiment_run_configuration_or_task_session_is_rejected() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(&coordinator, "experiment_mismatch", "run_mismatch");
    let mut ingress = ingress(&experiment, &run, "auth_mismatch");
    let correlation = correlation(
        &experiment,
        &run,
        "task:expected",
        "session:expected",
        "source:expected",
        IsolationClassification::Unknown,
    );
    let adapter = FakeAdapter::ended(Some(correlation.clone()));
    coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();

    let wrong_scope = self::correlation(
        &experiment,
        &RunId::parse("run_wrong_link").unwrap(),
        "task:expected",
        "session:expected",
        "source:expected",
        IsolationClassification::Unknown,
    );
    assert_eq!(
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &wrong_scope,
                &FakeObservationResolver { stored: None },
                LINKED,
            )
            .unwrap_err(),
        RuntimeError::ObservationCorrelationUnverified
    );

    let other_exp = ExecutionCorrelationEvidence::from_verified_adapter(
        ExperimentId::parse("experiment_other").unwrap(),
        run.clone(),
        0,
        correlation.observation_id().as_str(),
        "task:expected",
        "session:expected",
        "source:expected",
        IsolationClassification::Unknown,
    )
    .unwrap();
    assert_eq!(
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &other_exp,
                &FakeObservationResolver { stored: None },
                LINKED,
            )
            .unwrap_err(),
        RuntimeError::ObservationCorrelationUnverified
    );

    let wrong_observation = observation(
        experiment.plan(),
        ObservationLifecycle::Finalized,
        "task:different",
        "session:different",
        "source:different",
    );
    assert_eq!(
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &correlation,
                &FakeObservationResolver {
                    stored: Some(stored(wrong_observation)),
                },
                LINKED,
            )
            .unwrap_err(),
        RuntimeError::ObservationCorrelationUnverified
    );

    let mut wrong_configuration = observation(
        experiment.plan(),
        ObservationLifecycle::Finalized,
        "task:expected",
        "session:expected",
        "source:expected",
    );
    wrong_configuration.configuration.model = config_value("other-model");
    assert_eq!(
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &correlation,
                &FakeObservationResolver {
                    stored: Some(stored(wrong_configuration)),
                },
                LINKED,
            )
            .unwrap_err(),
        RuntimeError::Benchmark(BenchmarkError::ConfigurationMismatch)
    );
}

#[test]
fn duplicate_observation_claim_is_shared_across_coordinators_and_survives_restart() {
    let directory = TestDirectory::new();
    let first_guard = directory.guard();
    let first = BenchmarkRuntimeCoordinator::new_simulation(first_guard.clone());
    let (mut experiment_a, run_a) = prepared_ready(&first, "experiment_claim_a", "run_claim_a");
    let correlation_a = correlation(
        &experiment_a,
        &run_a,
        "task:shared",
        "session:shared",
        "source:shared",
        IsolationClassification::Unknown,
    );
    let mut ingress_a = ingress(&experiment_a, &run_a, "auth_claim_a");
    let adapter_a = FakeAdapter::ended(Some(correlation_a.clone()));
    first
        .execute(
            &mut experiment_a,
            &run_a,
            &mut ingress_a,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter_a.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    let shared_observation = observation(
        experiment_a.plan(),
        ObservationLifecycle::Finalized,
        "task:shared",
        "session:shared",
        "source:shared",
    );
    let resolver = FakeObservationResolver {
        stored: Some(stored(shared_observation.clone())),
    };
    first
        .reconcile_observation(&mut experiment_a, &run_a, &correlation_a, &resolver, LINKED)
        .unwrap();

    let second = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment_b, run_b) = prepared_ready(&second, "experiment_claim_b", "run_claim_b");
    let correlation_b = correlation(
        &experiment_b,
        &run_b,
        "task:shared",
        "session:shared",
        "source:shared",
        IsolationClassification::Unknown,
    );
    let mut ingress_b = ingress(&experiment_b, &run_b, "auth_claim_b");
    let adapter_b = FakeAdapter::ended(Some(correlation_b.clone()));
    second
        .execute(
            &mut experiment_b,
            &run_b,
            &mut ingress_b,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter_b.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    assert_eq!(
        second
            .reconcile_observation(&mut experiment_b, &run_b, &correlation_b, &resolver, LINKED)
            .unwrap_err(),
        RuntimeError::ObservationClaimConflict
    );
    assert_eq!(
        experiment_b.run(&run_b).unwrap().state(),
        BenchmarkRunState::AwaitingObservation
    );
    assert_eq!(
        FileBenchmarkDispatchGuard::open(&directory.0)
            .unwrap()
            .observation_claim_owner(correlation_a.observation_id())
            .unwrap()
            .unwrap()
            .0
            .as_str(),
        "experiment_claim_a"
    );
}

#[test]
fn incomplete_and_invalid_observations_remain_incomplete_and_quality_is_not_promoted() {
    for lifecycle in [
        ObservationLifecycle::Incomplete,
        ObservationLifecycle::Invalid,
    ] {
        let directory = TestDirectory::new();
        let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
        let (mut experiment, run) = prepared_ready(
            &coordinator,
            if lifecycle == ObservationLifecycle::Incomplete {
                "experiment_incomplete_observation"
            } else {
                "experiment_invalid_observation"
            },
            if lifecycle == ObservationLifecycle::Incomplete {
                "run_incomplete_observation"
            } else {
                "run_invalid_observation"
            },
        );
        let task_id = if lifecycle == ObservationLifecycle::Incomplete {
            "task:terminal_incomplete"
        } else {
            "task:terminal_invalid"
        };
        let session_id = if lifecycle == ObservationLifecycle::Incomplete {
            "session:terminal_incomplete"
        } else {
            "session:terminal_invalid"
        };
        let source_id = if lifecycle == ObservationLifecycle::Incomplete {
            "source:terminal_incomplete"
        } else {
            "source:terminal_invalid"
        };
        let correlation = correlation(
            &experiment,
            &run,
            task_id,
            session_id,
            source_id,
            IsolationClassification::Unknown,
        );
        let mut ingress = ingress(&experiment, &run, "auth_terminal_incomplete");
        let adapter = FakeAdapter::ended(Some(correlation.clone()));
        coordinator
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap();
        let mut terminal =
            observation(experiment.plan(), lifecycle, task_id, session_id, source_id);
        terminal.summary_quality = QualityGrade::D;
        let original = terminal.clone();
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &correlation,
                &FakeObservationResolver {
                    stored: Some(stored(terminal)),
                },
                LINKED,
            )
            .unwrap();
        assert_eq!(
            experiment.run(&run).unwrap().state(),
            BenchmarkRunState::Incomplete
        );
        assert_eq!(original.summary_quality, QualityGrade::D);
        assert_eq!(
            experiment.run(&run).unwrap().isolation_status(),
            IsolationClassification::Unknown
        );
    }
}

#[test]
fn reset_evidence_for_one_meter_does_not_rewrite_the_other_meter() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) = prepared_ready(
        &coordinator,
        "experiment_meter_independent",
        "run_meter_independent",
    );
    let correlation = correlation(
        &experiment,
        &run,
        "task:meter_independent",
        "session:meter_independent",
        "source:meter_independent",
        IsolationClassification::Unknown,
    );
    let mut ingress = ingress(&experiment, &run, "auth_meter_independent");
    let adapter = FakeAdapter::ended(Some(correlation.clone()));
    coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    let mut evidence = observation(
        experiment.plan(),
        ObservationLifecycle::Incomplete,
        "task:meter_independent",
        "session:meter_independent",
        "source:meter_independent",
    );
    evidence.quota_evidence.five_hour.status.validity = EvidenceValidity::Incomplete;
    evidence.quota_evidence.five_hour.reset_status = ResetStatus::Detected;
    evidence.quota_evidence.five_hour.delta_percentage_points = None;
    let weekly_before = evidence.quota_evidence.weekly.clone();
    coordinator
        .reconcile_observation(
            &mut experiment,
            &run,
            &correlation,
            &FakeObservationResolver {
                stored: Some(stored(evidence.clone())),
            },
            LINKED,
        )
        .unwrap();
    assert_eq!(
        evidence.quota_evidence.five_hour.reset_status,
        ResetStatus::Detected
    );
    assert_eq!(evidence.quota_evidence.weekly, weekly_before);
    assert_eq!(
        experiment.run(&run).unwrap().state(),
        BenchmarkRunState::Incomplete
    );
}

#[test]
fn replay_after_restart_reports_uncertain_reservation_without_auto_execution() {
    let directory = TestDirectory::new();
    let scope = DispatchScope::new(
        ExperimentId::parse("experiment_restart").unwrap(),
        RunId::parse("run_restart").unwrap(),
        0,
        AuthorizationId::parse("auth_restart").unwrap(),
    );
    FileBenchmarkDispatchGuard::open(&directory.0)
        .unwrap()
        .reserve(&scope, RESERVED)
        .unwrap();
    let restarted = BenchmarkRuntimeCoordinator::new_disabled(directory.guard());
    assert_eq!(
        restarted.recover_dispatch_state(&scope).unwrap(),
        DispatchJournalState::ReservedOutcomeUnknown
    );
    let (mut experiment, run) =
        prepared_ready(&restarted, "experiment_restart_other", "run_restart_other");
    let mut ingress = ingress(&experiment, &run, "auth_restart_other");
    let adapter = FakeAdapter::ended(None);
    assert_eq!(
        restarted
            .execute(
                &mut experiment,
                &run,
                &mut ingress,
                &FakeWorkloadResolver { accepts_key: true },
                Some(Arc::new(adapter.clone())),
                policy(Duration::from_secs(2)),
                CancellationToken::default(),
                &FixedClock::standard(),
            )
            .unwrap_err(),
        RuntimeError::ExecutionDisabled
    );
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn missing_meter_coverage_is_rejected_and_never_becomes_zero_usage() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_disabled(directory.guard());
    let plan = plan(
        "experiment_meter_coverage",
        1,
        vec![QuotaMeterType::FiveHour, QuotaMeterType::Weekly],
    );
    let verified = GateEvidence::verified(EvidenceBasis::AuthorizedTestFixture);
    let snapshot = RuntimeEvidenceSnapshot::new(
        Some(configuration()),
        verified,
        meter_evidence(&[QuotaMeterType::FiveHour], verified),
        meter_evidence(plan.target_quota_meters(), verified),
        IsolationClassification::Unknown,
        GateEvidence::unknown(UnknownEvidenceReason::ExecutionBoundaryNotObserved),
        PREFLIGHT_EVALUATED,
    );
    let mut experiment = BenchmarkExperiment::new(plan).unwrap();
    let run = RunId::parse("run_meter_coverage").unwrap();
    experiment.create_run(run.clone(), 0).unwrap();
    let result = coordinator.evaluate_preflight(
        &mut experiment,
        &run,
        PREFLIGHT_STARTED,
        &FixedEvidenceSource { snapshot },
    );
    assert_eq!(
        result.unwrap_err(),
        RuntimeError::Benchmark(BenchmarkError::InvalidPreflight)
    );
}

#[test]
fn no_shell_command_or_environment_payload_exists_in_execution_contract() {
    let workload = WorkloadKey::parse("workload_a").unwrap();
    assert_eq!(workload.as_str(), "workload_a");
    let serialized = format!("{workload:?}");
    assert!(!serialized.contains("shell"));
    assert!(!serialized.contains("environment"));
}

#[test]
fn evidence_wait_and_elapsed_bounds_reject_zero_and_unbounded_policies() {
    assert_eq!(
        RuntimeExecutionPolicy::new(
            Duration::ZERO,
            Duration::from_secs(1),
            CancellationPolicy::RequestWhenSupported,
        )
        .unwrap_err(),
        RuntimeAdapterError::InvalidExecutionPolicy
    );
    assert_eq!(
        RuntimeExecutionPolicy::new(
            Duration::from_secs(1),
            Duration::from_secs(24 * 60 * 60 + 1),
            CancellationPolicy::StopWaitingOnly,
        )
        .unwrap_err(),
        RuntimeAdapterError::InvalidExecutionPolicy
    );
}

#[test]
fn observation_id_mismatch_fails_before_trusted_binding_and_claim() {
    let directory = TestDirectory::new();
    let guard = directory.guard();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(guard.clone());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_bad_obs_id", "run_bad_obs_id");
    let correlation = correlation(
        &experiment,
        &run,
        "task:identity",
        "session:identity",
        "source:identity",
        IsolationClassification::Unknown,
    );
    let mut ingress = ingress(&experiment, &run, "auth_bad_obs_id");
    let adapter = FakeAdapter::ended(Some(correlation.clone()));
    coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    let mut wrong = observation(
        experiment.plan(),
        ObservationLifecycle::Finalized,
        "task:identity",
        "session:identity",
        "source:identity",
    );
    wrong.observation_id = format!("obs:{}", "0".repeat(64));
    assert_eq!(
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &correlation,
                &FakeObservationResolver {
                    stored: Some(stored(wrong)),
                },
                LINKED,
            )
            .unwrap_err(),
        RuntimeError::ObservationCorrelationUnverified
    );
    assert!(guard
        .observation_claim_owner(correlation.observation_id())
        .unwrap()
        .is_none());
}

#[test]
fn source_identity_is_part_of_trusted_correlation() {
    let directory = TestDirectory::new();
    let coordinator = BenchmarkRuntimeCoordinator::new_simulation(directory.guard());
    let (mut experiment, run) =
        prepared_ready(&coordinator, "experiment_source_id", "run_source_id");
    let correlation = correlation(
        &experiment,
        &run,
        "task:source",
        "session:source",
        "source:expected",
        IsolationClassification::Unknown,
    );
    let mut ingress = ingress(&experiment, &run, "auth_source_id");
    let adapter = FakeAdapter::ended(Some(correlation.clone()));
    coordinator
        .execute(
            &mut experiment,
            &run,
            &mut ingress,
            &FakeWorkloadResolver { accepts_key: true },
            Some(Arc::new(adapter.clone())),
            policy(Duration::from_secs(2)),
            CancellationToken::default(),
            &FixedClock::standard(),
        )
        .unwrap();
    let wrong_source = observation(
        experiment.plan(),
        ObservationLifecycle::Finalized,
        "task:source",
        "session:source",
        "source:other",
    );
    assert_eq!(
        coordinator
            .reconcile_observation(
                &mut experiment,
                &run,
                &correlation,
                &FakeObservationResolver {
                    stored: Some(stored(wrong_source)),
                },
                LINKED,
            )
            .unwrap_err(),
        RuntimeError::ObservationCorrelationUnverified
    );
}
