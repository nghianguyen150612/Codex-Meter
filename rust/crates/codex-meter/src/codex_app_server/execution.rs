//! Interactive local Codex task execution.
//!
//! This capability is intentionally separate from `CodexAppServer`, which
//! remains a metadata-only management client.

use super::approvals::{ApprovalBroker, ApprovalChoice, ApprovalView};
use super::client::{AppServerInfo, CodexAppServer};
use super::errors::CodexAppError;
use super::execution_events::{ExecutionMachine, ExecutionNotification, ExecutionPhase};
use super::execution_protocol::{
    required_string, resolve_workspace, validate_thread_id, ApprovalPolicy, SandboxMode,
    TurnStatus, VerifiedSecurity, MAX_ACTIVITY_ENTRIES, MAX_RENDERED_OUTPUT_BYTES,
};
use super::execution_recovery::{
    AuthoritativeStatus, ExecutionOrigin, ExecutionRecoveryRecord, MutationIntent, RecoveryJournal,
    RecoveryJournalError, RecoveryOutcome, RecoveryPhase,
};
use super::models::{parse_model, ModelCatalog, ModelInfo};
use super::protocol::REQUEST_TIMEOUT_SECS;
use super::protocol::{CLIENT_NAME, CLIENT_TITLE};
use super::transport::{ClientCapability, IncomingMessage, ProcessTransport};
use crate::tui::composer::{
    ModelSelection, PreparedTask, ProjectAssociation, ReasoningSelection, TargetKind,
    ValidationStatus, WorkspaceSelection, MAX_PROMPT_BYTES,
};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

const EXECUTION_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const MODEL_PAGE_SIZE: u32 = 100;
const TURN_PAGE_SIZE: u32 = 50;
const MAX_RECONCILIATION_PAGES: usize = 20;
const AUDITED_EXECUTION_RUNTIME: &str = "codex_cli_rs/0.163.0";

#[derive(Clone)]
pub struct ExecutionPreflight {
    pub target_kind: TargetKind,
    pub thread_id: Option<String>,
    pub workspace: String,
    pub model: String,
    pub reasoning: String,
    pub sandbox: Option<SandboxMode>,
    pub approval: Option<ApprovalPolicy>,
    pub approval_reviewer: String,
    pub network_access: Option<bool>,
    pub existing_settings_pending: bool,
    pub quota_snapshot: super::super::tui::composer::PreflightSnapshot,
}

pub enum ControllerEvent {
    State(ExecutionPhase),
    Preflight(ExecutionPreflight),
    Identity {
        thread_id: Option<String>,
        turn_id: Option<String>,
    },
    Activity(String),
    Output(String),
    Diff(String),
    Approval(ApprovalView),
    ApprovalResolved(String),
    Status(String),
    ToolOutput(String),
}

pub struct InteractiveExecution {
    transport: ProcessTransport,
    info: AppServerInfo,
    task: PreparedTask,
    machine: ExecutionMachine,
    preflight: ExecutionPreflight,
    security: Option<VerifiedSecurity>,
    workspace: Option<PathBuf>,
    original_thread_cwd: Option<PathBuf>,
    model_catalog: ModelCatalog,
    model_for_turn: Option<String>,
    effort_for_turn: Option<String>,
    inherited_model: Option<String>,
    inherited_effort: Option<String>,
    broker: Option<ApprovalBroker>,
    journal: RecoveryJournal,
    recovery_record: Option<ExecutionRecoveryRecord>,
    output: String,
    tool_output: String,
    last_agent_item_id: Option<String>,
    last_tool_item_id: Option<String>,
    current_diff: String,
    activity: VecDeque<String>,
    seen_turn_ids: HashSet<String>,
    interrupt_sent: bool,
    output_changed: bool,
    tool_output_changed: bool,
    diff_changed: bool,
    reported_noncritical_drops: u64,
    stage: ExistingStage,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ExistingStage {
    None,
    NeedsInspectionConfirmation,
    FinalConfirmation,
}

impl InteractiveExecution {
    pub fn prepare(task: PreparedTask) -> Result<Self, CodexAppError> {
        let transport = ProcessTransport::spawn(
            OsStr::new("codex"),
            &[OsStr::new("app-server")],
            ClientCapability::InteractiveExecution,
        )?;
        Self::prepare_with_transport(task, transport)
    }

    fn prepare_with_transport(
        task: PreparedTask,
        transport: ProcessTransport,
    ) -> Result<Self, CodexAppError> {
        #[cfg(test)]
        let journal = {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT_TEST_JOURNAL: AtomicU64 = AtomicU64::new(1);
            let path = std::env::temp_dir().join(format!(
                "codex-meter-execution-test-{}-{}",
                std::process::id(),
                NEXT_TEST_JOURNAL.fetch_add(1, Ordering::Relaxed)
            ));
            RecoveryJournal::open_at(&path).map_err(map_recovery_error)?
        };
        #[cfg(not(test))]
        let journal = RecoveryJournal::open_default().map_err(map_recovery_error)?;
        Self::prepare_with_journal(task, transport, journal)
    }

    fn prepare_with_journal(
        task: PreparedTask,
        transport: ProcessTransport,
        journal: RecoveryJournal,
    ) -> Result<Self, CodexAppError> {
        validate_prepared_task(&task)?;
        let info = initialize(&transport)?;
        verify_execution_runtime(&info)?;
        let model_catalog = list_models(&transport)?;
        let (
            workspace,
            existing_thread_id,
            original_thread_cwd,
            security,
            preflight,
            stage,
            inherited_model,
            inherited_effort,
        ) = match task.target_kind {
            TargetKind::NewThread => {
                let workspace = match &task.workspace {
                    WorkspaceSelection::CurrentDirectory(path) => resolve_workspace(path)?,
                    WorkspaceSelection::Unselected => {
                        return Err(CodexAppError::UnsafeExecutionConfiguration)
                    }
                };
                let config = call(
                    &transport,
                    "config/read",
                    json!({"cwd":workspace.to_string_lossy(),"includeLayers":false}),
                )?;
                let security = VerifiedSecurity::from_effective_config(&config)?;
                let inherited_model = config_value(&config, "model", 1024)?;
                let inherited_effort = config_value(&config, "model_reasoning_effort", 64)?;
                let model = verify_model_and_effort(&task, &model_catalog)?;
                let preflight = make_preflight(
                    &task,
                    &model_catalog,
                    PreflightContext {
                        thread_id: None,
                        workspace: Some(&workspace),
                        observed_model: model.as_deref().or(inherited_model.as_deref()),
                        observed_effort: inherited_effort.as_deref(),
                        security: Some(security),
                        existing_settings_pending: false,
                    },
                );
                (
                    Some(workspace),
                    None,
                    None,
                    Some(security),
                    preflight,
                    ExistingStage::None,
                    inherited_model,
                    inherited_effort,
                )
            }
            TargetKind::ExistingThread => {
                let thread_id = task
                    .target_thread_id
                    .as_deref()
                    .ok_or(CodexAppError::UnknownThread)?;
                validate_thread_id(thread_id)?;
                let response = call(
                    &transport,
                    "thread/read",
                    json!({"threadId":thread_id,"includeTurns":false}),
                )?;
                let thread = response
                    .get("thread")
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                let returned_id = required_string(thread, "id", 128)?;
                if returned_id != thread_id {
                    return Err(CodexAppError::UnknownThread);
                }
                ensure_thread_idle(thread)?;
                let cwd = required_string(thread, "cwd", 32 * 1024)?;
                let cwd = resolve_workspace(Path::new(cwd))?;
                let model = verify_model_and_effort(&task, &model_catalog)?;
                let observed_model = thread.get("model");
                let observed_model = optional_config_value(observed_model, 1024)?;
                let observed_effort = optional_config_value(thread.get("reasoningEffort"), 64)?;
                let preflight = make_preflight(
                    &task,
                    &model_catalog,
                    PreflightContext {
                        thread_id: Some(thread_id),
                        workspace: Some(&cwd),
                        observed_model: observed_model.as_deref().or(model.as_deref()),
                        observed_effort: observed_effort.as_deref(),
                        security: None,
                        existing_settings_pending: true,
                    },
                );
                (
                    None,
                    Some(thread_id.to_owned()),
                    Some(cwd),
                    None,
                    preflight,
                    ExistingStage::NeedsInspectionConfirmation,
                    observed_model,
                    observed_effort,
                )
            }
        };
        let mut machine = ExecutionMachine::default();
        if let Some(thread_id) = existing_thread_id {
            machine.set_thread(thread_id)?;
        }
        machine.transition(ExecutionPhase::Ready)?;
        machine.transition(ExecutionPhase::AwaitingConfirmation)?;
        let model_for_turn = match &task.model_selection {
            ModelSelection::Explicit(model) => Some(model.clone()),
            _ => None,
        };
        let effort_for_turn = verify_model_and_effort(&task, &model_catalog)?;
        Ok(Self {
            transport,
            info,
            task,
            machine,
            preflight,
            security,
            workspace,
            original_thread_cwd,
            model_catalog,
            model_for_turn,
            effort_for_turn,
            inherited_model,
            inherited_effort,
            broker: None,
            journal,
            recovery_record: None,
            output: String::new(),
            tool_output: String::new(),
            last_agent_item_id: None,
            last_tool_item_id: None,
            current_diff: String::new(),
            activity: VecDeque::new(),
            seen_turn_ids: HashSet::new(),
            interrupt_sent: false,
            output_changed: false,
            tool_output_changed: false,
            diff_changed: false,
            reported_noncritical_drops: 0,
            stage,
        })
    }

    pub fn info(&self) -> &AppServerInfo {
        &self.info
    }

    pub fn preflight(&self) -> &ExecutionPreflight {
        &self.preflight
    }

    pub fn phase(&self) -> ExecutionPhase {
        self.machine.phase()
    }

    pub fn cancel_before_submission(&mut self) -> Result<(), CodexAppError> {
        if self.machine.phase() != ExecutionPhase::AwaitingConfirmation
            || self.machine.turn_id().is_some()
        {
            return Err(CodexAppError::InvalidExecutionTransition);
        }
        if self
            .recovery_record
            .as_ref()
            .is_some_and(|record| record.recovery_required)
        {
            return Err(CodexAppError::ExecutionRecoveryUnavailable);
        }
        if self.recovery_record.is_some() {
            self.update_recovery_record(
                |record| {
                    record.execution_phase = RecoveryPhase::Cancelled;
                    record.recovery_required = false;
                },
                false,
            )?;
        }
        Ok(())
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.machine.thread_id()
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.machine.turn_id()
    }

    fn create_recovery_record(
        &mut self,
        origin: ExecutionOrigin,
        known_thread_id: Option<String>,
    ) -> Result<(), CodexAppError> {
        self.recovery_record = Some(
            self.journal
                .create(origin, known_thread_id)
                .map_err(map_recovery_error)?,
        );
        Ok(())
    }

    fn persist_recovery_state(
        &mut self,
        intent: MutationIntent,
        phase: RecoveryPhase,
        recovery_required: bool,
    ) -> Result<(), CodexAppError> {
        self.update_recovery_record(
            |record| {
                record.mutation_intent = intent;
                record.execution_phase = phase;
                record.recovery_required = recovery_required;
            },
            false,
        )
    }

    fn persist_thread_identity(&mut self, thread_id: String) -> Result<(), CodexAppError> {
        self.update_recovery_record(
            |record| {
                record.known_thread_id = Some(thread_id);
                record.execution_phase = RecoveryPhase::ThreadIdentityKnown;
                // A durable thread identity without a turn-start intent is
                // proof that this controller has not submitted the task.
                // `turn/start` gets its own write-ahead record immediately
                // before that RPC.
                record.recovery_required = false;
            },
            false,
        )
    }

    fn persist_turn_identity(
        &mut self,
        turn_id: String,
        status: TurnStatus,
    ) -> Result<(), CodexAppError> {
        self.update_recovery_record(
            |record| {
                record.known_turn_id = Some(turn_id);
                record.last_authoritative_status = Some(authoritative_status(status));
                record.execution_phase = if status == TurnStatus::InProgress {
                    RecoveryPhase::TurnIdentityKnown
                } else {
                    recovery_phase(status)
                };
                record.recovery_required = status == TurnStatus::InProgress;
            },
            false,
        )
    }

