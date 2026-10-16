//! Deterministic in-memory benchmark run state machine.

use std::collections::BTreeMap;

use crate::telemetry::NormalizedObservation;

use super::evidence::{
    verify_observation_link, BenchmarkDiagnostic, MeasurementCompletionStatus, ObservationId,
    TrustedRuntimeBinding, VerifiedObservationLink,
};
use super::model::{parse_utc_timestamp, AuthorizationId, BenchmarkPlan, ExperimentId, RunId};
use super::validation::{BenchmarkPreflight, IsolationClassification, PreflightOutcome};
use super::BenchmarkError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BenchmarkRunState {
    Planned,
    PreflightPending,
    Blocked,
    Unknown,
    Ready,
    Authorized,
    Running,
    AwaitingObservation,
    Completed,
    Incomplete,
    Aborted,
}

/// In-memory uniqueness authority shared by all active experiments. A runtime
/// must pass the same ledger to every completion operation in its process.
#[derive(Debug, Default)]
pub struct BenchmarkObservationClaims {
    owners: BTreeMap<ObservationId, (ExperimentId, RunId, u32)>,
}

impl BenchmarkObservationClaims {
    pub(crate) fn len(&self) -> usize {
        self.owners.len()
    }

    pub(crate) fn contains_owner(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
    ) -> bool {
        self.owners.get(observation_id)
            == Some(&(experiment_id.clone(), run_id.clone(), repetition_index))
    }

