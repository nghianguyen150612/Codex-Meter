use super::client::CodexAppServer;
use super::errors::CodexAppError;
use super::sessions::ThreadRecord;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const GIT_QUERY_TIMEOUT: Duration = Duration::from_secs(2);
const PROJECT_DISCOVERY_BUDGET: Duration = Duration::from_secs(8);
const MAX_GIT_METADATA_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAssociationKind {
    CodexProjectId,
    LocalGitRepository,
    WorkingDirectory,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectAssociation {
    /// Opaque canonical identifier supplied by Codex, if one was available.
    pub codex_project_id: Option<String>,
    pub kind: ProjectAssociationKind,
    pub is_git_worktree: Option<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectSummary {
    /// In-memory alias for this response. No project path or remote URL is
    /// emitted or persisted.
    pub project_alias: String,
    pub codex_project_id: Option<String>,
    pub association: ProjectAssociationKind,
    pub session_count: usize,
    pub archived_session_count: usize,
    pub repository_count: Option<usize>,
    pub worktree_count: Option<usize>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum GroupKey {
    CodexProject(String),
    GitRepository(PathBuf),
    WorkingDirectory(PathBuf),
    Unavailable(usize),
}

#[derive(Default)]
struct ProjectCounts {
    codex_project_id: Option<String>,
    association: Option<ProjectAssociationKind>,
    sessions: usize,
    archived: usize,
    repositories: BTreeSet<PathBuf>,
    worktrees: BTreeSet<PathBuf>,
    git_context_checked: bool,
    git_context_incomplete: bool,
}

#[derive(Clone)]
enum GitContext {
    Repository {
        working_tree: PathBuf,
        common_directory: PathBuf,
        linked_worktree: bool,
    },
    NonGitWorkingDirectory(PathBuf),
    Unavailable,
}

impl CodexAppServer {
    /// Groups local sessions using the server's project identifier when
    /// available, then read-only local Git metadata, then an exact accessible
    /// working directory. Paths and remotes never cross this API boundary.
    pub fn list_projects(&self) -> Result<Vec<ProjectSummary>, CodexAppError> {
        let records = self.fetch_project_records()?;
        Ok(group_projects(&records))
    }
}

pub(super) fn association_for_session(
    codex_project_id: Option<&str>,
    working_directory: Option<&Path>,
) -> ProjectAssociation {
    let git_context = working_directory.map_or(GitContext::Unavailable, read_git_context);
    let (kind, is_git_worktree) = match (&git_context, codex_project_id) {
        (_, Some(_)) => (
            ProjectAssociationKind::CodexProjectId,
            match &git_context {
                GitContext::Repository {
                    linked_worktree, ..
                } => Some(*linked_worktree),
                _ => None,
            },
        ),
        (
            GitContext::Repository {
                linked_worktree, ..
            },
            None,
        ) => (
            ProjectAssociationKind::LocalGitRepository,
            Some(*linked_worktree),
        ),
        (GitContext::NonGitWorkingDirectory(_), None) => {
            (ProjectAssociationKind::WorkingDirectory, None)
        }
        (GitContext::Unavailable, None) => (ProjectAssociationKind::Unavailable, None),
    };
    ProjectAssociation {
        codex_project_id: codex_project_id.map(str::to_owned),
        kind,
        is_git_worktree,
    }
}

fn group_projects(records: &[ThreadRecord]) -> Vec<ProjectSummary> {
    let mut groups: BTreeMap<GroupKey, ProjectCounts> = BTreeMap::new();
    let mut git_cache = std::collections::HashMap::<PathBuf, GitContext>::new();
    let started_at = Instant::now();
    for (unknown_index, record) in records.iter().enumerate() {
        let git_context = match record.working_directory.as_ref() {
            Some(path) => {
                if let Some(context) = git_cache.get(path) {
                    context.clone()
                } else if started_at.elapsed() < PROJECT_DISCOVERY_BUDGET {
                    let context = read_git_context(path);
                    git_cache.insert(path.clone(), context.clone());
                    context
                } else {
                    GitContext::Unavailable
                }
            }
            None => GitContext::Unavailable,
        };
        let key = if let Some(project_id) = record.metadata.project_id.as_ref() {
            GroupKey::CodexProject(project_id.clone())
        } else {
            match &git_context {
                GitContext::Repository {
                    common_directory, ..
                } => GroupKey::GitRepository(common_directory.clone()),
                GitContext::NonGitWorkingDirectory(path) => {
                    GroupKey::WorkingDirectory(path.clone())
                }
                GitContext::Unavailable => GroupKey::Unavailable(unknown_index),
            }
        };
        let counts = groups.entry(key).or_default();
        counts.sessions += 1;
        if record.metadata.archived == Some(true) {
            counts.archived += 1;
        }
        if let Some(project_id) = record.metadata.project_id.as_ref() {
            counts.codex_project_id = Some(project_id.clone());
            counts.association = Some(ProjectAssociationKind::CodexProjectId);
        }
        match git_context {
            GitContext::Repository {
                working_tree,
                common_directory,
                ..
            } => {
                counts.git_context_checked = true;
                counts.repositories.insert(common_directory);
                counts.worktrees.insert(working_tree);
                if counts.association.is_none() {
                    counts.association = Some(ProjectAssociationKind::LocalGitRepository);
                }
            }
            GitContext::NonGitWorkingDirectory(_) => {
                counts.git_context_checked = true;
                if counts.association.is_none() {
                    counts.association = Some(ProjectAssociationKind::WorkingDirectory);
                }
            }
            GitContext::Unavailable => {
                counts.git_context_incomplete = true;
                if counts.association.is_none() {
                    counts.association = Some(ProjectAssociationKind::Unavailable);
                }
            }
        }
    }

    groups
        .into_values()
        .enumerate()
        .map(|(index, counts)| ProjectSummary {
            project_alias: format!("project-{:03}", index + 1),
            codex_project_id: counts.codex_project_id,
            association: counts
                .association
                .unwrap_or(ProjectAssociationKind::Unavailable),
            session_count: counts.sessions,
            archived_session_count: counts.archived,
            repository_count: (counts.git_context_checked && !counts.git_context_incomplete)
                .then_some(counts.repositories.len()),
            worktree_count: (counts.git_context_checked && !counts.git_context_incomplete)
                .then_some(counts.worktrees.len()),
        })
        .collect()
}

fn read_git_context(path: &Path) -> GitContext {
    if !path.is_absolute() {
        return GitContext::Unavailable;
    }
    let Ok(metadata) = fs::metadata(path) else {
        return GitContext::Unavailable;
    };
    if !metadata.is_dir() {
        return GitContext::Unavailable;
    }
    let Ok(working_directory) = fs::canonicalize(path) else {
        return GitContext::Unavailable;
    };
    let mut child = match Command::new("git")
        .arg("-C")
        .arg(&working_directory)
        .args(["rev-parse", "--show-toplevel", "--git-common-dir"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return GitContext::Unavailable,
    };
    let deadline = Instant::now() + GIT_QUERY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return GitContext::Unavailable;
            }
        }
    };
    let Ok(output) = child.wait_with_output() else {
        return GitContext::Unavailable;
    };
    if !status.is_some_and(|status| status.success()) {
        return GitContext::NonGitWorkingDirectory(working_directory);
    }
    if output.stdout.len() > MAX_GIT_METADATA_BYTES {
        return GitContext::Unavailable;
    }
    let Ok(text) = std::str::from_utf8(&output.stdout) else {
        return GitContext::Unavailable;
    };
    let mut lines = text.lines();
    let (Some(root), Some(common_directory), None) = (lines.next(), lines.next(), lines.next())
    else {
        return GitContext::Unavailable;
    };
    if root.is_empty()
        || common_directory.is_empty()
        || root.chars().any(char::is_control)
        || common_directory.chars().any(char::is_control)
    {
        return GitContext::Unavailable;
    }
    let root = PathBuf::from(root);
    let root = if root.is_absolute() {
        root
    } else {
        working_directory.join(root)
    };
    let common_directory = PathBuf::from(common_directory);
    let common_directory = if common_directory.is_absolute() {
        common_directory
    } else {
        working_directory.join(common_directory)
    };
    let (Ok(working_tree), Ok(common_directory)) =
        (fs::canonicalize(root), fs::canonicalize(common_directory))
    else {
        return GitContext::Unavailable;
    };
    let linked_worktree = fs::metadata(working_tree.join(".git"))
        .map(|metadata| metadata.is_file())
        .unwrap_or(false);
    GitContext::Repository {
        working_tree,
        common_directory,
        linked_worktree,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::sessions::{
        HistoryAvailability, RuntimeStatus, SessionMetadata, SessionOrigin, SessionSource,
        UsageLinkAvailability,
    };

    fn record(
        thread_id: &str,
        project_id: Option<&str>,
        cwd: Option<&Path>,
        archived: bool,
    ) -> ThreadRecord {
        ThreadRecord {
            metadata: SessionMetadata {
                thread_id: thread_id.to_owned(),
                session_id: Some("session-tree".to_owned()),
                name: None,
                created_at: None,
                updated_at: None,
                runtime_status: RuntimeStatus::Idle,
                source: Some(SessionSource::Cli),
                archived: Some(archived),
                pinned: None,
                loaded: Some(false),
                model: None,
                model_provider: None,
                reasoning_effort: None,
                forked_from_id: None,
                parent_thread_id: None,
                project_id: project_id.map(str::to_owned),
                history_availability: HistoryAvailability::MetadataAvailable,
                origin: SessionOrigin::LocalCodex,
                usage_link: UsageLinkAvailability::Unavailable,
            },
            working_directory: cwd.map(Path::to_path_buf),
        }
    }

    #[test]
    fn codex_project_id_groups_multiple_sessions_without_paths() {
        let temp =
            std::env::temp_dir().join(format!("codex-meter-project-test-{}", std::process::id()));
        let first = temp.join("first");
        let second = temp.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let sessions = vec![
            record("one", Some("opaque-project"), Some(&first), false),
            record("two", Some("opaque-project"), Some(&second), true),
        ];
        let projects = group_projects(&sessions);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].session_count, 2);
        assert_eq!(projects[0].archived_session_count, 1);
        assert_eq!(
            projects[0].codex_project_id.as_deref(),
            Some("opaque-project")
        );
        let json = serde_json::to_string(&projects).unwrap();
        assert!(!json.contains(temp.to_string_lossy().as_ref()));
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn separate_non_git_directories_are_not_merged() {
        if !git_available() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("codex-meter-projects-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let projects = group_projects(&[
            record("one", None, Some(&first), false),
            record("two", None, Some(&second), false),
        ]);
        assert_eq!(projects.len(), 2);
        assert!(projects
            .iter()
            .all(|project| project.repository_count == Some(0)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incomplete_git_evidence_does_not_look_like_complete_project_counts() {
        let root = std::env::temp_dir().join(format!(
            "codex-meter-incomplete-projects-{}",
            std::process::id()
        ));
        let available = root.join("available");
        let inaccessible = root.join("missing");
        fs::create_dir_all(&available).unwrap();
        let projects = group_projects(&[
            record("one", Some("opaque-project"), Some(&available), false),
            record("two", Some("opaque-project"), Some(&inaccessible), false),
        ]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].session_count, 2);
        assert_eq!(projects[0].repository_count, None);
        assert_eq!(projects[0].worktree_count, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn independent_git_repositories_have_separate_opaque_aliases() {
        if !git_available() {
            return;
        }
        let root = std::env::temp_dir().join(format!("codex-meter-repos-{}", std::process::id()));
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        for directory in [&first, &second] {
            let status = Command::new("git")
                .arg("-C")
                .arg(directory)
                .args(["init", "--quiet"])
                .status();
            if status.is_err() || !status.unwrap().success() {
                let _ = fs::remove_dir_all(root);
                return;
            }
        }
        let projects = group_projects(&[
            record("one", None, Some(&first), false),
            record("two", None, Some(&second), false),
        ]);
        assert_eq!(projects.len(), 2);
        assert!(projects
            .iter()
            .all(|project| project.repository_count == Some(1)));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn linked_worktrees_share_a_repository_but_keep_distinct_worktree_counts() {
        if !git_available() {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("codex-meter-worktrees-{}", std::process::id()));
        let main = root.join("main");
        let linked = root.join("linked");
        let hooks = root.join("hooks");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        let init = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["init", "--quiet"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !init.is_ok_and(|status| status.success()) {
            let _ = fs::remove_dir_all(root);
            return;
        }
        let hooks_path = hooks.to_string_lossy().replace('\\', "/");
        let hooks_config = format!("core.hooksPath={hooks_path}");
        let commit = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args([
                "-c",
                "user.name=Codex Meter Test",
                "-c",
                "user.email=codex-meter@example.invalid",
                "-c",
                hooks_config.as_str(),
                "commit",
                "--allow-empty",
                "--quiet",
                "-m",
                "fixture",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !commit.is_ok_and(|status| status.success()) {
            let _ = fs::remove_dir_all(root);
            return;
        }
        let add_worktree = Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&linked)
            .arg("HEAD")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if !add_worktree.is_ok_and(|status| status.success()) {
            let _ = fs::remove_dir_all(root);
            return;
        }
        let projects = group_projects(&[
            record("one", None, Some(&main), false),
            record("two", None, Some(&linked), false),
        ]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].repository_count, Some(1));
        assert_eq!(projects[0].worktree_count, Some(2));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_working_directory_evidence_stays_unavailable() {
        let path = std::env::temp_dir()
            .join(format!("codex-meter-missing-{}", std::process::id()))
            .join("not-present");
        let projects = group_projects(&[record("one", None, Some(&path), false)]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].association, ProjectAssociationKind::Unavailable);
        assert_eq!(projects[0].repository_count, None);
        let encoded = serde_json::to_string(&projects).unwrap();
        assert!(!encoded.contains(path.to_string_lossy().as_ref()));
    }

    #[test]
    fn relative_working_directory_is_not_resolved_against_codex_meters_cwd() {
        let projects = group_projects(&[record("one", None, Some(Path::new(".")), false)]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].association, ProjectAssociationKind::Unavailable);
        assert_eq!(projects[0].repository_count, None);
    }

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}