    fn persist_authoritative_status(&mut self, status: TurnStatus) -> Result<(), CodexAppError> {
        self.update_recovery_record(
            |record| {
                record.last_authoritative_status = Some(authoritative_status(status));
                record.execution_phase = if status == TurnStatus::InProgress {
                    RecoveryPhase::Running
                } else {
                    recovery_phase(status)
                };
                record.recovery_required = status == TurnStatus::InProgress;
            },
            false,
        )
    }

    fn update_recovery_record(
        &mut self,
        update: impl FnOnce(&mut ExecutionRecoveryRecord),
        guard_new_mutation: bool,
    ) -> Result<(), CodexAppError> {
        let mut proposed = self
            .recovery_record
            .clone()
            .ok_or(CodexAppError::ExecutionRecoveryUnavailable)?;
        update(&mut proposed);
        let result = if guard_new_mutation {
            self.journal.update_for_new_mutation(&proposed)
        } else {
            self.journal.update(&proposed)
        };
        self.recovery_record = Some(result.map_err(map_recovery_error)?);
        Ok(())
    }

    fn arm_new_mutation(
        &mut self,
        intent: MutationIntent,
        phase: RecoveryPhase,
    ) -> Result<(), CodexAppError> {
        self.update_recovery_record(
            |record| {
                record.mutation_intent = intent;
                record.execution_phase = phase;
                // Conservatively require recovery from the moment intent is
                // durable; a crash can happen before the transport write.
                record.recovery_required = true;
            },
            true,
        )
    }

    fn mark_unknown(&mut self) {
        let _ = self.persist_recovery_state(
            self.recovery_record
                .as_ref()
                .map_or(MutationIntent::None, |record| record.mutation_intent),
            RecoveryPhase::OutcomeUnknown,
            true,
        );
        if !self.machine.phase().terminal()
            && self.machine.phase() != ExecutionPhase::OutcomeUnknown
        {
            let _ = self.machine.transition(ExecutionPhase::OutcomeUnknown);
        }
    }

    fn enter_running(&mut self, events: &mut Vec<ControllerEvent>) -> bool {
        if let Err(error) = self.persist_authoritative_status(TurnStatus::InProgress) {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status(format!(
                "Codex reported that the turn is running, but the state could not be durably recorded ({error}). Outcome unknown."
            )));
            return false;
        }
        if self.machine.transition(ExecutionPhase::Running).is_ok() {
            events.push(ControllerEvent::State(ExecutionPhase::Running));
            true
        } else {
            false
        }
    }

    fn record_approval_response_intent(&mut self) -> Result<(), CodexAppError> {
        let terminal = self.recovery_record.as_ref().is_some_and(|record| {
            matches!(
                record.last_authoritative_status,
                Some(
                    AuthoritativeStatus::Completed
                        | AuthoritativeStatus::Failed
                        | AuthoritativeStatus::Interrupted
                )
            )
        });
        if terminal {
            let phase = self
                .recovery_record
                .as_ref()
                .map_or(RecoveryPhase::RecoveryRequired, |record| {
                    record.execution_phase
                });
            self.update_recovery_record(
                |record| {
                    record.mutation_intent = MutationIntent::ApprovalResponse;
                    record.execution_phase = phase;
                    record.recovery_required = false;
                },
                false,
            )
        } else {
            self.persist_recovery_state(
                MutationIntent::ApprovalResponse,
                RecoveryPhase::ApprovalResponseIntentRecorded,
                true,
            )?;
            self.persist_recovery_state(
                MutationIntent::ApprovalResponse,
                RecoveryPhase::ApprovalResponseMayHaveBeenSent,
                true,
            )
        }
    }

    fn send_approval_response(&mut self, id: Value, response: Value) -> Result<(), CodexAppError> {
        let terminal = self.recovery_status_is_terminal();
        self.record_approval_response_intent()?;
        if let Err(error) = self.transport.respond_server_request(id, response) {
            if !terminal {
                self.mark_unknown();
            }
            return Err(error);
        }
        Ok(())
    }

    fn reject_server_request(
        &mut self,
        id: Value,
        code: i64,
        message: &'static str,
    ) -> Result<(), CodexAppError> {
        let terminal = self.recovery_status_is_terminal();
        self.record_approval_response_intent()?;
        if let Err(error) = self.transport.reject_server_request(id, code, message) {
            if !terminal {
                self.mark_unknown();
            }
            return Err(error);
        }
        Ok(())
    }

