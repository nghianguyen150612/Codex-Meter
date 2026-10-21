use super::errors::CodexAppError;
use super::protocol::{CLIENT_NAME, CLIENT_TITLE, REQUEST_TIMEOUT_SECS};
use super::transport::ProcessTransport;
use serde_json::{json, Value};
use std::ffi::OsStr;
#[cfg(test)]
use std::ffi::OsString;
#[cfg(test)]
use std::path::PathBuf;
use std::time::Duration;

/// Server facts returned by `initialize`. The Codex home path is validated and
/// immediately discarded; it is never exposed through this interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppServerInfo {
    pub user_agent: String,
    pub platform_family: String,
    pub platform_os: String,
}

/// An initialized local Codex App Server connection over its default stdio
/// transport.
///
/// This client identifies itself as Codex Meter and deliberately exposes only
/// the verified read and lifecycle operations in this crate. It has no API for
/// starting or steering model turns.
pub struct CodexAppServer {
    transport: ProcessTransport,
    info: AppServerInfo,
}

impl CodexAppServer {
    /// Starts `codex app-server` from PATH without invoking a shell.
    pub fn connect() -> Result<Self, CodexAppError> {
        Self::connect_with_program(OsStr::new("codex"))
    }

    /// Starts an installed Codex executable without invoking a shell.
    ///
    /// This is useful to applications embedding the reusable session service
    /// when the Codex executable is not named `codex` on PATH.
    pub fn connect_with_program(program: &OsStr) -> Result<Self, CodexAppError> {
        let args = [OsStr::new("app-server")];
        let transport = ProcessTransport::spawn(program, &args)?;
        Self::initialize_transport(transport)
    }

    fn initialize_transport(transport: ProcessTransport) -> Result<Self, CodexAppError> {
        let result = transport
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": CLIENT_NAME,
                        "title": CLIENT_TITLE,
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
                Duration::from_secs(REQUEST_TIMEOUT_SECS),
            )
            .map_err(initialization_error)?;
        let info = parse_initialize_response(&result)?;
        transport
            .notify("initialized", json!({}))
            .map_err(initialization_error)?;
        transport.mark_initialized();
        Ok(Self { transport, info })
    }

    #[cfg(test)]
    pub(super) fn connect_test_server(mode: &str) -> Result<Self, CodexAppError> {
        let program = std::env::var_os("PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                OsString::from("python")
            } else {
                OsString::from("python3")
            }
        });
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_app_server.py");
        let args = [script.as_os_str(), OsStr::new(mode)];
        let transport = ProcessTransport::spawn(&program, &args)?;
        Self::initialize_transport(transport)
    }

    /// Returns the protocol metadata advertised by this app-server process.
    pub fn info(&self) -> &AppServerInfo {
        &self.info
    }

    /// Returns the next notification method name, if one is queued.
    ///
    /// Notification payloads are discarded because they can contain prompts,
    /// tool data, or filesystem details.
    pub fn try_next_notification(&self) -> Option<String> {
        self.transport.try_notification()
    }

    /// Number of notification method names dropped after the bounded queue
    /// reached capacity.
    pub fn dropped_notification_count(&self) -> u64 {
        self.transport.dropped_notifications()
    }

    pub(super) fn call(&self, method: &'static str, params: Value) -> Result<Value, CodexAppError> {
        self.transport
            .request(method, params, Duration::from_secs(REQUEST_TIMEOUT_SECS))
    }
}

fn parse_initialize_response(value: &Value) -> Result<AppServerInfo, CodexAppError> {
    let object = value
        .as_object()
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    let required = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or(CodexAppError::ProtocolVersionMismatch)
    };
    // Validate presence and type, then discard this sensitive absolute path.
    let _codex_home = required("codexHome")?;
    Ok(AppServerInfo {
        user_agent: required("userAgent")?,
        platform_family: required("platformFamily")?,
        platform_os: required("platformOs")?,
    })
}

fn initialization_error(error: CodexAppError) -> CodexAppError {
    match error {
        CodexAppError::AppServerUnsupported
        | CodexAppError::AppServerStartupFailure
        | CodexAppError::AuthenticationUnavailable
        | CodexAppError::RequestTimeout
        | CodexAppError::ProtocolVersionMismatch => error,
        CodexAppError::UnsupportedMethod("initialize") => CodexAppError::AppServerUnsupported,
        _ => CodexAppError::InitializationFailure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_handshake_keeps_only_bounded_non_path_metadata() {
        let client = CodexAppServer::connect_test_server("sessions").unwrap();
        assert_eq!(client.info().user_agent, "codex_cli_rs/test");
        assert_eq!(client.info().platform_family, "unix");
        assert_eq!(client.info().platform_os, "linux");
        let encoded = format!("{:?}", client.info());
        assert!(!encoded.contains("/not-retained"));
    }

    #[test]
    fn initialization_errors_are_classified_without_server_messages() {
        for (mode, expected) in [
            (
                "unsupported_initialize",
                CodexAppError::AppServerUnsupported,
            ),
            (
                "initialization_failure",
                CodexAppError::InitializationFailure,
            ),
            (
                "authentication_unavailable",
                CodexAppError::AuthenticationUnavailable,
            ),
            (
                "initialization_malformed",
                CodexAppError::ProtocolVersionMismatch,
            ),
            ("startup_failure", CodexAppError::AppServerStartupFailure),
        ] {
            let error = CodexAppServer::connect_test_server(mode)
                .err()
                .expect("initialization should fail");
            assert_eq!(error, expected, "mode: {mode}");
            assert!(!error.to_string().contains("unauthorized"));
            assert!(!error.to_string().contains("not-retained"));
        }
    }

    #[test]
    fn a_missing_explicit_codex_executable_is_typed() {
        let missing = std::env::temp_dir().join(format!(
            "codex-meter-p033-no-such-executable-{}",
            std::process::id()
        ));
        assert_eq!(
            CodexAppServer::connect_with_program(missing.as_os_str()).err(),
            Some(CodexAppError::CodexCliUnavailable)
        );
    }
}
