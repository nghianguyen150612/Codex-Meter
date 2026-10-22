//! Typed execution lifecycle, progress, and bounded presentation events.

use serde_json::Value;

use super::errors::CodexAppError;
use super::execution_protocol::{
    clean_user_text, required_string, TurnStatus, MAX_EVENT_TEXT_BYTES,
};
use super::transport::IncomingMessage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionPhase {
    Idle,
    Ready,
    AwaitingConfirmation,
    Submitting,
    ThreadCreated,
    TurnStarting,
    SubmissionAccepted,
    Running,
    AwaitingApproval,
    InterruptRequested,
    Reconciling,
    Completed,
    Failed,
    Interrupted,
    OutcomeUnknown,
}

impl ExecutionPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Ready => "Ready",
            Self::AwaitingConfirmation => "Awaiting confirmation",
            Self::Submitting => "Submitting",
            Self::ThreadCreated => "Thread created",
            Self::TurnStarting => "Starting turn",
            Self::SubmissionAccepted => "Submission accepted",
            Self::Running => "Running",
            Self::AwaitingApproval => "Awaiting approval",
            Self::InterruptRequested => "Interrupt requested",
            Self::Reconciling => "Reconciling",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::OutcomeUnknown => "Outcome unknown",
        }
    }

    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Interrupted)
    }
}

#[derive(Clone)]
pub struct ExecutionMachine {
    phase: ExecutionPhase,
    thread_id: Option<String>,
    turn_id: Option<String>,
    interrupt_sent: bool,
}

impl Default for ExecutionMachine {
    fn default() -> Self {
        Self {
            phase: ExecutionPhase::Idle,
            thread_id: None,
            turn_id: None,
            interrupt_sent: false,
        }
    }
}

impl ExecutionMachine {
    pub fn phase(&self) -> ExecutionPhase {
        self.phase
    }

