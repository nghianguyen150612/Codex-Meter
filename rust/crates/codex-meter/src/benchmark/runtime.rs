//! Authorized benchmark orchestration boundary.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use crate::storage::{SqliteStore, StoredObservation};
use crate::telemetry::{
    ConfigurationIdentity, MetricAvailability, MetricProvenance, NormalizedObservation,
};

use super::adapter::{
    AdapterExecutionResult, AdapterExecutionStatus, BenchmarkExecutionAdapter,
    BenchmarkWorkloadResolver, CancellationToken, ExecutionControl, ExecutionCorrelationEvidence,
    RuntimeAdapterError, RuntimeExecutionPolicy,
};
use super::dispatch::{
    BenchmarkDispatchGuard, DispatchJournalError, DispatchJournalState, DispatchScope,
    DispatchTerminalOutcome,
};
use super::evidence::{ObservationId, TrustedRuntimeBinding};
use super::lifecycle::{
    BenchmarkExperiment, BenchmarkObservationClaims, BenchmarkRunState, ExplicitRunAuthorization,
};
use super::model::{validate_configuration, AuthorizationId, ExperimentId, RunId};
use super::validation::{
    BenchmarkPreflight, EvidenceBasis, GateBlockReason, GateEvidence, IsolationClassification,
    MeterPreflightEvidence, PreflightOutcome, UnknownEvidenceReason,
};
use super::BenchmarkError;

/// Version of P025 authorization consumption, dispatch, correlation, and
/// restart handling. P024 framework semantics remain at 1.0.0.
pub const BENCHMARK_RUNTIME_VERSION: &str = "1.0.0";

const MAX_SHARED_OBSERVATION_CLAIMS: usize = 10_000;

enum WorkerExecutionResult {
    CancelledBeforeStart,
    Returned(Box<Result<AdapterExecutionResult, RuntimeAdapterError>>),
}

/// Requested runtime capability. Production construction currently supports
/// only `Disabled`; simulation construction exists only in unit tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionMode {
    Disabled,
    Simulation,
    Live,
}

/// Evidence returned by a trusted runtime capability source for one plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEvidenceSnapshot {
    observed_configuration: Option<ConfigurationIdentity>,
    telemetry_source: GateEvidence,
    quota_meter_availability: Vec<MeterPreflightEvidence>,
    quota_window_continuity: Vec<MeterPreflightEvidence>,
    isolation: IsolationClassification,
    execution_boundary_readiness: GateEvidence,
    evaluated_at: String,
}

impl RuntimeEvidenceSnapshot {
    #[allow(clippy::too_many_arguments)]
    #[cfg(test)]
    pub(crate) fn new(
        observed_configuration: Option<ConfigurationIdentity>,
        telemetry_source: GateEvidence,
        quota_meter_availability: Vec<MeterPreflightEvidence>,
        quota_window_continuity: Vec<MeterPreflightEvidence>,
        isolation: IsolationClassification,
        execution_boundary_readiness: GateEvidence,
        evaluated_at: impl Into<String>,
    ) -> Self {
        Self {
            observed_configuration,
            telemetry_source,
            quota_meter_availability,
            quota_window_continuity,
            isolation,
            execution_boundary_readiness,
            evaluated_at: evaluated_at.into(),
        }
    }

    pub fn unknown_for_plan(plan: &super::BenchmarkPlan, evaluated_at: impl Into<String>) -> Self {
        let unknown = GateEvidence::unknown(UnknownEvidenceReason::EvidenceUnavailable);
        Self {
            observed_configuration: None,
            telemetry_source: GateEvidence::unknown(
                UnknownEvidenceReason::TelemetrySourceNotObserved,
            ),
            quota_meter_availability: plan
                .target_quota_meters()
                .iter()
                .copied()
                .map(|meter| {
                    MeterPreflightEvidence::new(
                        meter,
                        GateEvidence::unknown(UnknownEvidenceReason::QuotaMeterNotObserved),
                    )
                })
                .collect(),
            quota_window_continuity: plan
                .target_quota_meters()
                .iter()
                .copied()
                .map(|meter| {
                    MeterPreflightEvidence::new(
                        meter,
                        GateEvidence::unknown(
                            UnknownEvidenceReason::QuotaWindowContinuityNotObserved,
                        ),
                    )
                })
                .collect(),
            isolation: IsolationClassification::Unknown,
            execution_boundary_readiness: unknown,
            evaluated_at: evaluated_at.into(),
        }
    }

    fn into_preflight(
        self,
        plan: &super::BenchmarkPlan,
    ) -> Result<BenchmarkPreflight, BenchmarkError> {
        let configuration_readiness =
            configuration_readiness(plan.configuration(), self.observed_configuration.as_ref());
        BenchmarkPreflight::new(
            plan,
            configuration_readiness,
            self.telemetry_source,
            self.quota_meter_availability,
            self.quota_window_continuity,
            self.isolation,
            self.execution_boundary_readiness,
            self.evaluated_at,
        )
    }
}

