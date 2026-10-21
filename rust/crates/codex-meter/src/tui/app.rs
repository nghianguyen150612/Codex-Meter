use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

#[cfg(test)]
use super::backend::TestAppServerMode;
use super::backend::{Backend, BackendEvent, MutationKind};
use super::render;
use super::state::{
    looks_like_path, AppServerStatus, AppState, HistoryMode, Overlay, Screen, SearchInput,
    SearchKind, LIST_PAGE_SIZE,
};
use super::theme::Theme;
use crate::cli::{CliError, TuiArguments};
use crate::codex_app_server::ArchiveFilter;

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub fn run(arguments: TuiArguments) -> Result<String, CliError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(CliError {
            code: "tui_requires_tty",
            message: "`codex-meter tui` requires stdin and stdout to be interactive terminals"
                .to_owned(),
        });
    }
    let database_path = match arguments.database.as_ref() {
        Some(path) => path.clone(),
        None => crate::cli::default_database_path().map_err(|_| CliError {
            code: "database_location_unavailable",
            message: "could not determine the default Codex Meter database location".to_owned(),
        })?,
    };
    let backend = Backend::new(&arguments, database_path).map_err(|_| CliError {
        code: "tui_worker_start_failed",
        message: "could not start bounded local data workers".to_owned(),
    })?;
    let lifecycle = TerminalLifecycle::enter().map_err(|_| CliError {
        code: "tui_terminal_setup_failed",
        message: "could not initialize the terminal; terminal settings were restored".to_owned(),
    })?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(terminal_error)?;
    let mut state = AppState::default();
    let theme = Theme::detect();
    request_screen(&mut state, &backend, Screen::Overview);
    terminal.clear().map_err(terminal_error)?;

    let result = run_loop(&mut terminal, &mut state, &backend, theme);
    drop(terminal);
    drop(lifecycle);
    result?;
    Ok(String::new())
}

fn run_loop(
    terminal: &mut TuiTerminal,
    state: &mut AppState,
    backend: &Backend,
    theme: Theme,
) -> Result<(), CliError> {
    let mut redraw = true;
    while !state.quitting {
        while let Some(event) = backend.try_recv() {
            redraw |= apply_backend_event(state, backend, event);
        }
        if redraw {
            terminal
                .draw(|frame| render::draw(frame, state, theme))
                .map_err(terminal_error)?;
            redraw = false;
        }
        if !event::poll(Duration::from_millis(100)).map_err(terminal_error)? {
            continue;
        }
        match event::read().map_err(terminal_error)? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                redraw |= handle_key(state, backend, key);
            }
            Event::Resize(_, _) => redraw = true,
            _ => {}
        }
    }
    Ok(())
}

