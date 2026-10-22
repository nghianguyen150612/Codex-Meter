//! Bounded, explicit broker for the three stable execution approval methods.

use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

use super::errors::CodexAppError;
use super::execution_events::request_id_key;
use super::execution_protocol::{clean_user_text, required_string};
use super::protocol::valid_request_id;

const MAX_PENDING_APPROVALS: usize = 32;
const MAX_SEEN_APPROVAL_IDS: usize = 1024;
const MAX_APPROVAL_TEXT_BYTES: usize = 16 * 1024;
const MAX_SCOPE_ENTRIES: usize = 64;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum ApprovalKind {
    CommandExecution,
    FileChange,
    Permissions,
}

impl ApprovalKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::CommandExecution => "Command execution",
            Self::FileChange => "File change",
            Self::Permissions => "Permission request",
        }
    }
}

#[derive(Clone, Copy)]
pub enum ApprovalChoice {
    AllowOnce,
    Deny,
    Cancel,
}

/// Safe, bounded details for a transient approval dialog. It intentionally
/// has no `Debug` implementation because the values can contain commands and
/// private paths.
#[derive(Clone, Eq, PartialEq)]
pub struct ApprovalView {
    pub request_id: String,
    pub kind: ApprovalKind,
    pub requested_action: String,
    pub working_context: Option<String>,
    pub details: String,
    pub allow_once_available: bool,
    pub network_request: bool,
}

struct PendingApproval {
    server_id: Value,
    view: ApprovalView,
    permission_subset: Option<Value>,
    resolved: bool,
}

pub struct ApprovalBroker {
    expected_thread_id: String,
    expected_turn_id: String,
    pending: HashMap<String, PendingApproval>,
    seen_ids: HashSet<String>,
}

impl ApprovalBroker {
    pub fn new(thread_id: String, turn_id: String) -> Self {
        Self {
            expected_thread_id: thread_id,
            expected_turn_id: turn_id,
            pending: HashMap::new(),
            seen_ids: HashSet::new(),
        }
    }

