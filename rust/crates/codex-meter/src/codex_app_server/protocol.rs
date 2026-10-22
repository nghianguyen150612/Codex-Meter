use super::transport::ClientCapability;
use serde_json::{json, Value};

pub(crate) const CLIENT_NAME: &str = "codex-meter";
pub(crate) const CLIENT_TITLE: &str = "Codex Meter";
pub(crate) const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_PENDING_REQUESTS: usize = 256;
pub(crate) const MAX_REQUEST_ID: u64 = u32::MAX as u64;
pub(crate) const NOTIFICATION_QUEUE_CAPACITY: usize = 128;
pub(crate) const REQUEST_TIMEOUT_SECS: u64 = 10;

pub(crate) fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"id": id, "method": method, "params": params})
}

pub(crate) fn notification(method: &str, params: Value) -> Value {
    if params.as_object().is_some_and(serde_json::Map::is_empty) {
        json!({"method": method})
    } else {
        json!({"method": method, "params": params})
    }
}

pub(crate) fn reject_server_request(id: Value) -> Value {
    json!({
        "id": id,
        "error": {
            "code": -32601,
            "message": "Codex Meter does not approve server-initiated requests"
        }
    })
}

pub(crate) fn reject_overflowed_request(id: Value) -> Value {
    json!({"id": id, "error": {"code": -32603, "message": "Codex Meter could not preserve the approval request"}})
}

pub(crate) fn server_response(id: Value, result: Value) -> Value {
    json!({"id": id, "result": result})
}

pub(crate) fn server_error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"id": id, "error": {"code": code, "message": message}})
}

pub(crate) fn valid_request_id(id: &Value) -> bool {
    matches!(id, Value::String(value) if !value.is_empty() && value.len() <= 128)
        || id.as_i64().is_some()
        || id.as_u64().is_some()
}

pub(crate) fn request_allowed(capability: ClientCapability, method: &str) -> bool {
    if method == "initialize" {
        return true;
    }
    match capability {
        ClientCapability::ReadOnlyManagement => !matches!(
            method,
            "thread/start" | "turn/start" | "turn/steer" | "turn/interrupt"
        ),
        ClientCapability::InteractiveExecution => matches!(
            method,
            "config/read"
                | "model/list"
                | "thread/read"
                | "thread/turns/list"
                | "thread/start"
                | "thread/resume"
                | "turn/start"
                | "turn/interrupt"
        ),
    }
}

pub(crate) fn server_request_supported(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
    )
}

pub(crate) fn notification_supported(method: &str) -> bool {
    matches!(
        method,
        "thread/started"
            | "thread/status/changed"
            | "turn/started"
            | "turn/completed"
            | "turn/plan/updated"
            | "turn/diff/updated"
            | "item/started"
            | "item/completed"
            | "item/agentMessage/delta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta"
            | "serverRequest/resolved"
    )
}

pub(crate) fn notification_is_critical(method: &str) -> bool {
    matches!(
        method,
        "thread/started"
            | "thread/status/changed"
            | "turn/started"
            | "turn/completed"
            | "item/started"
            | "item/completed"
            | "serverRequest/resolved"
    )
}
