//! Narrow provider-independent adapter contracts for benchmark execution.

use std::time::{Duration, Instant};

use super::evidence::ObservationId;
use super::model::{ExperimentId, RunId, WorkloadKey};
use super::validation::IsolationClassification;
use super::BenchmarkError;

const MAX_RUNTIME_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// Policy bounds for one execution and its later evidence lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeExecutionPolicy {
    maximum_elapsed: Duration,
    maximum_evidence_wait: Duration,
    cancellation_policy: CancellationPolicy,
}

impl RuntimeExecutionPolicy {
    pub fn new(
        maximum_elapsed: Duration,
        maximum_evidence_wait: Duration,
        cancellation_policy: CancellationPolicy,
    ) -> Result<Self, RuntimeAdapterError> {
        if maximum_elapsed.is_zero()
            || maximum_elapsed > MAX_RUNTIME_DURATION
            || maximum_evidence_wait.is_zero()
            || maximum_evidence_wait > MAX_RUNTIME_DURATION
        {
            return Err(RuntimeAdapterError::InvalidExecutionPolicy);
        }
        Ok(Self {
            maximum_elapsed,
            maximum_evidence_wait,
            cancellation_policy,
        })
    }

    pub fn maximum_elapsed(self) -> Duration {
        self.maximum_elapsed
    }

    pub fn maximum_evidence_wait(self) -> Duration {
        self.maximum_evidence_wait
    }

    pub fn cancellation_policy(self) -> CancellationPolicy {
        self.cancellation_policy
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancellationPolicy {
    RequestWhenSupported,
    StopWaitingOnly,
}

/// Cancellation signal passed to one adapter invocation.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(std::sync::Arc<std::sync::Mutex<CancellationState>>);

#[derive(Debug, Default)]
struct CancellationState {
    cancelled: bool,
    dispatch_closed: bool,
    execution_started: bool,
}

impl CancellationToken {
    pub fn cancel(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.cancelled = true;
            state.dispatch_closed = true;
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.cancelled)
    }

    /// Atomically closes the cancellation-before-dispatch window. If this
    /// returns true, the adapter invocation has begun for cancellation-policy
    /// purposes; a later cancellation is a request to the active adapter.
    pub(crate) fn begin_execution(&self) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        if state.cancelled || state.dispatch_closed {
            return false;
        }
        state.execution_started = true;
        true
    }

    pub(crate) fn prevent_new_execution_if_not_started(&self) {
        if let Ok(mut state) = self.0.lock() {
            if !state.execution_started {
                state.dispatch_closed = true;
            }
        }
    }
}

/// Bounded invocation context. Adapters must return by `deadline` and must
/// honor cancellation where their underlying runtime supports it.
#[derive(Clone, Debug)]
pub struct ExecutionControl {
    deadline: Instant,
    cancellation: CancellationToken,
    cancellation_policy: CancellationPolicy,
}

