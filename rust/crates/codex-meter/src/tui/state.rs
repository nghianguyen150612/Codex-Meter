use std::collections::{HashMap, VecDeque};

use crate::codex_app_server::{
    ApprovalView, ArchiveFilter, ExecutionPhase, ExecutionPreflight, ExecutionRecoveryRecord,
    ModelCatalog, ProjectSummary, SessionDetail, SessionMetadata,
};
use crate::storage::{EvidenceDetail, QuotaHistoryPage, TokenHistoryCursor, TokenHistoryPage};

use super::backend::OverviewData;
use super::composer::{
    ComposerFocus, DraftState, ModelSelection, PreparedTask, PromptEditor, ReasoningSelection,
    TargetKind, WorkspaceSelection,
};
use super::drafts::{DraftListing, DraftSummary};

pub const LIST_PAGE_SIZE: usize = 50;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Screen {
    Overview,
    Sessions,
    Projects,
    Models,
    History,
    Composer,
}

impl Screen {
    pub const ALL: [Self; 6] = [
        Self::Overview,
        Self::Sessions,
        Self::Projects,
        Self::Models,
        Self::History,
        Self::Composer,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Sessions => 1,
            Self::Projects => 2,
            Self::Models => 3,
            Self::History => 4,
            Self::Composer => 5,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Sessions => "Sessions",
            Self::Projects => "Projects",
            Self::Models => "Models",
            Self::History => "History",
            Self::Composer => "Composer",
        }
    }

    pub const fn from_index(index: usize) -> Self {
        Self::ALL[index % Self::ALL.len()]
    }
}

#[derive(Clone, Debug)]
pub struct Resource<T> {
    pub value: Option<T>,
    pub loading: bool,
    pub error: Option<String>,
    pub request_id: u64,
}

impl<T> Default for Resource<T> {
    fn default() -> Self {
        Self {
            value: None,
            loading: false,
            error: None,
            request_id: 0,
        }
    }
}

impl<T> Resource<T> {
    pub fn begin(&mut self, request_id: u64) {
        self.request_id = request_id;
        self.loading = true;
        self.error = None;
    }