    fn deny_pending_approvals(&mut self) -> Result<(), CodexAppError> {
        let pending = self
            .broker
            .as_mut()
            .map_or_else(Vec::new, ApprovalBroker::deny_all);
        if pending.is_empty() {
            return Ok(());
        }
        let terminal = self.recovery_status_is_terminal();
        self.record_approval_response_intent()?;
        for (id, response) in pending {
            if let Err(error) = self.transport.respond_server_request(id, response) {
                if !terminal {
                    self.mark_unknown();
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn recovery_status_is_terminal(&self) -> bool {
        self.recovery_record.as_ref().is_some_and(|record| {
            matches!(
                record.last_authoritative_status,
                Some(
                    AuthoritativeStatus::Completed
                        | AuthoritativeStatus::Failed
                        | AuthoritativeStatus::Interrupted
                )
            )
        })
    }

    pub fn output(&self) -> &str {
        &self.output
    }

    pub fn activity(&self) -> impl Iterator<Item = &str> {
        self.activity.iter().map(String::as_str)
    }

    pub fn current_diff(&self) -> &str {
        &self.current_diff
    }

    pub fn approvals(&self) -> impl Iterator<Item = &ApprovalView> {
        self.broker.iter().flat_map(ApprovalBroker::pending)
    }

    /// Existing threads require an explicit first confirmation before Codex
    /// loads the thread and reports its stored security policy. The turn is not
    /// started by this operation.
    pub fn inspect_existing_after_confirmation(
        &mut self,
    ) -> Result<ExecutionPreflight, CodexAppError> {
        if self.stage != ExistingStage::NeedsInspectionConfirmation
            || self.machine.phase() != ExecutionPhase::AwaitingConfirmation
        {
            return Err(CodexAppError::InvalidExecutionTransition);
        }
        let thread_id = self
            .preflight
            .thread_id
            .as_deref()
            .ok_or(CodexAppError::UnknownThread)?
            .to_owned();
        if self.recovery_record.is_none() {
            self.create_recovery_record(ExecutionOrigin::ExistingThread, Some(thread_id.clone()))?;
        }
        self.arm_new_mutation(
            MutationIntent::ThreadResume,
            RecoveryPhase::ThreadResumeIntentRecorded,
        )?;
        self.persist_recovery_state(
            MutationIntent::ThreadResume,
            RecoveryPhase::ThreadResumeMayHaveBeenSent,
            true,
        )?;
        let response = call(
            &self.transport,
            "thread/resume",
            json!({"threadId":thread_id.as_str(),"excludeTurns":true}),
        );
        let response = match response {
            Ok(response) => response,
            Err(error) => {
                if is_uncertain_error(&error) {
                    self.mark_unknown();
                } else {
                    self.persist_recovery_state(
                        MutationIntent::ThreadResume,
                        RecoveryPhase::ThreadIdentityKnown,
                        false,
                    )?;
                }
                return Err(error);
            }
        };
        let Some(thread) = response.get("thread") else {
            self.mark_unknown();
            return Err(CodexAppError::ProtocolVersionMismatch);
        };
        let returned_id = match required_string(thread, "id", 128) {
            Ok(id) => id,
            Err(error) => {
                self.mark_unknown();
                return Err(error);
            }
        };
        if returned_id != thread_id {
            self.mark_unknown();
            return Err(CodexAppError::UnknownThread);
        }
        self.persist_recovery_state(
            MutationIntent::ThreadResume,
            RecoveryPhase::ThreadIdentityKnown,
            false,
        )?;
        ensure_thread_idle(thread)?;
        let security = VerifiedSecurity::from_thread_response(&response)?;
        let resumed_cwd = resolve_workspace(Path::new(required_string(thread, "cwd", 32 * 1024)?))?;
        if self.original_thread_cwd.as_ref() != Some(&resumed_cwd) {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        let observed_model = thread.get("model");
        let observed_model = optional_config_value(observed_model, 1024)?;
        let observed_effort = optional_config_value(thread.get("reasoningEffort"), 64)?;
        if let Some(selected) = self.model_for_turn.as_deref() {
            if !self
                .model_catalog
                .models
                .iter()
                .any(|model| model.id == selected)
            {
                return Err(CodexAppError::UnsafeExecutionConfiguration);
            }
        } else if observed_model.is_none() {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        self.inherited_model = observed_model.clone();
        self.inherited_effort = observed_effort.clone();
        self.security = Some(security);
        self.machine.set_thread(thread_id.clone())?;
        self.preflight = make_preflight(
            &self.task,
            &self.model_catalog,
            PreflightContext {
                thread_id: Some(&thread_id),
                workspace: Some(&resumed_cwd),
                observed_model: observed_model.as_deref(),
                observed_effort: observed_effort.as_deref(),
                security: Some(security),
                existing_settings_pending: false,
            },
        );
        self.stage = ExistingStage::FinalConfirmation;
        self.machine.transition(ExecutionPhase::Ready)?;
        self.machine
            .transition(ExecutionPhase::AwaitingConfirmation)?;
        Ok(self.preflight.clone_for_tui())
    }

    /// Explicitly submits the validated plain-text prompt. A timeout after a
    /// potentially accepted request transitions to OutcomeUnknown and is
    /// never retried automatically.
    pub fn submit(&mut self) -> Result<Vec<ControllerEvent>, CodexAppError> {
        if self.machine.phase() != ExecutionPhase::AwaitingConfirmation
            || (self.task.target_kind == TargetKind::ExistingThread
                && self.stage != ExistingStage::FinalConfirmation)
            || (self.task.target_kind == TargetKind::NewThread && self.stage != ExistingStage::None)
        {
            return Err(CodexAppError::MutationAlreadyPending);
        }
        self.machine.transition(ExecutionPhase::Submitting)?;
        let mut events = vec![ControllerEvent::State(ExecutionPhase::Submitting)];
        if self.recovery_record.is_none() {
            let origin = if self.task.target_kind == TargetKind::NewThread {
                ExecutionOrigin::NewThread
            } else {
                ExecutionOrigin::ExistingThread
            };
            let selected_thread = self.machine.thread_id().map(str::to_owned);
            if let Err(error) = self.create_recovery_record(origin, selected_thread) {
                self.machine.transition(ExecutionPhase::Failed)?;
                events.push(ControllerEvent::State(ExecutionPhase::Failed));
                events.push(ControllerEvent::Status(error.to_string()));
                return Ok(events);
            }
        }
        if let Err(error) = self.revalidate_before_submit() {
            if error == CodexAppError::ExecutionPreflightChanged {
                self.machine.transition(ExecutionPhase::Ready)?;
                self.machine
                    .transition(ExecutionPhase::AwaitingConfirmation)?;
                events.push(ControllerEvent::Preflight(self.preflight.clone_for_tui()));
                events.push(ControllerEvent::State(ExecutionPhase::AwaitingConfirmation));
                events.push(ControllerEvent::Status(
                    "Codex model or security configuration changed. Review the updated preflight and confirm again; no turn was submitted.".to_owned(),
                ));
                return Ok(events);
            }
            let state = if self.machine.phase() == ExecutionPhase::OutcomeUnknown {
                ExecutionPhase::OutcomeUnknown
            } else {
                self.machine.transition(ExecutionPhase::Failed)?;
                ExecutionPhase::Failed
            };
            events.push(ControllerEvent::State(state));
            events.push(ControllerEvent::Status(format!(
                "Task was not submitted: {error}"
            )));
            return Ok(events);
        }

        if self.task.target_kind == TargetKind::NewThread {
            match self.start_new_thread() {
                Ok(thread_id) => {
                    self.machine.set_thread(thread_id.clone())?;
                    self.machine.transition(ExecutionPhase::ThreadCreated)?;
                    events.push(ControllerEvent::State(ExecutionPhase::ThreadCreated));
                    events.push(ControllerEvent::Identity {
                        thread_id: Some(thread_id.clone()),
                        turn_id: None,
                    });
                    self.activity_line("Codex created the new thread.");
                    events.push(ControllerEvent::Activity(
                        "Codex created the new thread.".to_owned(),
                    ));
                }
                Err(error) => {
                    let thread_created = self.machine.thread_id().is_some();
                    if thread_created {
                        self.machine.transition(ExecutionPhase::ThreadCreated)?;
                        events.push(ControllerEvent::State(ExecutionPhase::ThreadCreated));
                        events.push(ControllerEvent::Identity {
                            thread_id: self.machine.thread_id().map(str::to_owned),
                            turn_id: None,
                        });
                    }
                    let journal_still_ambiguous =
                        self.recovery_record.as_ref().is_some_and(|record| {
                            record.recovery_required
                                && matches!(
                                    record.execution_phase,
                                    RecoveryPhase::ThreadStartIntentRecorded
                                        | RecoveryPhase::ThreadStartMayHaveBeenSent
                                )
                        });
                    let state = if journal_still_ambiguous
                        || (is_uncertain_error(&error) && !thread_created)
                    {
                        ExecutionPhase::OutcomeUnknown
                    } else {
                        ExecutionPhase::Failed
                    };
                    self.machine.transition(state)?;
                    events.push(ControllerEvent::State(state));
                    events.push(ControllerEvent::Status(error.to_string()));
                    return Ok(events);
                }
            }
        }

        if let Err(error) = self.arm_new_mutation(
            MutationIntent::TurnStart,
            RecoveryPhase::TurnStartIntentRecorded,
        ) {
            let state = if error == CodexAppError::UnacknowledgedExecutionRecovery {
                ExecutionPhase::Failed
            } else {
                ExecutionPhase::OutcomeUnknown
            };
            if state == ExecutionPhase::OutcomeUnknown {
                self.mark_unknown();
            } else {
                self.machine.transition(state)?;
            }
            events.push(ControllerEvent::State(state));
            events.push(ControllerEvent::Status(error.to_string()));
            return Ok(events);
        }
        if let Err(error) = self.persist_recovery_state(
            MutationIntent::TurnStart,
            RecoveryPhase::TurnStartMayHaveBeenSent,
            true,
        ) {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status(error.to_string()));
            return Ok(events);
        }
        self.machine.transition(ExecutionPhase::TurnStarting)?;
        events.push(ControllerEvent::State(ExecutionPhase::TurnStarting));
        let thread_id = self
            .machine
            .thread_id()
            .ok_or(CodexAppError::ProtocolVersionMismatch)?
            .to_owned();
        let mut params = json!({
            "threadId": thread_id,
            "input": [{"type":"text","text":self.task.prompt_text}]
        });
        if let Some(model) = &self.model_for_turn {
            params["model"] = Value::String(model.clone());
        }
        if let Some(effort) = &self.effort_for_turn {
            params["effort"] = Value::String(effort.clone());
        }
        match call(&self.transport, "turn/start", params) {
            Ok(response) => {
                let parsed_turn = response.get("turn").and_then(|turn| {
                    Some((
                        required_string(turn, "id", 128).ok()?.to_owned(),
                        TurnStatus::parse(turn.get("status")?).ok()?,
                    ))
                });
                let Some((turn_id, status)) = parsed_turn else {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status("Codex accepted a turn request but returned no validated turn identity. Outcome unknown. Inspect the Codex session before submitting another task.".to_owned()));
                    return Ok(events);
                };
                if !self.seen_turn_ids.insert(turn_id.clone()) {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status("Codex returned a reused turn ID; inspect the Codex session before submitting another task.".to_owned()));
                    return Ok(events);
                }
                if let Err(error) = self.persist_turn_identity(turn_id.clone(), status) {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status(format!(
                        "Codex may have accepted the turn, but its identity could not be durably recorded ({error}). Outcome unknown."
                    )));
                    return Ok(events);
                }
                self.machine.set_turn(turn_id)?;
                let turn_failure = response
                    .get("turn")
                    .and_then(super::execution_events::turn_failure_summary);
                let turn_output = response
                    .get("turn")
                    .and_then(super::execution_events::final_agent_message);
                events.push(ControllerEvent::Identity {
                    thread_id: Some(thread_id.clone()),
                    turn_id: self.machine.turn_id().map(str::to_owned),
                });
                match status {
                    TurnStatus::InProgress => {
                        self.machine
                            .transition(ExecutionPhase::SubmissionAccepted)?;
                        events.push(ControllerEvent::State(ExecutionPhase::SubmissionAccepted));
                        events.push(ControllerEvent::Status(
                            "Codex accepted the turn request. Waiting for its lifecycle event."
                                .to_owned(),
                        ));
                    }
                    terminal => {
                        if let Some(output) = turn_output {
                            self.output = output;
                            events.push(ControllerEvent::Output(self.output.clone()));
                        }
                        self.machine.finish(terminal)?;
                        events.push(ControllerEvent::State(self.machine.phase()));
                        if terminal == TurnStatus::Failed {
                            events.push(ControllerEvent::Status(turn_failure.unwrap_or_else(
                                || {
                                    "Codex reported task failure; details are unavailable."
                                        .to_owned()
                                },
                            )));
                        } else if terminal == TurnStatus::Interrupted {
                            events.push(ControllerEvent::Status(
                                "Codex reported the turn interrupted.".to_owned(),
                            ));
                        }
                    }
                }
                if let Some(turn_id) = self.machine.turn_id() {
                    self.broker = Some(ApprovalBroker::new(thread_id, turn_id.to_owned()));
                }
                self.activity_line("Codex accepted the task submission.");
                events.push(ControllerEvent::Activity(
                    "Codex accepted the task submission.".to_owned(),
                ));
            }
            Err(error) => {
                let state = if is_uncertain_error(&error) {
                    self.mark_unknown();
                    ExecutionPhase::OutcomeUnknown
                } else if self
                    .persist_recovery_state(MutationIntent::TurnStart, RecoveryPhase::Failed, false)
                    .is_ok()
                {
                    ExecutionPhase::Failed
                } else {
                    self.mark_unknown();
                    ExecutionPhase::OutcomeUnknown
                };
                if state == ExecutionPhase::Failed {
                    self.machine.transition(state)?;
                }
                events.push(ControllerEvent::State(state));
                events.push(ControllerEvent::Status(
                    if state == ExecutionPhase::OutcomeUnknown {
                        "Outcome unknown. Inspect the Codex session before submitting another task."
                            .to_owned()
                    } else {
                        error.to_string()
                    },
                ));
            }
        }
        Ok(events)
    }

    pub fn decide_approval(
        &mut self,
        request_id: &str,
        choice: ApprovalChoice,
    ) -> Result<(), CodexAppError> {
        if self.machine.phase().terminal()
            || self.machine.phase() == ExecutionPhase::InterruptRequested
        {
            return Err(CodexAppError::StaleApprovalRequest);
        }
        let (id, response) = self
            .broker
            .as_mut()
            .ok_or(CodexAppError::StaleApprovalRequest)?
            .decision(request_id, choice)?;
        self.send_approval_response(id, response)?;
        if self
            .broker
            .as_ref()
            .is_some_and(|broker| broker.pending_count() == 0)
            && self.machine.phase() == ExecutionPhase::AwaitingApproval
        {
            self.machine.transition(ExecutionPhase::Running)?;
        }
        Ok(())
    }

    pub fn interrupt(&mut self) -> Result<Vec<ControllerEvent>, CodexAppError> {
        if self.interrupt_sent {
            return Err(CodexAppError::MutationAlreadyPending);
        }
        let thread_id = self
            .machine
            .thread_id()
            .ok_or(CodexAppError::InvalidExecutionTransition)?
            .to_owned();
        let turn_id = self
            .machine
            .turn_id()
            .ok_or(CodexAppError::InvalidExecutionTransition)?
            .to_owned();
        self.machine.request_interrupt()?;
        let mut events = vec![ControllerEvent::State(ExecutionPhase::InterruptRequested)];
        if let Err(error) = self.deny_pending_approvals() {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status(format!(
                "An outstanding approval could not be safely denied before interruption ({error}). Outcome unknown."
            )));
            return Ok(events);
        }
        if let Err(error) = self.arm_new_mutation(
            MutationIntent::TurnInterrupt,
            RecoveryPhase::InterruptIntentRecorded,
        ) {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status(error.to_string()));
            return Ok(events);
        }
        if let Err(error) = self.persist_recovery_state(
            MutationIntent::TurnInterrupt,
            RecoveryPhase::InterruptMayHaveBeenSent,
            true,
        ) {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status(error.to_string()));
            return Ok(events);
        }
        self.interrupt_sent = true;
        match call(
            &self.transport,
            "turn/interrupt",
            json!({"threadId":thread_id,"turnId":turn_id}),
        ) {
            Ok(_) => {
                if let Err(error) = self.persist_recovery_state(
                    MutationIntent::TurnInterrupt,
                    RecoveryPhase::InterruptAcknowledged,
                    true,
                ) {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status(error.to_string()));
                    return Ok(events);
                }
                events.push(ControllerEvent::Status("Codex acknowledged the interrupt request; waiting for authoritative turn completion.".to_owned()));
                self.activity_line("Interrupt request acknowledged by Codex.");
                events.push(ControllerEvent::Activity(
                    "Interrupt request acknowledged by Codex.".to_owned(),
                ));
            }
            Err(error) => {
                self.mark_unknown();
                events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                events.push(ControllerEvent::Status("Outcome unknown after interrupt request. Reconcile the Codex session before submitting another task.".to_owned()));
                if is_uncertain_error(&error) {
                    return Ok(events);
                }
                events.push(ControllerEvent::Activity(
                    "The interrupt request was not acknowledged.".to_owned(),
                ));
            }
        }
        Ok(events)
    }

    pub fn reconcile(&mut self) -> Vec<ControllerEvent> {
        let mut events = Vec::new();
        if self.machine.phase() != ExecutionPhase::OutcomeUnknown {
            events.push(ControllerEvent::Status(
                "The execution does not require reconciliation.".to_owned(),
            ));
            return events;
        }
        if self
            .machine
            .transition(ExecutionPhase::Reconciling)
            .is_err()
        {
            events.push(ControllerEvent::Status(
                "Outcome unknown. Inspect the Codex session before submitting another task."
                    .to_owned(),
            ));
            return events;
        }
        events.push(ControllerEvent::State(ExecutionPhase::Reconciling));
        let Some(thread_id) = self.machine.thread_id().map(str::to_owned) else {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status(
                "Outcome unknown. Inspect the Codex session before submitting another task."
                    .to_owned(),
            ));
            return events;
        };
        let Some(turn_id) = self.machine.turn_id().map(str::to_owned) else {
            self.mark_unknown();
            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
            events.push(ControllerEvent::Status("Codex returned no turn identity. Outcome unknown. Inspect the Codex session before submitting another task.".to_owned()));
            return events;
        };
        match self.reconnect_and_read_turn(&thread_id, &turn_id) {
            Ok(Some(
                status @ (TurnStatus::Completed | TurnStatus::Failed | TurnStatus::Interrupted),
            )) => {
                if let Err(error) = self.persist_authoritative_status(status) {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status(format!(
                        "Codex returned a terminal turn state, but it could not be durably recorded ({error}). Outcome unknown."
                    )));
                } else if self.machine.finish(status).is_ok() {
                    events.push(ControllerEvent::State(self.machine.phase()));
                    events.push(ControllerEvent::Status(
                        "Codex turn state reconciled from the authoritative turn list.".to_owned(),
                    ));
                }
            }
            Ok(Some(TurnStatus::InProgress)) => {
                if let Err(error) = self.persist_authoritative_status(TurnStatus::InProgress) {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status(error.to_string()));
                } else if self.machine.transition(ExecutionPhase::Running).is_ok() {
                    events.push(ControllerEvent::State(ExecutionPhase::Running));
                    events.push(ControllerEvent::Status(
                        "Codex reports the known turn is still running.".to_owned(),
                    ));
                }
            }
            Ok(None) => {
                self.mark_unknown();
                events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                events.push(ControllerEvent::Status(
                    "Outcome unknown. Inspect the Codex session before submitting another task."
                        .to_owned(),
                ));
            }
            Err(error) => {
                self.mark_unknown();
                events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                events.push(ControllerEvent::Status(format!(
                    "Codex could not authoritatively reconcile this turn ({error}). Outcome unknown. Inspect the Codex session before submitting another task."
                )));
            }
        }
        events
    }

    /// Drains a bounded batch of App Server messages. Stream deltas update one
    /// transient output snapshot per poll so bursts cannot block the TUI.
    pub fn drain_events(&mut self) -> Vec<ControllerEvent> {
        let mut events = Vec::new();
        if self.transport.critical_event_loss() {
            let _ = self.deny_pending_approvals();
            if !self.machine.phase().terminal()
                && self.machine.phase() != ExecutionPhase::OutcomeUnknown
                && self
                    .machine
                    .transition(ExecutionPhase::OutcomeUnknown)
                    .is_ok()
            {
                self.persist_recovery_state(
                    self.recovery_record
                        .as_ref()
                        .map_or(MutationIntent::None, |record| record.mutation_intent),
                    RecoveryPhase::OutcomeUnknown,
                    true,
                )
                .ok();
                events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                events.push(ControllerEvent::Status("A critical Codex lifecycle or approval event could not be preserved. Outcome unknown. Inspect the Codex session before submitting another task.".to_owned()));
            }
            return events;
        }
        for _ in 0..super::protocol::NOTIFICATION_QUEUE_CAPACITY {
            let Some(incoming) = self.transport.try_incoming() else {
                break;
            };
            match incoming {
                IncomingMessage::ServerRequest { id, method, params } => {
                    if self.machine.phase() == ExecutionPhase::InterruptRequested
                        || self.machine.phase().terminal()
                    {
                        let result = match method.as_str() {
                            "item/commandExecution/requestApproval"
                            | "item/fileChange/requestApproval" => json!({"decision":"decline"}),
                            "item/permissions/requestApproval" => {
                                json!({"permissions":{},"scope":"turn"})
                            }
                            _ => json!({}),
                        };
                        let _ = self.send_approval_response(id, result);
                        continue;
                    }
                    let Some(thread_id) = self.machine.thread_id() else {
                        let _ = self.reject_server_request(
                            id,
                            -32602,
                            "Codex Meter has no validated thread for this approval",
                        );
                        events.push(ControllerEvent::Status("An approval request arrived without a known thread. The request was not approved.".to_owned()));
                        continue;
                    };
                    let Some(turn_id) = self.machine.turn_id() else {
                        let _ = self.reject_server_request(
                            id,
                            -32602,
                            "Codex Meter has no validated turn for this approval",
                        );
                        events.push(ControllerEvent::Status("An approval request arrived without a known turn. The request was not approved.".to_owned()));
                        continue;
                    };
                    let diff = self.current_diff.clone();
                    let broker = self.broker.get_or_insert_with(|| {
                        ApprovalBroker::new(thread_id.to_owned(), turn_id.to_owned())
                    });
                    match broker.receive(id.clone(), &method, &params, &diff) {
                        Ok(view) => {
                            if self.machine.phase() == ExecutionPhase::Running
                                || self.machine.phase() == ExecutionPhase::SubmissionAccepted
                            {
                                let _ = self.machine.transition(ExecutionPhase::AwaitingApproval);
                                events
                                    .push(ControllerEvent::State(ExecutionPhase::AwaitingApproval));
                            }
                            self.activity_line("Codex requested approval.");
                            events.push(ControllerEvent::Approval(view));
                        }
                        Err(_) => {
                            let _ = self.reject_server_request(
                                id,
                                -32602,
                                "Codex Meter rejected an invalid or out-of-scope approval request",
                            );
                            if matches!(
                                self.machine.phase(),
                                ExecutionPhase::Running
                                    | ExecutionPhase::SubmissionAccepted
                                    | ExecutionPhase::AwaitingApproval
                            ) {
                                self.mark_unknown();
                                events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                                events.push(ControllerEvent::Status("An approval request could not be validated. Outcome unknown. Inspect the Codex session before submitting another task.".to_owned()));
                            }
                        }
                    }
                }
                notification => match ExecutionNotification::parse(notification) {
                    Ok(Some(notification)) => self.apply_notification(notification, &mut events),
                    Ok(None) => {}
                    Err(_) => {
                        if !self.machine.phase().terminal()
                            && self.machine.phase() != ExecutionPhase::OutcomeUnknown
                            && self
                                .machine
                                .transition(ExecutionPhase::OutcomeUnknown)
                                .is_ok()
                        {
                            self.persist_recovery_state(
                                self.recovery_record
                                    .as_ref()
                                    .map_or(MutationIntent::None, |record| record.mutation_intent),
                                RecoveryPhase::OutcomeUnknown,
                                true,
                            )
                            .ok();
                            events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                            events.push(ControllerEvent::Status("A Codex execution event did not match the validated protocol shape. Outcome unknown. Inspect the Codex session before submitting another task.".to_owned()));
                        }
                    }
                },
            }
        }
        if self.output_changed {
            self.output_changed = false;
            events.push(ControllerEvent::Output(self.output.clone()));
        }
        if self.tool_output_changed {
            self.tool_output_changed = false;
            events.push(ControllerEvent::ToolOutput(self.tool_output.clone()));
        }
        if self.diff_changed {
            self.diff_changed = false;
            events.push(ControllerEvent::Diff(self.current_diff.clone()));
        }
        if let Some(error) = self.transport.connection_error() {
            let previous_phase = self.machine.phase();
            let next = match previous_phase {
                ExecutionPhase::SubmissionAccepted
                | ExecutionPhase::Running
                | ExecutionPhase::AwaitingApproval
                | ExecutionPhase::InterruptRequested => Some(ExecutionPhase::OutcomeUnknown),
                ExecutionPhase::AwaitingConfirmation => Some(ExecutionPhase::Failed),
                _ => None,
            };
            if next.is_some_and(|next| self.machine.transition(next).is_ok()) {
                if self.machine.phase() == ExecutionPhase::OutcomeUnknown {
                    self.persist_recovery_state(
                        self.recovery_record
                            .as_ref()
                            .map_or(MutationIntent::None, |record| record.mutation_intent),
                        RecoveryPhase::OutcomeUnknown,
                        true,
                    )
                    .ok();
                }
                let _ = self.deny_pending_approvals();
                let next = self.machine.phase();
                events.push(ControllerEvent::State(next));
                if next == ExecutionPhase::OutcomeUnknown {
                    events.push(ControllerEvent::Status(format!(
                        "Codex connection was lost ({error}). Outcome unknown. Inspect the Codex session before submitting another task."
                    )));
                } else {
                    events.push(ControllerEvent::Status(format!(
                        "Codex connection was lost ({error}) before task submission. No task was submitted; refresh preflight before trying again."
                    )));
                }
            }
        }
        let drops = self.transport.dropped_notifications();
        if drops > self.reported_noncritical_drops {
            self.reported_noncritical_drops = drops;
            events.push(ControllerEvent::Status("Some noncritical streamed progress was coalesced; lifecycle and approval events remain intact.".to_owned()));
        }
        events
    }

    fn apply_notification(
        &mut self,
        event: ExecutionNotification,
        events: &mut Vec<ControllerEvent>,
    ) {
        match event {
            ExecutionNotification::ThreadStarted { thread_id } => {
                if self.machine.matches_active(&thread_id, None) {
                    self.activity_line("Codex confirmed the thread started.");
                    events.push(ControllerEvent::Activity(
                        "Codex confirmed the thread started.".to_owned(),
                    ));
                }
            }
            ExecutionNotification::TurnStarted { thread_id, turn_id } => {
                if !self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    return;
                }
                if self.machine.phase() == ExecutionPhase::SubmissionAccepted
                    && self.enter_running(events)
                {
                    self.activity_line("Codex turn started.");
                    events.push(ControllerEvent::Activity("Codex turn started.".to_owned()));
                }
            }
            ExecutionNotification::TurnCompleted {
                thread_id,
                turn_id,
                status,
                failure_summary,
                final_agent_message,
            } => {
                if !self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    return;
                }
                if status == TurnStatus::InProgress {
                    if self.machine.phase() != ExecutionPhase::OutcomeUnknown
                        && self
                            .machine
                            .transition(ExecutionPhase::OutcomeUnknown)
                            .is_ok()
                    {
                        self.persist_recovery_state(
                            self.recovery_record
                                .as_ref()
                                .map_or(MutationIntent::None, |record| record.mutation_intent),
                            RecoveryPhase::OutcomeUnknown,
                            true,
                        )
                        .ok();
                        events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                        events.push(ControllerEvent::Status(
                            "Codex sent a nonterminal turn completion event. Outcome unknown."
                                .to_owned(),
                        ));
                    }
                    return;
                }
                if let Err(error) = self.persist_authoritative_status(status) {
                    self.mark_unknown();
                    events.push(ControllerEvent::State(ExecutionPhase::OutcomeUnknown));
                    events.push(ControllerEvent::Status(format!(
                        "Codex reported a terminal turn state, but it could not be durably recorded ({error}). Outcome unknown."
                    )));
                    return;
                }
                if self.machine.phase() == ExecutionPhase::OutcomeUnknown
                    && self
                        .machine
                        .transition(ExecutionPhase::Reconciling)
                        .is_err()
                {
                    return;
                }
                if self.machine.finish(status).is_ok() {
                    if let Some(output) = final_agent_message {
                        self.output.clear();
                        append_bounded(&mut self.output, &output, MAX_RENDERED_OUTPUT_BYTES);
                        self.output_changed = true;
                    }
                    let _ = self.deny_pending_approvals();
                    events.push(ControllerEvent::State(self.machine.phase()));
                    let status_line = match status {
                        TurnStatus::Completed => "Codex reported the turn completed.",
                        TurnStatus::Failed => "Codex reported the turn failed.",
                        TurnStatus::Interrupted => "Codex reported the turn interrupted.",
                        TurnStatus::InProgress => unreachable!(),
                    };
                    self.activity_line(status_line);
                    events.push(ControllerEvent::Activity(status_line.to_owned()));
                    if status == TurnStatus::Failed {
                        events.push(ControllerEvent::Status(failure_summary.unwrap_or_else(
                            || "Codex reported task failure; details are unavailable.".to_owned(),
                        )));
                    }
                }
            }
            ExecutionNotification::ItemStarted {
                thread_id,
                turn_id,
                item,
            } => {
                if !self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    return;
                }
                if self.machine.phase() == ExecutionPhase::SubmissionAccepted {
                    self.enter_running(events);
                }
                self.activity_line(&format!("Codex started {item}."));
                events.push(ControllerEvent::Activity(format!("Codex started {item}.")));
            }
            ExecutionNotification::ItemCompleted {
                thread_id,
                turn_id,
                item,
            } => {
                if self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    self.activity_line(&format!("Codex completed {item}."));
                    events.push(ControllerEvent::Activity(format!(
                        "Codex completed {item}."
                    )));
                }
            }
            ExecutionNotification::AgentMessageCompleted {
                thread_id,
                turn_id,
                item_id,
                text,
            } => {
                if self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    self.output.clear();
                    append_bounded(&mut self.output, &text, MAX_RENDERED_OUTPUT_BYTES);
                    self.output_changed = true;
                    self.activity_line(&format!(
                        "Codex completed user-facing response item {item_id}."
                    ));
                    events.push(ControllerEvent::Activity(format!(
                        "Codex completed user-facing response item {item_id}."
                    )));
                }
            }
            ExecutionNotification::AgentDelta {
                thread_id,
                turn_id,
                item_id,
                text,
            } => {
                if self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    if self.last_agent_item_id.as_deref() != Some(&item_id) {
                        self.activity_line(&format!(
                            "Codex streamed user-facing response item {item_id}."
                        ));
                        events.push(ControllerEvent::Activity(format!(
                            "Codex streamed user-facing response item {item_id}."
                        )));
                        self.last_agent_item_id = Some(item_id);
                    }
                    append_bounded(&mut self.output, &text, MAX_RENDERED_OUTPUT_BYTES);
                    self.output_changed = true;
                    if self.machine.phase() == ExecutionPhase::SubmissionAccepted
                        && self.machine.transition(ExecutionPhase::Running).is_ok()
                    {
                        events.push(ControllerEvent::State(ExecutionPhase::Running));
                    }
                }
            }
            ExecutionNotification::ToolOutput {
                thread_id,
                turn_id,
                item_id,
                text,
            } => {
                if self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    if self.last_tool_item_id.as_deref() != Some(&item_id) {
                        self.activity_line(&format!("Codex streamed tool output item {item_id}."));
                        events.push(ControllerEvent::Activity(format!(
                            "Codex streamed tool output item {item_id}."
                        )));
                        self.last_tool_item_id = Some(item_id);
                    }
                    append_bounded(&mut self.tool_output, &text, MAX_RENDERED_OUTPUT_BYTES);
                    self.tool_output_changed = true;
                }
            }
            ExecutionNotification::DiffUpdated {
                thread_id,
                turn_id,
                diff,
            } => {
                if self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    self.current_diff = diff;
                    self.diff_changed = true;
                    self.activity_line("Codex proposed file changes.");
                    events.push(ControllerEvent::Activity(
                        "Codex proposed file changes.".to_owned(),
                    ));
                }
            }
            ExecutionNotification::PlanUpdated { thread_id, turn_id } => {
                if self.machine.matches_active(&thread_id, Some(&turn_id)) {
                    self.activity_line("Codex updated the task plan.");
                    events.push(ControllerEvent::Activity(
                        "Codex updated the task plan.".to_owned(),
                    ));
                }
            }
            ExecutionNotification::ThreadStatus { thread_id, status } => {
                if self.machine.matches_active(&thread_id, None) {
                    self.activity_line(&format!("Codex thread status: {status}."));
                    events.push(ControllerEvent::Activity(format!(
                        "Codex thread status: {status}."
                    )));
                }
            }
            ExecutionNotification::RequestResolved {
                thread_id,
                request_id,
            } => {
                if self.machine.matches_active(&thread_id, None) {
                    if let Some(broker) = self.broker.as_mut() {
                        if broker.resolve_server_notice(&request_id, &thread_id) {
                            events.push(ControllerEvent::ApprovalResolved(request_id));
                            events.push(ControllerEvent::Status(
                                "A pending approval was resolved by Codex.".to_owned(),
                            ));
                            if broker.pending_count() == 0
                                && self.machine.phase() == ExecutionPhase::AwaitingApproval
                                && self.machine.transition(ExecutionPhase::Running).is_ok()
                            {
                                events.push(ControllerEvent::State(ExecutionPhase::Running));
                            }
                        }
                    }
                }
            }
        }
    }

    fn start_new_thread(&mut self) -> Result<String, CodexAppError> {
        let cwd = self
            .workspace
            .as_ref()
            .ok_or(CodexAppError::UnsafeExecutionConfiguration)?;
        let security = self
            .security
            .ok_or(CodexAppError::UnsafeExecutionConfiguration)?;
        let mut params = json!({
            "cwd": cwd.to_string_lossy(),
            "sandbox": security.sandbox.wire(),
            "approvalPolicy": security.approval.wire(),
            "approvalsReviewer": "user",
        });
        if security.sandbox == SandboxMode::WorkspaceWrite {
            params["config"] = json!({"sandbox_workspace_write":{"network_access":false}});
        }
        if let Some(model) = &self.model_for_turn {
            params["model"] = Value::String(model.clone());
        }
        self.arm_new_mutation(
            MutationIntent::ThreadStart,
            RecoveryPhase::ThreadStartIntentRecorded,
        )?;
        self.persist_recovery_state(
            MutationIntent::ThreadStart,
            RecoveryPhase::ThreadStartMayHaveBeenSent,
            true,
        )?;
        let response = call(&self.transport, "thread/start", params)?;
        let thread = response
            .get("thread")
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let id = required_string(thread, "id", 128)?.to_owned();
        validate_thread_id(&id)?;
        self.machine.set_thread(id.clone())?;
        self.persist_thread_identity(id.clone())?;
        let actual = VerifiedSecurity::from_thread_response(&response)?;
        if Some(actual) != self.security {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        if let Some(selected) = self.model_for_turn.as_deref() {
            if required_string(&response, "model", 1024)? != selected {
                return Err(CodexAppError::UnsafeExecutionConfiguration);
            }
        }
        let returned_cwd =
            resolve_workspace(Path::new(required_string(&response, "cwd", 32 * 1024)?))?;
        if Some(&returned_cwd) != self.workspace.as_ref() {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        let actual_model = required_string(&response, "model", 1024)?;
        self.preflight.model = match &self.task.model_selection {
            ModelSelection::Inherit => format!(
                "Inherited (Codex reported {})",
                bounded_display(actual_model, 128)
            ),
            ModelSelection::Explicit(_) => {
                format!("Selected ({})", bounded_display(actual_model, 128))
            }
            ModelSelection::Unavailable => "Unavailable".to_owned(),
        };
        if thread
            .get("status")
            .and_then(|status| status.get("type"))
            .and_then(Value::as_str)
            == Some("active")
        {
            return Err(CodexAppError::ActiveThreadConflict);
        }
        Ok(id)
    }

    fn revalidate_before_submit(&mut self) -> Result<(), CodexAppError> {
        validate_prepared_task(&self.task)?;
        self.model_catalog = list_models(&self.transport)?;
        self.effort_for_turn = verify_model_and_effort(&self.task, &self.model_catalog)?;
        match self.task.target_kind {
            TargetKind::NewThread => {
                let current = resolve_workspace(
                    self.workspace
                        .as_deref()
                        .ok_or(CodexAppError::UnsafeExecutionConfiguration)?,
                )?;
                if Some(&current) != self.workspace.as_ref() {
                    return Err(CodexAppError::UnsafeExecutionConfiguration);
                }
                let response = call(
                    &self.transport,
                    "config/read",
                    json!({"cwd":current.to_string_lossy(),"includeLayers":false}),
                )?;
                let current_security = VerifiedSecurity::from_effective_config(&response)?;
                let current_model = config_value(&response, "model", 1024)?;
                let current_effort = config_value(&response, "model_reasoning_effort", 64)?;
                let inherited_model_changed =
                    matches!(&self.task.model_selection, ModelSelection::Inherit)
                        && current_model != self.inherited_model;
                let inherited_effort_changed =
                    matches!(&self.task.reasoning_selection, ReasoningSelection::Inherit)
                        && current_effort != self.inherited_effort;
                if Some(current_security) != self.security
                    || inherited_model_changed
                    || inherited_effort_changed
                {
                    self.security = Some(current_security);
                    self.inherited_model = current_model.clone();
                    self.inherited_effort = current_effort.clone();
                    self.preflight = make_preflight(
                        &self.task,
                        &self.model_catalog,
                        PreflightContext {
                            thread_id: None,
                            workspace: Some(&current),
                            observed_model: self
                                .model_for_turn
                                .as_deref()
                                .or(current_model.as_deref()),
                            observed_effort: current_effort.as_deref(),
                            security: self.security,
                            existing_settings_pending: false,
                        },
                    );
                    return Err(CodexAppError::ExecutionPreflightChanged);
                }
            }
            TargetKind::ExistingThread => {
                let thread_id = self
                    .preflight
                    .thread_id
                    .as_deref()
                    .ok_or(CodexAppError::UnknownThread)?
                    .to_owned();
                let response = call(
                    &self.transport,
                    "thread/read",
                    json!({"threadId":thread_id.as_str(),"includeTurns":false}),
                )?;
                let thread = response
                    .get("thread")
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                if required_string(thread, "id", 128)? != thread_id {
                    return Err(CodexAppError::UnknownThread);
                }
                ensure_thread_idle(thread)?;
                self.arm_new_mutation(
                    MutationIntent::ThreadResume,
                    RecoveryPhase::ThreadResumeIntentRecorded,
                )?;
                self.persist_recovery_state(
                    MutationIntent::ThreadResume,
                    RecoveryPhase::ThreadResumeMayHaveBeenSent,
                    true,
                )?;
                let resumed = call(
                    &self.transport,
                    "thread/resume",
                    json!({"threadId":thread_id.as_str(),"excludeTurns":true}),
                );
                let resumed = match resumed {
                    Ok(response) => response,
                    Err(error) => {
                        if is_uncertain_error(&error) {
                            self.mark_unknown();
                        } else {
                            self.persist_recovery_state(
                                MutationIntent::ThreadResume,
                                RecoveryPhase::ThreadIdentityKnown,
                                false,
                            )?;
                        }
                        return Err(error);
                    }
                };
                let Some(resumed_thread) = resumed.get("thread") else {
                    self.mark_unknown();
                    return Err(CodexAppError::ProtocolVersionMismatch);
                };
                let resumed_id = match required_string(resumed_thread, "id", 128) {
                    Ok(id) => id,
                    Err(error) => {
                        self.mark_unknown();
                        return Err(error);
                    }
                };
                if resumed_id != thread_id {
                    self.mark_unknown();
                    return Err(CodexAppError::UnknownThread);
                }
                self.persist_recovery_state(
                    MutationIntent::ThreadResume,
                    RecoveryPhase::ThreadIdentityKnown,
                    false,
                )?;
                ensure_thread_idle(resumed_thread)?;
                let actual_security = VerifiedSecurity::from_thread_response(&resumed)?;
                let current_model = optional_config_value(resumed_thread.get("model"), 1024)?;
                let current_effort =
                    optional_config_value(resumed_thread.get("reasoningEffort"), 64)?;
                if self.model_for_turn.is_none() && current_model.is_none() {
                    return Err(CodexAppError::UnsafeExecutionConfiguration);
                }
                let inherited_model_changed =
                    matches!(&self.task.model_selection, ModelSelection::Inherit)
                        && current_model != self.inherited_model;
                let inherited_effort_changed =
                    matches!(&self.task.reasoning_selection, ReasoningSelection::Inherit)
                        && current_effort != self.inherited_effort;
                if Some(actual_security) != self.security
                    || inherited_model_changed
                    || inherited_effort_changed
                {
                    self.security = Some(actual_security);
                    self.inherited_model = current_model.clone();
                    self.inherited_effort = current_effort.clone();
                    self.preflight = make_preflight(
                        &self.task,
                        &self.model_catalog,
                        PreflightContext {
                            thread_id: Some(&thread_id),
                            workspace: self.original_thread_cwd.as_deref(),
                            observed_model: self
                                .model_for_turn
                                .as_deref()
                                .or(current_model.as_deref()),
                            observed_effort: current_effort.as_deref(),
                            security: self.security,
                            existing_settings_pending: false,
                        },
                    );
                    return Err(CodexAppError::ExecutionPreflightChanged);
                }
                let resumed_cwd = resolve_workspace(Path::new(required_string(
                    resumed_thread,
                    "cwd",
                    32 * 1024,
                )?))?;
                if Some(&resumed_cwd) != self.original_thread_cwd.as_ref() {
                    return Err(CodexAppError::UnsafeExecutionConfiguration);
                }
            }
        }
        Ok(())
    }

    fn reconnect_and_read_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
    ) -> Result<Option<TurnStatus>, CodexAppError> {
        let transport = self.transport.reconnect()?;
        let info = initialize(&transport)?;
        verify_execution_runtime(&info)?;
        let response = call(
            &transport,
            "thread/read",
            json!({"threadId":thread_id,"includeTurns":false}),
        )?;
        let thread = response
            .get("thread")
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        if required_string(thread, "id", 128)? != thread_id {
            return Err(CodexAppError::UnknownThread);
        }
        let status = thread
            .get("status")
            .and_then(|status| status.get("type"))
            .and_then(Value::as_str);
        if !matches!(status, Some("active" | "idle" | "notLoaded")) {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        let mut params = json!({"threadId":thread_id,"limit":TURN_PAGE_SIZE,"itemsView":"notLoaded","sortDirection":"desc"});
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        loop {
            if let Some(cursor) = cursor.as_ref() {
                params["cursor"] = Value::String(cursor.clone());
            }
            let response = call(&transport, "thread/turns/list", params.clone())?;
            let object = response
                .as_object()
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            let turns = object
                .get("data")
                .and_then(Value::as_array)
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            for turn in turns.iter().take(TURN_PAGE_SIZE as usize) {
                if turn.get("id").and_then(Value::as_str) == Some(turn_id) {
                    let status = TurnStatus::parse(
                        turn.get("status")
                            .ok_or(CodexAppError::ProtocolVersionMismatch)?,
                    )?;
                    self.transport = transport;
                    self.info = info;
                    return Ok(Some(status));
                }
            }
            cursor = object
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 4096)
                .map(str::to_owned);
            if let Some(cursor) = cursor.as_ref() {
                if !seen_cursors.insert(cursor.clone()) {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
            } else {
                self.transport = transport;
                self.info = info;
                return Ok(None);
            }
        }
    }

    fn activity_line(&mut self, line: &str) {
        while self.activity.len() >= MAX_ACTIVITY_ENTRIES {
            self.activity.pop_front();
        }
        self.activity.push_back(line.to_owned());
    }
}

impl ExecutionPreflight {
    fn clone_for_tui(&self) -> Self {
        Self {
            target_kind: self.target_kind,
            thread_id: self.thread_id.clone(),
            workspace: self.workspace.clone(),
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
            sandbox: self.sandbox,
            approval: self.approval,
            approval_reviewer: self.approval_reviewer.clone(),
            network_access: self.network_access,
            existing_settings_pending: self.existing_settings_pending,
            quota_snapshot: self.quota_snapshot.clone(),
        }
    }
}

fn validate_prepared_task(task: &PreparedTask) -> Result<(), CodexAppError> {
    if task.validation_status != ValidationStatus::Ready
        || task.prompt_text.trim().is_empty()
        || task.prompt_text.len() > MAX_PROMPT_BYTES
        || task.prompt_text.contains('\0')
        || task
            .prompt_text
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
    {
        return Err(CodexAppError::InvalidPreparedTask);
    }
    if matches!(
        &task.project_association,
        ProjectAssociation::ResponseLocalAlias(_)
    ) {
        // A response-local P033 alias is deliberately not sent to Codex.
    }
    match task.target_kind {
        TargetKind::NewThread => {
            if !matches!(&task.workspace, WorkspaceSelection::CurrentDirectory(_))
                || task.target_thread_id.is_some()
            {
                return Err(CodexAppError::InvalidPreparedTask);
            }
        }
        TargetKind::ExistingThread => {
            validate_thread_id(
                task.target_thread_id
                    .as_deref()
                    .ok_or(CodexAppError::UnknownThread)?,
            )?;
        }
    }
    Ok(())
}

fn ensure_thread_idle(thread: &Value) -> Result<(), CodexAppError> {
    match thread
        .get("status")
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
    {
        Some("idle" | "notLoaded") => Ok(()),
        Some("active") => Err(CodexAppError::ActiveThreadConflict),
        _ => Err(CodexAppError::UnsafeExecutionConfiguration),
    }
}

struct PreflightContext<'a> {
    thread_id: Option<&'a str>,
    workspace: Option<&'a Path>,
    observed_model: Option<&'a str>,
    observed_effort: Option<&'a str>,
    security: Option<VerifiedSecurity>,
    existing_settings_pending: bool,
}