    pub fn thread_id(&self) -> Option<&str> {
        self.thread_id.as_deref()
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    pub fn transition(&mut self, next: ExecutionPhase) -> Result<(), CodexAppError> {
        let allowed = matches!(
            (self.phase, next),
            (ExecutionPhase::Idle, ExecutionPhase::Ready)
                | (ExecutionPhase::Idle, ExecutionPhase::Failed)
                | (ExecutionPhase::Ready, ExecutionPhase::AwaitingConfirmation)
                | (ExecutionPhase::Ready, ExecutionPhase::Failed)
                | (ExecutionPhase::AwaitingConfirmation, ExecutionPhase::Ready)
                | (
                    ExecutionPhase::AwaitingConfirmation,
                    ExecutionPhase::OutcomeUnknown
                )
                | (
                    ExecutionPhase::AwaitingConfirmation,
                    ExecutionPhase::Submitting
                )
                | (ExecutionPhase::AwaitingConfirmation, ExecutionPhase::Failed)
                | (ExecutionPhase::Submitting, ExecutionPhase::ThreadCreated)
                | (ExecutionPhase::Submitting, ExecutionPhase::TurnStarting)
                | (ExecutionPhase::Submitting, ExecutionPhase::Ready)
                | (ExecutionPhase::Submitting, ExecutionPhase::OutcomeUnknown)
                | (ExecutionPhase::Submitting, ExecutionPhase::Failed)
                | (ExecutionPhase::ThreadCreated, ExecutionPhase::TurnStarting)
                | (ExecutionPhase::ThreadCreated, ExecutionPhase::Failed)
                | (
                    ExecutionPhase::ThreadCreated,
                    ExecutionPhase::OutcomeUnknown
                )
                | (
                    ExecutionPhase::TurnStarting,
                    ExecutionPhase::SubmissionAccepted
                )
                | (ExecutionPhase::TurnStarting, ExecutionPhase::Running)
                | (ExecutionPhase::TurnStarting, ExecutionPhase::Completed)
                | (ExecutionPhase::TurnStarting, ExecutionPhase::Failed)
                | (ExecutionPhase::TurnStarting, ExecutionPhase::Interrupted)
                | (ExecutionPhase::TurnStarting, ExecutionPhase::OutcomeUnknown)
                | (ExecutionPhase::SubmissionAccepted, ExecutionPhase::Running)
                | (
                    ExecutionPhase::SubmissionAccepted,
                    ExecutionPhase::AwaitingApproval
                )
                | (
                    ExecutionPhase::SubmissionAccepted,
                    ExecutionPhase::Reconciling
                )
                | (
                    ExecutionPhase::SubmissionAccepted,
                    ExecutionPhase::Completed
                )
                | (ExecutionPhase::SubmissionAccepted, ExecutionPhase::Failed)
                | (
                    ExecutionPhase::SubmissionAccepted,
                    ExecutionPhase::Interrupted
                )
                | (
                    ExecutionPhase::SubmissionAccepted,
                    ExecutionPhase::OutcomeUnknown
                )
                | (ExecutionPhase::Running, ExecutionPhase::AwaitingApproval)
                | (ExecutionPhase::Running, ExecutionPhase::InterruptRequested)
                | (ExecutionPhase::Running, ExecutionPhase::Reconciling)
                | (ExecutionPhase::Running, ExecutionPhase::Completed)
                | (ExecutionPhase::Running, ExecutionPhase::Failed)
                | (ExecutionPhase::Running, ExecutionPhase::Interrupted)
                | (ExecutionPhase::Running, ExecutionPhase::OutcomeUnknown)
                | (ExecutionPhase::AwaitingApproval, ExecutionPhase::Running)
                | (
                    ExecutionPhase::AwaitingApproval,
                    ExecutionPhase::InterruptRequested
                )
                | (
                    ExecutionPhase::AwaitingApproval,
                    ExecutionPhase::Reconciling
                )
                | (ExecutionPhase::AwaitingApproval, ExecutionPhase::Completed)
                | (ExecutionPhase::AwaitingApproval, ExecutionPhase::Failed)
                | (
                    ExecutionPhase::AwaitingApproval,
                    ExecutionPhase::Interrupted
                )
                | (
                    ExecutionPhase::AwaitingApproval,
                    ExecutionPhase::OutcomeUnknown
                )
                | (ExecutionPhase::InterruptRequested, ExecutionPhase::Running)
                | (
                    ExecutionPhase::InterruptRequested,
                    ExecutionPhase::AwaitingApproval
                )
                | (
                    ExecutionPhase::InterruptRequested,
                    ExecutionPhase::Reconciling
                )
                | (
                    ExecutionPhase::InterruptRequested,
                    ExecutionPhase::Completed
                )
                | (ExecutionPhase::InterruptRequested, ExecutionPhase::Failed)
                | (
                    ExecutionPhase::InterruptRequested,
                    ExecutionPhase::Interrupted
                )
                | (
                    ExecutionPhase::InterruptRequested,
                    ExecutionPhase::OutcomeUnknown
                )
                | (ExecutionPhase::OutcomeUnknown, ExecutionPhase::Reconciling)
                | (ExecutionPhase::Reconciling, ExecutionPhase::Running)
                | (ExecutionPhase::Reconciling, ExecutionPhase::Completed)
                | (ExecutionPhase::Reconciling, ExecutionPhase::Failed)
                | (ExecutionPhase::Reconciling, ExecutionPhase::Interrupted)
                | (ExecutionPhase::Reconciling, ExecutionPhase::OutcomeUnknown)
        );
        if !allowed {
            return Err(CodexAppError::InvalidExecutionTransition);
        }
        self.phase = next;
        Ok(())
    }

    pub fn set_thread(&mut self, thread_id: String) -> Result<(), CodexAppError> {
        if thread_id.is_empty() || thread_id.len() > 128 || thread_id.chars().any(char::is_control)
        {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        self.thread_id = Some(thread_id);
        Ok(())
    }

    pub fn set_turn(&mut self, turn_id: String) -> Result<(), CodexAppError> {
        if turn_id.is_empty() || turn_id.len() > 128 || turn_id.chars().any(char::is_control) {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        self.turn_id = Some(turn_id);
        Ok(())
    }

    pub fn request_interrupt(&mut self) -> Result<(), CodexAppError> {
        if self.interrupt_sent {
            return Err(CodexAppError::MutationAlreadyPending);
        }
        self.transition(ExecutionPhase::InterruptRequested)?;
        self.interrupt_sent = true;
        Ok(())
    }

    pub fn matches_active(&self, thread_id: &str, turn_id: Option<&str>) -> bool {
        self.thread_id.as_deref() == Some(thread_id)
            && turn_id.is_none_or(|turn_id| self.turn_id.as_deref() == Some(turn_id))
    }

    pub fn finish(&mut self, status: TurnStatus) -> Result<(), CodexAppError> {
        let next = match status {
            TurnStatus::Completed => ExecutionPhase::Completed,
            TurnStatus::Failed => ExecutionPhase::Failed,
            TurnStatus::Interrupted => ExecutionPhase::Interrupted,
            TurnStatus::InProgress => return Err(CodexAppError::ProtocolVersionMismatch),
        };
        self.transition(next)
    }
}

pub(crate) enum ExecutionNotification {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted {
        thread_id: String,
        turn_id: String,
    },
    TurnCompleted {
        thread_id: String,
        turn_id: String,
        status: TurnStatus,
        failure_summary: Option<String>,
        final_agent_message: Option<String>,
    },
    ItemStarted {
        thread_id: String,
        turn_id: String,
        item: String,
    },
    ItemCompleted {
        thread_id: String,
        turn_id: String,
        item: String,
    },
    AgentMessageCompleted {
        thread_id: String,
        turn_id: String,
        item_id: String,
        text: String,
    },
    AgentDelta {
        thread_id: String,
        turn_id: String,
        item_id: String,
        text: String,
    },
    ToolOutput {
        thread_id: String,
        turn_id: String,
        item_id: String,
        text: String,
    },
    DiffUpdated {
        thread_id: String,
        turn_id: String,
        diff: String,
    },
    PlanUpdated {
        thread_id: String,
        turn_id: String,
    },
    ThreadStatus {
        thread_id: String,
        status: String,
    },
    RequestResolved {
        thread_id: String,
        request_id: String,
    },
}

impl ExecutionNotification {
    pub(crate) fn parse(incoming: IncomingMessage) -> Result<Option<Self>, CodexAppError> {
        let IncomingMessage::Notification { method, params } = incoming else {
            return Ok(None);
        };
        let event = match method.as_str() {
            "thread/started" => Self::ThreadStarted {
                thread_id: bounded_id(
                    params
                        .get("thread")
                        .ok_or(CodexAppError::ProtocolVersionMismatch)?,
                    "id",
                )?,
            },
            "turn/started" => {
                let (thread_id, turn_id, _) = parse_turn(&params)?;
                Self::TurnStarted { thread_id, turn_id }
            }
            "turn/completed" => {
                let (thread_id, turn_id, status) = parse_turn(&params)?;
                let turn = params
                    .get("turn")
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                Self::TurnCompleted {
                    thread_id,
                    turn_id,
                    status,
                    failure_summary: turn_failure_summary(turn),
                    final_agent_message: final_agent_message(turn),
                }
            }
            "item/started" | "item/completed" => {
                let thread_id = bounded_id(&params, "threadId")?;
                let turn_id = bounded_id(&params, "turnId")?;
                let item = params
                    .get("item")
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                let kind = bounded_id(item, "type")?;
                let id = bounded_id(item, "id")?;
                if method == "item/completed" && kind == "agentMessage" {
                    let text = required_string(item, "text", MAX_EVENT_TEXT_BYTES)?;
                    return Ok(Some(Self::AgentMessageCompleted {
                        thread_id,
                        turn_id,
                        item_id: id,
                        text: super::execution_protocol::clean_user_text(
                            text,
                            MAX_EVENT_TEXT_BYTES,
                        ),
                    }));
                }
                let label = format!("{kind} · {id}");
                if method == "item/started" {
                    Self::ItemStarted {
                        thread_id,
                        turn_id,
                        item: label,
                    }
                } else {
                    Self::ItemCompleted {
                        thread_id,
                        turn_id,
                        item: label,
                    }
                }
            }
            "item/agentMessage/delta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta" => {
                let thread_id = bounded_id(&params, "threadId")?;
                let turn_id = bounded_id(&params, "turnId")?;
                let item_id = bounded_id(&params, "itemId")?;
                let raw = required_string(&params, "delta", MAX_EVENT_TEXT_BYTES)?;
                let text = clean_user_text(raw, MAX_EVENT_TEXT_BYTES);
                if method == "item/agentMessage/delta" {
                    Self::AgentDelta {
                        thread_id,
                        turn_id,
                        item_id,
                        text,
                    }
                } else {
                    Self::ToolOutput {
                        thread_id,
                        turn_id,
                        item_id,
                        text,
                    }
                }
            }
            "turn/diff/updated" => {
                let thread_id = bounded_id(&params, "threadId")?;
                let turn_id = bounded_id(&params, "turnId")?;
                let raw = required_string(&params, "diff", MAX_EVENT_TEXT_BYTES)?;
                Self::DiffUpdated {
                    thread_id,
                    turn_id,
                    diff: clean_user_text(raw, MAX_EVENT_TEXT_BYTES),
                }
            }
            "turn/plan/updated" => Self::PlanUpdated {
                thread_id: bounded_id(&params, "threadId")?,
                turn_id: bounded_id(&params, "turnId")?,
            },
            "thread/status/changed" => Self::ThreadStatus {
                thread_id: bounded_id(&params, "threadId")?,
                status: params
                    .get("status")
                    .and_then(|value| value.get("type"))
                    .and_then(Value::as_str)
                    .filter(|value| value.len() <= 64)
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?
                    .to_owned(),
            },
            "serverRequest/resolved" => Self::RequestResolved {
                thread_id: bounded_id(&params, "threadId")?,
                request_id: params
                    .get("requestId")
                    .filter(|id| super::protocol::valid_request_id(id))
                    .map(request_id_key)
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?,
            },
            _ => return Ok(None),
        };
        Ok(Some(event))
    }
}

pub(crate) fn turn_failure_summary(turn: &Value) -> Option<String> {
    let error = turn.get("error")?;
    let kind = error.get("codexErrorInfo")?;
    let key = if let Some(value) = kind.as_str() {
        value
    } else {
        kind.as_object()?.keys().next()?.as_str()
    };
    Some(match key {
        "contextWindowExceeded" => "Codex reached the context window limit.".to_owned(),
        "sessionBudgetExceeded" => "Codex reached the session budget limit.".to_owned(),
        "usageLimitExceeded" | "rateLimitExceeded" => {
            "Codex reported a usage or rate limit.".to_owned()
        }
        "unauthorized" => "Codex authentication was rejected.".to_owned(),
        "sandboxError" => "Codex reported a sandbox restriction.".to_owned(),
        "serverOverloaded" => "The Codex service reported temporary overload.".to_owned(),
        "flexUnavailable" => "The requested Codex service tier was unavailable.".to_owned(),
        "responseStreamDisconnected" | "responseStreamConnectionFailed" => {
            "The Codex response stream disconnected before completion.".to_owned()
        }
        "httpConnectionFailed" => "Codex could not connect to the provider.".to_owned(),
        "cyberPolicy" | "misalignmentPolicyViolation" => {
            "Codex stopped the task due to a safety policy.".to_owned()
        }
        "other" | "internalServerError" | "badRequest" | "tooManyDenials" => {
            "Codex reported a task failure; additional details are unavailable.".to_owned()
        }
        _ => "Codex reported an unrecognized task failure.".to_owned(),
    })
}

pub(crate) fn final_agent_message(turn: &Value) -> Option<String> {
    turn.get("items")?
        .as_array()?
        .iter()
        .rev()
        .take(128)
        .find(|item| item.get("type").and_then(Value::as_str) == Some("agentMessage"))
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
        .map(|text| clean_user_text(text, MAX_EVENT_TEXT_BYTES))
        .filter(|text| !text.is_empty())
}

fn parse_turn(params: &Value) -> Result<(String, String, TurnStatus), CodexAppError> {
    let thread_id = bounded_id(params, "threadId")?;
    let turn = params
        .get("turn")
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    let turn_id = bounded_id(turn, "id")?;
    let status = TurnStatus::parse(
        turn.get("status")
            .ok_or(CodexAppError::ProtocolVersionMismatch)?,
    )?;
    Ok((thread_id, turn_id, status))
}

fn bounded_id(value: &Value, key: &str) -> Result<String, CodexAppError> {
    required_string(value, key, 128).map(str::to_owned)
}

pub(crate) fn request_id_key(id: &Value) -> String {
    match id {
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn state_machine_rejects_completion_without_turn_start_or_acceptance() {
        let mut machine = ExecutionMachine::default();
        assert_eq!(
            machine.transition(ExecutionPhase::Completed),
            Err(CodexAppError::InvalidExecutionTransition)
        );
    }

    #[test]
    fn lifecycle_requires_authoritative_terminal_status() {
        let mut machine = ExecutionMachine::default();
        machine.transition(ExecutionPhase::Ready).unwrap();
        machine
            .transition(ExecutionPhase::AwaitingConfirmation)
            .unwrap();
        machine.transition(ExecutionPhase::Submitting).unwrap();
        machine.transition(ExecutionPhase::TurnStarting).unwrap();
        machine
            .transition(ExecutionPhase::SubmissionAccepted)
            .unwrap();
        machine.transition(ExecutionPhase::Running).unwrap();
        assert_eq!(machine.phase(), ExecutionPhase::Running);
        machine.finish(TurnStatus::Interrupted).unwrap();
        assert_eq!(machine.phase(), ExecutionPhase::Interrupted);
    }

    #[test]
    fn notifications_retain_thread_turn_and_item_association() {
        let event = ExecutionNotification::parse(IncomingMessage::Notification {
            method: "item/agentMessage/delta".to_owned(),
            params: json!({"threadId":"thread-a","turnId":"turn-a","itemId":"item-a","delta":"hello"}),
        })
        .unwrap();
        assert!(
            matches!(event, Some(ExecutionNotification::AgentDelta { thread_id, turn_id, item_id, text }) if thread_id == "thread-a" && turn_id == "turn-a" && item_id == "item-a" && text == "hello")
        );
    }

    #[test]
    fn turn_started_notification_does_not_claim_completion() {
        let event = ExecutionNotification::parse(IncomingMessage::Notification {
            method: "turn/started".to_owned(),
            params: json!({"threadId":"thread-a","turn":{"id":"turn-a","status":"inProgress"}}),
        })
        .unwrap();
        assert!(matches!(
            event,
            Some(ExecutionNotification::TurnStarted { .. })
        ));
    }

    #[test]
    fn terminal_turn_uses_only_the_last_user_facing_agent_message() {
        let event = ExecutionNotification::parse(IncomingMessage::Notification {
            method: "turn/completed".to_owned(),
            params: json!({
                "threadId":"thread-a",
                "turn":{"id":"turn-a","status":"completed","items":[
                    {"id":"reasoning-a","type":"reasoning","text":"INTERNAL_REASONING_SENTINEL"},
                    {"id":"message-a","type":"agentMessage","text":"First response"},
                    {"id":"message-b","type":"agentMessage","text":"Final response"}
                ]}
            }),
        })
        .unwrap();
        assert!(matches!(
            event,
            Some(ExecutionNotification::TurnCompleted {
                thread_id,
                turn_id,
                status: TurnStatus::Completed,
                final_agent_message: Some(text),
                ..
            }) if thread_id == "thread-a" && turn_id == "turn-a" && text == "Final response"
        ));
        assert_eq!(
            final_agent_message(&json!({"items":[
                {"type":"reasoning","text":"INTERNAL_REASONING_SENTINEL"}
            ]})),
            None
        );
    }
}