    pub fn finish(&mut self, request_id: u64, result: Result<T, String>) -> bool {
        if self.request_id != request_id {
            return false;
        }
        self.loading = false;
        match result {
            Ok(value) => {
                self.value = Some(value);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryScope {
    Today,
    Week,
    All,
}

impl HistoryScope {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Week => "Week",
            Self::All => "All history",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Today => Self::Week,
            Self::Week => Self::All,
            Self::All => Self::Today,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryMode {
    Tokens,
    Quota,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchKind {
    Text,
    Model,
}

#[derive(Clone, Debug)]
pub struct SearchInput {
    pub screen: Screen,
    pub kind: SearchKind,
    pub value: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    Help,
    ForkConfirm { thread_id: String },
    SyncConfirm,
    SessionDetail,
    ModelDetail,
    ProjectDetail,
    EvidenceDetail,
    ComposerModelPicker,
    ComposerReasoningPicker,
    ComposerTargetPicker,
    ComposerDraftTitle,
    ComposerDraftLibrary,
    ComposerDraftDeleteConfirm,
    ComposerTemplatePicker,
    ComposerPreflight,
    ExecutionConfirm,
    ExecutionView,
    ExecutionApproval,
    ExecutionExitConfirm,
    ExecutionRecovery,
    UnsavedDraftExit,
}

pub struct ComposerState {
    pub editor: PromptEditor,
    pub focus: ComposerFocus,
    pub target_kind: TargetKind,
    pub selected_thread_id: Option<String>,
    pub workspace: WorkspaceSelection,
    pub model_selection: ModelSelection,
    pub reasoning_selection: ReasoningSelection,
    pub draft_state: DraftState,
    pub dirty: bool,
    pub prepared_task: Option<PreparedTask>,
    pub model_filter: String,
    pub model_picker_selected: usize,
    pub session_picker_selected: usize,
    pub draft_listing: Option<DraftListing>,
    pub draft_selected: usize,
    pub loaded_draft_id: Option<String>,
    pub loaded_draft_digest: Option<String>,
    pub draft_title: String,
    pub selected_template: usize,
    pub action_selected: usize,
    pub preflight_scroll: u16,
    pub status: Option<String>,
}

/// Bounded, transient execution presentation. It never persists the prompt or
/// output to the telemetry database or saved-draft store.
pub struct ExecutionUiState {
    pub id: u64,
    pub preflight: Option<ExecutionPreflight>,
    pub phase: ExecutionPhase,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub output: String,
    pub tool_output: String,
    pub diff: String,
    pub activity: VecDeque<String>,
    pub approvals: VecDeque<ApprovalView>,
    pub status: Option<String>,
    pub pending: bool,
    pub scroll: u16,
    pub show_details: bool,
    pub exit_choice: usize,
}

impl Default for ExecutionUiState {
    fn default() -> Self {
        Self {
            id: 0,
            preflight: None,
            phase: ExecutionPhase::Idle,
            thread_id: None,
            turn_id: None,
            output: String::new(),
            tool_output: String::new(),
            diff: String::new(),
            activity: VecDeque::new(),
            approvals: VecDeque::new(),
            status: None,
            pending: false,
            scroll: 0,
            show_details: false,
            exit_choice: 0,
        }
    }
}

impl Default for ComposerState {
    fn default() -> Self {
        Self {
            editor: PromptEditor::new(),
            focus: ComposerFocus::Prompt,
            target_kind: TargetKind::NewThread,
            selected_thread_id: None,
            workspace: WorkspaceSelection::Unselected,
            model_selection: ModelSelection::Inherit,
            reasoning_selection: ReasoningSelection::Inherit,
            draft_state: DraftState::Empty,
            dirty: false,
            prepared_task: None,
            model_filter: String::new(),
            model_picker_selected: 0,
            session_picker_selected: 0,
            draft_listing: None,
            draft_selected: 0,
            loaded_draft_id: None,
            loaded_draft_digest: None,
            draft_title: String::new(),
            selected_template: 0,
            action_selected: 0,
            preflight_scroll: 0,
            status: None,
        }
    }
}

impl ComposerState {
    pub fn invalidate(&mut self) {
        self.prepared_task = None;
        self.dirty = true;
        self.draft_state = if self.editor.text().is_empty() {
            DraftState::Editing
        } else {
            DraftState::Modified
        };
        self.status = None;
    }

    pub fn has_unsaved_content(&self) -> bool {
        self.dirty
    }

    pub fn current_reasoning_name(&self) -> &str {
        match &self.reasoning_selection {
            ReasoningSelection::Inherit => "Inherit from Codex configuration",
            ReasoningSelection::ModelDefault => "Model reported default",
            ReasoningSelection::Explicit(effort) => effort,
            ReasoningSelection::NeedsChoice(_) => "Choose a supported effort",
            ReasoningSelection::Unavailable => "Unavailable",
        }
    }

    pub fn saved_drafts(&self) -> &[DraftSummary] {
        self.draft_listing
            .as_ref()
            .map_or(&[], |listing| listing.drafts.as_slice())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppServerStatus {
    Unknown,
    Connecting,
    Connected,
    Unavailable,
}

pub struct AppState {
    pub active: Screen,
    pub overlay: Option<Overlay>,
    pub search: Option<SearchInput>,
    pub searches: HashMap<Screen, String>,
    pub status: Option<String>,
    pub app_server_status: AppServerStatus,
    pub overview: Resource<OverviewData>,
    pub sessions: Resource<Vec<SessionMetadata>>,
    pub session_detail: Resource<SessionDetail>,
    pub projects: Resource<Vec<ProjectSummary>>,
    pub models: Resource<ModelCatalog>,
    pub history: Resource<TokenHistoryPage>,
    pub quota_history: Resource<QuotaHistoryPage>,
    pub evidence_detail: Resource<EvidenceDetail>,
    pub composer: ComposerState,
    pub execution: ExecutionUiState,
    pub recovery_records: Vec<ExecutionRecoveryRecord>,
    pub recovery_error: Option<String>,
    pub recovery_selected: usize,
    pub recovery_show_details: bool,
    pub recovery_pending: bool,
    pub recovery_request_id: u64,
    pub session_archive_filter: ArchiveFilter,
    pub sessions_active_only: bool,
    pub session_page: usize,
    pub project_page: usize,
    pub model_page: usize,
    pub selected_session: usize,
    pub selected_project: usize,
    pub selected_model: usize,
    pub selected_history: usize,
    pub history_scope: HistoryScope,
    pub history_mode: HistoryMode,
    pub history_model_filter: Option<String>,
    pub history_page_cursors: Vec<Option<TokenHistoryCursor>>,
    pub quota_page_cursors: Vec<Option<crate::storage::QuotaHistoryCursor>>,
    pub mutation_pending: bool,
    pub mutation_request_id: u64,
    next_request_id: u64,
    pub quitting: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            active: Screen::Overview,
            overlay: None,
            search: None,
            searches: HashMap::new(),
            status: None,
            app_server_status: AppServerStatus::Unknown,
            overview: Resource::default(),
            sessions: Resource::default(),
            session_detail: Resource::default(),
            projects: Resource::default(),
            models: Resource::default(),
            history: Resource::default(),
            quota_history: Resource::default(),
            evidence_detail: Resource::default(),
            composer: ComposerState::default(),
            execution: ExecutionUiState::default(),
            recovery_records: Vec::new(),
            recovery_error: None,
            recovery_selected: 0,
            recovery_show_details: false,
            recovery_pending: false,
            recovery_request_id: 0,
            session_archive_filter: ArchiveFilter::NonArchived,
            sessions_active_only: false,
            session_page: 0,
            project_page: 0,
            model_page: 0,
            selected_session: 0,
            selected_project: 0,
            selected_model: 0,
            selected_history: 0,
            history_scope: HistoryScope::All,
            history_mode: HistoryMode::Tokens,
            history_model_filter: None,
            history_page_cursors: vec![None],
            quota_page_cursors: vec![None],
            mutation_pending: false,
            mutation_request_id: 0,
            next_request_id: 0,
            quitting: false,
        }
    }
}

impl AppState {
    pub fn next_request_id(&mut self) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id
    }

    pub fn visible_sessions(&self) -> Vec<&SessionMetadata> {
        let needle = self
            .searches
            .get(&Screen::Sessions)
            .map(String::as_str)
            .unwrap_or("");
        self.sessions
            .value
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|session| match self.session_archive_filter {
                ArchiveFilter::NonArchived => session.archived != Some(true),
                ArchiveFilter::Archived => session.archived == Some(true),
                ArchiveFilter::All => true,
            })
            .filter(|session| {
                !self.sessions_active_only || session.runtime_status.as_str() == "active"
            })
            .filter(|session| {
                needle.is_empty()
                    || contains(&session.thread_id, needle)
                    || session
                        .model
                        .as_deref()
                        .is_some_and(|value| contains(value, needle))
                    || session
                        .source
                        .as_ref()
                        .is_some_and(|value| contains(value.as_str(), needle))
            })
            .collect()
    }

    pub fn visible_projects(&self) -> Vec<&ProjectSummary> {
        let needle = self
            .searches
            .get(&Screen::Projects)
            .map(String::as_str)
            .unwrap_or("");
        self.projects
            .value
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|project| {
                needle.is_empty()
                    || contains(&project.project_alias, needle)
                    || project
                        .codex_project_id
                        .as_deref()
                        .is_some_and(|value| contains(value, needle))
                    || contains(&format!("{:?}", project.association), needle)
            })
            .collect()
    }

    pub fn visible_sessions_page(&self) -> Vec<&SessionMetadata> {
        page_slice(&self.visible_sessions(), self.session_page)
    }

    pub fn visible_projects_page(&self) -> Vec<&ProjectSummary> {
        page_slice(&self.visible_projects(), self.project_page)
    }

    pub fn visible_models(&self) -> Vec<&crate::codex_app_server::ModelInfo> {
        let needle = self
            .searches
            .get(&Screen::Models)
            .map(String::as_str)
            .unwrap_or("");
        self.models
            .value
            .as_ref()
            .map(|catalog| catalog.models.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|model| contains(&model.id, needle) || contains(&model.display_name, needle))
            .collect()
    }

    pub fn visible_models_page(&self) -> Vec<&crate::codex_app_server::ModelInfo> {
        page_slice(&self.visible_models(), self.model_page)
    }

    pub fn visible_token_events(&self) -> Vec<&crate::storage::TokenUsageEvidence> {
        let needle = self
            .searches
            .get(&Screen::History)
            .map(String::as_str)
            .unwrap_or("");
        self.history
            .value
            .as_ref()
            .map(|page| {
                page.events
                    .iter()
                    .filter(|event| {
                        needle.is_empty()
                            || contains(&event.event_id, needle)
                            || event
                                .model
                                .as_deref()
                                .is_some_and(|value| contains(value, needle))
                            || event
                                .reasoning_level
                                .as_deref()
                                .is_some_and(|value| contains(value, needle))
                            || event
                                .raw_total
                                .is_some_and(|value| value.to_string().contains(needle))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn visible_quota_samples(&self) -> Vec<&crate::storage::QuotaSnapshotEvidence> {
        let needle = self
            .searches
            .get(&Screen::History)
            .map(String::as_str)
            .unwrap_or("");
        self.quota_history
            .value
            .as_ref()
            .map(|page| {
                page.samples
                    .iter()
                    .filter(|sample| {
                        needle.is_empty()
                            || contains(&sample.sample_id, needle)
                            || contains(&sample.observed_at, needle)
                            || format!("{:?}", sample.meter_type)
                                .to_lowercase()
                                .contains(&needle.to_lowercase())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn selected_session_metadata(&self) -> Option<&SessionMetadata> {
        self.visible_sessions_page()
            .get(self.selected_session)
            .copied()
    }

    pub fn selected_project_summary(&self) -> Option<&ProjectSummary> {
        self.visible_projects_page()
            .get(self.selected_project)
            .copied()
    }

    pub fn selected_model_info(&self) -> Option<&crate::codex_app_server::ModelInfo> {
        self.visible_models_page().get(self.selected_model).copied()
    }

    pub fn move_selection(&mut self, delta: isize) {
        let length = match self.active {
            Screen::Overview | Screen::Composer => return,
            Screen::Sessions => self.visible_sessions_page().len(),
            Screen::Projects => self.visible_projects_page().len(),
            Screen::Models => self.visible_models_page().len(),
            Screen::History => {
                let length = if self.history_mode == HistoryMode::Quota {
                    self.visible_quota_samples().len()
                } else {
                    self.visible_token_events().len()
                };
                length
            }
        };
        let selected = match self.active {
            Screen::Overview | Screen::Composer => return,
            Screen::Sessions => &mut self.selected_session,
            Screen::Projects => &mut self.selected_project,
            Screen::Models => &mut self.selected_model,
            Screen::History => &mut self.selected_history,
        };
        if length == 0 {
            *selected = 0;
        } else {
            *selected = ((*selected as isize + delta).rem_euclid(length as isize)) as usize;
        }
    }

    pub fn clamp_selection(&mut self) {
        self.session_page = clamp_page(self.session_page, self.visible_sessions().len());
        self.project_page = clamp_page(self.project_page, self.visible_projects().len());
        self.model_page = clamp_page(self.model_page, self.visible_models().len());
        self.selected_session = self
            .selected_session
            .min(self.visible_sessions_page().len().saturating_sub(1));
        self.selected_project = self
            .selected_project
            .min(self.visible_projects_page().len().saturating_sub(1));
        self.selected_model = self
            .selected_model
            .min(self.visible_models_page().len().saturating_sub(1));
        let history_len = if self.history_mode == HistoryMode::Quota {
            self.visible_quota_samples().len()
        } else {
            self.visible_token_events().len()
        };
        self.selected_history = self.selected_history.min(history_len.saturating_sub(1));
    }

    pub fn change_list_page(&mut self, direction: isize) {
        let count = match self.active {
            Screen::Sessions => self.visible_sessions().len(),
            Screen::Projects => self.visible_projects().len(),
            Screen::Models => self.visible_models().len(),
            _ => return,
        };
        let pages = count.div_ceil(LIST_PAGE_SIZE).max(1);
        let page = match self.active {
            Screen::Sessions => &mut self.session_page,
            Screen::Projects => &mut self.project_page,
            Screen::Models => &mut self.model_page,
            _ => return,
        };
        *page = (*page as isize + direction).clamp(0, pages.saturating_sub(1) as isize) as usize;
        match self.active {
            Screen::Sessions => self.selected_session = 0,
            Screen::Projects => self.selected_project = 0,
            Screen::Models => self.selected_model = 0,
            Screen::Overview | Screen::History | Screen::Composer => {}
        }
    }

    pub fn page_count(&self) -> usize {
        let count = match self.active {
            Screen::Sessions => self.visible_sessions().len(),
            Screen::Projects => self.visible_projects().len(),
            Screen::Models => self.visible_models().len(),
            _ => 0,
        };
        count.div_ceil(LIST_PAGE_SIZE).max(1)
    }
}

fn page_slice<'a, T>(values: &[&'a T], page: usize) -> Vec<&'a T> {
    values
        .iter()
        .skip(page.saturating_mul(LIST_PAGE_SIZE))
        .take(LIST_PAGE_SIZE)
        .copied()
        .collect()
}

fn clamp_page(page: usize, count: usize) -> usize {
    page.min(count.saturating_sub(1) / LIST_PAGE_SIZE)
}

fn contains(value: &str, needle: &str) -> bool {
    value.to_lowercase().contains(&needle.to_lowercase())
}

pub(super) fn looks_like_path(value: &str) -> bool {
    let value = value.trim();
    value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('/')
        || value.contains('\\')
        || value.starts_with('~')
        || (value.len() >= 2
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{
        HistoryAvailability, RuntimeStatus, SessionOrigin, UsageLinkAvailability,
    };

    #[test]
    fn screen_tabs_wrap_and_keep_stable_numbering() {
        assert_eq!(Screen::from_index(0), Screen::Overview);
        assert_eq!(Screen::from_index(4), Screen::History);
        assert_eq!(Screen::from_index(5), Screen::Composer);
        assert_eq!(Screen::from_index(6), Screen::Overview);
        assert_eq!(
            Screen::ALL.map(Screen::label),
            ["Overview", "Sessions", "Projects", "Models", "History", "Composer"]
        );
    }

    #[test]
    fn stale_out_of_order_results_do_not_replace_newer_data() {
        let mut resource = Resource::<u32>::default();
        resource.begin(1);
        resource.begin(2);
        assert!(!resource.finish(1, Ok(10)));
        assert!(resource.finish(2, Ok(20)));
        assert_eq!(resource.value, Some(20));
    }

    #[test]
    fn search_and_filter_modes_are_local_and_empty_results_are_safe() {
        let mut state = AppState::default();
        state.searches.insert(Screen::Sessions, "not-found".into());
        state.clamp_selection();
        assert!(state.visible_sessions().is_empty());
        assert_eq!(state.selected_session, 0);
        assert_eq!(HistoryScope::Today.next(), HistoryScope::Week);
        assert_eq!(HistoryScope::Week.next(), HistoryScope::All);
        assert_eq!(HistoryScope::All.next(), HistoryScope::Today);
    }

    #[test]
    fn large_session_lists_use_bounded_pages_and_keep_selection_in_page() {
        let mut state = AppState {
            active: Screen::Sessions,
            ..AppState::default()
        };
        state.sessions.value = Some((0..103).map(session).collect());
        assert_eq!(state.visible_sessions_page().len(), 50);
        state.change_list_page(1);
        assert_eq!(state.session_page, 1);
        assert_eq!(state.visible_sessions_page().len(), 50);
        assert_eq!(
            state.selected_session_metadata().unwrap().thread_id,
            "thread-50"
        );
        state.change_list_page(1);
        assert_eq!(state.session_page, 2);
        assert_eq!(state.visible_sessions_page().len(), 3);
        state.change_list_page(1);
        assert_eq!(state.session_page, 2);
        state.change_list_page(-1);
        assert_eq!(state.session_page, 1);
    }

    fn session(index: usize) -> SessionMetadata {
        SessionMetadata {
            thread_id: format!("thread-{index}"),
            session_id: None,
            name: None,
            created_at: None,
            updated_at: None,
            runtime_status: RuntimeStatus::Idle,
            source: None,
            archived: Some(false),
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
        }
    }
}