fn make_preflight(
    task: &PreparedTask,
    catalog: &ModelCatalog,
    context: PreflightContext<'_>,
) -> ExecutionPreflight {
    let PreflightContext {
        thread_id,
        workspace,
        observed_model,
        observed_effort,
        security,
        existing_settings_pending,
    } = context;
    let model = match &task.model_selection {
        ModelSelection::Inherit => observed_model.map_or_else(
            || "Inherited from Codex configuration".to_owned(),
            |value| {
                format!(
                    "Inherited (Codex currently reports {})",
                    bounded_display(value, 128)
                )
            },
        ),
        ModelSelection::Explicit(id) => catalog
            .models
            .iter()
            .find(|model| &model.id == id)
            .map(|model| {
                format!(
                    "{} ({})",
                    bounded_display(&model.display_name, 96),
                    bounded_display(id, 96)
                )
            })
            .unwrap_or_else(|| "Selected model unavailable".to_owned()),
        ModelSelection::Unavailable => "Unavailable".to_owned(),
    };
    let reasoning = match &task.reasoning_selection {
        ReasoningSelection::Inherit => observed_effort.map_or_else(
            || "Inherited from Codex configuration".to_owned(),
            |value| {
                format!(
                    "Inherited (Codex currently reports {})",
                    bounded_display(value, 64)
                )
            },
        ),
        ReasoningSelection::ModelDefault => "Selected model default".to_owned(),
        ReasoningSelection::Explicit(value) => format!("Explicit ({})", bounded_display(value, 64)),
        ReasoningSelection::NeedsChoice(_) => "Choice required".to_owned(),
        ReasoningSelection::Unavailable => "Unavailable".to_owned(),
    };
    let (sandbox, approval, network_access) = security.map_or((None, None, None), |value| {
        (
            Some(value.sandbox),
            Some(value.approval),
            Some(value.network_access),
        )
    });
    let approval_reviewer = if existing_settings_pending {
        "Pending thread inspection".to_owned()
    } else if task.target_kind == TargetKind::NewThread {
        "User review explicitly requested; Codex response will be checked".to_owned()
    } else if security.is_some() {
        "User (confirmed by Codex)".to_owned()
    } else {
        "Unknown".to_owned()
    };
    ExecutionPreflight {
        target_kind: task.target_kind,
        thread_id: thread_id.map(str::to_owned),
        workspace: workspace.map_or_else(
            || "Unavailable".to_owned(),
            |value| value.to_string_lossy().into_owned(),
        ),
        model,
        reasoning,
        sandbox,
        approval,
        approval_reviewer,
        network_access,
        existing_settings_pending,
        quota_snapshot: task.preflight_snapshot.clone(),
    }
}