fn handle_key(state: &mut AppState, backend: &Backend, key: KeyEvent) -> bool {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        state.quitting = true;
        return true;
    }
    if state.search.is_some() {
        return handle_search_input(state, backend, key);
    }
    if let Some(overlay) = state.overlay.clone() {
        return handle_overlay(state, backend, overlay, key);
    }
    match key.code {
        KeyCode::Char('q') => state.quitting = true,
        KeyCode::Tab => switch_screen(state, backend, 1),
        KeyCode::BackTab => switch_screen(state, backend, -1),
        KeyCode::Char('1'..='5') => {
            let KeyCode::Char(number) = key.code else {
                unreachable!()
            };
            let index = number as usize - '1' as usize;
            state.active = Screen::from_index(index);
            state.status = None;
            request_screen(state, backend, state.active);
        }
        KeyCode::Up | KeyCode::Char('k') => state.move_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => state.move_selection(1),
        KeyCode::Enter => open_selected(state, backend),
        KeyCode::Char('/') if state.active != Screen::Overview => {
            state.search = Some(SearchInput {
                screen: state.active,
                kind: SearchKind::Text,
                value: state
                    .searches
                    .get(&state.active)
                    .cloned()
                    .unwrap_or_default(),
            });
        }
        KeyCode::Char('?') => state.overlay = Some(Overlay::Help),
        KeyCode::Char('r') => {
            state.status = Some("Refreshing the active view...".to_owned());
            refresh_screen(state, backend);
        }
        KeyCode::Char('s') if !state.mutation_pending => state.overlay = Some(Overlay::SyncConfirm),
        KeyCode::Char('s') => {
            state.status = Some("Wait for the current explicit action to finish.".to_owned())
        }
        KeyCode::Char('f') if state.active == Screen::Sessions => {
            state.session_archive_filter = match state.session_archive_filter {
                ArchiveFilter::NonArchived => ArchiveFilter::Archived,
                ArchiveFilter::Archived => ArchiveFilter::All,
                ArchiveFilter::All => ArchiveFilter::NonArchived,
            };
            state.session_page = 0;
            state.selected_session = 0;
            state.clamp_selection();
        }
        KeyCode::Char('a') if state.active == Screen::Sessions => {
            state.sessions_active_only = !state.sessions_active_only;
            state.session_page = 0;
            state.selected_session = 0;
            state.clamp_selection();
        }
        KeyCode::Char('L') if state.active == Screen::Sessions => {
            load_selected_session(state, backend)
        }
        KeyCode::Char('F') if state.active == Screen::Sessions => fork_selected_session(state),
        KeyCode::Char('m')
            if state.active == Screen::History && state.history_mode == HistoryMode::Tokens =>
        {
            state.search = Some(SearchInput {
                screen: Screen::History,
                kind: SearchKind::Model,
                value: state.history_model_filter.clone().unwrap_or_default(),
            });
        }
        KeyCode::Char('w') if state.active == Screen::History => {
            state.history_scope = state.history_scope.next();
            state.history_page_cursors = vec![None];
            state.selected_history = 0;
            state.history.value = None;
            state.history.error = None;
            request_screen(state, backend, Screen::History);
        }
        KeyCode::Char('t') if state.active == Screen::History => {
            state.history_mode = if state.history_mode == HistoryMode::Tokens {
                HistoryMode::Quota
            } else {
                HistoryMode::Tokens
            };
            state.selected_history = 0;
            request_screen(state, backend, Screen::History);
        }
        KeyCode::Char('n') | KeyCode::PageDown if state.active == Screen::History => {
            next_history_page(state, backend)
        }
        KeyCode::Char('p') | KeyCode::PageUp if state.active == Screen::History => {
            previous_history_page(state, backend)
        }
        KeyCode::Char('[') if state.active == Screen::History => {
            previous_history_page(state, backend)
        }
        KeyCode::Char(']') if state.active == Screen::History => next_history_page(state, backend),
        KeyCode::Char('n') | KeyCode::PageDown
            if matches!(
                state.active,
                Screen::Sessions | Screen::Projects | Screen::Models
            ) =>
        {
            state.change_list_page(1);
        }
        KeyCode::Char('p') | KeyCode::PageUp
            if matches!(
                state.active,
                Screen::Sessions | Screen::Projects | Screen::Models
            ) =>
        {
            state.change_list_page(-1);
        }
        _ => return false,
    }
    true
}

fn handle_search_input(state: &mut AppState, backend: &Backend, key: KeyEvent) -> bool {
    let Some(search) = state.search.as_mut() else {
        return false;
    };
    match key.code {
        KeyCode::Esc => state.search = None,
        KeyCode::Enter => {
            let search = state.search.take().expect("search mode is active");
            match search.kind {
                SearchKind::Text => {
                    state
                        .searches
                        .insert(search.screen, search.value.trim().to_owned());
                    state.clamp_selection();
                    state.status =
                        Some("Search applied to privacy-safe displayed metadata.".to_owned());
                }
                SearchKind::Model => {
                    let value = search.value.trim();
                    state.history_model_filter = (!value.is_empty()).then(|| value.to_owned());
                    state.history_page_cursors = vec![None];
                    state.selected_history = 0;
                    state.history.value = None;
                    state.history.error = None;
                    request_screen(state, backend, Screen::History);
                }
            }
        }
        KeyCode::Backspace => {
            search.value.pop();
        }
        KeyCode::Char(character) if !character.is_control() => search.value.push(character),
        _ => return false,
    }
    true
}