    pub(crate) fn has_conflicting_owner(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
    ) -> bool {
        self.owners.get(observation_id).is_some_and(|owner| {
            owner.0 != *experiment_id || owner.1 != *run_id || owner.2 != repetition_index
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplicitRunAuthorization {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
    authorization_id: AuthorizationId,
    authorized_at: String,
}

impl ExplicitRunAuthorization {
    /// Construct a one-run authorization event from an explicit caller action.
    /// This event never authorizes another run or a future invocation.
    pub fn new(
        experiment_id: ExperimentId,
        run_id: RunId,
        repetition_index: u32,
        authorization_id: AuthorizationId,
        authorized_at: impl Into<String>,
    ) -> Result<Self, BenchmarkError> {
        let authorized_at = authorized_at.into();
        if parse_utc_timestamp(&authorized_at).is_err() {
            return Err(BenchmarkError::InvalidTimestamp);
        }
        Ok(Self {
            experiment_id,
            run_id,
            repetition_index,
            authorization_id,
            authorized_at,
        })
    }

    pub fn authorization_id(&self) -> &AuthorizationId {
        &self.authorization_id
    }

    pub fn authorized_at(&self) -> &str {
        &self.authorized_at
    }
}

/// Benchmark state is intentionally in memory in P024. The Observation
/// pipeline remains the authority for telemetry, quota evidence, and quality.
#[derive(Debug)]
pub struct BenchmarkExperiment {
    plan: BenchmarkPlan,
    runs: BTreeMap<RunId, BenchmarkRun>,
}

impl BenchmarkExperiment {
    pub fn new(plan: BenchmarkPlan) -> Result<Self, BenchmarkError> {
        plan.validate().map_err(BenchmarkError::InvalidPlan)?;
        Ok(Self {
            plan,
            runs: BTreeMap::new(),
        })
    }

    pub fn plan(&self) -> &BenchmarkPlan {
        &self.plan
    }

    pub fn run(&self, run_id: &RunId) -> Option<&BenchmarkRun> {
        self.runs.get(run_id)
    }

    pub fn runs(&self) -> impl Iterator<Item = &BenchmarkRun> {
        self.runs.values()
    }

    pub fn create_run(
        &mut self,
        run_id: RunId,
        repetition_index: u32,
    ) -> Result<(), BenchmarkError> {
        if self.runs.contains_key(&run_id) {
            return Err(BenchmarkError::DuplicateRun);
        }
        if self
            .runs
            .values()
            .any(|run| run.repetition_index == repetition_index)
        {
            return Err(BenchmarkError::DuplicateRepetition);
        }
        if self.runs.len() >= usize::from(self.plan.planned_run_count()) {
            return Err(BenchmarkError::RunLimitExceeded);
        }
        self.runs.insert(
            run_id.clone(),
            BenchmarkRun {
                experiment_id: self.plan.experiment_id().clone(),
                run_id,
                repetition_index,
                state: BenchmarkRunState::Planned,
                preflight_started_at: None,
                preflight: None,
                preflight_status: None,
                authorization: None,
                started_at: None,
                ended_at: None,
                finalized_at: None,
                observation_id: None,
                observation_linked_at: None,
                completion_status: None,
                isolation_status: IsolationClassification::Unknown,
                aborted_at: None,
                diagnostics: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn begin_preflight(
        &mut self,
        run_id: &RunId,
        started_at: impl Into<String>,
    ) -> Result<(), BenchmarkError> {
        let started_at = started_at.into();
        let timestamp =
            parse_utc_timestamp(&started_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let created_at = parse_utc_timestamp(self.plan.created_at())
            .map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let run = self.run_mut(run_id)?;
        if let Some(previous) = &run.preflight_started_at {
            return if previous == &started_at {
                Ok(())
            } else {
                Err(BenchmarkError::ConflictingReplay)
            };
        }
        if run.state != BenchmarkRunState::Planned {
            return Err(BenchmarkError::InvalidTransition);
        }
        if timestamp < created_at {
            return Err(BenchmarkError::InvalidTimeOrder);
        }
        run.preflight_started_at = Some(started_at);
        run.state = BenchmarkRunState::PreflightPending;
        Ok(())
    }

    pub fn evaluate_preflight(
        &mut self,
        run_id: &RunId,
        preflight: BenchmarkPreflight,
    ) -> Result<(), BenchmarkError> {
        if !preflight.belongs_to(&self.plan)
            || preflight.quota_meter_availability().len() != self.plan.target_quota_meters().len()
            || preflight.quota_window_continuity().len() != self.plan.target_quota_meters().len()
            || self.plan.target_quota_meters().iter().any(|meter| {
                !preflight
                    .quota_meter_availability()
                    .iter()
                    .any(|evidence| evidence.meter() == *meter)
                    || !preflight
                        .quota_window_continuity()
                        .iter()
                        .any(|evidence| evidence.meter() == *meter)
            })
        {
            return Err(BenchmarkError::InvalidPreflight);
        }
        let evaluated_at = parse_utc_timestamp(preflight.evaluated_at())
            .map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let run = self.run_mut(run_id)?;
        if let Some(previous) = &run.preflight {
            return if previous == &preflight {
                Ok(())
            } else {
                Err(BenchmarkError::ConflictingReplay)
            };
        }
        if run.state != BenchmarkRunState::PreflightPending {
            return Err(BenchmarkError::InvalidTransition);
        }
        let Some(started_at) = run.preflight_started_at.as_deref() else {
            return Err(BenchmarkError::InvalidTransition);
        };
        if evaluated_at
            < parse_utc_timestamp(started_at).map_err(|_| BenchmarkError::InvalidTimestamp)?
        {
            return Err(BenchmarkError::InvalidTimeOrder);
        }
        run.preflight_status = Some(preflight.outcome());
        run.isolation_status = IsolationClassification::Unknown;
        run.state = match preflight.outcome() {
            PreflightOutcome::Ready => BenchmarkRunState::Ready,
            PreflightOutcome::Blocked => BenchmarkRunState::Blocked,
            PreflightOutcome::Unknown => BenchmarkRunState::Unknown,
        };
        run.preflight = Some(preflight);
        Ok(())
    }

    pub fn authorize(
        &mut self,
        run_id: &RunId,
        authorization: ExplicitRunAuthorization,
    ) -> Result<(), BenchmarkError> {
        {
            let run = self.run(run_id).ok_or(BenchmarkError::RunNotFound)?;
            if let Some(previous) = &run.authorization {
                return if previous == &authorization {
                    Ok(())
                } else {
                    Err(BenchmarkError::ConflictingReplay)
                };
            }
            if authorization.experiment_id != run.experiment_id
                || authorization.run_id != run.run_id
                || authorization.repetition_index != run.repetition_index
            {
                return Err(BenchmarkError::AuthorizationScopeMismatch);
            }
            if run.state != BenchmarkRunState::Ready {
                return Err(match run.state {
                    BenchmarkRunState::Blocked => BenchmarkError::PreflightBlocked,
                    BenchmarkRunState::Unknown | BenchmarkRunState::PreflightPending => {
                        BenchmarkError::PreflightUnknown
                    }
                    _ => BenchmarkError::InvalidTransition,
                });
            }
            let preflight = run
                .preflight
                .as_ref()
                .ok_or(BenchmarkError::InvalidTransition)?;
            if parse_utc_timestamp(&authorization.authorized_at)
                .map_err(|_| BenchmarkError::InvalidTimestamp)?
                < parse_utc_timestamp(preflight.evaluated_at())
                    .map_err(|_| BenchmarkError::InvalidTimestamp)?
            {
                return Err(BenchmarkError::InvalidTimeOrder);
            }
        }
        if self.runs.values().any(|other| {
            other.run_id != *run_id
                && other.authorization.as_ref().is_some_and(|existing| {
                    existing.authorization_id == authorization.authorization_id
                })
        }) {
            return Err(BenchmarkError::AuthorizationScopeMismatch);
        }
        let run = self.run_mut(run_id)?;
        run.authorization = Some(authorization);
        run.state = BenchmarkRunState::Authorized;
        Ok(())
    }

    pub fn start_run(
        &mut self,
        run_id: &RunId,
        started_at: impl Into<String>,
    ) -> Result<(), BenchmarkError> {
        let started_at = started_at.into();
        let started =
            parse_utc_timestamp(&started_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let run = self.run_mut(run_id)?;
        if let Some(previous) = &run.started_at {
            return if previous == &started_at {
                Ok(())
            } else {
                Err(BenchmarkError::ConflictingReplay)
            };
        }
        if run.state != BenchmarkRunState::Authorized {
            return Err(if run.authorization.is_none() {
                BenchmarkError::AuthorizationRequired
            } else {
                BenchmarkError::InvalidTransition
            });
        }
        let authorization = run
            .authorization
            .as_ref()
            .ok_or(BenchmarkError::AuthorizationRequired)?;
        if started
            < parse_utc_timestamp(&authorization.authorized_at)
                .map_err(|_| BenchmarkError::InvalidTimestamp)?
        {
            return Err(BenchmarkError::InvalidTimeOrder);
        }
        run.started_at = Some(started_at);
        run.state = BenchmarkRunState::Running;
        Ok(())
    }

    pub fn mark_execution_ended(
        &mut self,
        run_id: &RunId,
        ended_at: impl Into<String>,
    ) -> Result<(), BenchmarkError> {
        let ended_at = ended_at.into();
        let ended = parse_utc_timestamp(&ended_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let run = self.run_mut(run_id)?;
        if let Some(previous) = &run.ended_at {
            return if previous == &ended_at {
                Ok(())
            } else {
                Err(BenchmarkError::ConflictingReplay)
            };
        }
        if run.state != BenchmarkRunState::Running {
            return Err(BenchmarkError::InvalidTransition);
        }
        let started = run
            .started_at
            .as_deref()
            .ok_or(BenchmarkError::InvalidTransition)?;
        if ended < parse_utc_timestamp(started).map_err(|_| BenchmarkError::InvalidTimestamp)? {
            return Err(BenchmarkError::InvalidTimeOrder);
        }
        run.ended_at = Some(ended_at);
        run.state = BenchmarkRunState::AwaitingObservation;
        Ok(())
    }

    /// Validate a trusted runtime binding against an independently produced
    /// terminal Observation. This does not alter the Observation or run.
    pub fn link_observation(
        &self,
        run_id: &RunId,
        binding: &TrustedRuntimeBinding,
        observation: &NormalizedObservation,
        linked_at: impl Into<String>,
    ) -> Result<VerifiedObservationLink, BenchmarkError> {
        let run = self.run(run_id).ok_or(BenchmarkError::RunNotFound)?;
        let linked_at = linked_at.into();
        if !matches!(
            run.state,
            BenchmarkRunState::AwaitingObservation
                | BenchmarkRunState::Completed
                | BenchmarkRunState::Incomplete
        ) {
            return Err(BenchmarkError::InvalidTransition);
        }
        let started_at = run
            .started_at
            .as_deref()
            .ok_or(BenchmarkError::InvalidTransition)?;
        let ended_at = run
            .ended_at
            .as_deref()
            .ok_or(BenchmarkError::InvalidTransition)?;
        let link = verify_observation_link(
            &run.experiment_id,
            &run.run_id,
            run.repetition_index,
            started_at,
            ended_at,
            self.plan.configuration(),
            self.plan.target_quota_meters(),
            binding,
            observation,
            &linked_at,
        )?;
        if let Some(existing) = &run.observation_id {
            if existing != &link.observation_id
                || run.observation_linked_at.as_deref() != Some(link.linked_at.as_str())
            {
                return Err(BenchmarkError::ConflictingReplay);
            }
        }
        if self.runs.values().any(|other| {
            other.run_id != *run_id && other.observation_id.as_ref() == Some(&link.observation_id)
        }) {
            return Err(BenchmarkError::DuplicateObservation);
        }
        Ok(link)
    }

    pub fn complete_run(
        &mut self,
        run_id: &RunId,
        link: VerifiedObservationLink,
        claims: &mut BenchmarkObservationClaims,
    ) -> Result<(), BenchmarkError> {
        let run = self.run(run_id).ok_or(BenchmarkError::RunNotFound)?;
        if link.experiment_id != run.experiment_id
            || link.run_id != run.run_id
            || link.repetition_index != run.repetition_index
        {
            return Err(BenchmarkError::ObservationUnresolved);
        }
        let target_state = match link.completion {
            MeasurementCompletionStatus::MeasurementComplete => BenchmarkRunState::Completed,
            MeasurementCompletionStatus::MeasurementIncomplete => BenchmarkRunState::Incomplete,
        };
        if matches!(
            run.state,
            BenchmarkRunState::Completed | BenchmarkRunState::Incomplete
        ) {
            let same = run.state == target_state
                && run.observation_id.as_ref() == Some(&link.observation_id)
                && run.observation_linked_at.as_deref() == Some(link.linked_at.as_str())
                && run.finalized_at.as_deref() == Some(link.finalized_at.as_str())
                && run.completion_status == Some(link.completion)
                && run.isolation_status == link.isolation
                && run.diagnostics == link.diagnostics;
            if !same {
                return Err(BenchmarkError::ConflictingReplay);
            }
            claim_observation(claims, &link)?;
            return Ok(());
        }
        if run.state != BenchmarkRunState::AwaitingObservation {
            return Err(BenchmarkError::InvalidTransition);
        }
        if self.runs.values().any(|other| {
            other.run_id != *run_id && other.observation_id.as_ref() == Some(&link.observation_id)
        }) {
            return Err(BenchmarkError::DuplicateObservation);
        }
        claim_observation(claims, &link)?;
        let run = self.run_mut(run_id)?;
        run.observation_id = Some(link.observation_id);
        run.observation_linked_at = Some(link.linked_at);
        run.finalized_at = Some(link.finalized_at);
        run.completion_status = Some(link.completion);
        run.isolation_status = link.isolation;
        run.diagnostics = link.diagnostics;
        run.state = target_state;
        Ok(())
    }

    pub fn abort_run(
        &mut self,
        run_id: &RunId,
        aborted_at: impl Into<String>,
    ) -> Result<(), BenchmarkError> {
        let aborted_at = aborted_at.into();
        let aborted =
            parse_utc_timestamp(&aborted_at).map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let plan_created_at = parse_utc_timestamp(self.plan.created_at())
            .map_err(|_| BenchmarkError::InvalidTimestamp)?;
        let run = self.run_mut(run_id)?;
        if let Some(previous) = &run.aborted_at {
            return if previous == &aborted_at {
                Ok(())
            } else {
                Err(BenchmarkError::ConflictingReplay)
            };
        }
        if matches!(
            run.state,
            BenchmarkRunState::Completed
                | BenchmarkRunState::Incomplete
                | BenchmarkRunState::Aborted
        ) {
            return Err(BenchmarkError::InvalidTransition);
        }
        let previous = run
            .latest_event_at()
            .map(parse_utc_timestamp)
            .transpose()
            .map_err(|_| BenchmarkError::InvalidTimestamp)?
            .unwrap_or(plan_created_at);
        if aborted < previous {
            return Err(BenchmarkError::InvalidTimeOrder);
        }
        if run.state == BenchmarkRunState::Running {
            run.diagnostics
                .push(BenchmarkDiagnostic::InterruptedExecution);
            run.diagnostics.sort_unstable();
            run.diagnostics.dedup();
        }
        run.aborted_at = Some(aborted_at);
        run.state = BenchmarkRunState::Aborted;
        Ok(())
    }

    fn run_mut(&mut self, run_id: &RunId) -> Result<&mut BenchmarkRun, BenchmarkError> {
        self.runs.get_mut(run_id).ok_or(BenchmarkError::RunNotFound)
    }
}

fn claim_observation(
    claims: &mut BenchmarkObservationClaims,
    link: &VerifiedObservationLink,
) -> Result<(), BenchmarkError> {
    let owner = (
        link.experiment_id.clone(),
        link.run_id.clone(),
        link.repetition_index,
    );
    match claims.owners.get(&link.observation_id) {
        Some(existing) if existing != &owner => Err(BenchmarkError::DuplicateObservation),
        Some(_) => Ok(()),
        None => {
            claims.owners.insert(link.observation_id.clone(), owner);
            Ok(())
        }
    }
}

#[derive(Debug)]
pub struct BenchmarkRun {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
    state: BenchmarkRunState,
    preflight_started_at: Option<String>,
    preflight: Option<BenchmarkPreflight>,
    preflight_status: Option<PreflightOutcome>,
    authorization: Option<ExplicitRunAuthorization>,
    started_at: Option<String>,
    ended_at: Option<String>,
    finalized_at: Option<String>,
    observation_id: Option<ObservationId>,
    observation_linked_at: Option<String>,
    completion_status: Option<MeasurementCompletionStatus>,
    isolation_status: IsolationClassification,
    aborted_at: Option<String>,
    diagnostics: Vec<BenchmarkDiagnostic>,
}

impl BenchmarkRun {
    pub fn experiment_id(&self) -> &ExperimentId {
        &self.experiment_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn repetition_index(&self) -> u32 {
        self.repetition_index
    }

    pub fn state(&self) -> BenchmarkRunState {
        self.state
    }

    pub fn preflight_status(&self) -> Option<PreflightOutcome> {
        self.preflight_status
    }

    pub(crate) fn preflight(&self) -> Option<&BenchmarkPreflight> {
        self.preflight.as_ref()
    }

    pub fn authorization(&self) -> Option<&ExplicitRunAuthorization> {
        self.authorization.as_ref()
    }

    pub fn started_at(&self) -> Option<&str> {
        self.started_at.as_deref()
    }

    pub fn ended_at(&self) -> Option<&str> {
        self.ended_at.as_deref()
    }

    pub fn finalized_at(&self) -> Option<&str> {
        self.finalized_at.as_deref()
    }

    pub fn observation_id(&self) -> Option<&ObservationId> {
        self.observation_id.as_ref()
    }

    pub fn completion_status(&self) -> Option<MeasurementCompletionStatus> {
        self.completion_status
    }

    pub fn isolation_status(&self) -> IsolationClassification {
        self.isolation_status
    }

    pub fn aborted_at(&self) -> Option<&str> {
        self.aborted_at.as_deref()
    }

    pub fn diagnostics(&self) -> &[BenchmarkDiagnostic] {
        &self.diagnostics
    }

    fn latest_event_at(&self) -> Option<&str> {
        self.aborted_at
            .as_deref()
            .or(self.ended_at.as_deref())
            .or(self.started_at.as_deref())
            .or_else(|| {
                self.authorization
                    .as_ref()
                    .map(|event| event.authorized_at.as_str())
            })
            .or_else(|| {
                self.preflight
                    .as_ref()
                    .map(BenchmarkPreflight::evaluated_at)
            })
            .or(self.preflight_started_at.as_deref())
    }
}