fn config_value(
    response: &Value,
    key: &str,
    max_bytes: usize,
) -> Result<Option<String>, CodexAppError> {
    let config = response
        .get("config")
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    optional_config_value(config.get(key), max_bytes)
}

fn optional_config_value(
    value: Option<&Value>,
    max_bytes: usize,
) -> Result<Option<String>, CodexAppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value))
            if !value.is_empty()
                && value.len() <= max_bytes
                && !value.chars().any(char::is_control) =>
        {
            Ok(Some(value.clone()))
        }
        _ => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

fn verify_model_and_effort(
    task: &PreparedTask,
    catalog: &ModelCatalog,
) -> Result<Option<String>, CodexAppError> {
    let selected = match &task.model_selection {
        ModelSelection::Inherit => None,
        ModelSelection::Explicit(id) => Some(
            catalog
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or(CodexAppError::SelectedModelUnavailable)?,
        ),
        ModelSelection::Unavailable => return Err(CodexAppError::SelectedModelUnavailable),
    };
    match &task.reasoning_selection {
        ReasoningSelection::Inherit => Ok(None),
        ReasoningSelection::ModelDefault => selected
            .map(|model| Some(model.default_reasoning_effort.clone()))
            .ok_or(CodexAppError::UnsupportedReasoningEffort),
        ReasoningSelection::Explicit(effort) => {
            let model = selected.ok_or(CodexAppError::UnsupportedReasoningEffort)?;
            if model
                .supported_reasoning_efforts
                .iter()
                .any(|supported| supported.id == *effort)
            {
                Ok(Some(effort.clone()))
            } else {
                Err(CodexAppError::UnsupportedReasoningEffort)
            }
        }
        ReasoningSelection::NeedsChoice(_) | ReasoningSelection::Unavailable => {
            Err(CodexAppError::UnsupportedReasoningEffort)
        }
    }
}