impl ExecutionControl {
    pub(crate) fn new(
        deadline: Instant,
        cancellation: CancellationToken,
        cancellation_policy: CancellationPolicy,
    ) -> Self {
        Self {
            deadline,
            cancellation,
            cancellation_policy,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn begin_execution(&self) -> bool {
        self.cancellation.begin_execution()
    }

    pub fn cancellation_policy(&self) -> CancellationPolicy {
        self.cancellation_policy
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// A registry-verified workload declaration. Its payload is deliberately not
/// part of the benchmark API or telemetry model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedBenchmarkWorkload {
    key: WorkloadKey,
}

impl ResolvedBenchmarkWorkload {
    pub fn key(&self) -> &WorkloadKey {
        &self.key
    }

    #[cfg(test)]
    pub(crate) fn for_test(key: WorkloadKey) -> Self {
        Self { key }
    }
}

/// Narrow exact-key lookup. Unknown keys must be rejected by the registry.
pub trait BenchmarkWorkloadResolver: Send + Sync {
    fn resolve(&self, key: &WorkloadKey) -> Result<ResolvedBenchmarkWorkload, RuntimeAdapterError>;
}

/// One adapter result contains only bounded status and timestamps. It cannot
/// contain prompt, response, command, environment, credential, or source data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterExecutionResult {
    status: AdapterExecutionStatus,
    started_at: Option<String>,
    ended_at: Option<String>,
    correlation: Option<ExecutionCorrelationEvidence>,
}

impl AdapterExecutionResult {
    pub fn new(
        status: AdapterExecutionStatus,
        started_at: Option<String>,
        ended_at: Option<String>,
    ) -> Self {
        Self {
            status,
            started_at,
            ended_at,
            correlation: None,
        }
    }

    pub fn status(&self) -> AdapterExecutionStatus {
        self.status
    }

    pub fn started_at(&self) -> Option<&str> {
        self.started_at.as_deref()
    }

    pub fn ended_at(&self) -> Option<&str> {
        self.ended_at.as_deref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterExecutionStatus {
    ExecutionEnded,
    Failed,
    Cancelled { confirmed: bool },
    TimedOut,
    OutcomeUnknown,
}

/// Independently established task/session/source identity returned by a
/// trusted runtime adapter. Fields are private so callers cannot turn an
/// Observation ID alone into a trusted association.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionCorrelationEvidence {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
    observation_id: ObservationId,
    task_id: String,
    session_id: String,
    source_instance_id: String,
    isolation: IsolationClassification,
}

impl ExecutionCorrelationEvidence {
    pub fn observation_id(&self) -> &ObservationId {
        &self.observation_id
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn source_instance_id(&self) -> &str {
        &self.source_instance_id
    }

    pub(crate) fn experiment_id(&self) -> &ExperimentId {
        &self.experiment_id
    }

    pub(crate) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub(crate) fn repetition_index(&self) -> u32 {
        self.repetition_index
    }

    pub(crate) fn isolation(&self) -> IsolationClassification {
        self.isolation
    }

    #[allow(dead_code)] // Reserved for a future validated in-crate runtime adapter.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_verified_adapter(
        experiment_id: ExperimentId,
        run_id: RunId,
        repetition_index: u32,
        observation_id: impl Into<String>,
        task_id: impl Into<String>,
        session_id: impl Into<String>,
        source_instance_id: impl Into<String>,
        isolation: IsolationClassification,
    ) -> Result<Self, RuntimeAdapterError> {
        let observation_id = ObservationId::parse(observation_id)
            .map_err(|_| RuntimeAdapterError::CorrelationUnverified)?;
        let task_id = task_id.into();
        let session_id = session_id.into();
        let source_instance_id = source_instance_id.into();
        if task_id.is_empty()
            || session_id.is_empty()
            || source_instance_id.is_empty()
            || task_id.len() > 256
            || session_id.len() > 256
            || source_instance_id.len() > 256
        {
            return Err(RuntimeAdapterError::CorrelationUnverified);
        }
        Ok(Self {
            experiment_id,
            run_id,
            repetition_index,
            observation_id,
            task_id,
            session_id,
            source_instance_id,
            isolation,
        })
    }
}

impl AdapterExecutionResult {
    #[allow(dead_code)] // Used by trusted in-crate adapters; synthetic tests also exercise it.
    pub(crate) fn with_correlation(mut self, correlation: ExecutionCorrelationEvidence) -> Self {
        self.correlation = Some(correlation);
        self
    }

    pub(crate) fn correlation(&self) -> Option<&ExecutionCorrelationEvidence> {
        self.correlation.as_ref()
    }
}

/// A single-use execution boundary. `validate` must not start workload work,
/// and implementations must not retry. The coordinator never passes arbitrary
/// command strings or execution payloads.
pub trait BenchmarkExecutionAdapter: Send + Sync {
    fn validate(
        &self,
        workload: &ResolvedBenchmarkWorkload,
        policy: RuntimeExecutionPolicy,
    ) -> Result<(), RuntimeAdapterError>;

    fn execute(
        &self,
        workload: &ResolvedBenchmarkWorkload,
        control: &ExecutionControl,
    ) -> Result<AdapterExecutionResult, RuntimeAdapterError>;
}

/// Privacy-safe adapter contract failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeAdapterError {
    AdapterUnavailable,
    UnknownWorkloadKey,
    InvalidExecutionPolicy,
    ExecutionFailed,
    ExecutionTimedOut,
    ExecutionCancelled,
    ExecutionOutcomeUnknown,
    CorrelationUnverified,
}

impl std::fmt::Display for RuntimeAdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::AdapterUnavailable => "benchmark execution adapter is unavailable",
            Self::UnknownWorkloadKey => "benchmark workload key is not registered",
            Self::InvalidExecutionPolicy => {
                "benchmark execution policy is outside supported bounds"
            }
            Self::ExecutionFailed => "benchmark execution failed",
            Self::ExecutionTimedOut => "benchmark execution exceeded its time bound",
            Self::ExecutionCancelled => "benchmark execution was cancelled",
            Self::ExecutionOutcomeUnknown => "benchmark execution outcome is unknown",
            Self::CorrelationUnverified => "runtime Observation correlation is unverified",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for RuntimeAdapterError {}

impl From<BenchmarkError> for RuntimeAdapterError {
    fn from(_: BenchmarkError) -> Self {
        Self::CorrelationUnverified
    }
}
