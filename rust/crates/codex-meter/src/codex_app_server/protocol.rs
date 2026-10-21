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