fn list_models(transport: &ProcessTransport) -> Result<ModelCatalog, CodexAppError> {
    let mut models: Vec<ModelInfo> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    loop {
        let mut params = json!({"limit":MODEL_PAGE_SIZE,"includeHidden":true});
        if let Some(cursor) = cursor.as_ref() {
            params["cursor"] = Value::String(cursor.clone());
        }
        let response = call(transport, "model/list", params)?;
        let object = response
            .as_object()
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let data = object
            .get("data")
            .and_then(Value::as_array)
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        for value in data {
            models.push(parse_model(value)?);
            if models.len() > 10_000 {
                return Err(CodexAppError::ResponseLimitExceeded);
            }
        }
        cursor = object
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .map(str::to_owned);
        if let Some(value) = cursor.as_ref() {
            if !seen.insert(value.clone()) {
                return Err(CodexAppError::ProtocolVersionMismatch);
            }
        } else {
            break;
        }
    }
    Ok(ModelCatalog {
        availability: "live",
        models,
    })
}

fn initialize(transport: &ProcessTransport) -> Result<AppServerInfo, CodexAppError> {
    let response = transport
        .request(
            "initialize",
            json!({"clientInfo":{"name":CLIENT_NAME,"title":CLIENT_TITLE,"version":env!("CARGO_PKG_VERSION")}}),
            Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )
        .map_err(|error| match error {
            CodexAppError::UnsupportedMethod("initialize") => CodexAppError::AppServerUnsupported,
            error => error,
        })?;
    let info = AppServerInfo {
        user_agent: required_string(&response, "userAgent", 1024)?.to_owned(),
        platform_family: required_string(&response, "platformFamily", 128)?.to_owned(),
        platform_os: required_string(&response, "platformOs", 128)?.to_owned(),
    };
    transport.notify("initialized", json!({}))?;
    transport.mark_initialized();
    Ok(info)
}

fn verify_execution_runtime(info: &AppServerInfo) -> Result<(), CodexAppError> {
    let Some(suffix) = info.user_agent.strip_prefix(AUDITED_EXECUTION_RUNTIME) else {
        return Err(CodexAppError::UnsupportedExecutionRuntime);
    };
    if suffix.is_empty() || suffix.starts_with([' ', '/', '+']) {
        Ok(())
    } else {
        Err(CodexAppError::UnsupportedExecutionRuntime)
    }
}

/// Explicit, read-only reconciliation for a journal record after restart.
/// It matches only the exact durable thread and turn IDs and never resumes a
/// thread or submits a replacement turn.
pub fn reconcile_recovery_record(
    journal: &RecoveryJournal,
    execution_id: &str,
) -> Result<RecoveryOutcome, RecoveryJournalError> {
    let mut record = journal.get(execution_id)?;
    if !record.recovery_required {
        return Ok(match record.last_authoritative_status {
            Some(AuthoritativeStatus::Completed) => RecoveryOutcome::RecoveredCompleted,
            Some(AuthoritativeStatus::Failed) => RecoveryOutcome::RecoveredFailed,
            Some(AuthoritativeStatus::Interrupted) => RecoveryOutcome::RecoveredInterrupted,
            Some(AuthoritativeStatus::InProgress) => RecoveryOutcome::StillRunning,
            None => RecoveryOutcome::OutcomeUnknown,
        });
    }
    let (Some(thread_id), Some(turn_id)) =
        (record.known_thread_id.clone(), record.known_turn_id.clone())
    else {
        record.execution_phase = RecoveryPhase::OutcomeUnknown;
        record.recovery_required = true;
        let _ = journal.update(&record)?;
        return Ok(RecoveryOutcome::OutcomeUnknown);
    };

    let client = match CodexAppServer::connect() {
        Ok(client) => client,
        Err(_) => return Ok(RecoveryOutcome::RecoveryUnavailable),
    };
    if verify_execution_runtime(client.info()).is_err() {
        return Ok(RecoveryOutcome::RecoveryUnavailable);
    }
    let thread_response = match client.call(
        "thread/read",
        json!({"threadId":thread_id,"includeTurns":false}),
    ) {
        Ok(response) => response,
        Err(CodexAppError::UnknownThread) => {
            record.execution_phase = RecoveryPhase::OutcomeUnknown;
            record.recovery_required = true;
            let _ = journal.update(&record)?;
            return Ok(RecoveryOutcome::OutcomeUnknown);
        }
        Err(_) => return Ok(RecoveryOutcome::RecoveryUnavailable),
    };
    let thread = thread_response
        .get("thread")
        .ok_or(RecoveryJournalError::CorruptJournal)?;
    if required_string(thread, "id", 128).ok() != Some(thread_id.as_str()) {
        record.execution_phase = RecoveryPhase::OutcomeUnknown;
        record.recovery_required = true;
        let _ = journal.update(&record)?;
        return Ok(RecoveryOutcome::OutcomeUnknown);
    }
    let status = thread
        .get("status")
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str);
    if !matches!(status, Some("active" | "idle" | "notLoaded")) {
        return Ok(RecoveryOutcome::RecoveryUnavailable);
    }

    let mut params = json!({
        "threadId":thread_id.as_str(),
        "limit":TURN_PAGE_SIZE,
        "itemsView":"notLoaded",
        "sortDirection":"desc"
    });
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    let mut matched = None;
    for _ in 0..MAX_RECONCILIATION_PAGES {
        if let Some(cursor_value) = cursor.as_ref() {
            params["cursor"] = Value::String(cursor_value.clone());
        } else {
            params
                .as_object_mut()
                .expect("object created above")
                .remove("cursor");
        }
        let response = match client.call("thread/turns/list", params.clone()) {
            Ok(response) => response,
            Err(_) => return Ok(RecoveryOutcome::RecoveryUnavailable),
        };
        let Some(object) = response.as_object() else {
            return Ok(RecoveryOutcome::RecoveryUnavailable);
        };
        let Some(turns) = object.get("data").and_then(Value::as_array) else {
            return Ok(RecoveryOutcome::RecoveryUnavailable);
        };
        if turns.len() > TURN_PAGE_SIZE as usize {
            return Ok(RecoveryOutcome::RecoveryUnavailable);
        }
        for turn in turns {
            if turn.get("id").and_then(Value::as_str) == Some(turn_id.as_str()) {
                matched = turn
                    .get("status")
                    .and_then(|value| TurnStatus::parse(value).ok());
                if matched.is_none() {
                    return Ok(RecoveryOutcome::RecoveryUnavailable);
                }
                break;
            }
        }
        if matched.is_some() {
            break;
        }
        cursor = match object.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(value) => match value.as_str() {
                Some(value) if !value.is_empty() && value.len() <= 4096 => Some(value.to_owned()),
                _ => return Ok(RecoveryOutcome::RecoveryUnavailable),
            },
        };
        let Some(cursor_value) = cursor.as_ref() else {
            break;
        };
        if !seen_cursors.insert(cursor_value.clone()) {
            return Ok(RecoveryOutcome::RecoveryUnavailable);
        }
    }

    let Some(status) = matched else {
        record.execution_phase = RecoveryPhase::OutcomeUnknown;
        record.recovery_required = true;
        let _ = journal.update(&record)?;
        return Ok(RecoveryOutcome::OutcomeUnknown);
    };
    record.last_authoritative_status = Some(authoritative_status(status));
    record.execution_phase = recovery_phase(status);
    record.recovery_required = status == TurnStatus::InProgress;
    let _ = journal.update(&record)?;
    Ok(match status {
        TurnStatus::InProgress => RecoveryOutcome::StillRunning,
        TurnStatus::Completed => RecoveryOutcome::RecoveredCompleted,
        TurnStatus::Failed => RecoveryOutcome::RecoveredFailed,
        TurnStatus::Interrupted => RecoveryOutcome::RecoveredInterrupted,
    })
}