fn handle_overlay(
    state: &mut AppState,
    backend: &Backend,
    overlay: Overlay,
    key: KeyEvent,
) -> bool {
    if matches!(key.code, KeyCode::Char('q')) {
        state.quitting = true;
        return true;
    }
    match overlay {
        Overlay::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')) {
                state.overlay = None;
            }
        }
        Overlay::ForkConfirm { thread_id } => match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                state.overlay = None;
                start_mutation(state, backend, MutationKind::Fork, Some(thread_id));
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.overlay = None;
                state.status = Some("Fork cancelled; no session was created.".to_owned());
            }
            _ => return false,
        },
        Overlay::SyncConfirm => match key.code {
            KeyCode::Char('y') | KeyCode::Enter => {
                state.overlay = None;
                start_mutation(state, backend, MutationKind::Sync, None);
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                state.overlay = None;
                state.status = Some(
                    "Local telemetry sync cancelled; the database was not changed.".to_owned(),
                );
            }
            _ => return false,
        },
        Overlay::SessionDetail => match key.code {
            KeyCode::Esc | KeyCode::Enter => state.overlay = None,
            KeyCode::Char('L') => {
                state.overlay = None;
                load_selected_session(state, backend);
            }
            KeyCode::Char('F') => {
                if let Some(thread_id) = state
                    .session_detail
                    .value
                    .as_ref()
                    .map(|detail| detail.session.thread_id.clone())
                {
                    if looks_like_path(&thread_id) {
                        state.status = Some(
                            "Session fork is unavailable for an unsafe thread identifier."
                                .to_owned(),
                        );
                        state.overlay = None;
                    } else {
                        state.overlay = Some(Overlay::ForkConfirm { thread_id });
                    }
                }
            }
            _ => return false,
        },
        Overlay::ModelDetail | Overlay::ProjectDetail | Overlay::EvidenceDetail => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                state.overlay = None;
            }
        }
    }
    true
}

fn switch_screen(state: &mut AppState, backend: &Backend, delta: isize) {
    let index =
        (state.active.index() as isize + delta).rem_euclid(Screen::ALL.len() as isize) as usize;
    state.active = Screen::from_index(index);
    state.status = None;
    request_screen(state, backend, state.active);
}

fn open_selected(state: &mut AppState, backend: &Backend) {
    match state.active {
        Screen::Overview => {}
        Screen::Sessions => {
            let Some(session) = state.selected_session_metadata() else {
                return;
            };
            let thread_id = session.thread_id.clone();
            let id = state.next_request_id();
            state.session_detail.value = None;
            state.session_detail.begin(id);
            state.overlay = Some(Overlay::SessionDetail);
            state.app_server_status = AppServerStatus::Connecting;
            if let Err(error) = backend.request_session_detail(id, thread_id) {
                state.session_detail.finish(id, Err(error.clone()));
                state.app_server_status = AppServerStatus::Unavailable;
            }
        }
        Screen::Projects => {
            if state.selected_project_summary().is_some() {
                state.overlay = Some(Overlay::ProjectDetail);
            }
        }
        Screen::Models => {
            if state.selected_model_info().is_some() {
                state.overlay = Some(Overlay::ModelDetail);
            }
        }
        Screen::History => {
            let evidence_id = if state.history_mode == HistoryMode::Quota {
                state
                    .visible_quota_samples()
                    .get(state.selected_history)
                    .copied()
                    .map(|sample| sample.sample_id.clone())
            } else {
                state
                    .visible_token_events()
                    .get(state.selected_history)
                    .copied()
                    .map(|event| event.event_id.clone())
            };
            if let Some(evidence_id) = evidence_id {
                let id = state.next_request_id();
                state.evidence_detail.value = None;
                state.evidence_detail.begin(id);
                state.overlay = Some(Overlay::EvidenceDetail);
                if let Err(error) = backend.request_inspect(id, evidence_id) {
                    state.evidence_detail.finish(id, Err(error));
                }
            }
        }
    }
}

