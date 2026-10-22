//! Stable App Server execution shapes audited against Codex CLI 0.163.0.
//!
//! These parsers intentionally select only identifiers, lifecycle status,
//! security configuration, and presentation content. Raw protocol values are
//! never retained by the execution controller.

use serde_json::Value;
use std::path::{Path, PathBuf};

use super::errors::CodexAppError;

pub(crate) const MAX_THREAD_ID_BYTES: usize = 128;
pub(crate) const MAX_EVENT_TEXT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RENDERED_OUTPUT_BYTES: usize = 256 * 1024;
pub(crate) const MAX_ACTIVITY_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TurnStatus {
    InProgress,
    Completed,
    Failed,
    Interrupted,
}

impl TurnStatus {
    pub(crate) fn parse(value: &Value) -> Result<Self, CodexAppError> {
        match value.as_str() {
            Some("inProgress") => Ok(Self::InProgress),
            Some("completed") => Ok(Self::Completed),
            Some("failed") => Ok(Self::Failed),
            Some("interrupted") => Ok(Self::Interrupted),
            _ => Err(CodexAppError::ProtocolVersionMismatch),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxMode {
    ReadOnly,
    WorkspaceWrite,
}

impl SandboxMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::WorkspaceWrite => "Workspace write",
        }
    }

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApprovalPolicy {
    Untrusted,
    OnRequest,
}

impl ApprovalPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Untrusted => "Untrusted",
            Self::OnRequest => "On request",
        }
    }

    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::Untrusted => "untrusted",
            Self::OnRequest => "on-request",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedSecurity {
    pub sandbox: SandboxMode,
    pub approval: ApprovalPolicy,
    pub network_access: bool,
}

impl VerifiedSecurity {
    pub(crate) fn from_effective_config(response: &Value) -> Result<Self, CodexAppError> {
        let config = response
            .get("config")
            .and_then(Value::as_object)
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let sandbox = match config.get("sandbox_mode").and_then(Value::as_str) {
            Some("read-only") => SandboxMode::ReadOnly,
            Some("workspace-write") => SandboxMode::WorkspaceWrite,
            Some("danger-full-access") => return Err(CodexAppError::UnsafeExecutionConfiguration),
            _ => return Err(CodexAppError::UnsafeExecutionConfiguration),
        };
        let approval = match config.get("approval_policy").and_then(Value::as_str) {
            Some("untrusted") => ApprovalPolicy::Untrusted,
            Some("on-request") => ApprovalPolicy::OnRequest,
            Some("never") => return Err(CodexAppError::UnsafeExecutionConfiguration),
            _ => return Err(CodexAppError::UnsafeExecutionConfiguration),
        };
        let network_access = if sandbox == SandboxMode::ReadOnly {
            false
        } else {
            let workspace_write = config
                .get("sandbox_workspace_write")
                .and_then(Value::as_object)
                .ok_or(CodexAppError::UnsafeExecutionConfiguration)?;
            if let Some(roots) = workspace_write.get("writable_roots") {
                if !roots.as_array().is_some_and(|roots| roots.is_empty()) {
                    return Err(CodexAppError::UnsafeExecutionConfiguration);
                }
            }
            workspace_write
                .get("network_access")
                .and_then(Value::as_bool)
                .ok_or(CodexAppError::UnsafeExecutionConfiguration)?
        };
        if network_access {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        Ok(Self {
            sandbox,
            approval,
            network_access,
        })
    }

    pub(crate) fn from_thread_response(response: &Value) -> Result<Self, CodexAppError> {
        if required_string(response, "approvalsReviewer", 64)? != "user" {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        let approval = match response.get("approvalPolicy").and_then(Value::as_str) {
            Some("untrusted") => ApprovalPolicy::Untrusted,
            Some("on-request") => ApprovalPolicy::OnRequest,
            Some("never") | None => return Err(CodexAppError::UnsafeExecutionConfiguration),
            Some(_) => return Err(CodexAppError::UnsafeExecutionConfiguration),
        };
        let sandbox = response
            .get("sandbox")
            .and_then(Value::as_object)
            .ok_or(CodexAppError::UnsafeExecutionConfiguration)?;
        let network_access = match sandbox.get("networkAccess") {
            None => false,
            Some(value) => value
                .as_bool()
                .ok_or(CodexAppError::UnsafeExecutionConfiguration)?,
        };
        let mode = match sandbox.get("type").and_then(Value::as_str) {
            Some("readOnly") => SandboxMode::ReadOnly,
            Some("workspaceWrite") => SandboxMode::WorkspaceWrite,
            Some("dangerFullAccess") | Some("externalSandbox") | None => {
                return Err(CodexAppError::UnsafeExecutionConfiguration)
            }
            Some(_) => return Err(CodexAppError::UnsafeExecutionConfiguration),
        };
        if network_access {
            return Err(CodexAppError::UnsafeExecutionConfiguration);
        }
        if mode == SandboxMode::WorkspaceWrite {
            if let Some(roots) = sandbox.get("writableRoots") {
                if !roots.as_array().is_some_and(|roots| roots.is_empty()) {
                    return Err(CodexAppError::UnsafeExecutionConfiguration);
                }
            }
        }
        Ok(Self {
            sandbox: mode,
            approval,
            network_access,
        })
    }
}

pub(crate) fn validate_thread_id(value: &str) -> Result<(), CodexAppError> {
    if value.is_empty() || value.len() > MAX_THREAD_ID_BYTES || value.chars().any(char::is_control)
    {
        return Err(CodexAppError::ProtocolVersionMismatch);
    }
    Ok(())
}

pub(crate) fn required_string<'a>(
    value: &'a Value,
    key: &str,
    max_bytes: usize,
) -> Result<&'a str, CodexAppError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= max_bytes && !text.contains('\0'))
        .ok_or(CodexAppError::ProtocolVersionMismatch)
}