/// Sends an interrupt only after an explicit user action and a fresh,
/// read-only check confirms that the exact durable turn is still running.
pub fn interrupt_recovery_record(
    journal: &RecoveryJournal,
    execution_id: &str,
) -> Result<RecoveryOutcome, RecoveryJournalError> {
    let mut record = journal.get(execution_id)?;
    let (Some(thread_id), Some(turn_id)) =
        (record.known_thread_id.clone(), record.known_turn_id.clone())
    else {
        return Ok(RecoveryOutcome::OutcomeUnknown);
    };
    let transport = match ProcessTransport::spawn(
        OsStr::new("codex"),
        &[OsStr::new("app-server")],
        ClientCapability::InteractiveExecution,
    ) {
        Ok(transport) => transport,
        Err(_) => return Ok(RecoveryOutcome::RecoveryUnavailable),
    };
    let info = match initialize(&transport) {
        Ok(info) => info,
        Err(_) => return Ok(RecoveryOutcome::RecoveryUnavailable),
    };
    if verify_execution_runtime(&info).is_err() {
        return Ok(RecoveryOutcome::RecoveryUnavailable);
    }
    let current = match recovery_turn_status(&transport, &thread_id, &turn_id) {
        Ok(Some(status)) => status,
        Ok(None) | Err(CodexAppError::UnknownThread) => {
            record.execution_phase = RecoveryPhase::OutcomeUnknown;
            record.recovery_required = true;
            let _ = journal.update(&record)?;
            return Ok(RecoveryOutcome::OutcomeUnknown);
        }
        Err(_) => return Ok(RecoveryOutcome::RecoveryUnavailable),
    };
    if current != TurnStatus::InProgress {
        record.last_authoritative_status = Some(authoritative_status(current));
        record.execution_phase = recovery_phase(current);
        record.recovery_required = false;
        let _ = journal.update(&record)?;
        return Ok(match current {
            TurnStatus::Completed => RecoveryOutcome::RecoveredCompleted,
            TurnStatus::Failed => RecoveryOutcome::RecoveredFailed,
            TurnStatus::Interrupted => RecoveryOutcome::RecoveredInterrupted,
            TurnStatus::InProgress => unreachable!(),
        });
    }

    record.last_authoritative_status = Some(AuthoritativeStatus::InProgress);
    record.mutation_intent = MutationIntent::TurnInterrupt;
    record.execution_phase = RecoveryPhase::InterruptIntentRecorded;
    record.recovery_required = true;
    record = journal.update_for_new_mutation(&record)?;
    record.execution_phase = RecoveryPhase::InterruptMayHaveBeenSent;
    record = journal.update(&record)?;
    match call(
        &transport,
        "turn/interrupt",
        json!({"threadId":thread_id.as_str(),"turnId":turn_id.as_str()}),
    ) {
        Ok(_) => {
            record.execution_phase = RecoveryPhase::InterruptAcknowledged;
            if journal.update(&record).is_err() {
                return Ok(RecoveryOutcome::OutcomeUnknown);
            }
            Ok(RecoveryOutcome::InterruptAcknowledged)
        }
        Err(error) if is_uncertain_error(&error) => Ok(RecoveryOutcome::OutcomeUnknown),
        Err(_) => {
            record.execution_phase = RecoveryPhase::Running;
            record.last_authoritative_status = Some(AuthoritativeStatus::InProgress);
            record.recovery_required = true;
            if journal.update(&record).is_err() {
                Ok(RecoveryOutcome::OutcomeUnknown)
            } else {
                Ok(RecoveryOutcome::StillRunning)
            }
        }
    }
}

fn recovery_turn_status(
    transport: &ProcessTransport,
    thread_id: &str,
    turn_id: &str,
) -> Result<Option<TurnStatus>, CodexAppError> {
    let response = call(
        transport,
        "thread/read",
        json!({"threadId":thread_id,"includeTurns":false}),
    )?;
    let thread = response
        .get("thread")
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    if required_string(thread, "id", 128)? != thread_id {
        return Err(CodexAppError::UnknownThread);
    }
    let thread_status = thread
        .get("status")
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str);
    if !matches!(thread_status, Some("active" | "idle" | "notLoaded")) {
        return Err(CodexAppError::ProtocolVersionMismatch);
    }
    let mut params = json!({
        "threadId":thread_id,
        "limit":TURN_PAGE_SIZE,
        "itemsView":"notLoaded",
        "sortDirection":"desc"
    });
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    for _ in 0..MAX_RECONCILIATION_PAGES {
        if let Some(value) = cursor.as_ref() {
            params["cursor"] = Value::String(value.clone());
        } else if let Some(object) = params.as_object_mut() {
            object.remove("cursor");
        }
        let response = call(transport, "thread/turns/list", params.clone())?;
        let object = response
            .as_object()
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let turns = object
            .get("data")
            .and_then(Value::as_array)
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        if turns.len() > TURN_PAGE_SIZE as usize {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        for turn in turns {
            if turn.get("id").and_then(Value::as_str) == Some(turn_id) {
                return TurnStatus::parse(
                    turn.get("status")
                        .ok_or(CodexAppError::ProtocolVersionMismatch)?,
                )
                .map(Some);
            }
        }
        cursor = match object.get("nextCursor") {
            None | Some(Value::Null) => None,
            Some(value) => value
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 4096)
                .map(str::to_owned),
        };
        let Some(value) = cursor.as_ref() else {
            return Ok(None);
        };
        if !seen.insert(value.clone()) {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
    }
    Ok(None)
}

fn call(
    transport: &ProcessTransport,
    method: &'static str,
    params: Value,
) -> Result<Value, CodexAppError> {
    transport.request(method, params, EXECUTION_RPC_TIMEOUT)
}

fn append_bounded(target: &mut String, value: &str, max_bytes: usize) {
    if value.is_empty() {
        return;
    }
    let mut append = value.to_owned();
    if append.len() > max_bytes {
        let mut start = append.len() - max_bytes;
        while !append.is_char_boundary(start) {
            start += 1;
        }
        append.drain(..start);
    }
    if target.len().saturating_add(append.len()) > max_bytes {
        let remove = target
            .len()
            .saturating_add(append.len())
            .saturating_sub(max_bytes);
        let mut end = remove.min(target.len());
        while !target.is_char_boundary(end) {
            end += 1;
        }
        target.drain(..end);
    }
    target.push_str(&append);
}

fn bounded_display(value: &str, max: usize) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_control() {
            continue;
        }
        if result.len().saturating_add(character.len_utf8()) > max {
            break;
        }
        result.push(character);
    }
    result
}

fn is_uncertain_error(error: &CodexAppError) -> bool {
    matches!(
        error,
        CodexAppError::RequestTimeout
            | CodexAppError::ProtocolVersionMismatch
            | CodexAppError::UnexpectedProcessExit
            | CodexAppError::ProcessIoFailure
            | CodexAppError::ServerShutdown
            | CodexAppError::CriticalEventLoss
            | CodexAppError::MalformedResponse
    )
}

fn map_recovery_error(error: RecoveryJournalError) -> CodexAppError {
    match error {
        RecoveryJournalError::UnacknowledgedRecovery => {
            CodexAppError::UnacknowledgedExecutionRecovery
        }
        RecoveryJournalError::ConcurrentUpdate => CodexAppError::ExecutionRecoveryConflict,
        _ => CodexAppError::ExecutionRecoveryUnavailable,
    }
}

fn authoritative_status(status: TurnStatus) -> AuthoritativeStatus {
    match status {
        TurnStatus::InProgress => AuthoritativeStatus::InProgress,
        TurnStatus::Completed => AuthoritativeStatus::Completed,
        TurnStatus::Failed => AuthoritativeStatus::Failed,
        TurnStatus::Interrupted => AuthoritativeStatus::Interrupted,
    }
}