fn load_selected_session(state: &mut AppState, backend: &Backend) {
    if state.mutation_pending {
        state.status = Some("Wait for the current explicit action to finish.".to_owned());
        return;
    }
    let Some(session) = state.selected_session_metadata() else {
        state.status = Some("No session is selected.".to_owned());
        return;
    };
    if looks_like_path(&session.thread_id) {
        state.status =
            Some("Session load is unavailable for an unsafe thread identifier.".to_owned());
        return;
    }
    start_mutation(
        state,
        backend,
        MutationKind::Resume,
        Some(session.thread_id.clone()),
    );
}

fn fork_selected_session(state: &mut AppState) {
    if state.mutation_pending {
        state.status = Some("Wait for the current explicit action to finish.".to_owned());
        return;
    }
    if let Some(session) = state.selected_session_metadata() {
        if looks_like_path(&session.thread_id) {
            state.status =
                Some("Session fork is unavailable for an unsafe thread identifier.".to_owned());
        } else {
            state.overlay = Some(Overlay::ForkConfirm {
                thread_id: session.thread_id.clone(),
            });
        }
    } else {
        state.status = Some("No session is selected.".to_owned());
    }
}

fn start_mutation(
    state: &mut AppState,
    backend: &Backend,
    kind: MutationKind,
    thread_id: Option<String>,
) {
    if state.mutation_pending {
        return;
    }
    let id = state.next_request_id();
    state.mutation_pending = true;
    state.mutation_request_id = id;
    state.status = Some(match kind {
        MutationKind::Resume => {
            "Loading the selected thread; no prompt or turn will be started...".to_owned()
        }
        MutationKind::Fork => "Creating a confirmed history fork...".to_owned(),
        MutationKind::Sync => "Running the confirmed one-shot local telemetry sync...".to_owned(),
    });
    let result = match kind {
        MutationKind::Resume => thread_id.map_or_else(
            || Err("No session is selected.".to_owned()),
            |thread_id| backend.request_resume(id, thread_id),
        ),
        MutationKind::Fork => thread_id.map_or_else(
            || Err("No session is selected.".to_owned()),
            |thread_id| backend.request_fork(id, thread_id),
        ),
        MutationKind::Sync => backend.request_sync(id),
    };
    if let Err(error) = result {
        state.mutation_pending = false;
        state.status = Some(error);
    }
}

fn request_screen(state: &mut AppState, backend: &Backend, screen: Screen) {
    match screen {
        Screen::Overview => {
            if state.overview.value.is_none() || state.overview.error.is_some() {
                request_overview(state, backend);
            }
            request_sessions(state, backend);
        }
        Screen::Sessions => request_sessions(state, backend),
        Screen::Projects => {
            if state.projects.value.is_none() || state.projects.error.is_some() {
                request_projects(state, backend);
            }
        }
        Screen::Models => {
            if state.models.value.is_none() || state.models.error.is_some() {
                request_models(state, backend);
            }
        }
        Screen::History => match state.history_mode {
            HistoryMode::Tokens => request_history_page(state, backend),
            HistoryMode::Quota => request_quota_page(state, backend),
        },
    }
}

fn refresh_screen(state: &mut AppState, backend: &Backend) {
    match state.active {
        Screen::Overview => {
            request_overview(state, backend);
            request_sessions_force(state, backend);
        }
        Screen::Sessions => request_sessions_force(state, backend),
        Screen::Projects => request_projects(state, backend),
        Screen::Models => request_models(state, backend),
        Screen::History if state.history_mode == HistoryMode::Tokens => {
            request_history_page(state, backend)
        }
        Screen::History => request_quota_page(state, backend),
    }
}

fn request_sessions(state: &mut AppState, backend: &Backend) {
    if state.sessions.value.is_none() || state.sessions.error.is_some() {
        request_sessions_force(state, backend);
    }
}

fn request_overview(state: &mut AppState, backend: &Backend) {
    let id = state.next_request_id();
    state.overview.begin(id);
    if let Err(error) = backend.request_overview(id) {
        state.overview.finish(id, Err(error.clone()));
        state.status = Some(error);
    }
}

