use super::args::{CliError, SessionForkArguments, SessionShowArguments, SessionsArguments};
use crate::codex_app_server::{
    CodexAppError, CodexAppServer, ProjectAssociationKind, SessionDetail, SessionListOptions,
};
use serde::Serialize;
use std::io::{self, IsTerminal, Read, Write};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Serialize)]
struct SessionsJson<'a> {
    schema_version: u32,
    generated_at: String,
    source: &'static str,
    availability: &'static str,
    sessions: &'a [crate::codex_app_server::SessionMetadata],
}

#[derive(Serialize)]
struct SessionDetailJson<'a> {
    schema_version: u32,
    generated_at: String,
    source: &'static str,
    #[serde(flatten)]
    detail: &'a SessionDetail,
}

pub(super) fn list(arguments: SessionsArguments) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(server_error)?;
    let sessions = client
        .list_sessions(&SessionListOptions {
            archive_filter: arguments.archive_filter,
            active_only: arguments.active_only,
            limit: arguments.limit,
        })
        .map_err(server_error)?;
    if arguments.json {
        return json_line(&SessionsJson {
            schema_version: 1,
            generated_at: now_utc(),
            source: "local_codex_app_server",
            availability: "available",
            sessions: &sessions,
        });
    }
    Ok(render_sessions(&sessions))
}

pub(super) fn show(arguments: SessionShowArguments) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(server_error)?;
    let detail = client
        .read_session(&arguments.thread_id)
        .map_err(server_error)?;
    if arguments.json {
        return json_line(&SessionDetailJson {
            schema_version: 1,
            generated_at: now_utc(),
            source: "local_codex_app_server",
            detail: &detail,
        });
    }
    Ok(render_detail(&detail))
}

pub(super) fn resume(thread_id: String) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(server_error)?;
    let session = client.resume_session(&thread_id).map_err(server_error)?;
    Ok(format!(
        "Codex Meter — Session loaded\n\nThread ID: {id}\nStatus:    {status}\n\nThe thread is loaded in Codex App Server. No prompt was sent and no turn was started. Codex Meter does not open an interactive Codex terminal.\n",
        id = safe_cell(&session.thread_id, 128),
        status = session.runtime_status.as_str(),
    ))
}

pub(super) fn fork(arguments: SessionForkArguments) -> Result<String, CliError> {
    if !arguments.yes && !confirm_fork()? {
        return Ok("Fork cancelled. No session was created.\n".to_owned());
    }
    let client = CodexAppServer::connect().map_err(server_error)?;
    let fork = client
        .fork_session(&arguments.thread_id)
        .map_err(server_error)?;
    let relationship = fork
        .forked_from_id
        .as_deref()
        .map(|id| safe_cell(id, 128))
        .unwrap_or_else(|| "unavailable".to_owned());
    Ok(format!(
        "Codex Meter — Session forked\n\nSource thread: {source}\nNew thread:    {new}\nForked from:   {relationship}\nStatus:        history copied; no turn started\n",
        source = safe_cell(&arguments.thread_id, 128),
        new = safe_cell(&fork.thread_id, 128),
    ))
}

fn confirm_fork() -> Result<bool, CliError> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(CliError {
            code: "confirmation_required",
            message: "forking copies conversation history; rerun with --yes to confirm".into(),
        });
    }
    eprint!("Forking copies this conversation's saved history. Continue? [y/N] ");
    io::stderr().flush().map_err(|_| CliError {
        code: "confirmation_failed",
        message: "could not read fork confirmation".into(),
    })?;
    let mut answer = Vec::with_capacity(8);
    let mut stdin = io::stdin();
    for _ in 0..64 {
        let mut byte = [0_u8; 1];
        let count = stdin.read(&mut byte).map_err(|_| CliError {
            code: "confirmation_failed",
            message: "could not read fork confirmation".into(),
        })?;
        if count == 0 || byte[0] == b'\n' || byte[0] == b'\r' {
            break;
        }
        answer.push(byte[0]);
    }
    let answer = String::from_utf8_lossy(&answer).trim().to_ascii_lowercase();
    Ok(matches!(answer.as_str(), "y" | "yes"))
}

fn render_sessions(sessions: &[crate::codex_app_server::SessionMetadata]) -> String {
    if sessions.is_empty() {
        return "Codex Meter — Sessions\n\nNo local sessions are currently reported by Codex App Server.\n".to_owned();
    }
    let mut output = String::from(
        "Codex Meter — Sessions\n\nTHREAD ID                            | NAME                         | STATUS       | SOURCE      | ARCHIVED | LOADED | UPDATED (UTC)\n",
    );
    for session in sessions {
        output.push_str(&format!(
            "{:<36} | {:<28} | {:<12} | {:<11} | {:<8} | {:<6} | {}\n",
            safe_cell(&session.thread_id, 36),
            safe_cell(session.name.as_deref().unwrap_or("unavailable"), 28),
            safe_cell(session.runtime_status.as_str(), 12),
            session.source.as_ref().map_or_else(
                || "unavailable".to_owned(),
                |source| source.as_str().to_owned()
            ),
            option_bool(session.archived),
            option_bool(session.loaded),
            format_time(session.updated_at),
        ));
    }
    output
}