/// Independent capability source. Implementations must report observed facts,
/// preserve unknown values, and never infer isolation from process absence.
pub trait BenchmarkRuntimeEvidenceSource: Send + Sync {
    fn collect(
        &self,
        plan: &super::BenchmarkPlan,
        run_id: &RunId,
    ) -> Result<RuntimeEvidenceSnapshot, RuntimeError>;
}

/// Runtime clock used to record explicit UTC evidence at transition time.
/// The coordinator queries it immediately before reservation, execution, and
/// result/recovery transitions.
pub trait BenchmarkRuntimeClock {
    fn now_utc(&self) -> Result<String, RuntimeError>;
}

/// Trusted one-run user action request. Authorization ingress must bind its
/// result to these exact identities and one invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunAuthorizationRequest {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
}

impl RunAuthorizationRequest {
    pub fn new(experiment_id: ExperimentId, run_id: RunId, repetition_index: u32) -> Self {
        Self {
            experiment_id,
            run_id,
            repetition_index,
        }
    }

    pub fn experiment_id(&self) -> &ExperimentId {
        &self.experiment_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn repetition_index(&self) -> u32 {
        self.repetition_index
    }
}

/// Sealed proof returned only by a trusted user-action ingress. P025 does not
/// provide a production constructor or ingress implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrustedRunAuthorization {
    authorization: ExplicitRunAuthorization,
    scope: RunAuthorizationRequest,
}

impl TrustedRunAuthorization {
    #[cfg(test)]
    fn new(authorization: ExplicitRunAuthorization, scope: RunAuthorizationRequest) -> Self {
        Self {
            authorization,
            scope,
        }
    }
}

/// Trusted authorization ingress. Implementing code must prove a current
/// explicit user action; constructing P024's public authorization value alone
/// does not implement this boundary.
pub trait BenchmarkAuthorizationIngress: Send {
    fn consume(
        &mut self,
        request: &RunAuthorizationRequest,
    ) -> Result<TrustedRunAuthorization, RuntimeError>;
}

/// Authoritative Observation lookup by independently established runtime
/// correlation. A caller-supplied Observation ID is never sufficient.
pub trait BenchmarkObservationResolver {
    fn resolve(
        &self,
        correlation: &ExecutionCorrelationEvidence,
    ) -> Result<Option<ResolvedAuthoritativeObservation>, RuntimeError>;
}

/// An Observation loaded from the authoritative storage interface. Its
/// constructor is sealed so resolver implementations cannot fabricate one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAuthoritativeObservation {
    stored: StoredObservation,
}

impl ResolvedAuthoritativeObservation {
    pub fn observation(&self) -> &NormalizedObservation {
        &self.stored.observation
    }

    pub fn storage_revision(&self) -> u64 {
        self.stored.storage_revision
    }

    #[cfg(test)]
    pub(crate) fn for_test(stored: StoredObservation) -> Self {
        Self { stored }
    }
}

/// Existing Rust Observation store resolver. It reads the persisted record and
/// never constructs or mutates Observation evidence.
pub struct SqliteBenchmarkObservationResolver<'a> {
    store: &'a SqliteStore,
}

impl<'a> SqliteBenchmarkObservationResolver<'a> {
    pub fn new(store: &'a SqliteStore) -> Self {
        Self { store }
    }
}

impl BenchmarkObservationResolver for SqliteBenchmarkObservationResolver<'_> {
    fn resolve(
        &self,
        correlation: &ExecutionCorrelationEvidence,
    ) -> Result<Option<ResolvedAuthoritativeObservation>, RuntimeError> {
        self.store
            .load_observation(correlation.observation_id().as_str())
            .map(|stored| stored.map(|stored| ResolvedAuthoritativeObservation { stored }))
            .map_err(|_| RuntimeError::RecoveryEvidenceIncomplete)
    }
}

/// Runtime statuses are distinct from P024 benchmark lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeExecutionStatus {
    NotStarted,
    DispatchReserved,
    Running,
    ExecutionEnded,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeCorrelationStatus {
    NotAvailable,
    AwaitingObservation,
    Verified,
    Unresolved,
}

/// Privacy-safe receipt for one reserved invocation attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeExecutionReceipt {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
    authorization_id: AuthorizationId,
    dispatch_reservation_id: String,
    status: RuntimeExecutionStatus,
    started_at: Option<String>,
    ended_at: Option<String>,
    correlation_status: RuntimeCorrelationStatus,
    correlation: Option<ExecutionCorrelationEvidence>,
    synthetic: bool,
}