fn request_sessions_force(state: &mut AppState, backend: &Backend) {
    let id = state.next_request_id();
    state.sessions.begin(id);
    state.app_server_status = AppServerStatus::Connecting;
    if let Err(error) = backend.request_sessions(id) {
        state.sessions.finish(id, Err(error.clone()));
        state.app_server_status = AppServerStatus::Unavailable;
        state.status = Some(error);
    }
}

fn request_projects(state: &mut AppState, backend: &Backend) {
    let id = state.next_request_id();
    state.projects.begin(id);
    state.app_server_status = AppServerStatus::Connecting;
    if let Err(error) = backend.request_projects(id) {
        state.projects.finish(id, Err(error.clone()));
        state.app_server_status = AppServerStatus::Unavailable;
        state.status = Some(error);
    }
}

fn request_models(state: &mut AppState, backend: &Backend) {
    let id = state.next_request_id();
    state.models.begin(id);
    state.app_server_status = AppServerStatus::Connecting;
    if let Err(error) = backend.request_models(id) {
        state.models.finish(id, Err(error.clone()));
        state.app_server_status = AppServerStatus::Unavailable;
        state.status = Some(error);
    }
}

fn request_history_page(state: &mut AppState, backend: &Backend) {
    let id = state.next_request_id();
    state.history.begin(id);
    let cursor = state.history_page_cursors.last().cloned().flatten();
    if let Err(error) = backend.request_history(
        id,
        state.history_scope,
        state.history_model_filter.clone(),
        cursor,
    ) {
        state.history.finish(id, Err(error));
    }
}

fn request_quota_page(state: &mut AppState, backend: &Backend) {
    let id = state.next_request_id();
    state.quota_history.begin(id);
    let cursor = state.quota_page_cursors.last().cloned().flatten();
    if let Err(error) = backend.request_quota_history(id, cursor) {
        state.quota_history.finish(id, Err(error));
    }
}

fn next_history_page(state: &mut AppState, backend: &Backend) {
    if state.history_mode == HistoryMode::Tokens {
        let Some(page) = state.history.value.as_ref() else {
            return;
        };
        let Some(cursor) = page.next_cursor.clone().filter(|_| page.has_more) else {
            return;
        };
        state.history_page_cursors.push(Some(cursor));
        state.history.value = None;
        state.history.error = None;
    } else {
        let Some(page) = state.quota_history.value.as_ref() else {
            return;
        };
        let Some(cursor) = page.next_cursor.clone().filter(|_| page.has_more) else {
            return;
        };
        state.quota_page_cursors.push(Some(cursor));
        state.quota_history.value = None;
        state.quota_history.error = None;
    }
    state.selected_history = 0;
    request_screen(state, backend, Screen::History);
}

fn previous_history_page(state: &mut AppState, backend: &Backend) {
    let can_go_back = if state.history_mode == HistoryMode::Tokens {
        state.history_page_cursors.len() > 1
    } else {
        state.quota_page_cursors.len() > 1
    };
    if !can_go_back {
        return;
    }
    if state.history_mode == HistoryMode::Tokens {
        state.history_page_cursors.pop();
        state.history.value = None;
        state.history.error = None;
    } else {
        state.quota_page_cursors.pop();
        state.quota_history.value = None;
        state.quota_history.error = None;
    }
    state.selected_history = 0;
    request_screen(state, backend, Screen::History);
}