    pub fn pending(&self) -> impl Iterator<Item = &ApprovalView> {
        self.pending.values().map(|value| &value.view)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn receive(
        &mut self,
        server_id: Value,
        method: &str,
        params: &Value,
        current_diff: &str,
    ) -> Result<ApprovalView, CodexAppError> {
        if !valid_request_id(&server_id) {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        let request_id = request_id_key(&server_id);
        if self.seen_ids.contains(&request_id) {
            return Err(CodexAppError::DuplicateApprovalRequest);
        }
        if self.seen_ids.len() >= MAX_SEEN_APPROVAL_IDS {
            return Err(CodexAppError::ResponseLimitExceeded);
        }
        self.seen_ids.insert(request_id.clone());
        if self.pending.len() >= MAX_PENDING_APPROVALS {
            return Err(CodexAppError::ResponseLimitExceeded);
        }
        let thread_id = bounded_required(params, "threadId")?;
        let turn_id = bounded_required(params, "turnId")?;
        if thread_id != self.expected_thread_id || turn_id != self.expected_turn_id {
            return Err(CodexAppError::WrongApprovalScope);
        }
        let item_id = bounded_required(params, "itemId")?;
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .map(|value| clean_user_text(value, 1024))
            .filter(|value| !value.is_empty());
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|value| value.len() <= MAX_APPROVAL_TEXT_BYTES && safe_scope_text(value))
            .map(|value| clean_user_text(value, 4096));

        let (
            kind,
            requested_action,
            details,
            allow_once_available,
            network_request,
            permission_subset,
        ) = match method {
            "item/commandExecution/requestApproval" => {
                let network = params.get("networkApprovalContext");
                let network_request = network.is_some_and(|value| !value.is_null());
                let raw_command = params.get("command").and_then(Value::as_str);
                let command = params
                    .get("command")
                    .and_then(Value::as_str)
                    .filter(|value| {
                        !value.is_empty()
                            && value.len() <= MAX_APPROVAL_TEXT_BYTES
                            && safe_scope_text(value)
                    })
                    .map(|value| clean_user_text(value, MAX_APPROVAL_TEXT_BYTES));
                let actions = params
                    .get("commandActions")
                    .and_then(Value::as_array)
                    .filter(|values| !values.is_empty() && values.len() <= MAX_SCOPE_ENTRIES)
                    .and_then(|values| {
                        values
                            .iter()
                            .map(|value| {
                                value
                                    .get("command")
                                    .and_then(Value::as_str)
                                    .filter(|value| {
                                        !value.is_empty()
                                            && value.len() <= MAX_APPROVAL_TEXT_BYTES
                                            && safe_scope_text(value)
                                    })
                                    .map(|value| clean_user_text(value, MAX_APPROVAL_TEXT_BYTES))
                            })
                            .collect::<Option<Vec<_>>>()
                    });
                let details = if network_request {
                    network_details(network.ok_or(CodexAppError::ProtocolVersionMismatch)?)?
                } else {
                    String::new()
                };
                let actions_verified = actions.is_some();
                let summary = command.or_else(|| actions.as_ref().map(|values| values.join("\n")));
                let allow = raw_command.is_some_and(|value| {
                    !value.is_empty()
                        && value.len() <= MAX_APPROVAL_TEXT_BYTES
                        && safe_scope_text(value)
                }) || actions_verified;
                (
                    ApprovalKind::CommandExecution,
                    summary.unwrap_or_else(|| "Command details are unavailable".to_owned()),
                    details,
                    allow,
                    network_request,
                    None,
                )
            }
            "item/fileChange/requestApproval" => {
                let diff = clean_user_text(current_diff, MAX_APPROVAL_TEXT_BYTES);
                let grant_root = params
                    .get("grantRoot")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let details = if let Some(root) = grant_root {
                    format!(
                            "Codex requested additional writes under {root} for the rest of the session. This session-wide expansion is unsupported by Codex Meter.\n\n{diff}"
                        )
                } else if diff.is_empty() {
                    "No validated diff details are available. Allowing this request is disabled."
                        .to_owned()
                } else {
                    diff
                };
                (
                    ApprovalKind::FileChange,
                    reason
                        .clone()
                        .unwrap_or_else(|| "Codex proposes file changes".to_owned()),
                    details,
                    !current_diff.is_empty() && grant_root.is_none(),
                    false,
                    None,
                )
            }
            "item/permissions/requestApproval" => {
                if cwd.is_none() {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
                let requested = params
                    .get("permissions")
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                let scope = validate_permission_scope(requested);
                let details = scope
                        .as_ref()
                        .map(|(_, summary)| summary.clone())
                        .unwrap_or_else(|| {
                            "The requested permission scope is unsupported or incomplete. Allowing this request is disabled.".to_owned()
                        });
                (
                    ApprovalKind::Permissions,
                    reason
                        .clone()
                        .unwrap_or_else(|| "Codex requests additional permissions".to_owned()),
                    details,
                    scope.is_some(),
                    scope.as_ref().is_some_and(|(value, _)| {
                        value
                            .get("network")
                            .and_then(|v| v.get("enabled"))
                            .and_then(Value::as_bool)
                            == Some(true)
                    }),
                    scope.map(|(value, _)| value),
                )
            }
            _ => return Err(CodexAppError::UnsupportedApprovalType),
        };
        let mut full_details = details;
        if let Some(reason) = reason {
            if full_details.is_empty() {
                full_details = reason;
            } else {
                full_details = format!("{reason}\n\n{full_details}");
            }
        }
        let view = ApprovalView {
            request_id: request_id.clone(),
            kind,
            requested_action: clean_user_text(
                &format!("{requested_action} · {item_id}"),
                MAX_APPROVAL_TEXT_BYTES,
            ),
            working_context: cwd,
            details: full_details,
            allow_once_available,
            network_request,
        };
        self.pending.insert(
            request_id,
            PendingApproval {
                server_id,
                view: view.clone(),
                permission_subset,
                resolved: false,
            },
        );
        Ok(view)
    }

    pub fn resolve_server_notice(&mut self, request_id: &str, thread_id: &str) -> bool {
        if thread_id != self.expected_thread_id {
            return false;
        }
        if let Some(pending) = self.pending.get_mut(request_id) {
            pending.resolved = true;
            self.pending.remove(request_id);
            true
        } else {
            false
        }
    }

    pub fn decision(
        &mut self,
        request_id: &str,
        choice: ApprovalChoice,
    ) -> Result<(Value, Value), CodexAppError> {
        let pending = self
            .pending
            .get(request_id)
            .ok_or(CodexAppError::StaleApprovalRequest)?;
        if pending.resolved {
            return Err(CodexAppError::StaleApprovalRequest);
        }
        let result = match pending.view.kind {
            ApprovalKind::CommandExecution | ApprovalKind::FileChange => {
                let decision = match choice {
                    ApprovalChoice::AllowOnce if pending.view.allow_once_available => "accept",
                    ApprovalChoice::AllowOnce => {
                        return Err(CodexAppError::UnsafeExecutionConfiguration)
                    }
                    ApprovalChoice::Deny => "decline",
                    ApprovalChoice::Cancel => "cancel",
                };
                json!({"decision": decision})
            }
            ApprovalKind::Permissions => match choice {
                ApprovalChoice::AllowOnce if pending.view.allow_once_available => json!({
                    "permissions": pending.permission_subset.as_ref().ok_or(CodexAppError::UnsafeExecutionConfiguration)?,
                    "scope": "turn"
                }),
                ApprovalChoice::AllowOnce => {
                    return Err(CodexAppError::UnsafeExecutionConfiguration)
                }
                ApprovalChoice::Deny | ApprovalChoice::Cancel => {
                    json!({"permissions": {}, "scope": "turn"})
                }
            },
        };
        let pending = self
            .pending
            .remove(request_id)
            .ok_or(CodexAppError::StaleApprovalRequest)?;
        Ok((pending.server_id, result))
    }

    pub fn deny_all(&mut self) -> Vec<(Value, Value)> {
        self.pending
            .drain()
            .map(|(_, pending)| {
                let result = match pending.view.kind {
                    ApprovalKind::CommandExecution | ApprovalKind::FileChange => {
                        json!({"decision":"decline"})
                    }
                    ApprovalKind::Permissions => json!({"permissions":{},"scope":"turn"}),
                };
                (pending.server_id, result)
            })
            .collect()
    }
}

fn bounded_required(params: &Value, key: &str) -> Result<String, CodexAppError> {
    Ok(required_string(params, key, 128)?.to_owned())
}

fn network_details(value: &Value) -> Result<String, CodexAppError> {
    let host = required_string(value, "host", 1024)?;
    let protocol = value
        .get("protocol")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 64)
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    if !safe_scope_text(host) || !safe_scope_text(protocol) {
        return Err(CodexAppError::ProtocolVersionMismatch);
    }
    Ok(format!("Network request to {host} using {protocol}."))
}

fn validate_permission_scope(value: &Value) -> Option<(Value, String)> {
    let profile = value.as_object()?;
    if profile
        .keys()
        .any(|key| key != "fileSystem" && key != "network")
    {
        return None;
    }
    let mut summary = Vec::new();
    let mut clean_profile = Map::new();
    if let Some(file_system) = profile.get("fileSystem").filter(|value| !value.is_null()) {
        let fs = file_system.as_object()?;
        if fs.keys().any(|key| {
            !matches!(
                key.as_str(),
                "entries" | "read" | "write" | "globScanMaxDepth"
            )
        }) {
            return None;
        }
        let mut clean_fs = Map::new();
        if let Some(entries) = fs.get("entries").filter(|value| !value.is_null()) {
            let entries = entries.as_array()?;
            if entries.len() > MAX_SCOPE_ENTRIES {
                return None;
            }
            let mut lines = Vec::new();
            for entry in entries {
                let item = entry.as_object()?;
                if item.keys().any(|key| key != "access" && key != "path") {
                    return None;
                }
                let access = item.get("access")?.as_str()?;
                if !matches!(access, "read" | "write" | "deny") {
                    return None;
                }
                let path = item.get("path")?;
                let label = validate_permission_path(path)?;
                lines.push(format!("{access}: {label}"));
            }
            summary.extend(lines);
            clean_fs.insert("entries".to_owned(), Value::Array(entries.clone()));
        }
        for key in ["read", "write"] {
            if let Some(paths) = fs.get(key).filter(|value| !value.is_null()) {
                let paths = paths.as_array()?;
                if paths.len() > MAX_SCOPE_ENTRIES {
                    return None;
                }
                let strings = paths
                    .iter()
                    .map(|path| {
                        path.as_str()
                            .filter(|path| path.len() <= 4096)
                            .map(str::to_owned)
                    })
                    .collect::<Option<Vec<_>>>()?;
                for path in &strings {
                    if !safe_scope_text(path) {
                        return None;
                    }
                    summary.push(format!("{key}: {path}"));
                }
                clean_fs.insert(key.to_owned(), json!(strings));
            }
        }
        if let Some(depth) = fs.get("globScanMaxDepth").filter(|value| !value.is_null()) {
            let depth = depth.as_u64().filter(|depth| (1..=256).contains(depth))?;
            clean_fs.insert("globScanMaxDepth".to_owned(), json!(depth));
            summary.push(format!("Glob scan depth: {depth}"));
        }
        clean_profile.insert("fileSystem".to_owned(), Value::Object(clean_fs));
    }
    if let Some(network) = profile.get("network").filter(|value| !value.is_null()) {
        let network = network.as_object()?;
        if network.keys().any(|key| key != "enabled") {
            return None;
        }
        let enabled = network.get("enabled")?.as_bool()?;
        summary.push(format!(
            "Network access: {}",
            if enabled { "requested" } else { "off" }
        ));
        clean_profile.insert("network".to_owned(), json!({"enabled":enabled}));
    }
    if clean_profile.is_empty() {
        return None;
    }
    let joined = summary.join("\n");
    if joined.is_empty() || joined.len() > MAX_APPROVAL_TEXT_BYTES {
        return None;
    }
    Some((Value::Object(clean_profile), joined))
}

fn validate_permission_path(value: &Value) -> Option<String> {
    let path = value.as_object()?;
    let kind = path.get("type")?.as_str()?;
    match kind {
        "path" if path.keys().all(|key| key == "type" || key == "path") => {
            let path = path.get("path")?.as_str()?.to_owned();
            (safe_scope_text(&path) && path.len() <= 4096).then_some(path)
        }
        "glob_pattern" if path.keys().all(|key| key == "type" || key == "pattern") => {
            let pattern = path.get("pattern")?.as_str()?.to_owned();
            (safe_scope_text(&pattern) && pattern.len() <= 4096).then(|| format!("glob {pattern}"))
        }
        "special" if path.keys().all(|key| key == "type" || key == "value") => {
            let special = path.get("value")?.as_object()?;
            let name = special.get("kind")?.as_str()?;
            let detail = match name {
                "root" | "minimal" | "tmpdir" | "slash_tmp"
                    if special.keys().all(|key| key == "kind") =>
                {
                    ""
                }
                "project_roots" if special.keys().all(|key| key == "kind" || key == "subpath") => {
                    let detail = special
                        .get("subpath")
                        .filter(|value| !value.is_null())
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !safe_scope_text(detail) || detail.len() > 4096 {
                        return None;
                    }
                    detail
                }
                _ => return None,
            };
            Some(if detail.is_empty() {
                name.to_owned()
            } else {
                format!("{name}: {detail}")
            })
        }
        _ => None,
    }
}

fn safe_scope_text(value: &str) -> bool {
    !value.contains('\0') && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn broker() -> ApprovalBroker {
        ApprovalBroker::new("thread-1".to_owned(), "turn-1".to_owned())
    }

    #[test]
    fn command_allow_once_uses_only_one_time_decision() {
        let mut broker = broker();
        let view = broker.receive(
            json!(1),
            "item/commandExecution/requestApproval",
            &json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","command":"cargo test"}),
            "",
        ).unwrap();
        assert!(view.allow_once_available);
        assert_eq!(
            broker
                .decision(&view.request_id, ApprovalChoice::AllowOnce)
                .unwrap()
                .1,
            json!({"decision":"accept"})
        );
    }

    #[test]
    fn command_denial_and_cancel_are_distinct() {
        for (choice, expected) in [
            (ApprovalChoice::Deny, "decline"),
            (ApprovalChoice::Cancel, "cancel"),
        ] {
            let mut broker = broker();
            let view = broker.receive(json!(1), "item/commandExecution/requestApproval", &json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","command":"pwd"}), "").unwrap();
            assert_eq!(
                broker.decision(&view.request_id, choice).unwrap().1["decision"],
                expected
            );
        }
    }

    #[test]
    fn file_change_allow_is_unavailable_without_validated_diff() {
        let mut broker = broker();
        let view = broker
            .receive(
                json!(1),
                "item/fileChange/requestApproval",
                &json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","startedAtMs":1}),
                "",
            )
            .unwrap();
        assert!(!view.allow_once_available);
        assert!(broker
            .decision(&view.request_id, ApprovalChoice::AllowOnce)
            .is_err());
    }

    #[test]
    fn file_change_rejects_session_permission_expansion() {
        let mut broker = broker();
        let view = broker
            .receive(
                json!(1),
                "item/fileChange/requestApproval",
                &json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","grantRoot":"/outside/workspace"}),
                "validated diff",
            )
            .unwrap();
        assert!(!view.allow_once_available);
        assert!(view
            .details
            .contains("session-wide expansion is unsupported"));
        assert!(broker
            .decision(&view.request_id, ApprovalChoice::AllowOnce)
            .is_err());
    }

    #[test]
    fn permission_approval_is_turn_scoped_and_exactly_the_validated_subset() {
        let mut broker = broker();
        let request = json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","cwd":"/repo","permissions":{"fileSystem":{"entries":[{"access":"write","path":{"type":"path","path":"/repo/generated"}}]},"network":{"enabled":false}}});
        let view = broker
            .receive(json!(1), "item/permissions/requestApproval", &request, "")
            .unwrap();
        assert!(view.allow_once_available);
        let response = broker
            .decision(&view.request_id, ApprovalChoice::AllowOnce)
            .unwrap()
            .1;
        assert_eq!(response["scope"], "turn");
        assert_eq!(response["permissions"], request["permissions"]);
    }

    #[test]
    fn denied_permission_request_returns_an_empty_turn_scoped_grant() {
        let mut broker = broker();
        let request = json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","cwd":"/repo","permissions":{"network":{"enabled":true}}});
        let view = broker
            .receive(json!(1), "item/permissions/requestApproval", &request, "")
            .unwrap();
        let response = broker
            .decision(&view.request_id, ApprovalChoice::Deny)
            .unwrap()
            .1;
        assert_eq!(response, json!({"permissions":{},"scope":"turn"}));
    }

    #[test]
    fn wrong_turn_and_reused_request_ids_are_rejected() {
        let mut broker = broker();
        let params =
            json!({"threadId":"thread-1","turnId":"turn-2","itemId":"item-1","command":"pwd"});
        assert!(matches!(
            broker.receive(
                json!(1),
                "item/commandExecution/requestApproval",
                &params,
                ""
            ),
            Err(CodexAppError::WrongApprovalScope)
        ));
        let valid =
            json!({"threadId":"thread-1","turnId":"turn-1","itemId":"item-1","command":"pwd"});
        let view = broker
            .receive(
                json!(2),
                "item/commandExecution/requestApproval",
                &valid,
                "",
            )
            .unwrap();
        broker
            .decision(&view.request_id, ApprovalChoice::Deny)
            .unwrap();
        assert!(matches!(
            broker.receive(
                json!(2),
                "item/commandExecution/requestApproval",
                &valid,
                ""
            ),
            Err(CodexAppError::DuplicateApprovalRequest)
        ));
    }

    #[test]
    fn approval_request_id_history_is_bounded_per_turn() {
        let mut broker = broker();
        let params = json!({
            "threadId":"thread-1",
            "turnId":"turn-1",
            "itemId":"item-1",
            "command":"pwd"
        });
        for id in 0..MAX_SEEN_APPROVAL_IDS {
            let view = broker
                .receive(
                    json!(id),
                    "item/commandExecution/requestApproval",
                    &params,
                    "",
                )
                .unwrap();
            broker
                .decision(&view.request_id, ApprovalChoice::Deny)
                .unwrap();
        }
        assert!(matches!(
            broker.receive(
                json!(MAX_SEEN_APPROVAL_IDS),
                "item/commandExecution/requestApproval",
                &params,
                ""
            ),
            Err(CodexAppError::ResponseLimitExceeded)
        ));
        assert_eq!(broker.seen_ids.len(), MAX_SEEN_APPROVAL_IDS);
    }
}