impl RuntimeExecutionReceipt {
    pub fn experiment_id(&self) -> &ExperimentId {
        &self.experiment_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn repetition_index(&self) -> u32 {
        self.repetition_index
    }

    pub fn authorization_id(&self) -> &AuthorizationId {
        &self.authorization_id
    }

    pub fn dispatch_reservation_id(&self) -> &str {
        &self.dispatch_reservation_id
    }

    pub fn status(&self) -> RuntimeExecutionStatus {
        self.status
    }

    pub fn started_at(&self) -> Option<&str> {
        self.started_at.as_deref()
    }

    pub fn ended_at(&self) -> Option<&str> {
        self.ended_at.as_deref()
    }

    pub fn correlation_status(&self) -> RuntimeCorrelationStatus {
        self.correlation_status
    }

    /// Opaque evidence can be returned to this crate's Observation resolver;
    /// its private fields prevent callers from manufacturing a binding.
    pub fn correlation_evidence(&self) -> Option<&ExecutionCorrelationEvidence> {
        self.correlation.as_ref()
    }

    /// True only for explicitly synthetic test execution.
    pub fn is_synthetic(&self) -> bool {
        self.synthetic
    }
}

/// Future-service coordinator. Normal production construction is disabled;
/// P025 deliberately supplies no live authorization ingress or provider
/// adapter.
pub struct BenchmarkRuntimeCoordinator {
    mode: ExecutionMode,
    dispatch_guard: Arc<dyn BenchmarkDispatchGuard>,
}

impl BenchmarkRuntimeCoordinator {
    pub fn new_disabled(dispatch_guard: Arc<dyn BenchmarkDispatchGuard>) -> Self {
        Self {
            mode: ExecutionMode::Disabled,
            dispatch_guard,
        }
    }

    #[cfg(test)]
    fn new_simulation(dispatch_guard: Arc<dyn BenchmarkDispatchGuard>) -> Self {
        Self {
            mode: ExecutionMode::Simulation,
            dispatch_guard,
        }
    }

    #[cfg(test)]
    fn new_live_for_test(dispatch_guard: Arc<dyn BenchmarkDispatchGuard>) -> Self {
        Self {
            mode: ExecutionMode::Live,
            dispatch_guard,
        }
    }

    pub fn execution_mode(&self) -> ExecutionMode {
        self.mode
    }

    fn is_synthetic_execution(&self) -> bool {
        cfg!(test) || self.mode == ExecutionMode::Simulation
    }

    /// Read a durable reservation after restart. This reports uncertainty and
    /// never resumes or invokes an execution adapter.
    pub fn recover_dispatch_state(
        &self,
        scope: &DispatchScope,
    ) -> Result<DispatchJournalState, RuntimeError> {
        self.dispatch_guard.state(scope).map_err(RuntimeError::from)
    }

    /// Collect and evaluate all P024 preflight gates. This never authorizes or
    /// starts an execution.
    pub fn evaluate_preflight(
        &self,
        experiment: &mut BenchmarkExperiment,
        run_id: &RunId,
        preflight_started_at: &str,
        source: &dyn BenchmarkRuntimeEvidenceSource,
    ) -> Result<BenchmarkPreflight, RuntimeError> {
        experiment.begin_preflight(run_id, preflight_started_at)?;
        let evidence = source.collect(experiment.plan(), run_id)?;
        let preflight = evidence.into_preflight(experiment.plan())?;
        experiment.evaluate_preflight(run_id, preflight.clone())?;
        Ok(preflight)
    }

    /// Consume one trusted action and attempt at most one dispatch. The default
    /// production coordinator returns `ExecutionDisabled` before it can
    /// consume authorization or invoke any adapter.
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        experiment: &mut BenchmarkExperiment,
        run_id: &RunId,
        ingress: &mut dyn BenchmarkAuthorizationIngress,
        workload_resolver: &dyn BenchmarkWorkloadResolver,
        adapter: Option<Arc<dyn BenchmarkExecutionAdapter>>,
        policy: RuntimeExecutionPolicy,
        cancellation: CancellationToken,
        clock: &dyn BenchmarkRuntimeClock,
    ) -> Result<RuntimeExecutionReceipt, RuntimeError> {
        if self.mode == ExecutionMode::Disabled {
            return Err(RuntimeError::ExecutionDisabled);
        }
        self.execute_enabled(
            experiment,
            run_id,
            ingress,
            workload_resolver,
            adapter,
            policy,
            cancellation,
            clock,
        )
    }