fn apply_backend_event(state: &mut AppState, backend: &Backend, event: BackendEvent) -> bool {
    match event {
        BackendEvent::Overview { id, result } => {
            let applied = state.overview.finish(id, result.map(|value| *value));
            if applied && state.overview.error.is_some() {
                state.status = state.overview.error.clone();
            }
            applied
        }
        BackendEvent::Sessions { id, result } => {
            let success = result.is_ok();
            let selected_id = state
                .selected_session_metadata()
                .map(|session| session.thread_id.clone());
            let applied = state.sessions.finish(id, result);
            if applied {
                state.app_server_status = if success {
                    AppServerStatus::Connected
                } else {
                    AppServerStatus::Unavailable
                };
                if let Some(selected_id) = selected_id {
                    if let Some(position) = state
                        .visible_sessions()
                        .iter()
                        .position(|session| session.thread_id == selected_id)
                    {
                        state.session_page = position / LIST_PAGE_SIZE;
                        state.selected_session = position % LIST_PAGE_SIZE;
                    }
                }
                if !success {
                    state.status = state.sessions.error.clone();
                }
                state.clamp_selection();
            }
            applied
        }
        BackendEvent::SessionDetail { id, result } => {
            let success = result.is_ok();
            let applied = state.session_detail.finish(id, result);
            if applied {
                state.app_server_status = if success {
                    AppServerStatus::Connected
                } else {
                    AppServerStatus::Unavailable
                };
            }
            applied
        }
        BackendEvent::Projects { id, result } => {
            let success = result.is_ok();
            let selected_alias = state
                .selected_project_summary()
                .map(|project| project.project_alias.clone());
            let applied = state.projects.finish(id, result);
            if applied {
                state.app_server_status = if success {
                    AppServerStatus::Connected
                } else {
                    AppServerStatus::Unavailable
                };
                if let Some(selected_alias) = selected_alias {
                    if let Some(position) = state
                        .visible_projects()
                        .iter()
                        .position(|project| project.project_alias == selected_alias)
                    {
                        state.project_page = position / LIST_PAGE_SIZE;
                        state.selected_project = position % LIST_PAGE_SIZE;
                    }
                }
                if !success {
                    state.status = state.projects.error.clone();
                }
                state.clamp_selection();
            }
            applied
        }
        BackendEvent::Models { id, result } => {
            let success = result.is_ok();
            let selected_model_id = state.selected_model_info().map(|model| model.id.clone());
            let applied = state.models.finish(id, result);
            if applied {
                state.app_server_status = if success {
                    AppServerStatus::Connected
                } else {
                    AppServerStatus::Unavailable
                };
                if let Some(selected_model_id) = selected_model_id {
                    if let Some(position) = state
                        .visible_models()
                        .iter()
                        .position(|model| model.id == selected_model_id)
                    {
                        state.model_page = position / LIST_PAGE_SIZE;
                        state.selected_model = position % LIST_PAGE_SIZE;
                    }
                }
                if !success {
                    state.status = state.models.error.clone();
                }
                state.clamp_selection();
            }
            applied
        }
        BackendEvent::History { id, result } => {
            let selected_id = state
                .visible_token_events()
                .get(state.selected_history)
                .map(|event| event.event_id.clone());
            let success = result.is_ok();
            let applied = state.history.finish(id, result);
            if applied {
                if let Some(selected_id) = selected_id {
                    if let Some(position) = state
                        .visible_token_events()
                        .iter()
                        .position(|event| event.event_id == selected_id)
                    {
                        state.selected_history = position;
                    }
                }
                if !success {
                    state.status = state.history.error.clone();
                }
                state.clamp_selection();
            }
            applied
        }
        BackendEvent::QuotaHistory { id, result } => {
            let selected_id = state
                .visible_quota_samples()
                .get(state.selected_history)
                .map(|sample| sample.sample_id.clone());
            let success = result.is_ok();
            let applied = state.quota_history.finish(id, result);
            if applied {
                if let Some(selected_id) = selected_id {
                    if let Some(position) = state
                        .visible_quota_samples()
                        .iter()
                        .position(|sample| sample.sample_id == selected_id)
                    {
                        state.selected_history = position;
                    }
                }
                if !success {
                    state.status = state.quota_history.error.clone();
                }
                state.clamp_selection();
            }
            applied
        }
        BackendEvent::EvidenceDetail { id, result } => state.evidence_detail.finish(id, result),
        BackendEvent::Mutation { id, kind, result } => {
            if !state.mutation_pending || id != state.mutation_request_id {
                return false;
            }
            state.mutation_pending = false;
            let succeeded = result.is_ok();
            state.status = Some(match result {
                Ok(message) => message,
                Err(error) => error,
            });
            match kind {
                MutationKind::Resume | MutationKind::Fork if succeeded => {
                    request_sessions_force(state, backend);
                }
                MutationKind::Sync if succeeded => {
                    request_overview(state, backend);
                    request_sessions_force(state, backend);
                    match state.active {
                        Screen::Projects => request_projects(state, backend),
                        Screen::Models => request_models(state, backend),
                        Screen::History if state.history_mode == HistoryMode::Tokens => {
                            request_history_page(state, backend)
                        }
                        Screen::History => request_quota_page(state, backend),
                        Screen::Overview | Screen::Sessions => {}
                    }
                }
                MutationKind::Resume | MutationKind::Fork | MutationKind::Sync => {}
            }
            true
        }
    }
}