pub(crate) fn resolve_workspace(path: &Path) -> Result<PathBuf, CodexAppError> {
    let resolved =
        std::fs::canonicalize(path).map_err(|_| CodexAppError::UnsafeExecutionConfiguration)?;
    if !resolved.is_dir() {
        return Err(CodexAppError::UnsafeExecutionConfiguration);
    }
    Ok(resolved)
}

pub(crate) fn clean_user_text(value: &str, max_bytes: usize) -> String {
    let mut result = String::new();
    for character in value.chars() {
        if character.is_control() && character != '\n' && character != '\t' {
            continue;
        }
        if result.len().saturating_add(character.len_utf8()) > max_bytes {
            break;
        }
        result.push(character);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn security_parser_rejects_widened_or_unknown_configuration() {
        for config in [
            json!({"approval_policy":"never","sandbox_mode":"workspace-write","sandbox_workspace_write":{"network_access":false}}),
            json!({"approval_policy":"on-request","sandbox_mode":"danger-full-access"}),
            json!({"approval_policy":"on-request","sandbox_mode":"workspace-write","sandbox_workspace_write":{"network_access":true}}),
            json!({"approval_policy":"on-request","sandbox_mode":"workspace-write"}),
        ] {
            assert_eq!(
                VerifiedSecurity::from_effective_config(&json!({"config":config})),
                Err(CodexAppError::UnsafeExecutionConfiguration)
            );
        }
    }

    #[test]
    fn security_parser_accepts_only_bounded_workspace_policies() {
        let security = VerifiedSecurity::from_effective_config(&json!({"config":{
            "approval_policy":"on-request",
            "sandbox_mode":"workspace-write",
            "sandbox_workspace_write":{"network_access":false}
        }}))
        .unwrap();
        assert_eq!(security.sandbox, SandboxMode::WorkspaceWrite);
        assert_eq!(security.approval, ApprovalPolicy::OnRequest);
        assert!(!security.network_access);
    }

    #[test]
    fn thread_response_rejects_extra_writable_roots_and_accepts_verified_policy() {
        let broad = json!({"approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite","networkAccess":false,"writableRoots":["/tmp"]}});
        assert_eq!(
            VerifiedSecurity::from_thread_response(&broad),
            Err(CodexAppError::UnsafeExecutionConfiguration)
        );
        let safe = json!({"approvalPolicy":"untrusted","approvalsReviewer":"user","sandbox":{"type":"workspaceWrite","networkAccess":false,"writableRoots":[]}});
        assert_eq!(
            VerifiedSecurity::from_thread_response(&safe)
                .unwrap()
                .sandbox,
            SandboxMode::WorkspaceWrite
        );
    }

    #[test]
    fn thread_response_rejects_automatic_approval_reviewer() {
        let response = json!({
            "approvalPolicy":"on-request",
            "approvalsReviewer":"auto_review",
            "sandbox":{"type":"readOnly","networkAccess":false}
        });
        assert_eq!(
            VerifiedSecurity::from_thread_response(&response),
            Err(CodexAppError::UnsafeExecutionConfiguration)
        );
    }

    #[test]
    fn terminal_control_sequences_are_removed_from_display_text() {
        assert_eq!(
            clean_user_text("ok\u{1b}[31mred\u{1b}[0m", 32),
            "ok[31mred[0m"
        );
    }
}
