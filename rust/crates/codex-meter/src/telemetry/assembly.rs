//! Ordered assembly of session, task, configuration, and token evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use super::codex_rollout::{EventMessage, RolloutRecordKind, TaskCompletion, TurnContextRecord};
use super::identity;
use super::incremental_jsonl::{
    ReadBatch, ReadItem, ReadItemOutcome, RejectedLineReason, SourceIdentity,
};
use super::normalized::{
    ConfigurationIdentity, ConfigurationPayload, ConfigurationPayloadKind, ConfigurationValue,
    LifecyclePayload, LifecyclePayloadKind, NormalizedConfigurationEvent, NormalizedEventType,
    NormalizedSessionEvent, NormalizedTokenEvent, SchemaVersion,
};
use super::token_normalization::{
    normalize_token_item, validate_timestamp, TimestampErrorReason, TokenNormalizationError,
    TokenNormalizationOutcome,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TelemetryState {
    pub session: Option<SessionContext>,
    pub thread_configuration: ConfigurationState,
    pub tasks: BTreeMap<String, TaskState>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionContext {
    pub session_id: String,
    pub thread_id: String,
    pub cli_version: String,
    pub model_provider: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConfigurationState {
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
    pub codex_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskState {
    pub task_id: String,
    pub root_task_id: Option<String>,
    pub lifecycle: TaskLifecycle,
    pub start_observed: bool,
    pub terminal_evidence: BTreeSet<TaskTerminalEvidence>,
    pub anomalies: BTreeSet<TaskAnomaly>,
    pub configuration_override: ConfigurationState,
    pub token_configuration_fingerprints: BTreeSet<String>,
    pub configuration_consistency: ConfigurationConsistency,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TaskLifecycle {
    Observed,
    Active,
    Completed,
    Failed,
    Aborted,
    ConflictingTerminalEvidence,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TaskTerminalEvidence {
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum TaskAnomaly {
    CompletionWithoutStart,
    AbortWithoutStart,
    RepeatedStart,
    ConflictingTerminalEvidence,
    TokenAfterTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigurationConsistency {
    NoTokenConsumption,
    Consistent,
    Mixed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssembledBatch {
    pub outputs: Vec<AssembledOutput>,
    pub next_state: TelemetryState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssembledOutput {
    SessionDetected(NormalizedSessionEvent),
    ConfigurationEvidence(NormalizedConfigurationEvent),
    TaskLifecycle(TaskLifecycleTransition),
    AttributedToken(AttributedTokenEvent),
    TokenSnapshot(TokenSnapshotEvidence),
    RejectedSourceLine { reason: RejectedLineReason },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskLifecycleTransition {
    pub evidence_id: String,
    pub task_id: Option<String>,
    pub root_task_id: Option<String>,
    pub event_at: String,
    pub transition: TaskLifecycleTransitionKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskLifecycleTransitionKind {
    Started,
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributedTokenEvent {
    pub normalized_event: NormalizedTokenEvent,
    pub effective_configuration: ConfigurationIdentity,
    pub configuration_fingerprint: String,
    pub task_lifecycle: Option<TaskLifecycle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenSnapshotEvidence {
    pub evidence_id: String,
    pub event_at: String,
    pub evidence: super::token_normalization::TokenEvidenceSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TelemetryAssemblyError {
    InvalidTimestamp(TimestampErrorReason),
    SessionIdentityConflict,
    TokenNormalization(TokenNormalizationError),
}

impl fmt::Display for TelemetryAssemblyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimestamp(reason) => {
                write!(
                    formatter,
                    "source timestamp is not valid UTC RFC 3339: {reason}"
                )
            }
            Self::SessionIdentityConflict => {
                formatter.write_str("source generation contains conflicting session identities")
            }
            Self::TokenNormalization(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TelemetryAssemblyError {}

pub fn assemble_batch(
    current_state: &TelemetryState,
    source: &SourceIdentity,
    batch: &ReadBatch,
) -> Result<AssembledBatch, TelemetryAssemblyError> {
    let mut next_state = current_state.clone();
    let mut outputs = Vec::new();
    for item in &batch.items {
        assemble_item(&mut next_state, source, item, &mut outputs)?;
    }
    Ok(AssembledBatch {
        outputs,
        next_state,
    })
}

fn assemble_item(
    state: &mut TelemetryState,
    source: &SourceIdentity,
    item: &ReadItem,
    outputs: &mut Vec<AssembledOutput>,
) -> Result<(), TelemetryAssemblyError> {
    let record = match &item.outcome {
        ReadItemOutcome::Rejected(rejected) => {
            outputs.push(AssembledOutput::RejectedSourceLine {
                reason: rejected.reason.clone(),
            });
            return Ok(());
        }
        ReadItemOutcome::Decoded(record) => record.as_ref(),
    };

    match &record.kind {
        RolloutRecordKind::SessionMeta(meta) => {
            validate_timestamp(&record.timestamp)
                .map_err(TelemetryAssemblyError::InvalidTimestamp)?;
            let session_id = identity::session_id(&meta.session_id);
            let thread_id = identity::thread_id(&meta.thread_id);
            if let Some(existing) = &state.session {
                if existing.session_id != session_id {
                    return Err(TelemetryAssemblyError::SessionIdentityConflict);
                }
            } else {
                state.session = Some(SessionContext {
                    session_id: session_id.clone(),
                    thread_id,
                    cli_version: meta.cli_version.clone(),
                    model_provider: meta.model_provider.clone(),
                });
                state.thread_configuration.codex_version = Some(meta.cli_version.clone());
                state.thread_configuration.model_provider = meta.model_provider.clone();
                outputs.push(AssembledOutput::SessionDetected(NormalizedSessionEvent {
                    schema_version: SchemaVersion::V1,
                    event_id: identity::session_detected_event_id(
                        source,
                        item.start_offset,
                        item.end_offset,
                        item.ordinal,
                    ),
                    event_type: NormalizedEventType::SessionDetected,
                    source_instance_id: identity::source_instance_id(source),
                    safe_cursor_id: Some(identity::cursor_id(source, item.end_offset)),
                    event_at: record.timestamp.clone(),
                    session_id: session_id.clone(),
                    payload: LifecyclePayload {
                        kind: LifecyclePayloadKind::Lifecycle,
                    },
                }));
            }
            if state.thread_configuration.codex_version.as_deref() != Some(&meta.cli_version)
                || state.thread_configuration.model_provider != meta.model_provider
            {
                state.thread_configuration.codex_version = Some(meta.cli_version.clone());
                state.thread_configuration.model_provider = meta.model_provider.clone();
            }
            emit_configuration(
                state,
                source,
                item,
                record.timestamp.as_str(),
                None,
                outputs,
            );
        }
        RolloutRecordKind::TurnContext(context) => {
            validate_timestamp(&record.timestamp)
                .map_err(TelemetryAssemblyError::InvalidTimestamp)?;
            apply_turn_context(state, context);
            let task_id = context.turn_id.as_deref().map(identity::task_id);
            if let Some(task_id) = &task_id {
                task_mut(state, task_id, context.root_turn_id.as_deref());
            }
            emit_configuration(
                state,
                source,
                item,
                record.timestamp.as_str(),
                task_id.as_deref(),
                outputs,
            );
        }
        RolloutRecordKind::EventMsg(event) => match event {
            EventMessage::ThreadSettingsApplied(settings) => {
                validate_timestamp(&record.timestamp)
                    .map_err(TelemetryAssemblyError::InvalidTimestamp)?;
                state.thread_configuration.model = Some(settings.model.clone());
                state.thread_configuration.model_provider = Some(settings.model_provider.clone());
                state.thread_configuration.reasoning_effort = settings.reasoning_effort.clone();
                state.thread_configuration.service_tier = settings.service_tier.clone();
                emit_configuration(
                    state,
                    source,
                    item,
                    record.timestamp.as_str(),
                    None,
                    outputs,
                );
            }
            EventMessage::TaskStarted(started) => {
                validate_timestamp(&record.timestamp)
                    .map_err(TelemetryAssemblyError::InvalidTimestamp)?;
                let task_id = identity::task_id(&started.turn_id);
                let root_task_id = started.root_turn_id.as_deref().map(identity::task_id);
                let task = task_mut(state, &task_id, started.root_turn_id.as_deref());
                let repeated = task.start_observed;
                task.start_observed = true;
                if repeated {
                    task.anomalies.insert(TaskAnomaly::RepeatedStart);
                }
                if matches!(task.lifecycle, TaskLifecycle::Observed) {
                    task.lifecycle = TaskLifecycle::Active;
                }
                outputs.push(AssembledOutput::TaskLifecycle(TaskLifecycleTransition {
                    evidence_id: identity::lifecycle_event_id(
                        source,
                        item.start_offset,
                        item.end_offset,
                        item.ordinal,
                    ),
                    task_id: Some(task_id),
                    root_task_id,
                    event_at: record.timestamp.clone(),
                    transition: TaskLifecycleTransitionKind::Started,
                }));
            }
            EventMessage::TaskCompleted(completed) => {
                validate_timestamp(&record.timestamp)
                    .map_err(TelemetryAssemblyError::InvalidTimestamp)?;
                let task_id = identity::task_id(&completed.turn_id);
                let task = task_mut(state, &task_id, None);
                let evidence = match completed.outcome {
                    TaskCompletion::Completed => TaskTerminalEvidence::Completed,
                    TaskCompletion::Failed => TaskTerminalEvidence::Failed,
                };
                terminal_update(task, evidence);
                outputs.push(AssembledOutput::TaskLifecycle(TaskLifecycleTransition {
                    evidence_id: identity::lifecycle_event_id(
                        source,
                        item.start_offset,
                        item.end_offset,
                        item.ordinal,
                    ),
                    task_id: Some(task_id),
                    root_task_id: task.root_task_id.clone(),
                    event_at: record.timestamp.clone(),
                    transition: match completed.outcome {
                        TaskCompletion::Completed => TaskLifecycleTransitionKind::Completed,
                        TaskCompletion::Failed => TaskLifecycleTransitionKind::Failed,
                    },
                }));
            }
            EventMessage::TurnAborted(aborted) => {
                validate_timestamp(&record.timestamp)
                    .map_err(TelemetryAssemblyError::InvalidTimestamp)?;
                let task_id = aborted.turn_id.as_deref().map(identity::task_id);
                let root_task_id = task_id
                    .as_ref()
                    .and_then(|id| state.tasks.get(id))
                    .and_then(|task| task.root_task_id.clone());
                if let Some(task_id_value) = &task_id {
                    let task = task_mut(state, task_id_value, None);
                    if !task.start_observed {
                        task.anomalies.insert(TaskAnomaly::AbortWithoutStart);
                    }
                    terminal_update(task, TaskTerminalEvidence::Aborted);
                }
                outputs.push(AssembledOutput::TaskLifecycle(TaskLifecycleTransition {
                    evidence_id: identity::lifecycle_event_id(
                        source,
                        item.start_offset,
                        item.end_offset,
                        item.ordinal,
                    ),
                    task_id,
                    root_task_id,
                    event_at: record.timestamp.clone(),
                    transition: TaskLifecycleTransitionKind::Aborted,
                }));
            }
            EventMessage::TokenCount(_) => {
                append_token_output(state, source, item, outputs)?;
            }
            EventMessage::ContextCompacted(_) | EventMessage::Unsupported { .. } => {}
        },
        RolloutRecordKind::TokenUsageRecord(_) => {
            append_token_output(state, source, item, outputs)?;
        }
        RolloutRecordKind::Unsupported(_) => {}
    }
    Ok(())
}

fn append_token_output(
    state: &mut TelemetryState,
    source: &SourceIdentity,
    item: &ReadItem,
    outputs: &mut Vec<AssembledOutput>,
) -> Result<(), TelemetryAssemblyError> {
    match normalize_token_item(source, item).map_err(TelemetryAssemblyError::TokenNormalization)? {
        TokenNormalizationOutcome::PerResponse {
            normalized_event, ..
        } => {
            let task_id = normalized_event.task_id.clone();
            let lifecycle = task_id
                .as_ref()
                .and_then(|id| state.tasks.get(id))
                .map(|task| task.lifecycle);
            if let Some(task_id) = &task_id {
                let defaults = state.thread_configuration.clone();
                let task_override = state
                    .tasks
                    .get(task_id)
                    .map_or_else(ConfigurationState::default, |task| {
                        task.configuration_override.clone()
                    });
                let configuration = effective_configuration(state, Some(task_id));
                let configuration_fingerprint = fingerprint(&defaults, &task_override);
                let task = task_mut(state, task_id, None);
                if matches!(
                    task.lifecycle,
                    TaskLifecycle::Completed
                        | TaskLifecycle::Failed
                        | TaskLifecycle::Aborted
                        | TaskLifecycle::ConflictingTerminalEvidence
                ) {
                    task.anomalies.insert(TaskAnomaly::TokenAfterTerminal);
                }
                task.token_configuration_fingerprints
                    .insert(configuration_fingerprint.clone());
                task.configuration_consistency = match task.token_configuration_fingerprints.len() {
                    0 => ConfigurationConsistency::NoTokenConsumption,
                    1 => ConfigurationConsistency::Consistent,
                    _ => ConfigurationConsistency::Mixed,
                };
                outputs.push(AssembledOutput::AttributedToken(AttributedTokenEvent {
                    normalized_event: *normalized_event,
                    effective_configuration: configuration,
                    configuration_fingerprint,
                    task_lifecycle: lifecycle,
                }));
            } else {
                outputs.push(AssembledOutput::AttributedToken(AttributedTokenEvent {
                    normalized_event: *normalized_event,
                    effective_configuration: effective_configuration(state, None),
                    configuration_fingerprint: fingerprint(
                        &state.thread_configuration,
                        &ConfigurationState::default(),
                    ),
                    task_lifecycle: lifecycle,
                }));
            }
        }
        TokenNormalizationOutcome::TokenCountSnapshot { raw_evidence } => {
            let record = match &item.outcome {
                ReadItemOutcome::Decoded(record) => record,
                ReadItemOutcome::Rejected(_) => return Ok(()),
            };
            outputs.push(AssembledOutput::TokenSnapshot(TokenSnapshotEvidence {
                evidence_id: identity::snapshot_evidence_id(
                    source,
                    item.start_offset,
                    item.end_offset,
                    item.ordinal,
                ),
                event_at: record.timestamp.clone(),
                evidence: *raw_evidence,
            }));
        }
        TokenNormalizationOutcome::NotTokenBearing => {}
        TokenNormalizationOutcome::RejectedSourceLine { reason } => {
            outputs.push(AssembledOutput::RejectedSourceLine { reason });
        }
    }
    Ok(())
}

fn apply_turn_context(state: &mut TelemetryState, context: &TurnContextRecord) {
    if let Some(turn_id) = &context.turn_id {
        let task = task_mut(
            state,
            &identity::task_id(turn_id),
            context.root_turn_id.as_deref(),
        );
        task.configuration_override.model = Some(context.model.clone());
        task.configuration_override.reasoning_effort = context.reasoning_effort.clone();
    }
}

fn emit_configuration(
    state: &TelemetryState,
    source: &SourceIdentity,
    item: &ReadItem,
    event_at: &str,
    task_id: Option<&str>,
    outputs: &mut Vec<AssembledOutput>,
) {
    let configuration = effective_configuration(state, task_id);
    outputs.push(AssembledOutput::ConfigurationEvidence(
        NormalizedConfigurationEvent {
            schema_version: SchemaVersion::V1,
            event_id: identity::configuration_event_id(
                source,
                item.start_offset,
                item.end_offset,
                item.ordinal,
            ),
            event_type: NormalizedEventType::ConfigurationEvidenceObserved,
            source_instance_id: identity::source_instance_id(source),
            safe_cursor_id: Some(identity::cursor_id(source, item.end_offset)),
            event_at: event_at.to_owned(),
            session_id: state
                .session
                .as_ref()
                .map(|session| session.session_id.clone()),
            task_id: task_id.map(str::to_owned),
            payload: ConfigurationPayload {
                kind: ConfigurationPayloadKind::Configuration,
                configuration,
            },
        },
    ));
}

fn effective_configuration(state: &TelemetryState, task_id: Option<&str>) -> ConfigurationIdentity {
    let mut model = state.thread_configuration.model.clone();
    let mut reasoning = state.thread_configuration.reasoning_effort.clone();
    if let Some(task) = task_id.and_then(|id| state.tasks.get(id)) {
        if task.configuration_override.model.is_some() {
            model = task.configuration_override.model.clone();
        }
        if task.configuration_override.reasoning_effort.is_some() {
            reasoning = task.configuration_override.reasoning_effort.clone();
        }
    }
    ConfigurationIdentity {
        plan: ConfigurationValue::unavailable(),
        model: model.map_or_else(
            ConfigurationValue::unavailable,
            ConfigurationValue::observed,
        ),
        reasoning_level: reasoning.map_or_else(
            ConfigurationValue::unavailable,
            ConfigurationValue::observed,
        ),
        speed_mode: ConfigurationValue::unavailable(),
        codex_version: state
            .thread_configuration
            .codex_version
            .clone()
            .map_or_else(
                ConfigurationValue::unavailable,
                ConfigurationValue::observed,
            ),
    }
}

fn fingerprint(defaults: &ConfigurationState, override_state: &ConfigurationState) -> String {
    identity::configuration_fingerprint(&[
        override_state
            .model
            .as_deref()
            .or(defaults.model.as_deref()),
        override_state
            .reasoning_effort
            .as_deref()
            .or(defaults.reasoning_effort.as_deref()),
        defaults.model_provider.as_deref(),
        defaults.service_tier.as_deref(),
        defaults.codex_version.as_deref(),
    ])
}

fn task_mut<'a>(
    state: &'a mut TelemetryState,
    task_id: &str,
    root_turn_id: Option<&str>,
) -> &'a mut TaskState {
    state
        .tasks
        .entry(task_id.to_owned())
        .or_insert_with(|| TaskState {
            task_id: task_id.to_owned(),
            root_task_id: root_turn_id.map(identity::task_id),
            lifecycle: TaskLifecycle::Observed,
            start_observed: false,
            terminal_evidence: BTreeSet::new(),
            anomalies: BTreeSet::new(),
            configuration_override: ConfigurationState::default(),
            token_configuration_fingerprints: BTreeSet::new(),
            configuration_consistency: ConfigurationConsistency::NoTokenConsumption,
        })
}

fn terminal_update(task: &mut TaskState, evidence: TaskTerminalEvidence) {
    if !task.start_observed {
        match evidence {
            TaskTerminalEvidence::Aborted => task.anomalies.insert(TaskAnomaly::AbortWithoutStart),
            TaskTerminalEvidence::Completed | TaskTerminalEvidence::Failed => {
                task.anomalies.insert(TaskAnomaly::CompletionWithoutStart)
            }
        };
    }
    if task
        .terminal_evidence
        .iter()
        .any(|existing| *existing != evidence)
    {
        task.anomalies
            .insert(TaskAnomaly::ConflictingTerminalEvidence);
        task.lifecycle = TaskLifecycle::ConflictingTerminalEvidence;
    } else if !matches!(task.lifecycle, TaskLifecycle::ConflictingTerminalEvidence) {
        task.lifecycle = match evidence {
            TaskTerminalEvidence::Completed => TaskLifecycle::Completed,
            TaskTerminalEvidence::Failed => TaskLifecycle::Failed,
            TaskTerminalEvidence::Aborted => TaskLifecycle::Aborted,
        };
    }
    task.terminal_evidence.insert(evidence);
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use crate::telemetry::codex_rollout::parse_rollout_record;
    use crate::telemetry::incremental_jsonl::{
        read_available, ReadItem, ReadItemOutcome, RolloutCursor,
    };

    fn item(input: &str, start: u64) -> ReadItem {
        let record = parse_rollout_record(input).expect("synthetic record should decode");
        ReadItem {
            start_offset: start,
            end_offset: start + input.len() as u64 + 1,
            ordinal: record.ordinal,
            outcome: ReadItemOutcome::Decoded(Box::new(record)),
        }
    }

    fn batch(items: Vec<ReadItem>) -> ReadBatch {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        ReadBatch {
            items,
            next_cursor: RolloutCursor::at_start(&source),
            has_incomplete_tail: false,
        }
    }

    fn fixture_batch(path: &str) -> (SourceIdentity, ReadBatch) {
        let source = SourceIdentity::new("assembly-fixture-rollout", "generation-a").unwrap();
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../",
            "fixtures/",
            "codex-rollout/v0.157.1/streams/",
            "session-task-flow.jsonl"
        ));
        let input = if path == "session-task-flow.jsonl" {
            input
        } else {
            panic!("unknown assembly fixture {path}");
        };
        let mut reader = Cursor::new(input.as_bytes().to_vec());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("assembly fixture should be readable");
        (source, batch)
    }

    #[test]
    fn empty_state_is_transactionally_replayable() {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/session-meta.json"
        ));
        let source_batch = batch(vec![item(input, 0)]);
        let first = assemble_batch(&TelemetryState::default(), &source, &source_batch).unwrap();
        let second = assemble_batch(&TelemetryState::default(), &source, &source_batch).unwrap();
        assert_eq!(first, second);
        assert!(first
            .outputs
            .iter()
            .any(|output| matches!(output, AssembledOutput::SessionDetected(_))));
    }

    #[test]
    fn completion_without_start_is_retained_without_fabricating_start() {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/task-complete.json"
        ));
        let result = assemble_batch(
            &TelemetryState::default(),
            &source,
            &batch(vec![item(input, 0)]),
        )
        .unwrap();
        let task = result.next_state.tasks.values().next().unwrap();
        assert!(!task.start_observed);
        assert!(task
            .anomalies
            .contains(&TaskAnomaly::CompletionWithoutStart));
        assert!(!result
            .outputs
            .iter()
            .any(|output| matches!(output, AssembledOutput::SessionDetected(_))));
    }

    #[test]
    fn normal_flow_detects_session_attributes_tokens_and_completes_task() {
        let (source, source_batch) = fixture_batch("session-task-flow.jsonl");
        let assembled = assemble_batch(&TelemetryState::default(), &source, &source_batch)
            .expect("normal flow should assemble");
        assert_eq!(
            assembled
                .outputs
                .iter()
                .filter(|output| matches!(output, AssembledOutput::SessionDetected(_)))
                .count(),
            1
        );
        assert!(assembled
            .outputs
            .iter()
            .any(|output| matches!(output, AssembledOutput::AttributedToken(_))));
        let token = assembled
            .outputs
            .iter()
            .find_map(|output| match output {
                AssembledOutput::AttributedToken(token) => Some(token),
                _ => None,
            })
            .expect("token output");
        assert_eq!(
            token.effective_configuration.model.value.as_deref(),
            Some("gpt-synthetic")
        );
        assert_eq!(
            token
                .effective_configuration
                .reasoning_level
                .value
                .as_deref(),
            Some("high")
        );
        assert_eq!(
            token.effective_configuration.codex_version.value.as_deref(),
            Some("0.157.1")
        );
        assert_eq!(
            token.effective_configuration.plan.availability,
            super::super::normalized::MetricAvailability::Unavailable
        );
        assert_eq!(
            token.effective_configuration.speed_mode.availability,
            super::super::normalized::MetricAvailability::Unavailable
        );
        let task = assembled
            .next_state
            .tasks
            .values()
            .next()
            .expect("task state");
        assert_eq!(task.lifecycle, TaskLifecycle::Completed);
        assert_eq!(
            task.configuration_consistency,
            ConfigurationConsistency::Consistent
        );
        let session_event = assembled
            .outputs
            .iter()
            .find_map(|output| match output {
                AssembledOutput::SessionDetected(event) => Some(event),
                _ => None,
            })
            .expect("session event");
        assert_eq!(
            session_event.event_type,
            NormalizedEventType::SessionDetected
        );
        assert_eq!(session_event.payload.kind, LifecyclePayloadKind::Lifecycle);
        let configuration_event = assembled
            .outputs
            .iter()
            .find_map(|output| match output {
                AssembledOutput::ConfigurationEvidence(event) => Some(event),
                _ => None,
            })
            .expect("configuration event");
        let serialized = serde_json::to_value(configuration_event).expect("configuration JSON");
        assert_eq!(serialized["payload"]["kind"], "configuration");
        assert_eq!(
            serialized["payload"]["configuration"]["plan"]["availability"],
            "unavailable"
        );
    }

    #[test]
    fn configuration_after_token_is_not_applied_retroactively() {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        let token = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/token-usage-record.json"
        ));
        let context = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/turn-context.json"
        ));
        let assembled = assemble_batch(
            &TelemetryState::default(),
            &source,
            &batch(vec![item(token, 0), item(context, 1000)]),
        )
        .expect("ordered configuration should assemble");
        let token = assembled
            .outputs
            .iter()
            .find_map(|output| match output {
                AssembledOutput::AttributedToken(token) => Some(token),
                _ => None,
            })
            .expect("token output");
        assert_eq!(
            token.effective_configuration.model.availability,
            super::super::normalized::MetricAvailability::Unavailable
        );
    }

    #[test]
    fn mixed_configuration_uses_only_token_fingerprints() {
        let source = SourceIdentity::new("assembly-mixed-rollout", "generation-a").unwrap();
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/streams/mixed-configuration.jsonl"
        ));
        let mut reader = Cursor::new(input.as_bytes().to_vec());
        let batch = read_available(&mut reader, &source, &RolloutCursor::at_start(&source))
            .expect("mixed fixture should be readable");
        let assembled = assemble_batch(&TelemetryState::default(), &source, &batch)
            .expect("mixed configuration should assemble");
        let task = assembled
            .next_state
            .tasks
            .values()
            .next()
            .expect("task state");
        assert_eq!(
            task.configuration_consistency,
            ConfigurationConsistency::Mixed
        );
        assert_eq!(task.token_configuration_fingerprints.len(), 2);
    }

    #[test]
    fn token_count_snapshot_does_not_create_attributed_consumption() {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        let input = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/token-count.json"
        ));
        let assembled = assemble_batch(
            &TelemetryState::default(),
            &source,
            &batch(vec![item(input, 0)]),
        )
        .expect("snapshot should assemble");
        assert!(assembled
            .outputs
            .iter()
            .any(|output| matches!(output, AssembledOutput::TokenSnapshot(_))));
        assert!(!assembled
            .outputs
            .iter()
            .any(|output| matches!(output, AssembledOutput::AttributedToken(_))));
    }

    #[test]
    fn conflicting_session_identity_rolls_back_candidate_state() {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        let first = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/session-meta.json"
        ));
        let second = first.replace("session-synthetic-001", "session-other-synthetic");
        let original = TelemetryState::default();
        let error = assemble_batch(
            &original,
            &source,
            &batch(vec![item(first, 0), item(&second, 1000)]),
        )
        .expect_err("conflicting sessions must fail");
        assert_eq!(error, TelemetryAssemblyError::SessionIdentityConflict);
        assert_eq!(original, TelemetryState::default());
    }

    #[test]
    fn conflicting_terminal_evidence_is_retained_and_token_after_terminal_is_not_dropped() {
        let source = SourceIdentity::new("assembly-test-rollout", "generation-a").unwrap();
        let started = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/task-started.json"
        ));
        let aborted = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/turn-aborted.json"
        ))
        .replace("turn-synthetic-003", "turn-synthetic-001");
        let completed = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/task-complete.json"
        ));
        let token = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/codex-rollout/v0.157.1/token-usage-record.json"
        ));
        let assembled = assemble_batch(
            &TelemetryState::default(),
            &source,
            &batch(vec![
                item(started, 0),
                item(&aborted, 100),
                item(completed, 200),
                item(token, 300),
            ]),
        )
        .expect("conflicting evidence should be retained");
        let task = assembled
            .next_state
            .tasks
            .values()
            .next()
            .expect("task state");
        assert_eq!(task.lifecycle, TaskLifecycle::ConflictingTerminalEvidence);
        assert!(task
            .anomalies
            .contains(&TaskAnomaly::ConflictingTerminalEvidence));
        assert!(task.anomalies.contains(&TaskAnomaly::TokenAfterTerminal));
        assert!(assembled
            .outputs
            .iter()
            .any(|output| matches!(output, AssembledOutput::AttributedToken(_))));
    }
}