fn terminal_error(_error: io::Error) -> CliError {
    CliError {
        code: "tui_terminal_error",
        message: "terminal I/O failed; terminal settings were restored".to_owned(),
    }
}

struct TerminalLifecycle {
    raw_mode: bool,
    alternate_screen: bool,
}

impl TerminalLifecycle {
    fn enter() -> io::Result<Self> {
        let mut lifecycle = Self {
            raw_mode: false,
            alternate_screen: false,
        };
        enable_raw_mode()?;
        lifecycle.raw_mode = true;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        lifecycle.alternate_screen = true;
        execute!(stdout, crossterm::cursor::Hide)?;
        Ok(lifecycle)
    }
}

impl Drop for TerminalLifecycle {
    fn drop(&mut self) {
        if self.alternate_screen {
            let mut stdout = io::stdout();
            let _ = execute!(stdout, crossterm::cursor::Show, LeaveAlternateScreen);
        }
        if self.raw_mode {
            let _ = disable_raw_mode();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_navigation_and_search_mode_do_not_conflict() {
        let backend = test_backend();
        let mut state = AppState::default();
        handle_key(&mut state, &backend, key(KeyCode::Tab));
        assert_eq!(state.active, Screen::Sessions);
        handle_key(&mut state, &backend, key(KeyCode::BackTab));
        assert_eq!(state.active, Screen::Overview);
        for (shortcut, expected) in [
            ('1', Screen::Overview),
            ('2', Screen::Sessions),
            ('3', Screen::Projects),
            ('4', Screen::Models),
            ('5', Screen::History),
        ] {
            handle_key(&mut state, &backend, key(KeyCode::Char(shortcut)));
            assert_eq!(state.active, expected);
            assert!(!state.mutation_pending);
        }
        state.search = Some(SearchInput {
            screen: Screen::Sessions,
            kind: SearchKind::Text,
            value: String::new(),
        });
        handle_key(&mut state, &backend, key(KeyCode::Char('q')));
        assert!(!state.quitting);
        assert_eq!(state.search.as_ref().unwrap().value, "q");
        handle_key(&mut state, &backend, key(KeyCode::Enter));
        assert_eq!(
            state.searches.get(&Screen::Sessions).map(String::as_str),
            Some("q")
        );
    }

    #[test]
    fn fork_and_sync_are_confirmed_and_duplicate_mutations_are_blocked() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Sessions;
        state.sessions.value = Some(vec![crate::codex_app_server::SessionMetadata {
            thread_id: "thread".into(),
            session_id: None,
            name: Some("safe fixture".into()),
            created_at: None,
            updated_at: None,
            runtime_status: crate::codex_app_server::RuntimeStatus::Idle,
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
            history_availability: crate::codex_app_server::HistoryAvailability::Unknown,
            origin: crate::codex_app_server::SessionOrigin::LocalCodex,
            usage_link: crate::codex_app_server::UsageLinkAvailability::Unavailable,
        }]);
        handle_key(&mut state, &backend, key(KeyCode::Char('F')));
        assert!(matches!(
            state.overlay.as_ref(),
            Some(Overlay::ForkConfirm { thread_id }) if thread_id == "thread"
        ));
        assert!(!state.mutation_pending);
        handle_key(&mut state, &backend, key(KeyCode::Esc));
        assert!(state.status.as_deref().unwrap().contains("cancelled"));
        handle_key(&mut state, &backend, key(KeyCode::Char('s')));
        assert_eq!(state.overlay, Some(Overlay::SyncConfirm));
        assert!(!state.mutation_pending);
        handle_key(&mut state, &backend, key(KeyCode::Char('n')));
        assert!(!state.mutation_pending);
        state.mutation_pending = true;
        handle_key(&mut state, &backend, key(KeyCode::Char('L')));
        assert!(state.mutation_pending);
    }

    #[test]
    fn session_actions_dispatch_only_after_explicit_keys_and_confirmed_fork() {
        let backend = Backend::new_test(
            std::env::temp_dir().join("codex-meter-tui-app-action-test.sqlite3"),
            TestAppServerMode::Sessions,
        )
        .unwrap();
        let mut unsafe_state = AppState::default();
        unsafe_state.active = Screen::Sessions;
        unsafe_state.sessions.value = Some(vec![test_session("/private/path/thread-id")]);
        handle_key(&mut unsafe_state, &backend, key(KeyCode::Char('F')));
        handle_key(&mut unsafe_state, &backend, key(KeyCode::Char('L')));
        assert!(unsafe_state.overlay.is_none());
        assert!(!unsafe_state.mutation_pending);
        assert!(unsafe_state
            .status
            .as_deref()
            .unwrap()
            .contains("unavailable"));

        let mut state = AppState::default();
        state.active = Screen::Sessions;
        state.sessions.value = Some(vec![test_session("thread-active")]);

        handle_key(&mut state, &backend, key(KeyCode::Char('j')));
        assert!(!state.mutation_pending);
        assert!(state.overlay.is_none());

        handle_key(&mut state, &backend, key(KeyCode::Char('L')));
        assert!(state.mutation_pending);
        handle_key(&mut state, &backend, key(KeyCode::Char('L')));
        let resume = wait_for_mutation(&mut state, &backend, MutationKind::Resume);
        apply_backend_event(&mut state, &backend, resume);
        assert!(!state.mutation_pending);
        assert!(state
            .status
            .as_deref()
            .unwrap()
            .contains("No prompt was sent"));

        handle_key(&mut state, &backend, key(KeyCode::Char('F')));
        assert!(matches!(
            state.overlay.as_ref(),
            Some(Overlay::ForkConfirm { thread_id }) if thread_id == "thread-active"
        ));
        assert!(!state.mutation_pending);
        handle_key(&mut state, &backend, key(KeyCode::Char('y')));
        assert!(state.mutation_pending);
        handle_key(&mut state, &backend, key(KeyCode::Char('y')));
        let fork = wait_for_mutation(&mut state, &backend, MutationKind::Fork);
        apply_backend_event(&mut state, &backend, fork);
        assert!(!state.mutation_pending);
        assert!(state.status.as_deref().unwrap().contains("Fork created"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn test_backend() -> Backend {
        Backend::new_test(
            std::path::PathBuf::from("/tmp/codex-meter-test.sqlite3"),
            TestAppServerMode::Sessions,
        )
        .unwrap()
    }

    fn test_session(thread_id: &str) -> crate::codex_app_server::SessionMetadata {
        crate::codex_app_server::SessionMetadata {
            thread_id: thread_id.to_owned(),
            session_id: None,
            name: Some("safe fixture".to_owned()),
            created_at: None,
            updated_at: None,
            runtime_status: crate::codex_app_server::RuntimeStatus::Idle,
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
            history_availability: crate::codex_app_server::HistoryAvailability::Unknown,
            origin: crate::codex_app_server::SessionOrigin::LocalCodex,
            usage_link: crate::codex_app_server::UsageLinkAvailability::Unavailable,
        }
    }

    fn wait_for_mutation(
        state: &mut AppState,
        backend: &Backend,
        expected: MutationKind,
    ) -> BackendEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            assert!(std::time::Instant::now() < deadline, "mutation timed out");
            if let Some(event) = backend.try_recv() {
                if matches!(
                    &event,
                    BackendEvent::Mutation { kind, .. } if *kind == expected
                ) {
                    return event;
                }
                apply_backend_event(state, backend, event);
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