fn render_detail(detail: &SessionDetail) -> String {
    let session = &detail.session;
    let source = session.source.as_ref().map_or_else(
        || "unavailable".to_owned(),
        |source| source.as_str().to_owned(),
    );
    let project = match detail.project.kind {
        ProjectAssociationKind::CodexProjectId => {
            detail.project.codex_project_id.as_deref().map_or_else(
                || "unavailable".to_owned(),
                |id| format!("Codex project {}", safe_cell(id, 128)),
            )
        }
        ProjectAssociationKind::LocalGitRepository
            if detail.project.is_git_worktree == Some(true) =>
        {
            "local Git worktree (path hidden)".to_owned()
        }
        ProjectAssociationKind::LocalGitRepository => {
            "local Git repository (path hidden)".to_owned()
        }
        ProjectAssociationKind::WorkingDirectory => "local directory (path hidden)".to_owned(),
        ProjectAssociationKind::Unavailable => "unavailable".to_owned(),
    };
    format!(
        "Codex Meter — Session\n\nThread ID:      {thread_id}\nSession ID:     {session_id}\nName:           {name}\nStatus:         {status}\nSource:         {source}\nCreated:        {created}\nUpdated:        {updated}\nArchived:       {archived}\nPinned:         {pinned}\nLoaded:         {loaded}\nProject:        {project}\nModel:          {model}\nProvider:       {provider}\nReasoning:      {reasoning}\nHistory:        {history}\nUsage link:     {usage}\nOrigin:         local Codex\n",
        thread_id = safe_cell(&session.thread_id, 128),
        session_id = display_optional(session.session_id.as_deref()),
        name = safe_cell(session.name.as_deref().unwrap_or("unavailable"), 256),
        status = safe_cell(session.runtime_status.as_str(), 128),
        created = format_time(session.created_at),
        updated = format_time(session.updated_at),
        archived = option_bool(session.archived),
        pinned = option_bool(session.pinned),
        loaded = option_bool(session.loaded),
        model = display_optional(session.model.as_deref()),
        provider = display_optional(session.model_provider.as_deref()),
        reasoning = display_optional(session.reasoning_effort.as_deref()),
        history = match session.history_availability {
            crate::codex_app_server::HistoryAvailability::MetadataAvailable => "metadata available",
            crate::codex_app_server::HistoryAvailability::Ephemeral => "ephemeral",
            crate::codex_app_server::HistoryAvailability::Unknown => "unavailable",
        },
        usage = "unavailable",
    )
}

fn format_time(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
        .and_then(|timestamp| timestamp.format(&Rfc3339).ok())
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn option_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unavailable",
    }
}

fn display_optional(value: Option<&str>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| safe_cell(value, 256))
}

fn safe_cell(value: &str, limit: usize) -> String {
    let mut value = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(limit)
        .collect::<String>();
    if value.trim().is_empty() {
        value = "unavailable".to_owned();
    }
    value
}

fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unavailable".to_owned())
}

fn json_line<T: Serialize>(value: &T) -> Result<String, CliError> {
    serde_json::to_string(value)
        .map(|json| format!("{json}\n"))
        .map_err(|_| CliError {
            code: "json_encoding_failed",
            message: "could not encode the App Server response as JSON".into(),
        })
}

fn server_error(error: CodexAppError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{
        HistoryAvailability, RuntimeStatus, SessionMetadata, SessionOrigin, UsageLinkAvailability,
    };

    #[test]
    fn session_json_is_versioned_and_does_not_have_a_path_field() {
        let detail = SessionDetail {
            session: SessionMetadata {
                thread_id: "thread-test".to_owned(),
                session_id: Some("session-tree-test".to_owned()),
                name: None,
                created_at: None,
                updated_at: None,
                runtime_status: RuntimeStatus::Idle,
                source: None,
                archived: None,
                pinned: None,
                loaded: None,
                model: None,
                model_provider: None,
                reasoning_effort: None,
                forked_from_id: None,
                parent_thread_id: None,
                project_id: None,
                history_availability: HistoryAvailability::Unknown,
                origin: SessionOrigin::LocalCodex,
                usage_link: UsageLinkAvailability::Unavailable,
            },
            project: crate::codex_app_server::ProjectAssociation {
                codex_project_id: None,
                kind: ProjectAssociationKind::Unavailable,
                is_git_worktree: None,
            },
        };
        let encoded = serde_json::to_value(SessionDetailJson {
            schema_version: 1,
            generated_at: "2026-10-21T00:00:00Z".to_owned(),
            source: "local_codex_app_server",
            detail: &detail,
        })
        .unwrap();
        assert_eq!(encoded["schema_version"], 1);
        assert_eq!(encoded["source"], "local_codex_app_server");
        assert_eq!(encoded["session"]["thread_id"], "thread-test");
        assert!(encoded.get("detail").is_none());
        assert!(encoded.get("working_directory").is_none());
    }

    #[test]
    fn terminal_cells_strip_control_characters() {
        assert_eq!(
            safe_cell("first\u{1b}[31m\nsecond", 64),
            "first [31m second"
        );
    }
}