    /// Reconcile one independently correlated existing Observation. This is
    /// separate from execution and never waits in an unbounded loop.
    pub fn reconcile_observation(
        &self,
        experiment: &mut BenchmarkExperiment,
        run_id: &RunId,
        correlation: &ExecutionCorrelationEvidence,
        resolver: &dyn BenchmarkObservationResolver,
        linked_at: &str,
    ) -> Result<BenchmarkRunState, RuntimeError> {
        let run = experiment.run(run_id).ok_or(BenchmarkError::RunNotFound)?;
        if correlation.experiment_id() != run.experiment_id()
            || correlation.run_id() != run.run_id()
            || correlation.repetition_index() != run.repetition_index()
        {
            return Err(RuntimeError::ObservationCorrelationUnverified);
        }
        let Some(stored) = resolver.resolve(correlation)? else {
            return Err(RuntimeError::ObservationNotYetAvailable);
        };
        verify_correlation(correlation, stored.observation())?;
        let binding = TrustedRuntimeBinding::from_trusted_runtime(
            run.experiment_id().clone(),
            run.run_id().clone(),
            run.repetition_index(),
            correlation.observation_id().as_str(),
            correlation.isolation(),
        )?;
        let link =
            experiment.link_observation(run_id, &binding, stored.observation(), linked_at)?;
        let observation_id = ObservationId::parse(stored.observation().observation_id.clone())?;
        let mut claims = shared_claims()
            .lock()
            .map_err(|_| RuntimeError::RecoveryEvidenceIncomplete)?;
        if claims.has_conflicting_owner(
            &observation_id,
            run.experiment_id(),
            run.run_id(),
            run.repetition_index(),
        ) {
            return Err(RuntimeError::ObservationClaimConflict);
        }
        let existing_owner = claims.contains_owner(
            &observation_id,
            run.experiment_id(),
            run.run_id(),
            run.repetition_index(),
        );
        if claims.len() >= MAX_SHARED_OBSERVATION_CLAIMS && !existing_owner {
            return Err(RuntimeError::RecoveryEvidenceIncomplete);
        }
        self.dispatch_guard.claim_observation(
            &observation_id,
            run.experiment_id(),
            run.run_id(),
            run.repetition_index(),
            linked_at,
        )?;
        experiment.complete_run(run_id, link, &mut claims)?;
        Ok(experiment
            .run(run_id)
            .ok_or(BenchmarkError::RunNotFound)?
            .state())
    }

