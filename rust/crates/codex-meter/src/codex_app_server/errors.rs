use std::fmt;

/// Typed failures from the installed Codex App Server integration.
///
/// Error values intentionally omit arbitrary server messages and process output:
/// those can contain paths or conversation data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexAppError {
    CodexCliUnavailable,
    AppServerUnsupported,
    UnsupportedExecutionRuntime,
    AppServerStartupFailure,
    InitializationFailure,
    AuthenticationUnavailable,
    UnsupportedMethod(&'static str),
    ProtocolVersionMismatch,
    MalformedResponse,
    RequestTimeout,
    ServerShutdown,
    UnexpectedProcessExit,
    ProcessIoFailure,
    RequestLimitExceeded,
    RequestIdExhausted,
    ResponseLimitExceeded,
    CriticalEventLoss,
    UnsafeExecutionConfiguration,
    InvalidExecutionTransition,
    MutationAlreadyPending,
    UnsupportedApprovalType,
    DuplicateApprovalRequest,
    WrongApprovalScope,
    StaleApprovalRequest,
    InvalidPreparedTask,
    SelectedModelUnavailable,
    UnsupportedReasoningEffort,
    ActiveThreadConflict,
    ExecutionPreflightChanged,
    UnknownThread,
    RemoteRequestRejected {
        method: &'static str,
        code: Option<i64>,
    },
}

impl CodexAppError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::CodexCliUnavailable => "codex_cli_unavailable",
            Self::AppServerUnsupported => "app_server_unsupported",
            Self::UnsupportedExecutionRuntime => "unsupported_execution_runtime",
            Self::AppServerStartupFailure => "app_server_startup_failure",
            Self::InitializationFailure => "initialization_failure",
            Self::AuthenticationUnavailable => "authentication_unavailable",
            Self::UnsupportedMethod(_) => "unsupported_method",
            Self::ProtocolVersionMismatch => "protocol_version_mismatch",
            Self::MalformedResponse => "malformed_response",
            Self::RequestTimeout => "request_timeout",
            Self::ServerShutdown => "server_shutdown",
            Self::UnexpectedProcessExit => "unexpected_process_exit",
            Self::ProcessIoFailure => "app_server_io_failure",
            Self::RequestLimitExceeded => "request_limit_exceeded",
            Self::RequestIdExhausted => "request_id_exhausted",
            Self::ResponseLimitExceeded => "app_server_response_limit_exceeded",
            Self::CriticalEventLoss => "critical_app_server_event_lost",
            Self::UnsafeExecutionConfiguration => "unsafe_execution_configuration",
            Self::InvalidExecutionTransition => "invalid_execution_transition",
            Self::MutationAlreadyPending => "execution_mutation_pending",
            Self::UnsupportedApprovalType => "unsupported_approval_type",
            Self::DuplicateApprovalRequest => "duplicate_approval_request",
            Self::WrongApprovalScope => "approval_scope_mismatch",
            Self::StaleApprovalRequest => "stale_approval_request",
            Self::InvalidPreparedTask => "invalid_prepared_task",
            Self::SelectedModelUnavailable => "selected_model_unavailable",
            Self::UnsupportedReasoningEffort => "unsupported_reasoning_effort",
            Self::ActiveThreadConflict => "active_thread_conflict",
            Self::ExecutionPreflightChanged => "execution_preflight_changed",
            Self::UnknownThread => "unknown_thread",
            Self::RemoteRequestRejected { .. } => "app_server_request_rejected",
        }
    }
}

impl fmt::Display for CodexAppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CodexCliUnavailable => write!(f, "Codex CLI was not found on PATH"),
            Self::AppServerUnsupported => {
                write!(f, "the installed Codex CLI does not support `app-server`")
            }
            Self::UnsupportedExecutionRuntime => write!(
                f,
                "this Codex App Server version has not been audited for task execution"
            ),
            Self::AppServerStartupFailure => write!(f, "Codex App Server failed to start"),
            Self::InitializationFailure => write!(f, "Codex App Server initialization failed"),
            Self::AuthenticationUnavailable => write!(
                f,
                "Codex App Server could not use Codex's existing authentication state"
            ),
            Self::UnsupportedMethod(method) => {
                write!(
                    f,
                    "the installed Codex App Server does not support `{method}`"
                )
            }
            Self::ProtocolVersionMismatch => write!(
                f,
                "Codex App Server returned a response outside the supported protocol shape"
            ),
            Self::MalformedResponse => write!(f, "Codex App Server returned a malformed message"),
            Self::RequestTimeout => {
                write!(f, "Codex App Server did not respond before the timeout")
            }
            Self::ServerShutdown => write!(f, "Codex App Server shut down during the request"),
            Self::UnexpectedProcessExit => {
                write!(f, "Codex App Server exited unexpectedly")
            }
            Self::ProcessIoFailure => write!(f, "communication with Codex App Server failed"),
            Self::RequestLimitExceeded => {
                write!(f, "too many Codex App Server requests are outstanding")
            }
            Self::RequestIdExhausted => {
                write!(
                    f,
                    "this Codex App Server connection exhausted its request IDs"
                )
            }
            Self::ResponseLimitExceeded => {
                write!(f, "Codex App Server exceeded the bounded response limit")
            }
            Self::CriticalEventLoss => write!(
                f,
                "Codex Meter could not preserve a critical App Server execution event"
            ),
            Self::UnsafeExecutionConfiguration => write!(
                f,
                "the current Codex sandbox or approval configuration could not be verified as safe"
            ),
            Self::InvalidExecutionTransition => {
                write!(
                    f,
                    "Codex App Server reported an invalid execution lifecycle transition"
                )
            }
            Self::MutationAlreadyPending => {
                write!(f, "an execution mutation is already pending")
            }
            Self::UnsupportedApprovalType => {
                write!(f, "Codex App Server requested an unsupported approval type")
            }
            Self::DuplicateApprovalRequest => {
                write!(f, "Codex App Server reused an approval request ID")
            }
            Self::WrongApprovalScope => {
                write!(
                    f,
                    "Codex App Server approval did not match the active thread and turn"
                )
            }
            Self::StaleApprovalRequest => {
                write!(f, "the approval request is no longer pending")
            }
            Self::InvalidPreparedTask => {
                write!(f, "the prepared task failed fresh execution validation")
            }
            Self::SelectedModelUnavailable => {
                write!(
                    f,
                    "the selected model is no longer in the live Codex catalog"
                )
            }
            Self::UnsupportedReasoningEffort => {
                write!(f, "the selected reasoning effort is no longer supported")
            }
            Self::ActiveThreadConflict => {
                write!(f, "the selected Codex thread already has an active turn")
            }
            Self::ExecutionPreflightChanged => {
                write!(
                    f,
                    "execution settings changed; review a fresh preflight before submitting"
                )
            }
            Self::UnknownThread => write!(f, "the requested thread is unavailable"),
            Self::RemoteRequestRejected { method, code } => {
                if let Some(code) = code {
                    write!(f, "Codex App Server rejected `{method}` (code {code})")
                } else {
                    write!(f, "Codex App Server rejected `{method}`")
                }
            }
        }
    }
}

impl std::error::Error for CodexAppError {}