fn recovery_phase(status: TurnStatus) -> RecoveryPhase {
    match status {
        TurnStatus::InProgress => RecoveryPhase::Running,
        TurnStatus::Completed => RecoveryPhase::Completed,
        TurnStatus::Failed => RecoveryPhase::Failed,
        TurnStatus::Interrupted => RecoveryPhase::Interrupted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    fn prepared_task(target_kind: TargetKind) -> PreparedTask {
        let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
        PreparedTask {
            target_kind,
            target_thread_id: (target_kind == TargetKind::ExistingThread)
                .then(|| "thread-existing".to_owned()),
            workspace: if target_kind == TargetKind::NewThread {
                WorkspaceSelection::CurrentDirectory(workspace)
            } else {
                WorkspaceSelection::Unselected
            },
            project_association: ProjectAssociation::Unavailable,
            model_selection: ModelSelection::Inherit,
            reasoning_selection: ReasoningSelection::Inherit,
            prompt_text: "P036_SYNTHETIC_PROMPT".to_owned(),
            configuration_source: "Codex configuration",
            validation_status: ValidationStatus::Ready,
            validation_messages: Vec::new(),
            preflight_snapshot: Default::default(),
        }
    }

    fn fake_transport(mode: &str) -> (ProcessTransport, PathBuf) {
        let workspace = std::env::current_dir().unwrap().canonicalize().unwrap();
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_app_server.py");
        let python = if cfg!(windows) { "python" } else { "python3" };
        let args = [
            OsStr::new("-u"),
            fixture.as_os_str(),
            OsStr::new(mode),
            workspace.as_os_str(),
        ];
        let transport = ProcessTransport::spawn(
            OsStr::new(python),
            &args,
            ClientCapability::InteractiveExecution,
        )
        .expect("fake App Server process starts");
        (transport, workspace)
    }

    fn new_fake_execution(mode: &str, target: TargetKind) -> InteractiveExecution {
        let (transport, _) = fake_transport(mode);
        InteractiveExecution::prepare_with_transport(prepared_task(target), transport)
            .expect("fake App Server performs fresh execution preflight")
    }

    fn drain_to_terminal(controller: &mut InteractiveExecution) -> Vec<ControllerEvent> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut events = Vec::new();
        let mut statuses = Vec::new();
        while !controller.phase().terminal() && Instant::now() < deadline {
            let batch = controller.drain_events();
            statuses.extend(batch.iter().filter_map(|event| match event {
                ControllerEvent::Status(value) => Some(value.clone()),
                _ => None,
            }));
            events.extend(batch);
            if !controller.phase().terminal() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(controller.phase().terminal(), "fake turn should reach an authoritative terminal state; phase={:?}; statuses={statuses:?}", controller.phase());
        events.extend(controller.drain_events());
        events
    }

    #[test]
    fn output_buffer_stays_bounded_at_utf8_boundaries() {
        let mut output = String::from("éééé");
        append_bounded(&mut output, "世界", 8);
        assert!(output.len() <= 8);
        assert!(output.is_char_boundary(output.len()));
        assert_eq!(output, "é世界");
    }

    #[test]
    fn uncertain_rpc_failures_are_never_classified_as_failed_turns() {
        assert!(is_uncertain_error(&CodexAppError::RequestTimeout));
        assert!(!is_uncertain_error(&CodexAppError::RemoteRequestRejected {
            method: "turn/start",
            code: Some(-32602),
        }));
    }

    #[test]
    fn execution_preflight_fails_closed_for_unverified_runtime_versions() {
        assert!(verify_execution_runtime(&AppServerInfo {
            user_agent: "codex_cli_rs/0.163.0".to_owned(),
            platform_family: "unix".to_owned(),
            platform_os: "linux".to_owned(),
        })
        .is_ok());
        assert!(verify_execution_runtime(&AppServerInfo {
            user_agent: "codex_cli_rs/0.163.0-dev".to_owned(),
            platform_family: "unix".to_owned(),
            platform_os: "linux".to_owned(),
        })
        .is_err());
        assert_eq!(
            verify_execution_runtime(&AppServerInfo {
                user_agent: "codex_cli_rs/0.162.0".to_owned(),
                platform_family: "unix".to_owned(),
                platform_os: "linux".to_owned(),
            }),
            Err(CodexAppError::UnsupportedExecutionRuntime)
        );
    }

    #[test]
    fn execution_preflight_rejects_an_unaudited_fake_server_version_before_catalog_access() {
        let (transport, _) = fake_transport("execution_unsupported_version");
        assert!(matches!(
            InteractiveExecution::prepare_with_transport(
                prepared_task(TargetKind::NewThread),
                transport
            ),
            Err(CodexAppError::UnsupportedExecutionRuntime)
        ));
    }

    #[test]
    fn fake_server_new_thread_submission_waits_for_authoritative_completion() {
        let mut controller = new_fake_execution("execution_new", TargetKind::NewThread);
        assert_eq!(controller.phase(), ExecutionPhase::AwaitingConfirmation);
        assert_eq!(
            controller.preflight().sandbox,
            Some(SandboxMode::WorkspaceWrite)
        );
        let submitted = controller.submit().unwrap();
        assert!(submitted.iter().any(|event| matches!(
            event,
            ControllerEvent::State(ExecutionPhase::SubmissionAccepted)
        )));
        assert_eq!(controller.phase(), ExecutionPhase::SubmissionAccepted);
        assert_eq!(controller.thread_id(), Some("thread-created"));
        assert_eq!(controller.turn_id(), Some("turn-p036"));
        let events = drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
        assert!(controller
            .output()
            .contains("Final response from fake Codex."));
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Activity(value) if value.contains("item-agent")
        )));
    }

    #[test]
    fn terminal_start_response_provides_user_facing_result_without_exposing_reasoning() {
        let mut controller =
            new_fake_execution("execution_terminal_response", TargetKind::NewThread);
        let events = controller.submit().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
        assert_eq!(controller.output(), "Authoritative final response");
        assert!(!controller.output().contains("INTERNAL_REASONING_SENTINEL"));
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Output(value) if value == "Authoritative final response"
        )));
    }

    #[test]
    fn terminal_turn_without_final_agent_output_remains_completed_without_invented_text() {
        let mut controller = new_fake_execution("execution_missing_output", TargetKind::NewThread);
        controller.submit().unwrap();
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
        assert!(controller.output().is_empty());
    }

    #[test]
    fn known_turn_reconciles_after_connection_loss_without_replaying_submission() {
        let mut controller = new_fake_execution("execution_reconcile", TargetKind::NewThread);
        controller.submit().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while controller.phase() != ExecutionPhase::OutcomeUnknown && Instant::now() < deadline {
            controller.drain_events();
            if controller.phase() != ExecutionPhase::OutcomeUnknown {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert_eq!(controller.phase(), ExecutionPhase::OutcomeUnknown);
        assert_eq!(controller.thread_id(), Some("thread-created"));
        assert_eq!(controller.turn_id(), Some("turn-p036"));

        let events = controller.reconcile();
        let statuses = events
            .iter()
            .filter_map(|event| match event {
                ControllerEvent::Status(message) => Some(message.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            controller.phase(),
            ExecutionPhase::Completed,
            "{statuses:?}"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Status(message) if message.contains("authoritative turn list")
        )));
    }

    #[test]
    fn fake_server_existing_thread_requires_inspection_then_preserves_identity() {
        let mut controller = new_fake_execution("execution_existing", TargetKind::ExistingThread);
        assert!(controller.preflight().existing_settings_pending);
        let inspected = controller.inspect_existing_after_confirmation().unwrap();
        assert!(!inspected.existing_settings_pending);
        assert_eq!(inspected.thread_id.as_deref(), Some("thread-existing"));
        assert_eq!(inspected.approval_reviewer, "User (confirmed by Codex)");
        controller.submit().unwrap();
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
        assert_eq!(controller.thread_id(), Some("thread-existing"));
    }

    #[test]
    fn changed_inherited_new_thread_configuration_invalidates_confirmation() {
        let mut controller =
            new_fake_execution("execution_new_model_changed", TargetKind::NewThread);
        assert!(controller
            .preflight()
            .model
            .contains("currently reports fixture-model"));
        let events = controller.submit().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::AwaitingConfirmation);
        assert_eq!(controller.thread_id(), None);
        assert_eq!(controller.turn_id(), None);
        assert!(controller.preflight().model.contains("changed-model"));
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Status(message) if message.contains("changed") && message.contains("confirm again")
        )));
    }

    #[test]
    fn changed_inherited_existing_thread_model_invalidates_confirmation() {
        let mut controller = new_fake_execution(
            "execution_existing_model_changed",
            TargetKind::ExistingThread,
        );
        controller.inspect_existing_after_confirmation().unwrap();
        assert!(controller
            .preflight()
            .model
            .contains("currently reports fixture-model"));
        let events = controller.submit().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::AwaitingConfirmation);
        assert_eq!(controller.thread_id(), Some("thread-existing"));
        assert_eq!(controller.turn_id(), None);
        assert!(controller.preflight().model.contains("changed-model"));
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Status(message) if message.contains("changed") && message.contains("confirm again")
        )));
    }

    #[test]
    fn fake_server_active_existing_thread_is_refused_before_resume_or_submission() {
        let (transport, _) = fake_transport("execution_active_thread");
        assert!(matches!(
            InteractiveExecution::prepare_with_transport(
                prepared_task(TargetKind::ExistingThread),
                transport
            ),
            Err(CodexAppError::ActiveThreadConflict)
        ));
    }

    #[test]
    fn fake_server_approval_does_not_block_notifications_or_auto_accept() {
        let mut controller = new_fake_execution("execution_approval", TargetKind::NewThread);
        controller.submit().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut pending = None;
        while pending.is_none() && Instant::now() < deadline {
            for event in controller.drain_events() {
                if let ControllerEvent::Approval(view) = event {
                    pending = Some(view);
                }
            }
            if pending.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let approval =
            pending.expect("explicit command approval should arrive without blocking the RPC loop");
        assert!(approval.allow_once_available);
        assert_eq!(controller.phase(), ExecutionPhase::AwaitingApproval);
        controller
            .decide_approval(&approval.request_id, ApprovalChoice::AllowOnce)
            .unwrap();
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
    }

    #[test]
    fn fake_server_file_change_approval_requires_explicit_one_time_acceptance() {
        let mut controller = new_fake_execution("execution_file_approval", TargetKind::NewThread);
        controller.submit().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut pending = None;
        while pending.is_none() && Instant::now() < deadline {
            for event in controller.drain_events() {
                if let ControllerEvent::Approval(view) = event {
                    pending = Some(view);
                }
            }
            if pending.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let approval = pending.expect("file change approval should be routed to the UI");
        assert!(approval.kind == super::super::approvals::ApprovalKind::FileChange);
        assert!(approval.allow_once_available);
        assert!(approval.details.contains("approved content"));
        controller
            .decide_approval(&approval.request_id, ApprovalChoice::AllowOnce)
            .unwrap();
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
    }

    #[test]
    fn fake_server_permission_approval_grants_only_the_validated_turn_subset() {
        let mut controller =
            new_fake_execution("execution_permission_approval", TargetKind::NewThread);
        controller.submit().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut pending = None;
        while pending.is_none() && Instant::now() < deadline {
            for event in controller.drain_events() {
                if let ControllerEvent::Approval(view) = event {
                    pending = Some(view);
                }
            }
            if pending.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let approval = pending.expect("permission approval should be routed to the UI");
        assert!(approval.kind == super::super::approvals::ApprovalKind::Permissions);
        assert!(approval.allow_once_available);
        assert!(approval.details.contains("/repo/generated"));
        controller
            .decide_approval(&approval.request_id, ApprovalChoice::AllowOnce)
            .unwrap();
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Completed);
    }

    #[test]
    fn fake_server_interrupt_acknowledgement_is_not_final_interruption() {
        let mut controller = new_fake_execution("execution_interrupt", TargetKind::NewThread);
        controller.submit().unwrap();
        let started = Instant::now() + Duration::from_secs(3);
        while controller.phase() == ExecutionPhase::SubmissionAccepted && Instant::now() < started {
            controller.drain_events();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(controller.phase(), ExecutionPhase::Running);
        controller.interrupt().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::InterruptRequested);
        assert!(matches!(
            controller.interrupt(),
            Err(CodexAppError::MutationAlreadyPending)
        ));
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Interrupted);
    }

    #[test]
    fn interrupt_while_approval_is_pending_denies_the_request_and_waits_for_terminal_state() {
        let mut controller =
            new_fake_execution("execution_approval_interrupt", TargetKind::NewThread);
        controller.submit().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut approval = None;
        while approval.is_none() && Instant::now() < deadline {
            for event in controller.drain_events() {
                if let ControllerEvent::Approval(view) = event {
                    approval = Some(view);
                }
            }
            if approval.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(approval.is_some());
        let events = controller.interrupt().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::InterruptRequested);
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Status(message) if message.contains("acknowledged")
        )));
        drain_to_terminal(&mut controller);
        assert_eq!(controller.phase(), ExecutionPhase::Interrupted);
    }

    #[test]
    fn lost_turn_start_response_is_uncertain_and_never_replayed() {
        let mut controller = new_fake_execution("execution_lost_turn_start", TargetKind::NewThread);
        let events = controller.submit().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::OutcomeUnknown);
        assert_eq!(controller.thread_id(), Some("thread-created"));
        assert!(controller.turn_id().is_none());
        assert!(events.iter().any(|event| matches!(
            event,
            ControllerEvent::Status(value) if value.contains("Outcome unknown")
        )));
        assert!(controller.submit().is_err());
    }

    #[test]
    fn lost_thread_start_response_is_uncertain_without_replacement_thread() {
        let (transport, _) = fake_transport("execution_lost_thread_start");
        let mut controller = InteractiveExecution::prepare_with_transport(
            prepared_task(TargetKind::NewThread),
            transport,
        )
        .unwrap();
        controller.submit().unwrap();
        assert_eq!(controller.phase(), ExecutionPhase::OutcomeUnknown);
        assert!(controller.thread_id().is_none());
        assert!(controller.submit().is_err());
    }
}
