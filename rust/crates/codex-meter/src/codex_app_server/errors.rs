use std::fmt;

/// Typed failures from the installed Codex App Server integration.
///
/// Error values intentionally omit arbitrary server messages and process output:
/// those can contain paths or conversation data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CodexAppError {
    CodexCliUnavailable,
    AppServerUnsupported,
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
