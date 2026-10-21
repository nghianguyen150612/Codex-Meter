use super::client::CodexAppServer;
use super::errors::CodexAppError;
use super::models::optional_cursor;
use super::projects::{association_for_session, ProjectAssociation};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::PathBuf;

const SESSION_PAGE_SIZE: u32 = 100;
const LOADED_PAGE_SIZE: u32 = 500;
const MAX_SESSION_RECORDS: usize = 10_000;
const MAX_THREAD_ID_BYTES: usize = 128;
const MAX_METADATA_STRING_BYTES: usize = 4 * 1024;
const MAX_WORKING_DIRECTORY_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ArchiveFilter {
    #[default]
    NonArchived,
    Archived,
    All,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionListOptions {
    pub archive_filter: ArchiveFilter,
    pub active_only: bool,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Active,
    Idle,
    NotLoaded,
    SystemError,
    Unknown(String),
}

impl RuntimeStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Idle => "idle",
            Self::NotLoaded => "not_loaded",
            Self::SystemError => "system_error",
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionSource {
    Cli,
    Vscode,
    Exec,
    AppServer,
    Unknown,
    Custom,
    SubAgent,
    SubAgentReview,
    SubAgentCompact,
    SubAgentThreadSpawn,
    SubAgentOther,
}

impl SessionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Vscode => "vscode",
            Self::Exec => "exec",
            Self::AppServer => "app server",
            Self::Unknown => "unknown",
            Self::Custom => "custom",
            Self::SubAgent => "sub-agent",
            Self::SubAgentReview => "sub-agent review",
            Self::SubAgentCompact => "sub-agent compact",
            Self::SubAgentThreadSpawn => "sub-agent thread spawn",
            Self::SubAgentOther => "other sub-agent",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionOrigin {
    LocalCodex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAvailability {
    MetadataAvailable,
    Ephemeral,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageLinkAvailability {
    Unavailable,
}

/// Session data safe for callers and terminal output. It never contains a
/// working-directory path, preview, transcript item, or App Server payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionMetadata {
    pub thread_id: String,
    pub session_id: Option<String>,
    pub name: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub runtime_status: RuntimeStatus,
    pub source: Option<SessionSource>,
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub loaded: Option<bool>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub reasoning_effort: Option<String>,
    pub forked_from_id: Option<String>,
    pub parent_thread_id: Option<String>,
    pub project_id: Option<String>,
    pub history_availability: HistoryAvailability,
    pub origin: SessionOrigin,
    pub usage_link: UsageLinkAvailability,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SessionDetail {
    pub session: SessionMetadata,
    pub project: ProjectAssociation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ForkedSession {
    pub thread_id: String,
    pub session_id: Option<String>,
    /// The source thread id returned by Codex, when present.
    pub forked_from_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TurnMetadata {
    pub turn_id: String,
    pub status: String,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Clone, Debug)]
pub(super) struct ThreadRecord {
    pub(super) metadata: SessionMetadata,
    pub(super) working_directory: Option<PathBuf>,
}

impl CodexAppServer {
    /// Lists local threads with the App Server's opaque cursor pagination.
    /// `loaded` is `None` when the installed protocol does not implement
    /// `thread/loaded/list`.
    pub fn list_sessions(
        &self,
        options: &SessionListOptions,
    ) -> Result<Vec<SessionMetadata>, CodexAppError> {
        let mut records = self.fetch_session_records(options)?;
        let loaded = match self.fetch_loaded_thread_ids() {
            Ok(ids) => Some(ids),
            Err(CodexAppError::UnsupportedMethod(_)) => None,
            Err(error) => return Err(error),
        };
        for record in &mut records {
            record.metadata.loaded = loaded
                .as_ref()
                .map(|ids| ids.contains(&record.metadata.thread_id));
        }
        Ok(records.into_iter().map(|record| record.metadata).collect())
    }

    /// Reads metadata without resuming a thread or loading its turns.
    pub fn read_session(&self, thread_id: &str) -> Result<SessionDetail, CodexAppError> {
        let record = self.read_thread_record(thread_id)?;
        let loaded = match self.fetch_loaded_thread_ids() {
            Ok(ids) => Some(ids.contains(&record.metadata.thread_id)),
            Err(CodexAppError::UnsupportedMethod(_)) => None,
            Err(error) => return Err(error),
        };
        let mut session = record.metadata;
        session.loaded = loaded;
        let project = association_for_session(
            session.project_id.as_deref(),
            record.working_directory.as_deref(),
        );
        Ok(SessionDetail { session, project })
    }

    /// Loads a thread in the local App Server without changing configuration or
    /// starting a turn. The response excludes turn items.
    pub fn resume_session(&self, thread_id: &str) -> Result<SessionMetadata, CodexAppError> {
        let response = self.call(
            "thread/resume",
            serde_json::json!({"threadId": thread_id, "excludeTurns": true}),
        )?;
        let thread = response
            .get("thread")
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let record = parse_thread(thread, None)?;
        if record.metadata.thread_id != thread_id {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        Ok(record.metadata)
    }

    /// Creates a new thread by cloning the source history. This operation is
    /// called only by the explicit `session fork` CLI action and returns no
    /// turn items or transcript content.
    pub fn fork_session(&self, thread_id: &str) -> Result<ForkedSession, CodexAppError> {
        let response = self.call(
            "thread/fork",
            serde_json::json!({"threadId": thread_id, "excludeTurns": true}),
        )?;
        let thread = response
            .get("thread")
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let record = parse_thread(thread, None)?;
        if record.metadata.thread_id == thread_id
            || record
                .metadata
                .forked_from_id
                .as_deref()
                .is_some_and(|parent| parent != thread_id)
        {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        Ok(ForkedSession {
            thread_id: record.metadata.thread_id,
            session_id: record.metadata.session_id,
            forked_from_id: record.metadata.forked_from_id,
        })
    }

    /// Pages turn metadata without resuming the thread. The request uses
    /// `itemsView: "notLoaded"`; any server response that includes items is
    /// rejected and never exposed to callers.
    pub fn list_turn_metadata(&self, thread_id: &str) -> Result<Vec<TurnMetadata>, CodexAppError> {
        let mut turns = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        loop {
            let mut params = serde_json::Map::new();
            params.insert("threadId".into(), Value::String(thread_id.to_owned()));
            params.insert("limit".into(), Value::from(SESSION_PAGE_SIZE));
            params.insert("itemsView".into(), Value::String("notLoaded".into()));
            params.insert("sortDirection".into(), Value::String("desc".into()));
            if let Some(cursor) = cursor.as_ref() {
                params.insert("cursor".into(), Value::String(cursor.clone()));
            }
            let response = self.call("thread/turns/list", Value::Object(params))?;
            let object = response
                .as_object()
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            let data = object
                .get("data")
                .and_then(Value::as_array)
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            for item in data {
                turns.push(parse_turn_metadata(item)?);
                if turns.len() > MAX_SESSION_RECORDS {
                    return Err(CodexAppError::ResponseLimitExceeded);
                }
            }
            cursor = optional_cursor(object.get("nextCursor"))?;
            if let Some(cursor) = cursor.as_ref() {
                if !seen_cursors.insert(cursor.clone()) {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
            } else {
                break;
            }
        }
        Ok(turns)
    }

    pub(super) fn fetch_project_records(&self) -> Result<Vec<ThreadRecord>, CodexAppError> {
        self.fetch_session_records(&SessionListOptions {
            archive_filter: ArchiveFilter::All,
            active_only: false,
            limit: None,
        })
    }

    fn fetch_session_records(
        &self,
        options: &SessionListOptions,
    ) -> Result<Vec<ThreadRecord>, CodexAppError> {
        let mut records = match options.archive_filter {
            ArchiveFilter::NonArchived => fetch_scope(self, false, options)?,
            ArchiveFilter::Archived => fetch_scope(self, true, options)?,
            ArchiveFilter::All => {
                let mut unarchived = fetch_scope(self, false, options)?;
                unarchived.extend(fetch_scope(self, true, options)?);
                unarchived
            }
        };
        records.sort_by(|left, right| {
            right
                .metadata
                .created_at
                .cmp(&left.metadata.created_at)
                .then_with(|| left.metadata.thread_id.cmp(&right.metadata.thread_id))
        });
        records.dedup_by(|left, right| left.metadata.thread_id == right.metadata.thread_id);
        if let Some(limit) = options.limit {
            records.truncate(limit);
        }
        Ok(records)
    }

    fn read_thread_record(&self, thread_id: &str) -> Result<ThreadRecord, CodexAppError> {
        let response = self.call(
            "thread/read",
            serde_json::json!({"threadId": thread_id, "includeTurns": false}),
        )?;
        let thread = response
            .get("thread")
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let record = parse_thread(thread, None)?;
        if record.metadata.thread_id != thread_id {
            return Err(CodexAppError::ProtocolVersionMismatch);
        }
        Ok(record)
    }

    fn fetch_loaded_thread_ids(&self) -> Result<HashSet<String>, CodexAppError> {
        let mut ids = HashSet::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        loop {
            let mut params = Map::new();
            params.insert("limit".into(), Value::from(LOADED_PAGE_SIZE));
            if let Some(cursor) = cursor.as_ref() {
                params.insert("cursor".into(), Value::String(cursor.clone()));
            }
            let response = self.call("thread/loaded/list", Value::Object(params))?;
            let object = response
                .as_object()
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            let data = object
                .get("data")
                .and_then(Value::as_array)
                .ok_or(CodexAppError::ProtocolVersionMismatch)?;
            for id in data {
                let id = id
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                ids.insert(id.to_owned());
                if ids.len() > MAX_SESSION_RECORDS {
                    return Err(CodexAppError::ResponseLimitExceeded);
                }
            }
            cursor = optional_cursor(object.get("nextCursor"))?;
            if let Some(cursor) = cursor.as_ref() {
                if !seen_cursors.insert(cursor.clone()) {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
            } else {
                break;
            }
        }
        Ok(ids)
    }
}

fn fetch_scope(
    client: &CodexAppServer,
    archived: bool,
    options: &SessionListOptions,
) -> Result<Vec<ThreadRecord>, CodexAppError> {
    let mut records: Vec<ThreadRecord> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    loop {
        if options.limit.is_some_and(|limit| {
            records
                .iter()
                .filter(|record| {
                    !options.active_only || record.metadata.runtime_status == RuntimeStatus::Active
                })
                .count()
                >= limit
        }) {
            break;
        }
        let mut params = Map::new();
        params.insert("limit".into(), Value::from(SESSION_PAGE_SIZE));
        params.insert("archived".into(), Value::Bool(archived));
        params.insert("useStateDbOnly".into(), Value::Bool(true));
        params.insert(
            "sourceKinds".into(),
            serde_json::json!([
                "cli",
                "vscode",
                "exec",
                "appServer",
                "subAgent",
                "subAgentReview",
                "subAgentCompact",
                "subAgentThreadSpawn",
                "subAgentOther",
                "unknown"
            ]),
        );
        if let Some(cursor) = cursor.as_ref() {
            params.insert("cursor".into(), Value::String(cursor.clone()));
        }
        let response = client.call("thread/list", Value::Object(params))?;
        let object = response
            .as_object()
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        let data = object
            .get("data")
            .and_then(Value::as_array)
            .ok_or(CodexAppError::ProtocolVersionMismatch)?;
        for item in data {
            let record = parse_thread(item, Some(archived))?;
            if !options.active_only || record.metadata.runtime_status == RuntimeStatus::Active {
                records.push(record);
            }
            if records.len() > MAX_SESSION_RECORDS {
                return Err(CodexAppError::ResponseLimitExceeded);
            }
        }
        cursor = optional_cursor(object.get("nextCursor"))?;
        if let Some(cursor) = cursor.as_ref() {
            if !seen_cursors.insert(cursor.clone()) {
                return Err(CodexAppError::ProtocolVersionMismatch);
            }
        } else {
            break;
        }
    }
    Ok(records)
}

fn parse_thread(value: &Value, archived: Option<bool>) -> Result<ThreadRecord, CodexAppError> {
    let object = value
        .as_object()
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    let thread_id = required_string(object, "id")?;
    let runtime_status = parse_runtime_status(object.get("status"))?;
    let ephemeral = optional_bool(object, "ephemeral")?.unwrap_or(false);
    let pinned = optional_bool(object, "isPinned")?;
    Ok(ThreadRecord {
        metadata: SessionMetadata {
            thread_id,
            session_id: optional_string(object, "sessionId")?,
            name: optional_string(object, "name")?,
            created_at: optional_i64(object, "createdAt")?,
            updated_at: optional_i64(object, "updatedAt")?,
            runtime_status,
            source: parse_source(object.get("source"))?,
            archived,
            pinned,
            loaded: None,
            model: optional_string(object, "model")?,
            model_provider: optional_string(object, "modelProvider")?,
            reasoning_effort: optional_string(object, "reasoningEffort")?,
            forked_from_id: optional_string(object, "forkedFromId")?,
            parent_thread_id: optional_string(object, "parentThreadId")?,
            project_id: optional_string(object, "projectId")?,
            history_availability: if ephemeral {
                HistoryAvailability::Ephemeral
            } else {
                HistoryAvailability::MetadataAvailable
            },
            origin: SessionOrigin::LocalCodex,
            usage_link: UsageLinkAvailability::Unavailable,
        },
        working_directory: optional_path(object, "cwd")?,
    })
}

fn parse_turn_metadata(value: &Value) -> Result<TurnMetadata, CodexAppError> {
    let object = value
        .as_object()
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    let items = object
        .get("items")
        .and_then(Value::as_array)
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    if !items.is_empty() {
        return Err(CodexAppError::ProtocolVersionMismatch);
    }
    let status = object
        .get("status")
        .and_then(Value::as_str)
        .ok_or(CodexAppError::ProtocolVersionMismatch)?
        .to_owned();
    Ok(TurnMetadata {
        turn_id: required_string(object, "id")?,
        status,
        started_at: optional_i64(object, "startedAt")?,
        completed_at: optional_i64(object, "completedAt")?,
    })
}

fn parse_runtime_status(value: Option<&Value>) -> Result<RuntimeStatus, CodexAppError> {
    let status = value
        .and_then(Value::as_object)
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .ok_or(CodexAppError::ProtocolVersionMismatch)?;
    if status.len() > 128 {
        return Err(CodexAppError::ProtocolVersionMismatch);
    }
    Ok(match status {
        "active" => RuntimeStatus::Active,
        "idle" => RuntimeStatus::Idle,
        "notLoaded" => RuntimeStatus::NotLoaded,
        "systemError" => RuntimeStatus::SystemError,
        other => RuntimeStatus::Unknown(other.to_owned()),
    })
}

fn parse_source(value: Option<&Value>) -> Result<Option<SessionSource>, CodexAppError> {
    let Some(value) = value else { return Ok(None) };
    let source = match value {
        Value::String(value) => match value.as_str() {
            "cli" => SessionSource::Cli,
            "vscode" => SessionSource::Vscode,
            "exec" => SessionSource::Exec,
            "appServer" => SessionSource::AppServer,
            "unknown" => SessionSource::Unknown,
            "subAgentReview" => SessionSource::SubAgentReview,
            "subAgentCompact" => SessionSource::SubAgentCompact,
            "subAgentThreadSpawn" => SessionSource::SubAgentThreadSpawn,
            "subAgentOther" => SessionSource::SubAgentOther,
            _ => SessionSource::Unknown,
        },
        Value::Object(value) => {
            if let Some(custom) = value.get("custom") {
                if !valid_source_detail(custom) {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
                SessionSource::Custom
            } else {
                parse_subagent_source(value.get("subAgent"))?
            }
        }
        _ => return Err(CodexAppError::ProtocolVersionMismatch),
    };
    Ok(Some(source))
}

fn parse_subagent_source(value: Option<&Value>) -> Result<SessionSource, CodexAppError> {
    match value {
        Some(Value::String(source)) if source == "review" => Ok(SessionSource::SubAgentReview),
        Some(Value::String(source)) if source == "compact" => Ok(SessionSource::SubAgentCompact),
        Some(Value::String(source)) if source == "memory_consolidation" => {
            Ok(SessionSource::SubAgentOther)
        }
        Some(Value::String(source)) if valid_source_text(source) => Ok(SessionSource::SubAgent),
        Some(Value::Object(source)) => {
            if let Some(spawn) = source.get("thread_spawn") {
                let spawn = spawn
                    .as_object()
                    .ok_or(CodexAppError::ProtocolVersionMismatch)?;
                let depth = spawn.get("depth").and_then(Value::as_i64);
                let parent = spawn.get("parent_thread_id").and_then(Value::as_str);
                if depth.is_none()
                    || parent.is_none_or(|parent| {
                        parent.is_empty()
                            || parent.len() > MAX_THREAD_ID_BYTES
                            || parent.chars().any(char::is_control)
                    })
                {
                    return Err(CodexAppError::ProtocolVersionMismatch);
                }
                Ok(SessionSource::SubAgentThreadSpawn)
            } else if source.get("other").is_some_and(valid_source_detail) {
                Ok(SessionSource::SubAgentOther)
            } else {
                Err(CodexAppError::ProtocolVersionMismatch)
            }
        }
        _ => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

fn valid_source_detail(value: &Value) -> bool {
    value.as_str().is_some_and(valid_source_text)
}

fn valid_source_text(source: &str) -> bool {
    !source.is_empty()
        && source.len() <= MAX_METADATA_STRING_BYTES
        && !source.chars().any(char::is_control)
}

fn required_string(object: &Map<String, Value>, key: &str) -> Result<String, CodexAppError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| value.len() <= MAX_THREAD_ID_BYTES)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or(CodexAppError::ProtocolVersionMismatch)
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<String>, CodexAppError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= MAX_METADATA_STRING_BYTES => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value.clone()))
            }
        }
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

fn optional_path(object: &Map<String, Value>, key: &str) -> Result<Option<PathBuf>, CodexAppError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= MAX_WORKING_DIRECTORY_BYTES => {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PathBuf::from(value)))
            }
        }
        Some(Value::String(_)) => Err(CodexAppError::ResponseLimitExceeded),
        Some(_) => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, CodexAppError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

fn optional_i64(object: &Map<String, Value>, key: &str) -> Result<Option<i64>, CodexAppError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_i64()
            .map(Some)
            .ok_or(CodexAppError::ProtocolVersionMismatch),
        Some(_) => Err(CodexAppError::ProtocolVersionMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::client::CodexAppServer;

    fn client(mode: &str) -> CodexAppServer {
        CodexAppServer::connect_test_server(mode).expect("fake App Server initializes")
    }

    #[test]
    fn session_listing_follows_cursors_and_keeps_thread_and_session_ids_distinct() {
        let sessions = client("sessions")
            .list_sessions(&SessionListOptions {
                archive_filter: ArchiveFilter::All,
                active_only: false,
                limit: None,
            })
            .unwrap();
        assert_eq!(
            sessions
                .iter()
                .map(|session| session.thread_id.as_str())
                .collect::<Vec<_>>(),
            ["thread-archived", "thread-active", "thread-idle"]
        );
        assert!(sessions[0].archived.unwrap());
        assert!(!sessions[1].archived.unwrap());
        assert_eq!(sessions[1].session_id.as_deref(), Some("session-tree-root"));
        assert_ne!(
            sessions[1].thread_id,
            sessions[1].session_id.as_deref().unwrap()
        );
        assert_eq!(sessions[1].runtime_status, RuntimeStatus::Active);
        assert_eq!(sessions[2].runtime_status, RuntimeStatus::Idle);
        assert_eq!(sessions[1].loaded, Some(true));
        assert_eq!(sessions[2].loaded, Some(false));
        assert_eq!(sessions[1].pinned, None);
        assert_eq!(sessions[1].usage_link, UsageLinkAvailability::Unavailable);
        let encoded = serde_json::to_string(&sessions).unwrap();
        assert!(!encoded.contains("SENSITIVE_PROMPT_SENTINEL"));
        assert!(!encoded.contains("/private/path/"));
    }

    #[test]
    fn active_filter_uses_runtime_status_not_loaded_state() {
        let sessions = client("sessions")
            .list_sessions(&SessionListOptions {
                archive_filter: ArchiveFilter::NonArchived,
                active_only: true,
                limit: None,
            })
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].runtime_status, RuntimeStatus::Active);
        assert_eq!(sessions[0].loaded, Some(true));
    }

    #[test]
    fn empty_history_and_unknown_runtime_status_remain_explicit() {
        let empty = client("empty_sessions")
            .list_sessions(&SessionListOptions {
                archive_filter: ArchiveFilter::All,
                active_only: false,
                limit: None,
            })
            .unwrap();
        assert!(empty.is_empty());

        let unknown = client("unknown_status")
            .list_sessions(&SessionListOptions {
                archive_filter: ArchiveFilter::NonArchived,
                active_only: false,
                limit: Some(1),
            })
            .unwrap();
        assert_eq!(
            unknown[0].runtime_status,
            RuntimeStatus::Unknown("futureStatus".to_owned())
        );
    }

    #[test]
    fn stable_subagent_source_kinds_remain_distinguishable() {
        for (value, expected) in [
            (
                serde_json::json!({"subAgent":"review"}),
                SessionSource::SubAgentReview,
            ),
            (
                serde_json::json!({"subAgent":"compact"}),
                SessionSource::SubAgentCompact,
            ),
            (
                serde_json::json!({"subAgent":"memory_consolidation"}),
                SessionSource::SubAgentOther,
            ),
            (
                serde_json::json!({"subAgent":{"thread_spawn":{"depth":1,"parent_thread_id":"parent","agent_path":"sensitive"}}}),
                SessionSource::SubAgentThreadSpawn,
            ),
            (
                serde_json::json!({"subAgent":{"other":"sensitive"}}),
                SessionSource::SubAgentOther,
            ),
        ] {
            let parsed = parse_source(Some(&value)).unwrap().unwrap();
            assert_eq!(parsed, expected);
            assert!(!serde_json::to_string(&parsed)
                .unwrap()
                .contains("sensitive"));
        }
        assert_eq!(
            serde_json::to_string(&SessionSource::SubAgentThreadSpawn).unwrap(),
            "\"subAgentThreadSpawn\""
        );
    }

    #[test]
    fn metadata_read_does_not_resume_or_load_turn_content() {
        let detail = client("sessions").read_session("thread-active").unwrap();
        assert_eq!(detail.session.thread_id, "thread-active");
        assert_eq!(
            detail.session.session_id.as_deref(),
            Some("session-tree-root")
        );
        assert_eq!(detail.session.archived, None);
        assert_eq!(detail.session.pinned, None);
        assert_eq!(detail.session.loaded, Some(true));
        assert_eq!(
            detail.session.project_id.as_deref(),
            Some("opaque-project-id")
        );
        let encoded = serde_json::to_string(&detail).unwrap();
        assert!(!encoded.contains("SENSITIVE_PROMPT_SENTINEL"));
        assert!(!encoded.contains("/private/path/"));
    }

    #[test]
    fn resume_and_fork_use_metadata_only_responses_and_keep_relationships() {
        let client = client("sessions");
        let resumed = client.resume_session("thread-active").unwrap();
        assert_eq!(resumed.thread_id, "thread-active");
        let forked = client.fork_session("thread-active").unwrap();
        assert_eq!(forked.thread_id, "thread-forked");
        assert_eq!(forked.forked_from_id.as_deref(), Some("thread-active"));
        assert_eq!(forked.session_id.as_deref(), Some("session-tree-root"));
    }

    #[test]
    fn turn_metadata_uses_not_loaded_items_and_server_cursors() {
        let turns = client("sessions")
            .list_turn_metadata("thread-active")
            .unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_id, "turn-1");
        assert_eq!(turns[1].status, "interrupted");
    }

    #[test]
    fn unsupported_loaded_state_is_explicit_and_unknown_threads_are_typed() {
        let sessions = client("loaded_unsupported")
            .list_sessions(&SessionListOptions {
                archive_filter: ArchiveFilter::NonArchived,
                active_only: false,
                limit: Some(1),
            })
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].loaded, None);
        assert_eq!(
            client("unknown_thread")
                .read_session("does-not-exist")
                .unwrap_err(),
            CodexAppError::UnknownThread
        );
    }
}
