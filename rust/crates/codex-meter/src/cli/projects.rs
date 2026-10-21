use super::args::{CliError, ProjectsArguments};
use crate::codex_app_server::{
    CodexAppError, CodexAppServer, ProjectAssociationKind, ProjectSummary,
};
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Serialize)]
struct ProjectsJson<'a> {
    schema_version: u32,
    generated_at: String,
    source: &'static str,
    projects: &'a [ProjectSummary],
}

pub(super) fn run(arguments: ProjectsArguments) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(server_error)?;
    let projects = client.list_projects().map_err(server_error)?;
    if arguments.json {
        return serde_json::to_string(&ProjectsJson {
            schema_version: 1,
            generated_at: now_utc(),
            source: "local_codex_app_server",
            projects: &projects,
        })
        .map(|json| format!("{json}\n"))
        .map_err(|_| CliError {
            code: "json_encoding_failed",
            message: "could not encode project associations as JSON".into(),
        });
    }
    if projects.is_empty() {
        return Ok("Codex Meter — Projects\n\nNo local sessions are currently reported by Codex App Server.\n".to_owned());
    }
    let mut output = String::from(
        "Codex Meter — Projects (paths and remotes hidden)\n\nPROJECT     | CODEX PROJECT ID          | ASSOCIATION         | SESSIONS | ARCHIVED | REPOSITORIES | WORKTREES\n",
    );
    for project in projects {
        output.push_str(&format!(
            "{:<11} | {:<25} | {:<19} | {:>8} | {:>8} | {:>12} | {}\n",
            project.project_alias,
            project
                .codex_project_id
                .as_deref()
                .map_or_else(|| "unavailable".to_owned(), |id| safe_cell(id, 25)),
            association_label(project.association),
            project.session_count,
            project.archived_session_count,
            count_label(project.repository_count),
            count_label(project.worktree_count),
        ));
    }
    Ok(output)
}

fn association_label(kind: ProjectAssociationKind) -> &'static str {
    match kind {
        ProjectAssociationKind::CodexProjectId => "Codex project",
        ProjectAssociationKind::LocalGitRepository => "local Git repository",
        ProjectAssociationKind::WorkingDirectory => "local directory",
        ProjectAssociationKind::Unavailable => "unavailable",
    }
}

fn count_label(value: Option<usize>) -> String {
    value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
}

fn safe_cell(value: &str, limit: usize) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(limit)
        .collect()
}

fn now_utc() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unavailable".to_owned())
}

fn server_error(error: CodexAppError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}