    #[allow(clippy::too_many_arguments)]
    fn execute_enabled(
        &self,
        experiment: &mut BenchmarkExperiment,
        run_id: &RunId,
        ingress: &mut dyn BenchmarkAuthorizationIngress,
        workload_resolver: &dyn BenchmarkWorkloadResolver,
        adapter: Option<Arc<dyn BenchmarkExecutionAdapter>>,
        policy: RuntimeExecutionPolicy,
        cancellation: CancellationToken,
        clock: &dyn BenchmarkRuntimeClock,
    ) -> Result<RuntimeExecutionReceipt, RuntimeError> {
        if cancellation.is_cancelled() {
            return Err(RuntimeError::ExecutionCancelled);
        }
        let run = experiment.run(run_id).ok_or(BenchmarkError::RunNotFound)?;
        if run.preflight_status() != Some(PreflightOutcome::Ready) {
            return Err(RuntimeError::PreflightNotReady);
        }
        if self.mode == ExecutionMode::Live
            && run
                .preflight()
                .is_some_and(BenchmarkPreflight::contains_test_fixture_evidence)
        {
            return Err(RuntimeError::TestFixtureEvidenceRejected);
        }
        let request = RunAuthorizationRequest::new(
            run.experiment_id().clone(),
            run.run_id().clone(),
            run.repetition_index(),
        );
        let trusted = ingress.consume(&request)?;
        if trusted.scope != request || trusted.authorization.authorization_id().as_str().is_empty()
        {
            return Err(RuntimeError::AuthorizationScopeMismatch);
        }
        experiment.authorize(run_id, trusted.authorization.clone())?;
        let scope = DispatchScope::new(
            request.experiment_id,
            request.run_id,
            request.repetition_index,
            trusted.authorization.authorization_id().clone(),
        );
        if cancellation.is_cancelled() {
            return Err(RuntimeError::ExecutionCancelled);
        }
        let adapter = adapter.ok_or(RuntimeError::AdapterUnavailable)?;
        let workload = workload_resolver.resolve(experiment.plan().workload_key())?;
        adapter.validate(&workload, policy)?;
        if cancellation.is_cancelled() {
            return Err(RuntimeError::ExecutionCancelled);
        }
        let reserved_at = clock.now_utc()?;
        validate_event_time(&reserved_at)?;
        let authorized_at =
            super::model::parse_utc_timestamp(trusted.authorization.authorized_at())
                .map_err(|_| RuntimeError::InvalidTimestamp)?;
        let reserved = super::model::parse_utc_timestamp(&reserved_at)
            .map_err(|_| RuntimeError::InvalidTimestamp)?;
        if reserved < authorized_at {
            return Err(RuntimeError::InvalidTimestamp);
        }
        let reservation = self.dispatch_guard.reserve(&scope, &reserved_at)?;
        let slot = self.dispatch_guard.acquire_execution_slot(&reservation)?;
        if cancellation.is_cancelled() {
            let result_observed_at = clock.now_utc()?;
            self.dispatch_guard.finish(
                &reservation,
                DispatchTerminalOutcome::CancelledBeforeDispatch,
                &result_observed_at,
            )?;
            drop(slot);
            return Ok(receipt(
                &scope,
                reservation.reservation_id(),
                RuntimeExecutionStatus::Cancelled,
                None,
                None,
                RuntimeCorrelationStatus::NotAvailable,
                None,
                self.is_synthetic_execution(),
            ));
        }
        let execution_started_at = clock.now_utc()?;
        validate_event_time(&execution_started_at)?;
        let requested_start = super::model::parse_utc_timestamp(&execution_started_at)
            .map_err(|_| RuntimeError::InvalidTimestamp)?;
        if requested_start < reserved {
            return Err(RuntimeError::InvalidTimestamp);
        }
        experiment.start_run(run_id, &execution_started_at)?;
        let start = Instant::now();
        let deadline = start
            .checked_add(policy.maximum_elapsed())
            .ok_or(RuntimeError::InvalidExecutionPolicy)?;
        let control =
            ExecutionControl::new(deadline, cancellation.clone(), policy.cancellation_policy());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker_adapter = Arc::clone(&adapter);
        let worker_workload = workload.clone();
        let worker_control = control.clone();
        let worker = std::thread::Builder::new()
            .name("codex-meter-benchmark-runner".to_owned())
            .spawn(move || {
                let result = if worker_control.begin_execution() {
                    WorkerExecutionResult::Returned(Box::new(
                        worker_adapter.execute(&worker_workload, &worker_control),
                    ))
                } else {
                    WorkerExecutionResult::CancelledBeforeStart
                };
                let _ = sender.send((result, slot));
            });
        if worker.is_err() {
            let result_observed_at = clock.now_utc()?;
            experiment.abort_run(run_id, &result_observed_at)?;
            self.dispatch_guard.finish(
                &reservation,
                DispatchTerminalOutcome::NotStarted,
                &result_observed_at,
            )?;
            return Ok(receipt(
                &scope,
                reservation.reservation_id(),
                RuntimeExecutionStatus::NotStarted,
                None,
                None,
                RuntimeCorrelationStatus::NotAvailable,
                None,
                self.is_synthetic_execution(),
            ));
        }
        match receiver.recv_timeout(policy.maximum_elapsed()) {
            Ok((WorkerExecutionResult::CancelledBeforeStart, slot)) => {
                let result_observed_at = clock.now_utc()?;
                experiment.abort_run(run_id, &result_observed_at)?;
                self.dispatch_guard.finish(
                    &reservation,
                    DispatchTerminalOutcome::CancelledBeforeDispatch,
                    &result_observed_at,
                )?;
                drop(slot);
                Ok(receipt(
                    &scope,
                    reservation.reservation_id(),
                    RuntimeExecutionStatus::Cancelled,
                    None,
                    None,
                    RuntimeCorrelationStatus::NotAvailable,
                    None,
                    self.is_synthetic_execution(),
                ))
            }
            Ok((WorkerExecutionResult::Returned(result), slot)) => {
                let result_observed_at = clock.now_utc()?;
                validate_event_time(&result_observed_at)?;
                let result_time = super::model::parse_utc_timestamp(&result_observed_at)
                    .map_err(|_| RuntimeError::InvalidTimestamp)?;
                if result_time < requested_start {
                    return Err(RuntimeError::InvalidTimestamp);
                }
                self.dispatch_guard
                    .acknowledge(&reservation, &result_observed_at)?;
                let elapsed = start.elapsed();
                let outcome = match *result {
                    Err(_) => {
                        self.abort_unknown(experiment, run_id, &reservation, &result_observed_at)?;
                        receipt(
                            &scope,
                            reservation.reservation_id(),
                            RuntimeExecutionStatus::OutcomeUnknown,
                            Some(execution_started_at),
                            None,
                            RuntimeCorrelationStatus::Unresolved,
                            None,
                            self.is_synthetic_execution(),
                        )
                    }
                    Ok(result) => self.finish_adapter_result(
                        experiment,
                        run_id,
                        &scope,
                        &reservation,
                        result,
                        elapsed,
                        policy,
                        &execution_started_at,
                        &result_observed_at,
                    )?,
                };
                drop(slot);
                Ok(outcome)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                match policy.cancellation_policy() {
                    super::adapter::CancellationPolicy::RequestWhenSupported => {
                        cancellation.cancel();
                    }
                    super::adapter::CancellationPolicy::StopWaitingOnly => {
                        cancellation.prevent_new_execution_if_not_started();
                    }
                }
                let result_observed_at = clock.now_utc()?;
                experiment.abort_run(run_id, &result_observed_at)?;
                self.dispatch_guard.finish(
                    &reservation,
                    DispatchTerminalOutcome::TimedOut,
                    &result_observed_at,
                )?;
                Ok(receipt(
                    &scope,
                    reservation.reservation_id(),
                    RuntimeExecutionStatus::OutcomeUnknown,
                    Some(execution_started_at),
                    None,
                    RuntimeCorrelationStatus::Unresolved,
                    None,
                    self.is_synthetic_execution(),
                ))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let result_observed_at = clock.now_utc()?;
                self.abort_unknown(experiment, run_id, &reservation, &result_observed_at)?;
                Ok(receipt(
                    &scope,
                    reservation.reservation_id(),
                    RuntimeExecutionStatus::OutcomeUnknown,
                    Some(execution_started_at),
                    None,
                    RuntimeCorrelationStatus::Unresolved,
                    None,
                    self.is_synthetic_execution(),
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_adapter_result(
        &self,
        experiment: &mut BenchmarkExperiment,
        run_id: &RunId,
        scope: &DispatchScope,
        reservation: &super::dispatch::DispatchReservation,
        result: AdapterExecutionResult,
        elapsed: std::time::Duration,
        policy: RuntimeExecutionPolicy,
        execution_started_at: &str,
        result_observed_at: &str,
    ) -> Result<RuntimeExecutionReceipt, RuntimeError> {
        let mut status = result.status();
        if elapsed > policy.maximum_elapsed() {
            status = AdapterExecutionStatus::TimedOut;
        }
        if matches!(
            status,
            AdapterExecutionStatus::Cancelled { confirmed: false }
        ) {
            status = AdapterExecutionStatus::OutcomeUnknown;
        }
        let started_at = Some(execution_started_at.to_owned());
        let mut ended_at = result.ended_at().map(str::to_owned);
        if let Some(adapter_started_at) = result.started_at() {
            let adapter_started = super::model::parse_utc_timestamp(adapter_started_at);
            let lower_bound = super::model::parse_utc_timestamp(execution_started_at);
            let upper_bound = super::model::parse_utc_timestamp(result_observed_at);
            if adapter_started.is_err()
                || lower_bound.is_err()
                || upper_bound.is_err()
                || adapter_started < lower_bound
                || adapter_started > upper_bound
            {
                status = AdapterExecutionStatus::OutcomeUnknown;
            }
        }
        if let Some(adapter_ended_at) = ended_at.as_deref() {
            let adapter_ended = super::model::parse_utc_timestamp(adapter_ended_at);
            let lower_bound = super::model::parse_utc_timestamp(execution_started_at);
            let upper_bound = super::model::parse_utc_timestamp(result_observed_at);
            if adapter_ended.is_err()
                || lower_bound.is_err()
                || upper_bound.is_err()
                || adapter_ended < lower_bound
                || adapter_ended > upper_bound
            {
                status = AdapterExecutionStatus::OutcomeUnknown;
                ended_at = None;
            }
        }
        if matches!(
            status,
            AdapterExecutionStatus::ExecutionEnded | AdapterExecutionStatus::Failed
        ) && ended_at.is_none()
        {
            status = AdapterExecutionStatus::OutcomeUnknown;
        }
        let confirmed_end = matches!(
            status,
            AdapterExecutionStatus::ExecutionEnded
                | AdapterExecutionStatus::Failed
                | AdapterExecutionStatus::Cancelled { confirmed: true }
        ) && ended_at.is_some();
        if confirmed_end {
            let ended = ended_at
                .as_deref()
                .ok_or(RuntimeError::RecoveryEvidenceIncomplete)?;
            experiment.mark_execution_ended(run_id, ended)?;
        } else {
            experiment.abort_run(run_id, result_observed_at)?;
        }
        let terminal = match status {
            AdapterExecutionStatus::ExecutionEnded => DispatchTerminalOutcome::ExecutionEnded,
            AdapterExecutionStatus::Failed => DispatchTerminalOutcome::Failed,
            AdapterExecutionStatus::Cancelled { .. } => DispatchTerminalOutcome::Cancelled,
            AdapterExecutionStatus::TimedOut => DispatchTerminalOutcome::TimedOut,
            AdapterExecutionStatus::OutcomeUnknown => DispatchTerminalOutcome::OutcomeUnknown,
        };
        self.dispatch_guard
            .finish(reservation, terminal, result_observed_at)?;
        let runtime_status = match status {
            AdapterExecutionStatus::ExecutionEnded => RuntimeExecutionStatus::ExecutionEnded,
            AdapterExecutionStatus::Failed => RuntimeExecutionStatus::Failed,
            AdapterExecutionStatus::Cancelled { .. } => RuntimeExecutionStatus::Cancelled,
            AdapterExecutionStatus::TimedOut | AdapterExecutionStatus::OutcomeUnknown => {
                RuntimeExecutionStatus::OutcomeUnknown
            }
        };
        let correlation = result
            .correlation()
            .filter(|evidence| {
                confirmed_end
                    && evidence.experiment_id() == scope.experiment_id()
                    && evidence.run_id() == scope.run_id()
                    && evidence.repetition_index() == scope.repetition_index()
            })
            .cloned();
        let correlation_status = if correlation.is_some() {
            RuntimeCorrelationStatus::AwaitingObservation
        } else {
            RuntimeCorrelationStatus::Unresolved
        };
        Ok(receipt(
            scope,
            reservation.reservation_id(),
            runtime_status,
            started_at,
            ended_at,
            correlation_status,
            correlation,
            self.is_synthetic_execution(),
        ))
    }

    fn abort_unknown(
        &self,
        experiment: &mut BenchmarkExperiment,
        run_id: &RunId,
        reservation: &super::dispatch::DispatchReservation,
        result_observed_at: &str,
    ) -> Result<(), RuntimeError> {
        experiment.abort_run(run_id, result_observed_at)?;
        self.dispatch_guard.finish(
            reservation,
            DispatchTerminalOutcome::OutcomeUnknown,
            result_observed_at,
        )?;
        Ok(())
    }
}

fn shared_claims() -> &'static Mutex<BenchmarkObservationClaims> {
    static CLAIMS: OnceLock<Mutex<BenchmarkObservationClaims>> = OnceLock::new();
    CLAIMS.get_or_init(|| Mutex::new(BenchmarkObservationClaims::default()))
}

#[allow(clippy::too_many_arguments)]
fn receipt(
    scope: &DispatchScope,
    reservation_id: &str,
    status: RuntimeExecutionStatus,
    started_at: Option<String>,
    ended_at: Option<String>,
    correlation_status: RuntimeCorrelationStatus,
    correlation: Option<ExecutionCorrelationEvidence>,
    synthetic: bool,
) -> RuntimeExecutionReceipt {
    RuntimeExecutionReceipt {
        experiment_id: scope.experiment_id().clone(),
        run_id: scope.run_id().clone(),
        repetition_index: scope.repetition_index(),
        authorization_id: scope.authorization_id().clone(),
        dispatch_reservation_id: reservation_id.to_owned(),
        status,
        started_at,
        ended_at,
        correlation_status,
        correlation,
        synthetic,
    }
}

fn verify_correlation(
    correlation: &ExecutionCorrelationEvidence,
    observation: &NormalizedObservation,
) -> Result<(), RuntimeError> {
    if observation.observation_id != correlation.observation_id().as_str()
        || observation.task_id.as_deref() != Some(correlation.task_id())
        || observation.session_id.as_deref() != Some(correlation.session_id())
        || observation.source_instance_id.as_deref() != Some(correlation.source_instance_id())
        || !observation.identity_matches()
    {
        return Err(RuntimeError::ObservationCorrelationUnverified);
    }
    Ok(())
}

fn configuration_readiness(
    planned: &ConfigurationIdentity,
    observed: Option<&ConfigurationIdentity>,
) -> GateEvidence {
    let Some(observed) = observed else {
        return GateEvidence::unknown(UnknownEvidenceReason::ConfigurationNotObserved);
    };
    if validate_configuration(observed).is_err() {
        return GateEvidence::blocked(GateBlockReason::ConfigurationNotReady);
    }
    let mut unknown = false;
    for (planned, observed) in [
        (&planned.plan, &observed.plan),
        (&planned.model, &observed.model),
        (&planned.reasoning_level, &observed.reasoning_level),
        (&planned.speed_mode, &observed.speed_mode),
        (&planned.codex_version, &observed.codex_version),
    ] {
        match (
            planned.availability,
            planned.value.as_deref(),
            planned.provenance,
            observed.availability,
            observed.value.as_deref(),
            observed.provenance,
        ) {
            (
                MetricAvailability::Available,
                Some(planned_value),
                MetricProvenance::Observed | MetricProvenance::Derived,
                MetricAvailability::Available,
                Some(observed_value),
                MetricProvenance::Observed | MetricProvenance::Derived,
            ) if planned_value == observed_value => {}
            (
                MetricAvailability::Available,
                Some(_),
                MetricProvenance::Observed | MetricProvenance::Derived,
                MetricAvailability::Available,
                Some(_),
                MetricProvenance::Observed | MetricProvenance::Derived,
            ) => return GateEvidence::blocked(GateBlockReason::ConfigurationNotReady),
            (MetricAvailability::Unavailable, None, MetricProvenance::Unavailable, _, _, _)
            | (_, _, _, MetricAvailability::Unavailable, None, MetricProvenance::Unavailable) => {
                unknown = true
            }
            _ => return GateEvidence::blocked(GateBlockReason::ConfigurationNotReady),
        }
    }
    if unknown {
        GateEvidence::unknown(UnknownEvidenceReason::ConfigurationNotObserved)
    } else {
        GateEvidence::verified(EvidenceBasis::ObservedRuntime)
    }
}

fn validate_event_time(timestamp: &str) -> Result<(), RuntimeError> {
    super::model::parse_utc_timestamp(timestamp)
        .map(|_| ())
        .map_err(|_| RuntimeError::InvalidTimestamp)
}

/// Typed privacy-safe orchestration failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    ExecutionDisabled,
    AdapterUnavailable,
    PreflightNotReady,
    AuthorizationMissing,
    AuthorizationScopeMismatch,
    AuthorizationAlreadyConsumed,
    DispatchReservationConflict,
    DispatchPersistenceFailure,
    ExecutionTimedOut,
    ExecutionCancelled,
    ExecutionOutcomeUnknown,
    ObservationNotYetAvailable,
    ObservationCorrelationUnverified,
    ObservationClaimConflict,
    RecoveryEvidenceIncomplete,
    TestFixtureEvidenceRejected,
    InvalidExecutionPolicy,
    InvalidTimestamp,
    Benchmark(BenchmarkError),
    Adapter(RuntimeAdapterError),
    Dispatch(DispatchJournalError),
}

impl From<BenchmarkError> for RuntimeError {
    fn from(value: BenchmarkError) -> Self {
        Self::Benchmark(value)
    }
}

impl From<RuntimeAdapterError> for RuntimeError {
    fn from(value: RuntimeAdapterError) -> Self {
        match value {
            RuntimeAdapterError::AdapterUnavailable => Self::AdapterUnavailable,
            RuntimeAdapterError::UnknownWorkloadKey => {
                Self::Adapter(RuntimeAdapterError::UnknownWorkloadKey)
            }
            RuntimeAdapterError::ExecutionTimedOut => Self::ExecutionTimedOut,
            RuntimeAdapterError::ExecutionCancelled => Self::ExecutionCancelled,
            RuntimeAdapterError::ExecutionOutcomeUnknown => Self::ExecutionOutcomeUnknown,
            RuntimeAdapterError::CorrelationUnverified => Self::ObservationCorrelationUnverified,
            other => Self::Adapter(other),
        }
    }
}

impl From<DispatchJournalError> for RuntimeError {
    fn from(value: DispatchJournalError) -> Self {
        match value {
            DispatchJournalError::ReservationConflict => Self::DispatchReservationConflict,
            DispatchJournalError::ExecutionSlotBusy => Self::DispatchReservationConflict,
            DispatchJournalError::ObservationClaimConflict => Self::ObservationClaimConflict,
            DispatchJournalError::Io
            | DispatchJournalError::InvalidDirectory
            | DispatchJournalError::JournalCapacityExceeded
            | DispatchJournalError::RecordTooLarge => Self::DispatchPersistenceFailure,
            other => Self::Dispatch(other),
        }
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::ExecutionDisabled => "benchmark execution is disabled",
            Self::AdapterUnavailable => "benchmark execution adapter is unavailable",
            Self::PreflightNotReady => "benchmark preflight is not ready",
            Self::AuthorizationMissing => "trusted per-run authorization is required",
            Self::AuthorizationScopeMismatch => "authorization does not match this run",
            Self::AuthorizationAlreadyConsumed => "authorization was already consumed",
            Self::DispatchReservationConflict => "execution attempt already has a reservation",
            Self::DispatchPersistenceFailure => "durable dispatch reservation failed",
            Self::ExecutionTimedOut => "benchmark execution exceeded its time bound",
            Self::ExecutionCancelled => "benchmark execution was cancelled",
            Self::ExecutionOutcomeUnknown => "benchmark execution outcome is unknown",
            Self::ObservationNotYetAvailable => "authoritative Observation is not yet available",
            Self::ObservationCorrelationUnverified => {
                "runtime Observation correlation is unverified"
            }
            Self::ObservationClaimConflict => {
                "Observation is already claimed by another benchmark run"
            }
            Self::RecoveryEvidenceIncomplete => "runtime recovery evidence is incomplete",
            Self::TestFixtureEvidenceRejected => {
                "synthetic preflight evidence cannot authorize live execution"
            }
            Self::InvalidExecutionPolicy => {
                "benchmark execution policy is outside supported bounds"
            }
            Self::InvalidTimestamp => "benchmark runtime timestamp is invalid",
            Self::Benchmark(error) => return error.fmt(formatter),
            Self::Adapter(error) => return error.fmt(formatter),
            Self::Dispatch(error) => return error.fmt(formatter),
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RuntimeError {}

#[cfg(test)]
mod tests;
