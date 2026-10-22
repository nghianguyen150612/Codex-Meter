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
use super::composer::{
    ComposerFocus, DraftState, EditorError, ModelSelection, PreflightSnapshot, PreparedTask,
    ProjectAssociation, QuotaFreshness, QuotaObservation, ReasoningSelection, TargetKind,
    ValidationStatus, WorkspaceSelection, BUILT_IN_TEMPLATES, MAX_PROMPT_BYTES,
};
use super::drafts::{DraftContents, DraftError, DraftStore};
use super::render;
use super::state::{
    looks_like_path, AppServerStatus, AppState, HistoryMode, Overlay, Screen, SearchInput,
    SearchKind, LIST_PAGE_SIZE,
};
use super::theme::Theme;
use crate::cli::{CliError, TuiArguments};
use crate::codex_app_server::{
    ApprovalChoice, ArchiveFilter, ControllerEvent, ExecutionPhase, RecoveryJournal,
};

type TuiTerminal = Terminal<CrosstermBackend<Stdout>>;

pub fn run(arguments: TuiArguments) -> Result<String, CliError> {
    run_with_screen(arguments, Screen::Overview)
}

pub fn run_composer(arguments: TuiArguments) -> Result<String, CliError> {
    run_with_screen(arguments, Screen::Composer)
}

fn run_with_screen(arguments: TuiArguments, initial_screen: Screen) -> Result<String, CliError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(CliError {
            code: "tui_requires_tty",
            message: format!(
                "`codex-meter {}` requires stdin and stdout to be interactive terminals",
                if initial_screen == Screen::Composer {
                    "compose"
                } else {
                    "tui"
                }
            ),
        });
    }
    let database_path = match arguments.database.as_ref() {
        Some(path) => path.clone(),
        None => crate::cli::default_database_path().map_err(|_| CliError {
            code: "database_location_unavailable",
            message: "could not determine the default Codex Meter database location".to_owned(),
        })?,
    };
    // Read recovery before creating workers or entering raw mode. A slow store
    // read must not leave the terminal hidden or allow input before the warning.
    let recovery = RecoveryJournal::open_default().and_then(|journal| journal.unresolved());
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
    state.active = initial_screen;
    let theme = Theme::detect();
    match recovery {
        Ok(records) => {
            state.recovery_records = records;
            if !state.recovery_records.is_empty() {
                state.overlay = Some(Overlay::ExecutionRecovery);
            }
        }
        Err(error) => state.recovery_error = Some(error.to_string()),
    }
    request_screen(&mut state, &backend, initial_screen);
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
            Event::Paste(value) if state.active == Screen::Composer => {
                redraw |= handle_composer_paste(state, &value);
            }
            Event::Resize(_, _) => redraw = true,
            _ => {}
        }
    }
    Ok(())
}

fn handle_key(state: &mut AppState, backend: &Backend, key: KeyEvent) -> bool {
    if state.active == Screen::Composer {
        return handle_composer_key(state, backend, key);
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        if state.composer.has_unsaved_content() {
            state.active = Screen::Composer;
            state.overlay = Some(Overlay::UnsavedDraftExit);
        } else {
            state.quitting = true;
        }
        return true;
    }
    if state.search.is_some() {
        return handle_search_input(state, backend, key);
    }
    if let Some(overlay) = state.overlay.clone() {
        return handle_overlay(state, backend, overlay, key);
    }
    match key.code {
        KeyCode::Char('q') => {
            if state.composer.has_unsaved_content() {
                state.active = Screen::Composer;
                state.overlay = Some(Overlay::UnsavedDraftExit);
            } else {
                state.quitting = true;
            }
        }
        KeyCode::Tab => switch_screen(state, backend, 1),
        KeyCode::BackTab => switch_screen(state, backend, -1),
        KeyCode::Char('1'..='6') => {
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
        KeyCode::F(6) if !state.recovery_records.is_empty() => {
            state.overlay = Some(Overlay::ExecutionRecovery)
        }
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
        KeyCode::Char('c') if state.active == Screen::Sessions => {
            open_composer_for_selected_session(state, backend)
        }
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

fn handle_composer_key(state: &mut AppState, backend: &Backend, key: KeyEvent) -> bool {
    if key.code == KeyCode::F(6) && !state.recovery_records.is_empty() {
        state.overlay = Some(Overlay::ExecutionRecovery);
        return true;
    }
    if let Some(overlay) = state.overlay.clone() {
        if matches!(
            overlay,
            Overlay::ExecutionConfirm
                | Overlay::ExecutionView
                | Overlay::ExecutionApproval
                | Overlay::ExecutionExitConfirm
        ) {
            return handle_execution_key(state, backend, overlay, key);
        }
        return handle_composer_overlay(state, backend, overlay, key);
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    if key.code == KeyCode::F(5) {
        state.composer.status = Some("Refreshing session, model, and quota metadata...".into());
        refresh_screen(state, backend);
        return true;
    }
    match (key.code, control) {
        (KeyCode::Enter, true) => {
            open_preflight(state);
            return true;
        }
        (KeyCode::Char('s'), true) => {
            start_saving_draft(state);
            return true;
        }
        (KeyCode::Char('m'), true) => {
            open_model_picker(state);
            return true;
        }
        (KeyCode::Char('r'), true) => {
            open_reasoning_picker(state);
            return true;
        }
        (KeyCode::Char('d'), true) => {
            open_draft_library(state);
            return true;
        }
        (KeyCode::Char('c'), true) => {
            if state.composer.has_unsaved_content() {
                state.overlay = Some(Overlay::UnsavedDraftExit);
            } else {
                state.quitting = true;
            }
            return true;
        }
        _ => {}
    }

    if state.composer.focus != ComposerFocus::Prompt {
        if matches!(key.code, KeyCode::Char('q')) {
            if state.composer.dirty {
                state.overlay = Some(Overlay::UnsavedDraftExit);
            } else {
                state.quitting = true;
            }
            return true;
        }
        if matches!(key.code, KeyCode::Char('?') | KeyCode::F(1)) {
            state.overlay = Some(Overlay::Help);
            return true;
        }
        if let KeyCode::Char('1'..='6') = key.code {
            let KeyCode::Char(number) = key.code else {
                unreachable!()
            };
            state.active = Screen::from_index(number as usize - '1' as usize);
            request_screen(state, backend, state.active);
            return true;
        }
    }

    match key.code {
        KeyCode::Esc => {
            state.composer.focus = if state.composer.focus == ComposerFocus::Prompt {
                ComposerFocus::Actions
            } else {
                ComposerFocus::Prompt
            };
            true
        }
        KeyCode::Tab => {
            if state.composer.focus == ComposerFocus::Actions {
                switch_screen(state, backend, 1);
            } else {
                state.composer.focus = state.composer.focus.next();
            }
            true
        }
        KeyCode::BackTab => {
            if state.composer.focus == ComposerFocus::Target {
                switch_screen(state, backend, -1);
            } else {
                state.composer.focus = state.composer.focus.previous();
            }
            true
        }
        KeyCode::Left if state.composer.focus == ComposerFocus::Prompt => {
            state.composer.editor.move_left();
            true
        }
        KeyCode::Right if state.composer.focus == ComposerFocus::Prompt => {
            state.composer.editor.move_right();
            true
        }
        KeyCode::Up if state.composer.focus == ComposerFocus::Prompt => {
            state.composer.editor.move_vertical(-1);
            true
        }
        KeyCode::Down if state.composer.focus == ComposerFocus::Prompt => {
            state.composer.editor.move_vertical(1);
            true
        }
        KeyCode::PageUp if state.composer.focus == ComposerFocus::Prompt => {
            for _ in 0..8 {
                state.composer.editor.move_vertical(-1);
            }
            true
        }
        KeyCode::PageDown if state.composer.focus == ComposerFocus::Prompt => {
            for _ in 0..8 {
                state.composer.editor.move_vertical(1);
            }
            true
        }
        KeyCode::Home if state.composer.focus == ComposerFocus::Prompt => {
            state.composer.editor.move_home();
            true
        }
        KeyCode::End if state.composer.focus == ComposerFocus::Prompt => {
            state.composer.editor.move_end();
            true
        }
        KeyCode::Backspace if state.composer.focus == ComposerFocus::Prompt => {
            if state.composer.editor.backspace() {
                state.composer.invalidate();
            }
            true
        }
        KeyCode::Delete if state.composer.focus == ComposerFocus::Prompt => {
            if state.composer.editor.delete_forward() {
                state.composer.invalidate();
            }
            true
        }
        KeyCode::Enter if state.composer.focus == ComposerFocus::Prompt => {
            if state.composer.editor.insert_char('\n').is_ok() {
                state.composer.invalidate();
            } else {
                report_editor_error(state);
            }
            true
        }
        KeyCode::Char(character)
            if state.composer.focus == ComposerFocus::Prompt && !character.is_control() =>
        {
            if state.composer.editor.insert_char(character).is_ok() {
                state.composer.invalidate();
            } else {
                report_editor_error(state);
            }
            true
        }
        KeyCode::Enter => {
            match state.composer.focus {
                ComposerFocus::Target => open_target_picker(state),
                ComposerFocus::Workspace => select_current_directory(state),
                ComposerFocus::Model => open_model_picker(state),
                ComposerFocus::Reasoning => open_reasoning_picker(state),
                ComposerFocus::Actions => activate_composer_action(state),
                ComposerFocus::Prompt => unreachable!(),
            }
            true
        }
        KeyCode::Char('w') if state.composer.focus == ComposerFocus::Workspace => {
            select_current_directory(state);
            true
        }
        KeyCode::Char('c') if state.composer.focus == ComposerFocus::Workspace => {
            state.composer.workspace = WorkspaceSelection::Unselected;
            state.composer.invalidate();
            state.composer.status = Some("Workspace selection cleared.".to_owned());
            true
        }
        KeyCode::Char('s') if state.composer.focus == ComposerFocus::Actions => {
            start_saving_draft(state);
            true
        }
        KeyCode::Char('l') if state.composer.focus == ComposerFocus::Actions => {
            open_draft_library(state);
            true
        }
        KeyCode::Char('t') if state.composer.focus == ComposerFocus::Actions => {
            state.overlay = Some(Overlay::ComposerTemplatePicker);
            true
        }
        KeyCode::Char('r') if state.composer.focus == ComposerFocus::Actions => {
            open_preflight(state);
            true
        }
        KeyCode::Left | KeyCode::Right if state.composer.focus == ComposerFocus::Actions => {
            let direction = if key.code == KeyCode::Left { -1 } else { 1 };
            state.composer.action_selected =
                (state.composer.action_selected as isize + direction).rem_euclid(4) as usize;
            true
        }
        KeyCode::Char('j') | KeyCode::Down if state.composer.focus == ComposerFocus::Target => {
            state.composer.focus = ComposerFocus::Target;
            open_target_picker(state);
            true
        }
        KeyCode::Char('?') if state.composer.focus != ComposerFocus::Prompt => {
            state.overlay = Some(Overlay::Help);
            true
        }
        _ => false,
    }
}

fn handle_composer_paste(state: &mut AppState, value: &str) -> bool {
    match state.overlay.as_ref() {
        Some(Overlay::ComposerDraftTitle) => {
            let mut accepted = String::new();
            for character in value.chars() {
                if character.is_control() {
                    continue;
                }
                if state.composer.draft_title.len() + accepted.len() + character.len_utf8() > 384 {
                    break;
                }
                accepted.push(character);
            }
            state.composer.draft_title.push_str(&accepted);
            true
        }
        Some(Overlay::ComposerModelPicker) => {
            for character in value.chars().filter(|character| !character.is_control()) {
                if state.composer.model_filter.len() + character.len_utf8() > 128 {
                    break;
                }
                state.composer.model_filter.push(character);
            }
            reset_model_picker_selection(state);
            true
        }
        None if state.composer.focus == ComposerFocus::Prompt => {
            if state.composer.editor.insert_paste(value).is_ok() {
                state.composer.invalidate();
            } else {
                report_editor_error(state);
            }
            true
        }
        _ => false,
    }
}

fn report_editor_error(state: &mut AppState) {
    state.composer.status = Some(match state.composer.editor.error {
        Some(EditorError::TooLarge) => {
            format!("Prompt limit is {MAX_PROMPT_BYTES} bytes; the new input was rejected in full.")
        }
        Some(EditorError::UnsupportedControl) => {
            "Paste contains unsupported control characters; the input was rejected in full."
                .to_owned()
        }
        None => "Prompt edit failed; the current text is unchanged.".to_owned(),
    });
}

fn handle_composer_overlay(
    state: &mut AppState,
    backend: &Backend,
    overlay: Overlay,
    key: KeyEvent,
) -> bool {
    if overlay == Overlay::Help {
        return handle_overlay(state, backend, Overlay::Help, key);
    }
    if overlay == Overlay::ExecutionRecovery {
        return handle_overlay(state, backend, Overlay::ExecutionRecovery, key);
    }
    let text_entry = matches!(
        &overlay,
        Overlay::ComposerDraftTitle | Overlay::ComposerModelPicker
    );
    if (key.code == KeyCode::Char('q') && !text_entry)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
    {
        if state.composer.has_unsaved_content() {
            state.overlay = Some(Overlay::UnsavedDraftExit);
        } else {
            state.quitting = true;
            state.overlay = None;
        }
        return true;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('s') => {
                start_saving_draft(state);
                return true;
            }
            KeyCode::Char('d') => {
                open_draft_library(state);
                return true;
            }
            KeyCode::Char('c') => {
                state.overlay = Some(Overlay::UnsavedDraftExit);
                return true;
            }
            _ => {}
        }
    }
    match overlay {
        Overlay::ComposerModelPicker => match key.code {
            KeyCode::Esc => state.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                state.composer.model_picker_selected =
                    state.composer.model_picker_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let length = filtered_models(state).len() + 1;
                state.composer.model_picker_selected = state
                    .composer
                    .model_picker_selected
                    .saturating_add(1)
                    .min(length.saturating_sub(1));
            }
            KeyCode::PageUp => state.composer.model_picker_selected = 0,
            KeyCode::PageDown => {
                state.composer.model_picker_selected = filtered_models(state).len()
            }
            KeyCode::Backspace => {
                state.composer.model_filter.pop();
                reset_model_picker_selection(state);
            }
            KeyCode::Enter => select_model_from_picker(state),
            KeyCode::Char(character) if !character.is_control() => {
                if state.composer.model_filter.len() + character.len_utf8() <= 128 {
                    state.composer.model_filter.push(character);
                    reset_model_picker_selection(state);
                }
            }
            _ => return false,
        },
        Overlay::ComposerReasoningPicker => match key.code {
            KeyCode::Esc => state.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                state.composer.model_picker_selected =
                    state.composer.model_picker_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let length = reasoning_choices(state).len();
                state.composer.model_picker_selected = state
                    .composer
                    .model_picker_selected
                    .saturating_add(1)
                    .min(length.saturating_sub(1));
            }
            KeyCode::Enter => select_reasoning_from_picker(state),
            _ => return false,
        },
        Overlay::ComposerTargetPicker => match key.code {
            KeyCode::Esc => state.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                state.composer.session_picker_selected =
                    state.composer.session_picker_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let length = state
                    .sessions
                    .value
                    .as_ref()
                    .map_or(1, |sessions| sessions.len() + 1);
                state.composer.session_picker_selected = state
                    .composer
                    .session_picker_selected
                    .saturating_add(1)
                    .min(length.saturating_sub(1));
            }
            KeyCode::Enter => select_target_from_picker(state),
            _ => return false,
        },
        Overlay::ComposerDraftTitle => match key.code {
            KeyCode::Esc => state.overlay = None,
            KeyCode::Backspace => {
                state.composer.draft_title.pop();
            }
            KeyCode::Enter => save_draft_now(state),
            KeyCode::Char(character) if !character.is_control() => {
                if state.composer.draft_title.len() + character.len_utf8() <= 384 {
                    state.composer.draft_title.push(character);
                }
            }
            _ => return false,
        },
        Overlay::ComposerDraftLibrary => match key.code {
            KeyCode::Esc => state.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                state.composer.draft_selected = state.composer.draft_selected.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.composer.draft_selected = state
                    .composer
                    .draft_selected
                    .saturating_add(1)
                    .min(state.composer.saved_drafts().len().saturating_sub(1));
            }
            KeyCode::Enter => load_selected_draft(state),
            KeyCode::Char('d') => {
                if !state.composer.saved_drafts().is_empty() {
                    state.overlay = Some(Overlay::ComposerDraftDeleteConfirm);
                }
            }
            _ => return false,
        },
        Overlay::ComposerDraftDeleteConfirm => match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                state.overlay = Some(Overlay::ComposerDraftLibrary)
            }
            KeyCode::Char('y') | KeyCode::Enter => delete_selected_draft(state),
            _ => return false,
        },
        Overlay::ComposerTemplatePicker => match key.code {
            KeyCode::Esc => state.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                state.composer.selected_template =
                    state.composer.selected_template.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.composer.selected_template = state
                    .composer
                    .selected_template
                    .saturating_add(1)
                    .min(BUILT_IN_TEMPLATES.len().saturating_sub(1));
            }
            KeyCode::Enter => apply_template(state),
            _ => return false,
        },
        Overlay::ComposerPreflight => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => state.overlay = None,
            KeyCode::Up | KeyCode::Char('k') => {
                state.composer.preflight_scroll = state.composer.preflight_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.composer.preflight_scroll = state.composer.preflight_scroll.saturating_add(1)
            }
            KeyCode::Char('s') => start_saving_draft(state),
            KeyCode::Char('e') => begin_execution_preflight(state, backend),
            _ => return false,
        },
        Overlay::UnsavedDraftExit => match key.code {
            KeyCode::Esc | KeyCode::Char('n') => state.overlay = None,
            KeyCode::Char('y') | KeyCode::Enter => {
                state.overlay = None;
                state.quitting = true;
            }
            KeyCode::Char('s') => start_saving_draft(state),
            _ => return false,
        },
        _ => return handle_overlay(state, backend, overlay, key),
    }
    true
}

fn filtered_models(state: &AppState) -> Vec<&crate::codex_app_server::ModelInfo> {
    let filter = state.composer.model_filter.to_lowercase();
    state
        .models
        .value
        .as_ref()
        .map(|catalog| {
            catalog
                .models
                .iter()
                .filter(|model| {
                    filter.is_empty()
                        || model.id.to_lowercase().contains(&filter)
                        || model.display_name.to_lowercase().contains(&filter)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn open_model_picker(state: &mut AppState) {
    state.composer.focus = ComposerFocus::Model;
    state.composer.model_filter.clear();
    reset_model_picker_selection(state);
    if state.models.value.is_none() && state.models.error.is_none() {
        state.composer.status = Some("Loading live model catalog...".to_owned());
    }
    state.overlay = Some(Overlay::ComposerModelPicker);
}

fn reset_model_picker_selection(state: &mut AppState) {
    let selection = if state.composer.model_filter.is_empty() {
        match &state.composer.model_selection {
            ModelSelection::Explicit(selected) => filtered_models(state)
                .iter()
                .position(|model| &model.id == selected)
                .map_or(0, |index| index + 1),
            ModelSelection::Inherit | ModelSelection::Unavailable => 0,
        }
    } else if filtered_models(state).is_empty() {
        0
    } else {
        1
    };
    state.composer.model_picker_selected = selection;
}

fn select_model_from_picker(state: &mut AppState) {
    let selected = state.composer.model_picker_selected;
    if selected == 0 {
        let previous_effort = match &state.composer.reasoning_selection {
            ReasoningSelection::Explicit(value) => Some(value.clone()),
            ReasoningSelection::NeedsChoice(value) => value.clone(),
            _ => None,
        };
        state.composer.model_selection = ModelSelection::Inherit;
        if previous_effort.is_some() {
            state.composer.reasoning_selection = ReasoningSelection::NeedsChoice(previous_effort);
        }
        state.composer.invalidate();
        state.overlay = None;
        return;
    }
    let Some(model) = filtered_models(state)
        .get(selected - 1)
        .map(|model| (*model).clone())
    else {
        state.composer.status = Some("No live model is available at this selection.".to_owned());
        return;
    };
    let previous_effort = match &state.composer.reasoning_selection {
        ReasoningSelection::Explicit(value) => Some(value.clone()),
        ReasoningSelection::NeedsChoice(value) => value.clone(),
        _ => None,
    };
    let unsupported_effort = previous_effort
        .as_ref()
        .filter(|effort| {
            !model
                .supported_reasoning_efforts
                .iter()
                .any(|supported| &supported.id == *effort)
        })
        .cloned();
    state.composer.model_selection = ModelSelection::Explicit(model.id.clone());
    state.composer.invalidate();
    if let Some(effort) = unsupported_effort {
        state.composer.reasoning_selection = ReasoningSelection::NeedsChoice(Some(effort));
        state.composer.status = Some(
            "The previous reasoning effort is unsupported by this model; choose an option.".into(),
        );
    }
    state.overlay = None;
}

fn reasoning_choices(state: &AppState) -> Vec<(ReasoningSelection, String)> {
    let mut choices = vec![(
        ReasoningSelection::Inherit,
        "Inherit from Codex configuration".to_owned(),
    )];
    let ModelSelection::Explicit(model_id) = &state.composer.model_selection else {
        return choices;
    };
    let Some(model) = state
        .models
        .value
        .as_ref()
        .and_then(|catalog| catalog.models.iter().find(|model| &model.id == model_id))
    else {
        return choices;
    };
    choices.push((
        ReasoningSelection::ModelDefault,
        format!(
            "Model reported default ({})",
            model.default_reasoning_effort
        ),
    ));
    choices.extend(model.supported_reasoning_efforts.iter().map(|effort| {
        (
            ReasoningSelection::Explicit(effort.id.clone()),
            if effort.description.is_empty() {
                effort.id.clone()
            } else {
                format!("{} — {}", effort.id, effort.description)
            },
        )
    }));
    choices
}

fn open_reasoning_picker(state: &mut AppState) {
    state.composer.focus = ComposerFocus::Reasoning;
    state.composer.model_picker_selected = 0;
    state.overlay = Some(Overlay::ComposerReasoningPicker);
}

fn select_reasoning_from_picker(state: &mut AppState) {
    if let Some((selection, _)) = reasoning_choices(state).get(state.composer.model_picker_selected)
    {
        state.composer.reasoning_selection = selection.clone();
        state.composer.invalidate();
        state.overlay = None;
    } else {
        state.composer.status = Some("Choose an effort supported by the selected model.".into());
    }
}

fn open_target_picker(state: &mut AppState) {
    state.composer.focus = ComposerFocus::Target;
    state.composer.session_picker_selected = state
        .composer
        .selected_thread_id
        .as_ref()
        .and_then(|thread_id| {
            state
                .sessions
                .value
                .as_ref()?
                .iter()
                .position(|session| &session.thread_id == thread_id)
                .map(|index| index + 1)
        })
        .unwrap_or(0);
    state.overlay = Some(Overlay::ComposerTargetPicker);
}

fn select_target_from_picker(state: &mut AppState) {
    let selection = state.composer.session_picker_selected;
    if selection == 0 {
        state.composer.target_kind = TargetKind::NewThread;
        state.composer.selected_thread_id = None;
    } else if let Some(session) = state
        .sessions
        .value
        .as_ref()
        .and_then(|sessions| sessions.get(selection - 1))
    {
        state.composer.target_kind = TargetKind::ExistingThread;
        state.composer.selected_thread_id = Some(session.thread_id.clone());
    } else {
        state.composer.status =
            Some("Session metadata is unavailable; no target was selected.".into());
        return;
    }
    state.composer.invalidate();
    state.overlay = None;
}

fn select_current_directory(state: &mut AppState) {
    match std::env::current_dir() {
        Ok(path) => {
            state.composer.workspace = WorkspaceSelection::CurrentDirectory(path);
            state.composer.invalidate();
            state.composer.status = Some(
                "Current working directory selected explicitly; its path stays in memory only."
                    .to_owned(),
            );
        }
        Err(_) => {
            state.composer.status = Some("Current working directory is unavailable.".to_owned())
        }
    }
}

fn open_composer_for_selected_session(state: &mut AppState, backend: &Backend) {
    let Some(thread_id) = state
        .selected_session_metadata()
        .map(|session| session.thread_id.clone())
    else {
        state.status = Some("Select a session before opening the Composer.".to_owned());
        return;
    };
    state.active = Screen::Composer;
    state.composer.target_kind = TargetKind::ExistingThread;
    state.composer.selected_thread_id = Some(thread_id);
    state.composer.focus = ComposerFocus::Prompt;
    state.composer.invalidate();
    request_screen(state, backend, Screen::Composer);
}

fn activate_composer_action(state: &mut AppState) {
    match state.composer.action_selected {
        0 => start_saving_draft(state),
        1 => open_draft_library(state),
        2 => state.overlay = Some(Overlay::ComposerTemplatePicker),
        _ => open_preflight(state),
    }
}

fn start_saving_draft(state: &mut AppState) {
    if state.composer.loaded_draft_id.is_some() {
        save_draft_now(state);
    } else {
        state.composer.draft_title.clear();
        state.overlay = Some(Overlay::ComposerDraftTitle);
    }
}

fn draft_contents(state: &AppState, title: String) -> DraftContents {
    DraftContents {
        title,
        prompt: state.composer.editor.text().to_owned(),
        target_kind: state.composer.target_kind,
        target_thread_id: state.composer.selected_thread_id.clone(),
        workspace_selected: matches!(
            &state.composer.workspace,
            WorkspaceSelection::CurrentDirectory(_)
        ),
        model_selection: state.composer.model_selection.clone(),
        reasoning_selection: state.composer.reasoning_selection.clone(),
    }
}

fn save_draft_now(state: &mut AppState) {
    let title = state.composer.draft_title.trim().to_owned();
    if title.is_empty() {
        state.composer.status = Some("Enter a title to save this draft.".to_owned());
        return;
    }
    let contents = draft_contents(state, title.clone());
    let result = match DraftStore::open_default() {
        Ok(store) => match (
            state.composer.loaded_draft_id.as_deref(),
            state.composer.loaded_draft_digest.as_deref(),
        ) {
            (Some(id), Some(digest)) => store.replace(id, digest, contents),
            _ => store.save_new(contents),
        },
        Err(error) => Err(error),
    };
    match result {
        Ok(draft) => {
            state.composer.loaded_draft_id = Some(draft.id.clone());
            state.composer.loaded_draft_digest = Some(draft.digest);
            state.composer.draft_title = draft.contents.title;
            state.composer.dirty = false;
            state.composer.draft_state = DraftState::Saved;
            state.composer.prepared_task = None;
            state.composer.status = Some(
                "Draft saved by explicit request. Saved prompts are plaintext on disk.".to_owned(),
            );
            state.overlay = None;
        }
        Err(error) => {
            state.composer.status = Some(error.message().to_owned());
            if error != DraftError::Conflict {
                state.overlay = None;
            }
        }
    }
}

fn open_draft_library(state: &mut AppState) {
    match DraftStore::open_default().and_then(|store| store.list()) {
        Ok(listing) => {
            let skipped = listing.skipped_entries;
            let overflowed = listing.overflowed;
            state.composer.draft_listing = Some(listing);
            state.composer.draft_selected = 0;
            state.overlay = Some(Overlay::ComposerDraftLibrary);
            state.composer.status = if overflowed {
                Some("Draft listing is capped at 100 files; delete listed drafts before saving or replacing.".to_owned())
            } else if skipped > 0 {
                Some(format!(
                    "{skipped} malformed or unsafe saved draft entry/entries were skipped."
                ))
            } else {
                None
            };
        }
        Err(error) => state.composer.status = Some(error.message().to_owned()),
    }
}

fn load_selected_draft(state: &mut AppState) {
    if state.composer.dirty {
        state.composer.status = Some(
            "Save the current changes before loading another draft; current text was retained."
                .to_owned(),
        );
        return;
    }
    let Some(summary) = state
        .composer
        .saved_drafts()
        .get(state.composer.draft_selected)
        .cloned()
    else {
        state.composer.status = Some("No saved drafts are available.".to_owned());
        return;
    };
    match DraftStore::open_default().and_then(|store| store.load(&summary.id)) {
        Ok(draft) => {
            match super::composer::PromptEditor::from_text(draft.contents.prompt) {
                Ok(editor) => state.composer.editor = editor,
                Err(_) => {
                    state.composer.status = Some(
                        "The saved prompt exceeds the current size limit; it was not loaded."
                            .to_owned(),
                    );
                    return;
                }
            }
            state.composer.target_kind = draft.contents.target_kind;
            state.composer.selected_thread_id = draft.contents.target_thread_id;
            state.composer.workspace = WorkspaceSelection::Unselected;
            state.composer.model_selection = draft.contents.model_selection;
            state.composer.reasoning_selection = draft.contents.reasoning_selection;
            state.composer.draft_title = draft.contents.title;
            state.composer.loaded_draft_id = Some(draft.id);
            state.composer.loaded_draft_digest = Some(draft.digest);
            state.composer.dirty = false;
            state.composer.draft_state = DraftState::Saved;
            state.composer.prepared_task = None;
            state.composer.status = Some(
                if draft.contents.workspace_selected
                    && draft.contents.target_kind == TargetKind::NewThread
                {
                    "Saved draft loaded into memory. Reconfirm the workspace before preflight; no task was submitted.".into()
                } else {
                    "Saved draft loaded into memory; no task was submitted.".into()
                },
            );
            state.overlay = None;
        }
        Err(error) => state.composer.status = Some(error.message().to_owned()),
    }
}

fn delete_selected_draft(state: &mut AppState) {
    let Some(summary) = state
        .composer
        .saved_drafts()
        .get(state.composer.draft_selected)
        .cloned()
    else {
        state.overlay = Some(Overlay::ComposerDraftLibrary);
        return;
    };
    let result = DraftStore::open_default().and_then(|store| {
        let draft = store.load(&summary.id)?;
        store.delete(&summary.id, &draft.digest)
    });
    match result {
        Ok(()) => {
            open_draft_library(state);
            if state.composer.status.is_none() {
                state.composer.status =
                    Some("Saved draft deleted from local draft storage.".into());
            }
        }
        Err(error) => {
            state.composer.status = Some(error.message().to_owned());
            state.overlay = Some(Overlay::ComposerDraftLibrary);
        }
    }
}

fn apply_template(state: &mut AppState) {
    let Some((name, template)) = BUILT_IN_TEMPLATES.get(state.composer.selected_template) else {
        state.overlay = None;
        return;
    };
    match state.composer.editor.insert_paste(template) {
        Ok(()) => {
            state.composer.invalidate();
            state.composer.status = Some(format!("{name} template inserted for editing."));
        }
        Err(_) => report_editor_error(state),
    }
    state.composer.focus = ComposerFocus::Prompt;
    state.overlay = None;
}

fn open_preflight(state: &mut AppState) {
    let task = prepare_task(state);
    state.composer.draft_state = DraftState::Validated;
    state.composer.draft_state = match task.validation_status {
        ValidationStatus::Ready => DraftState::PreflightReady,
        ValidationStatus::Incomplete | ValidationStatus::Invalid => DraftState::Invalid,
    };
    state.composer.prepared_task = Some(task);
    state.composer.preflight_scroll = 0;
    state.overlay = Some(Overlay::ComposerPreflight);
}

fn begin_execution_preflight(state: &mut AppState, backend: &Backend) {
    let task = prepare_task(state);
    if task.validation_status != ValidationStatus::Ready {
        state.composer.status = Some(
            task.validation_messages
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(" "),
        );
        state.overlay = Some(Overlay::ComposerPreflight);
        return;
    }
    state.composer.prepared_task = Some(prepare_task(state));
    state.composer.draft_state = DraftState::PreflightReady;
    let id = state.next_request_id();
    state.execution = super::state::ExecutionUiState {
        id,
        phase: ExecutionPhase::Ready,
        pending: true,
        status: Some("Checking the current Codex model and security configuration. No thread or turn is being started.".to_owned()),
        ..super::state::ExecutionUiState::default()
    };
    state.overlay = Some(Overlay::ExecutionConfirm);
    if let Err(error) = backend.prepare_execution(id, task) {
        state.execution.pending = false;
        state.execution.status = Some(error);
    }
}

fn handle_execution_key(
    state: &mut AppState,
    backend: &Backend,
    overlay: Overlay,
    key: KeyEvent,
) -> bool {
    if state.execution.pending {
        if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
            return true;
        }
        return false;
    }
    match overlay {
        Overlay::ExecutionConfirm => match key.code {
            KeyCode::Esc | KeyCode::Char('n') => {
                if state.execution.preflight.is_none() {
                    state.execution = super::state::ExecutionUiState::default();
                    state.overlay = Some(Overlay::ComposerPreflight);
                    return true;
                }
                let id = state.execution.id;
                state.execution.pending = true;
                if let Err(error) = backend.cancel_execution(id) {
                    state.execution.pending = false;
                    state.execution.status = Some(error);
                }
                true
            }
            KeyCode::Enter | KeyCode::Char('y') => {
                if state.execution.phase != ExecutionPhase::AwaitingConfirmation {
                    return true;
                }
                let Some(preflight) = state.execution.preflight.as_ref() else {
                    return true;
                };
                if preflight.existing_settings_pending {
                    state.execution.pending = true;
                    state.execution.status = Some("Loading the selected thread's current security settings. No turn will start.".to_owned());
                    if let Err(error) = backend.inspect_execution(state.execution.id) {
                        state.execution.pending = false;
                        state.execution.status = Some(error);
                    }
                } else if preflight.sandbox.is_some()
                    && preflight.approval.is_some()
                    && preflight.network_access == Some(false)
                {
                    state.execution.phase = ExecutionPhase::Submitting;
                    state.execution.pending = true;
                    state.execution.status =
                        Some("Submitting the confirmed task to Codex.".to_owned());
                    state.overlay = Some(Overlay::ExecutionView);
                    if let Err(error) = backend.submit_execution(state.execution.id) {
                        state.execution.pending = false;
                        state.execution.phase = ExecutionPhase::Failed;
                        state.execution.status = Some(error);
                    }
                }
                true
            }
            _ => false,
        },
        Overlay::ExecutionApproval => {
            if key.code == KeyCode::Char('i')
                && matches!(
                    state.execution.phase,
                    ExecutionPhase::Running
                        | ExecutionPhase::SubmissionAccepted
                        | ExecutionPhase::AwaitingApproval
                )
            {
                state.execution.pending = true;
                state.execution.status = Some("Requesting interruption; pending approvals will be denied while Codex reports the final turn state.".to_owned());
                if let Err(error) = backend.interrupt_execution(state.execution.id) {
                    state.execution.pending = false;
                    state.execution.status = Some(error);
                } else {
                    state.execution.approvals.clear();
                    state.overlay = Some(Overlay::ExecutionView);
                }
                return true;
            }
            let Some(approval) = state.execution.approvals.front() else {
                state.overlay = Some(Overlay::ExecutionView);
                return true;
            };
            let request_id = approval.request_id.clone();
            let allow_available = approval.allow_once_available;
            let has_cancel = matches!(
                approval.kind,
                crate::codex_app_server::ApprovalKind::CommandExecution
                    | crate::codex_app_server::ApprovalKind::FileChange
            );
            let choice = match key.code {
                KeyCode::Enter | KeyCode::Char('n') | KeyCode::Char('d') => {
                    Some(ApprovalChoice::Deny)
                }
                KeyCode::Char('a') if allow_available => Some(ApprovalChoice::AllowOnce),
                KeyCode::Char('c') if has_cancel => Some(ApprovalChoice::Cancel),
                KeyCode::Esc => Some(ApprovalChoice::Deny),
                _ => None,
            };
            if let Some(choice) = choice {
                state.execution.pending = true;
                if let Err(error) = backend.decide_approval(state.execution.id, request_id, choice)
                {
                    state.execution.pending = false;
                    state.execution.status = Some(error);
                } else {
                    state.execution.approvals.pop_front();
                    state.overlay = if state.execution.approvals.is_empty() {
                        Some(Overlay::ExecutionView)
                    } else {
                        Some(Overlay::ExecutionApproval)
                    };
                }
            }
            true
        }
        Overlay::ExecutionView => match key.code {
            KeyCode::Esc | KeyCode::Enter if state.execution.phase.terminal() => {
                state.overlay = None;
                state.active = Screen::Overview;
                true
            }
            KeyCode::Char('q') | KeyCode::Char('c') if !state.execution.phase.terminal() => {
                state.execution.exit_choice = 0;
                state.overlay = Some(Overlay::ExecutionExitConfirm);
                true
            }
            KeyCode::Char('i')
                if matches!(
                    state.execution.phase,
                    ExecutionPhase::Running
                        | ExecutionPhase::SubmissionAccepted
                        | ExecutionPhase::AwaitingApproval
                ) =>
            {
                state.execution.pending = true;
                state.execution.status = Some(
                    "Requesting interruption from Codex; waiting for turn completion.".to_owned(),
                );
                if let Err(error) = backend.interrupt_execution(state.execution.id) {
                    state.execution.pending = false;
                    state.execution.status = Some(error);
                }
                true
            }
            KeyCode::Char('r') if state.execution.phase == ExecutionPhase::OutcomeUnknown => {
                state.execution.phase = ExecutionPhase::Reconciling;
                state.execution.pending = true;
                state.execution.status = Some("Reconciling with the Codex session.".to_owned());
                if let Err(error) = backend.reconcile_execution(state.execution.id) {
                    state.execution.pending = false;
                    state.execution.phase = ExecutionPhase::OutcomeUnknown;
                    state.execution.status = Some(error);
                }
                true
            }
            KeyCode::Char('a') if !state.execution.approvals.is_empty() => {
                state.overlay = Some(Overlay::ExecutionApproval);
                true
            }
            KeyCode::Char('d') => {
                state.execution.show_details = !state.execution.show_details;
                state.execution.scroll = 0;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.execution.scroll = state.execution.scroll.saturating_sub(1);
                true
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.execution.scroll = state.execution.scroll.saturating_add(1);
                true
            }
            KeyCode::PageUp => {
                state.execution.scroll = state.execution.scroll.saturating_sub(12);
                true
            }
            KeyCode::PageDown => {
                state.execution.scroll = state.execution.scroll.saturating_add(12);
                true
            }
            _ => false,
        },
        Overlay::ExecutionExitConfirm => match key.code {
            KeyCode::Esc | KeyCode::Char('c') => {
                state.overlay = Some(Overlay::ExecutionView);
                true
            }
            KeyCode::Char('i') | KeyCode::Enter => {
                if matches!(
                    state.execution.phase,
                    ExecutionPhase::Running
                        | ExecutionPhase::SubmissionAccepted
                        | ExecutionPhase::AwaitingApproval
                ) {
                    state.execution.pending = true;
                    state.status = Some("An active Codex turn cannot be detached safely here. Requesting interruption; keep this view open until the final turn state arrives.".to_owned());
                    if let Err(error) = backend.interrupt_execution(state.execution.id) {
                        state.execution.pending = false;
                        state.execution.status = Some(error);
                    } else {
                        state.overlay = Some(Overlay::ExecutionView);
                    }
                }
                true
            }
            _ => false,
        },
        _ => false,
    }
}

fn prepare_task(state: &AppState) -> PreparedTask {
    let mut messages = Vec::new();
    if state.composer.editor.text().trim().is_empty() {
        messages.push("Prompt is empty.".to_owned());
    }
    let (target_id, project_association) = match state.composer.target_kind {
        TargetKind::NewThread => {
            if !matches!(
                &state.composer.workspace,
                WorkspaceSelection::CurrentDirectory(_)
            ) {
                messages.push("Choose a workspace before preparing a new thread.".to_owned());
            }
            (None, None)
        }
        TargetKind::ExistingThread => match state.composer.selected_thread_id.as_ref() {
            None => {
                messages.push("Select an existing local session.".to_owned());
                (None, None)
            }
            Some(thread_id) => {
                let session = state.sessions.value.as_ref().and_then(|sessions| {
                    sessions
                        .iter()
                        .find(|session| &session.thread_id == thread_id)
                });
                match session {
                    Some(session) if session.archived == Some(true) => {
                        messages.push(
                            "Archived session metadata cannot be prepared for a new turn.".into(),
                        );
                        (Some(thread_id.clone()), session.project_id.clone())
                    }
                    Some(session) => {
                        if matches!(
                            &session.runtime_status,
                            crate::codex_app_server::RuntimeStatus::Active
                        ) {
                            messages.push("Session is active; P036 must verify turn availability before execution.".into());
                        } else {
                            messages.push("Session is listed in local history; P036 must revalidate availability.".into());
                        }
                        (Some(thread_id.clone()), session.project_id.clone())
                    }
                    None => {
                        messages.push(
                            "Selected session is unavailable in the current metadata list.".into(),
                        );
                        (Some(thread_id.clone()), None)
                    }
                }
            }
        },
    };
    let mut invalid = false;
    if state.app_server_status != AppServerStatus::Connected {
        messages.push("Codex App Server is unavailable or has not completed discovery.".into());
        invalid = true;
    }
    if state
        .models
        .value
        .as_ref()
        .is_none_or(|catalog| catalog.availability != "live")
    {
        messages.push(
            "Live model catalog is unavailable; model compatibility cannot be verified.".into(),
        );
        invalid = true;
    }
    let model_id = match &state.composer.model_selection {
        ModelSelection::Inherit => {
            messages.push(
                "Model is inherited from Codex configuration; the resolved model is unknown."
                    .into(),
            );
            None
        }
        ModelSelection::Unavailable => {
            messages.push("Model selection is unavailable.".into());
            invalid = true;
            None
        }
        ModelSelection::Explicit(model_id) => {
            let present =
                state.models.value.as_ref().is_some_and(|catalog| {
                    catalog.models.iter().any(|model| &model.id == model_id)
                });
            if !present {
                messages.push("Selected model is not present in the current live catalog.".into());
                invalid = true;
            }
            Some(model_id.as_str())
        }
    };
    let selected_model = model_id.and_then(|model_id| {
        state
            .models
            .value
            .as_ref()
            .and_then(|catalog| catalog.models.iter().find(|model| model.id == model_id))
    });
    match &state.composer.reasoning_selection {
        ReasoningSelection::Inherit => {
            messages.push("Reasoning effort is inherited from Codex configuration.".into())
        }
        ReasoningSelection::ModelDefault => match selected_model {
            Some(model) => messages.push(format!(
                "Reasoning effort uses the reported model default: {}.",
                model.default_reasoning_effort
            )),
            None => {
                messages.push(
                    "A model-reported reasoning default requires an explicit available model."
                        .into(),
                );
                invalid = true;
            }
        },
        ReasoningSelection::Explicit(effort) => match selected_model {
            Some(model)
                if model
                    .supported_reasoning_efforts
                    .iter()
                    .any(|supported| &supported.id == effort) => {}
            Some(_) => {
                messages.push("Selected reasoning effort is unsupported by this model.".into());
                invalid = true;
            }
            None => {
                messages.push(
                    "An explicit reasoning effort cannot be checked without a selected model."
                        .into(),
                );
                invalid = true;
            }
        },
        ReasoningSelection::NeedsChoice(previous) => {
            messages.push(match previous {
                Some(previous) => format!("Choose a supported reasoning effort; previous value '{previous}' is not accepted automatically."),
                None => "Choose a reasoning effort supported by the selected model.".into(),
            });
            invalid = true;
        }
        ReasoningSelection::Unavailable => {
            messages.push("Reasoning capability data is unavailable.".into());
            invalid = true;
        }
    }
    if let Some(version) = state
        .overview
        .value
        .as_ref()
        .and_then(|overview| overview.telemetry_version.as_deref())
    {
        if version != crate::telemetry::COMPATIBILITY_CLI_VERSION {
            messages.push("Quota/token source compatibility is unverified for the recorded telemetry version.".into());
        }
    } else {
        messages.push(
            "Source compatibility is unavailable because no telemetry version was observed.".into(),
        );
    }
    let quota_snapshot = preflight_quota_snapshot(state);
    if quota_snapshot.five_hour.is_none() {
        messages.push("Five-hour quota evidence is unavailable.".into());
    }
    if quota_snapshot.weekly.is_none() {
        messages.push("Weekly quota evidence is unavailable.".into());
    }
    messages
        .push("Expected task quota cost: Unavailable — insufficient calibrated evidence.".into());
    let has_blocking = invalid
        || messages.iter().any(|message| {
            message.starts_with("Prompt is empty")
                || message.starts_with("Choose a workspace")
                || message.starts_with("Select an existing")
                || message.starts_with("Archived session")
                || message.starts_with("Selected session is unavailable")
        });
    let validation_status = if has_blocking {
        if state.composer.editor.text().trim().is_empty()
            || (state.composer.target_kind == TargetKind::NewThread
                && matches!(&state.composer.workspace, WorkspaceSelection::Unselected))
            || state.app_server_status != AppServerStatus::Connected
            || state.models.value.is_none()
        {
            ValidationStatus::Incomplete
        } else {
            ValidationStatus::Invalid
        }
    } else {
        ValidationStatus::Ready
    };
    let model_inherited = matches!(&state.composer.model_selection, ModelSelection::Inherit);
    let reasoning_inherited = matches!(
        &state.composer.reasoning_selection,
        ReasoningSelection::Inherit
    );
    let configuration_source = match (model_inherited, reasoning_inherited) {
        (true, true) => "Model and reasoning inherited from Codex configuration",
        (true, false) => "Model inherited; reasoning prepared in Codex Meter",
        (false, true) => "Model prepared in Codex Meter; reasoning inherited",
        (false, false) => "Model and reasoning prepared in Codex Meter",
    };
    PreparedTask {
        target_kind: state.composer.target_kind,
        target_thread_id: target_id,
        workspace: state.composer.workspace.clone(),
        project_association: project_association.map_or(ProjectAssociation::Unavailable, |alias| {
            ProjectAssociation::ResponseLocalAlias(alias)
        }),
        model_selection: state.composer.model_selection.clone(),
        reasoning_selection: state.composer.reasoning_selection.clone(),
        prompt_text: state.composer.editor.text().to_owned(),
        configuration_source,
        validation_status,
        validation_messages: messages,
        preflight_snapshot: quota_snapshot,
    }
}

fn preflight_quota_snapshot(state: &AppState) -> PreflightSnapshot {
    let Some(overview) = state.overview.value.as_ref() else {
        return PreflightSnapshot {
            quota_coverage: Some("Local quota evidence is unavailable.".to_owned()),
            ..PreflightSnapshot::default()
        };
    };
    let Some(timeline) = overview.quota.as_ref() else {
        return PreflightSnapshot {
            quota_coverage: Some("No local quota sample is available.".to_owned()),
            ..PreflightSnapshot::default()
        };
    };
    let coverage = &timeline.evidence_quality;
    let coverage_message = if coverage.unsupported_source_versions > 0 {
        "Unsupported source version observed; evidence may be incomplete."
    } else if coverage.known_partial_collection > 0
        || coverage.malformed_or_incomplete_evidence > 0
        || coverage.source_continuity_ambiguities > 0
    {
        "Partial local evidence coverage; account-wide coverage is unavailable."
    } else {
        "Local account-meter snapshots only; source coverage is not account-wide."
    };
    let as_observation = |sample: &Option<crate::storage::QuotaSnapshotEvidence>| {
        sample.as_ref().map(|sample| {
            let freshness = if !sample.used_percent.is_finite()
                || !(0.0..=100.0).contains(&sample.used_percent)
            {
                QuotaFreshness::Invalid
            } else {
                match time::OffsetDateTime::parse(
                    &sample.observed_at,
                    &time::format_description::well_known::Rfc3339,
                ) {
                    Err(_) => QuotaFreshness::TimeUnavailable,
                    Ok(observed)
                        if observed - time::OffsetDateTime::now_utc()
                            > time::Duration::minutes(5) =>
                    {
                        QuotaFreshness::TimeUnavailable
                    }
                    Ok(observed)
                        if time::OffsetDateTime::now_utc() - observed
                            > time::Duration::hours(24) =>
                    {
                        QuotaFreshness::Stale
                    }
                    Ok(_) => QuotaFreshness::Fresh,
                }
            };
            QuotaObservation {
                used_percent: sample.used_percent,
                observed_at: sample.observed_at.clone(),
                reset_at: sample.reset_at.clone(),
                freshness,
            }
        })
    };
    let captured_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .ok();
    PreflightSnapshot {
        five_hour: as_observation(&timeline.five_hour),
        weekly: as_observation(&timeline.weekly),
        quota_coverage: Some(coverage_message.to_owned()),
        captured_at,
    }
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
        if overlay == Overlay::ExecutionRecovery {
            state.overlay = None;
            state.recovery_show_details = false;
            return true;
        }
        if state.composer.dirty {
            state.active = Screen::Composer;
            state.overlay = Some(Overlay::UnsavedDraftExit);
        } else {
            state.quitting = true;
        }
        return true;
    }
    match overlay {
        Overlay::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?')) {
                state.overlay = None;
            }
        }
        Overlay::ExecutionRecovery => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                state.overlay = None;
                state.recovery_show_details = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.recovery_selected = state.recovery_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.recovery_selected = state
                    .recovery_selected
                    .saturating_add(1)
                    .min(state.recovery_records.len().saturating_sub(1));
            }
            KeyCode::Enter | KeyCode::Char('v') => {
                state.recovery_show_details = !state.recovery_show_details;
            }
            KeyCode::Char('r') if !state.recovery_pending => {
                if let Some(record) = state.recovery_records.get(state.recovery_selected) {
                    let execution_id = record.execution_id.clone();
                    let id = state.next_request_id();
                    state.recovery_request_id = id;
                    state.recovery_pending = true;
                    state.status = Some("Inspecting the exact recorded thread and turn using read-only App Server requests.".to_owned());
                    if let Err(error) = backend.reconcile_recovery(id, execution_id) {
                        state.recovery_pending = false;
                        state.status = Some(error);
                    }
                }
            }
            KeyCode::Char('i') if !state.recovery_pending => {
                if let Some(record) = state.recovery_records.get(state.recovery_selected) {
                    if record.last_authoritative_status
                        == Some(crate::codex_app_server::AuthoritativeStatus::InProgress)
                    {
                        let execution_id = record.execution_id.clone();
                        let id = state.next_request_id();
                        state.recovery_request_id = id;
                        state.recovery_pending = true;
                        state.status = Some("Checking the exact recorded turn; an interrupt will be sent only if Codex still reports it running.".to_owned());
                        if let Err(error) = backend.interrupt_recovery(id, execution_id) {
                            state.recovery_pending = false;
                            state.status = Some(error);
                        }
                    }
                }
            }
            KeyCode::Char('a') if !state.recovery_pending => {
                if let Some(record) = state.recovery_records.get(state.recovery_selected) {
                    let execution_id = record.execution_id.clone();
                    let id = state.next_request_id();
                    state.recovery_request_id = id;
                    state.recovery_pending = true;
                    state.status = Some(
                        "Acknowledging the recovery warning; the execution remains unresolved."
                            .to_owned(),
                    );
                    if let Err(error) = backend.acknowledge_recovery(id, execution_id) {
                        state.recovery_pending = false;
                        state.status = Some(error);
                    }
                }
            }
            KeyCode::Char('f') if !state.recovery_pending => {
                let id = state.next_request_id();
                state.recovery_request_id = id;
                state.recovery_pending = true;
                if let Err(error) = backend.refresh_recovery(id) {
                    state.recovery_pending = false;
                    state.recovery_error = Some(error.clone());
                    state.status = Some(error);
                }
            }
            _ => return false,
        },
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
        Overlay::ComposerModelPicker
        | Overlay::ComposerReasoningPicker
        | Overlay::ComposerTargetPicker
        | Overlay::ComposerDraftTitle
        | Overlay::ComposerDraftLibrary
        | Overlay::ComposerDraftDeleteConfirm
        | Overlay::ComposerTemplatePicker
        | Overlay::ComposerPreflight
        | Overlay::ExecutionConfirm
        | Overlay::ExecutionView
        | Overlay::ExecutionApproval
        | Overlay::ExecutionExitConfirm
        | Overlay::UnsavedDraftExit => return false,
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
        Screen::Composer => {}
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
        Screen::Composer => {
            request_sessions(state, backend);
            if state.models.value.is_none() || state.models.error.is_some() {
                request_models(state, backend);
            }
            if state.overview.value.is_none() || state.overview.error.is_some() {
                request_overview(state, backend);
            }
        }
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
        Screen::Composer => {
            request_sessions_force(state, backend);
            request_models(state, backend);
            request_overview(state, backend);
        }
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
            if applied && state.composer.prepared_task.is_some() {
                state.composer.prepared_task = None;
                state.composer.draft_state = DraftState::Modified;
                state.composer.status =
                    Some("Quota evidence refreshed; review preflight again.".to_owned());
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
                if state.composer.prepared_task.is_some() {
                    state.composer.prepared_task = None;
                    state.composer.draft_state = DraftState::Modified;
                    state.composer.status =
                        Some("Session metadata refreshed; review target preflight again.".into());
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
                    state.composer.prepared_task = None;
                    state.composer.draft_state = DraftState::Invalid;
                    state.composer.status = Some(
                        "Live model discovery failed; the prompt and current selections were retained.".into(),
                    );
                } else {
                    revalidate_composer_model(state);
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
                        Screen::Overview | Screen::Sessions | Screen::Composer => {}
                    }
                }
                MutationKind::Resume | MutationKind::Fork | MutationKind::Sync => {}
            }
            true
        }
        BackendEvent::ExecutionPreflight { id, result } => {
            if id != state.execution.id {
                return false;
            }
            state.execution.pending = false;
            match result {
                Ok(preflight) => {
                    state.execution.thread_id = preflight.thread_id.clone();
                    state.execution.preflight = Some(preflight);
                    state.execution.phase = ExecutionPhase::AwaitingConfirmation;
                    state.execution.status = None;
                }
                Err(error) => {
                    state.execution.preflight = None;
                    state.execution.status = Some(error);
                }
            }
            state.overlay = Some(Overlay::ExecutionConfirm);
            true
        }
        BackendEvent::ExecutionProgress { id, event } => {
            if id != state.execution.id {
                return false;
            }
            match event {
                ControllerEvent::State(phase) => {
                    state.execution.phase = phase;
                    if phase == ExecutionPhase::AwaitingConfirmation {
                        state.overlay = Some(Overlay::ExecutionConfirm);
                    } else if phase.terminal() || phase == ExecutionPhase::OutcomeUnknown {
                        state.execution.approvals.clear();
                        if state.overlay == Some(Overlay::ExecutionApproval) {
                            state.overlay = Some(Overlay::ExecutionView);
                        }
                    }
                }
                ControllerEvent::Preflight(preflight) => {
                    state.execution.thread_id = preflight.thread_id.clone();
                    state.execution.preflight = Some(preflight);
                }
                ControllerEvent::Identity { thread_id, turn_id } => {
                    state.execution.thread_id = thread_id;
                    state.execution.turn_id = turn_id;
                }
                ControllerEvent::Activity(activity) => {
                    if state.execution.activity.len() >= 256 {
                        state.execution.activity.pop_front();
                    }
                    state.execution.activity.push_back(activity);
                }
                ControllerEvent::Output(output) => {
                    state.execution.output = bounded_execution_text(output, 256 * 1024);
                }
                ControllerEvent::ToolOutput(output) => {
                    state.execution.tool_output = bounded_execution_text(output, 256 * 1024);
                }
                ControllerEvent::Diff(diff) => {
                    state.execution.diff = bounded_execution_text(diff, 64 * 1024);
                }
                ControllerEvent::Approval(approval) => {
                    if !state
                        .execution
                        .approvals
                        .iter()
                        .any(|pending| pending.request_id == approval.request_id)
                    {
                        state.execution.approvals.push_back(approval);
                    }
                    if state.overlay == Some(Overlay::ExecutionView) {
                        state.overlay = Some(Overlay::ExecutionApproval);
                    }
                }
                ControllerEvent::ApprovalResolved(request_id) => {
                    state
                        .execution
                        .approvals
                        .retain(|value| value.request_id != request_id);
                    if state.execution.approvals.is_empty()
                        && state.overlay == Some(Overlay::ExecutionApproval)
                    {
                        state.overlay = Some(Overlay::ExecutionView);
                    }
                }
                ControllerEvent::Status(status) => state.execution.status = Some(status),
            }
            true
        }
        BackendEvent::ExecutionActionComplete { id } => {
            if id != state.execution.id {
                return false;
            }
            state.execution.pending = false;
            true
        }
        BackendEvent::ExecutionCancelled { id } => {
            if id != state.execution.id {
                return false;
            }
            state.execution = super::state::ExecutionUiState::default();
            state.overlay = Some(Overlay::ComposerPreflight);
            state.composer.status =
                Some("Execution confirmation cancelled. No task was submitted.".to_owned());
            true
        }
        BackendEvent::RecoverySnapshot { id, result } => {
            if id != state.recovery_request_id {
                return false;
            }
            state.recovery_pending = false;
            match result {
                Ok(records) => {
                    state.recovery_error = None;
                    state.recovery_records = records;
                    if !state.recovery_records.is_empty() && state.overlay.is_none() {
                        state.overlay = Some(Overlay::ExecutionRecovery);
                    } else if state.recovery_records.is_empty()
                        && state.overlay == Some(Overlay::ExecutionRecovery)
                    {
                        state.overlay = None;
                    }
                    state.recovery_selected = state
                        .recovery_selected
                        .min(state.recovery_records.len().saturating_sub(1));
                }
                Err(error) => {
                    state.recovery_error = Some(error.clone());
                    state.status = Some(format!("Execution recovery is unavailable: {error}"));
                }
            }
            true
        }
        BackendEvent::RecoveryActionComplete {
            id,
            outcome,
            records,
        } => {
            if id != state.recovery_request_id {
                return false;
            }
            state.recovery_pending = false;
            if let Ok(records) = records {
                state.recovery_records = records;
                state.recovery_selected = state
                    .recovery_selected
                    .min(state.recovery_records.len().saturating_sub(1));
            }
            state.status = Some(match outcome {
                Ok(None) => "Recovery warning acknowledged. Execution status remains unresolved until supported evidence is available.".to_owned(),
                Ok(Some(outcome)) => outcome.label().to_owned(),
                Err(error) => format!("Recovery action failed: {error}"),
            });
            if state.recovery_records.is_empty()
                && state.overlay == Some(Overlay::ExecutionRecovery)
            {
                state.overlay = None;
            }
            true
        }
    }
}

fn bounded_execution_text(mut value: String, max_bytes: usize) -> String {
    if value.len() > max_bytes {
        let mut start = value.len() - max_bytes;
        while !value.is_char_boundary(start) {
            start += 1;
        }
        value.drain(..start);
    }
    value
}

fn revalidate_composer_model(state: &mut AppState) {
    let had_preflight = state.composer.prepared_task.is_some();
    let ModelSelection::Explicit(model_id) = &state.composer.model_selection else {
        state.composer.prepared_task = None;
        return;
    };
    let model = state
        .models
        .value
        .as_ref()
        .and_then(|catalog| catalog.models.iter().find(|model| &model.id == model_id));
    let Some(model) = model else {
        state.composer.prepared_task = None;
        state.composer.draft_state = DraftState::Invalid;
        let previous_effort = match &state.composer.reasoning_selection {
            ReasoningSelection::Explicit(effort) => Some(Some(effort.clone())),
            ReasoningSelection::ModelDefault => Some(None),
            _ => None,
        };
        if let Some(previous_effort) = previous_effort {
            state.composer.reasoning_selection = ReasoningSelection::NeedsChoice(previous_effort);
            state.composer.dirty = true;
        }
        state.composer.status = Some(
            "The selected model is missing from the refreshed catalog; choose an available model or inherit configuration. Any previous effort was not applied.".into(),
        );
        return;
    };
    let unsupported = match &state.composer.reasoning_selection {
        ReasoningSelection::Explicit(effort)
            if !model
                .supported_reasoning_efforts
                .iter()
                .any(|supported| &supported.id == effort) =>
        {
            Some(effort.clone())
        }
        _ => None,
    };
    if let Some(effort) = unsupported {
        state.composer.reasoning_selection = ReasoningSelection::NeedsChoice(Some(effort));
        state.composer.draft_state = DraftState::Invalid;
        state.composer.dirty = true;
        state.composer.status = Some(
            "Catalog capabilities changed; select a supported reasoning effort. The old value was not applied.".into(),
        );
    }
    state.composer.prepared_task = None;
    if had_preflight && state.composer.status.is_none() {
        state.composer.draft_state = DraftState::Modified;
        state.composer.status =
            Some("Live model catalog refreshed; review preflight again.".to_owned());
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
    bracketed_paste: bool,
}

impl TerminalLifecycle {
    fn enter() -> io::Result<Self> {
        let mut lifecycle = Self {
            raw_mode: false,
            alternate_screen: false,
            bracketed_paste: false,
        };
        enable_raw_mode()?;
        lifecycle.raw_mode = true;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        lifecycle.alternate_screen = true;
        execute!(stdout, crossterm::event::EnableBracketedPaste)?;
        lifecycle.bracketed_paste = true;
        execute!(stdout, crossterm::cursor::Hide)?;
        Ok(lifecycle)
    }
}

impl Drop for TerminalLifecycle {
    fn drop(&mut self) {
        if self.alternate_screen {
            let mut stdout = io::stdout();
            if self.bracketed_paste {
                let _ = execute!(stdout, crossterm::event::DisableBracketedPaste);
            }
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
            ('6', Screen::Composer),
        ] {
            handle_key(&mut state, &backend, key(KeyCode::Char(shortcut)));
            assert_eq!(state.active, expected);
            assert!(!state.mutation_pending);
        }
        state.active = Screen::Sessions;
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
    fn prompt_editor_input_never_triggers_global_actions_or_session_mutations() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Composer;
        state.composer.focus = ComposerFocus::Prompt;
        for character in ['F', 's', 'q', '6'] {
            handle_key(&mut state, &backend, key(KeyCode::Char(character)));
        }
        assert_eq!(state.composer.editor.text(), "Fsq6");
        assert_eq!(state.active, Screen::Composer);
        assert!(state.overlay.is_none());
        assert!(!state.mutation_pending);
        assert!(handle_composer_paste(&mut state, " F s q 6 "));
        assert_eq!(state.composer.editor.text(), "Fsq6 F s q 6 ");
        assert!(!state.mutation_pending);

        let oversized = "x".repeat(MAX_PROMPT_BYTES + 1);
        assert!(handle_composer_paste(&mut state, &oversized));
        assert_eq!(state.composer.editor.len_bytes(), "Fsq6 F s q 6 ".len());
        assert!(state
            .composer
            .status
            .as_deref()
            .unwrap()
            .contains("rejected in full"));
    }

    #[test]
    fn pending_execution_and_approval_keys_are_ignored_until_the_worker_finishes() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Composer;
        state.overlay = Some(Overlay::ExecutionConfirm);
        state.execution.phase = ExecutionPhase::AwaitingConfirmation;
        state.execution.pending = true;
        handle_key(&mut state, &backend, key(KeyCode::Enter));
        assert_eq!(state.execution.phase, ExecutionPhase::AwaitingConfirmation);
        assert!(state.execution.pending);

        state.overlay = Some(Overlay::ExecutionApproval);
        state
            .execution
            .approvals
            .push_back(crate::codex_app_server::ApprovalView {
                request_id: "approval-test".to_owned(),
                kind: crate::codex_app_server::ApprovalKind::CommandExecution,
                requested_action: "cargo test".to_owned(),
                working_context: Some("workspace".to_owned()),
                details: String::new(),
                allow_once_available: true,
                network_request: false,
            });
        handle_key(&mut state, &backend, key(KeyCode::Char('a')));
        assert_eq!(state.execution.approvals.len(), 1);
        assert!(state.execution.pending);
    }

    #[test]
    fn q_is_text_in_the_model_filter_and_user_controlled_draft_title() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Composer;
        state.composer.dirty = true;
        state.overlay = Some(Overlay::ComposerModelPicker);
        handle_key(&mut state, &backend, key(KeyCode::Char('q')));
        assert_eq!(state.composer.model_filter, "q");
        assert_eq!(state.overlay, Some(Overlay::ComposerModelPicker));
        assert!(!state.quitting);

        state.overlay = Some(Overlay::ComposerDraftTitle);
        handle_key(&mut state, &backend, key(KeyCode::Char('q')));
        assert_eq!(state.composer.draft_title, "q");
        assert_eq!(state.overlay, Some(Overlay::ComposerDraftTitle));
        assert!(!state.quitting);
    }

    #[test]
    fn unsaved_composer_exit_is_confirmed_and_the_prompt_is_retained_until_then() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Composer;
        state
            .composer
            .editor
            .insert_paste("keep this in memory")
            .unwrap();
        state.composer.invalidate();
        handle_key(
            &mut state,
            &backend,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.overlay, Some(Overlay::UnsavedDraftExit));
        assert!(!state.quitting);
        assert_eq!(state.composer.editor.text(), "keep this in memory");
        handle_key(&mut state, &backend, key(KeyCode::Char('n')));
        assert!(state.overlay.is_none());
        assert!(!state.quitting);
        assert_eq!(state.composer.editor.text(), "keep this in memory");
    }

    #[test]
    fn selected_session_shortcut_opens_composer_without_resuming_the_session() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Sessions;
        state.sessions.value = Some(vec![test_session("thread-exact-019")]);
        handle_key(&mut state, &backend, key(KeyCode::Char('c')));
        assert_eq!(state.active, Screen::Composer);
        assert_eq!(state.composer.target_kind, TargetKind::ExistingThread);
        assert_eq!(
            state.composer.selected_thread_id.as_deref(),
            Some("thread-exact-019")
        );
        assert!(!state.mutation_pending);
        assert!(state.overlay.is_none());
    }

    #[test]
    fn preflight_requires_explicit_workspace_and_verifies_model_effort_capabilities() {
        let mut state = prepared_state();
        state.composer.model_selection = ModelSelection::Explicit("model-visible".into());
        state.composer.reasoning_selection = ReasoningSelection::Explicit("high".into());
        state.composer.workspace = WorkspaceSelection::Unselected;
        let missing_workspace = prepare_task(&state);
        assert_eq!(
            missing_workspace.validation_status,
            ValidationStatus::Incomplete
        );
        assert!(missing_workspace
            .validation_messages
            .iter()
            .any(|message| message.contains("Choose a workspace")));

        state.composer.workspace =
            WorkspaceSelection::CurrentDirectory(std::path::PathBuf::from("/workspace"));
        let valid = prepare_task(&state);
        assert_eq!(valid.validation_status, ValidationStatus::Ready);
        assert_eq!(valid.prompt_text, "prepared prompt");
        assert!(valid
            .validation_messages
            .iter()
            .any(|message| message.contains("Expected task quota cost: Unavailable")));

        state.composer.reasoning_selection = ReasoningSelection::Explicit("unsupported".into());
        let unsupported = prepare_task(&state);
        assert_eq!(unsupported.validation_status, ValidationStatus::Invalid);
        assert!(unsupported
            .validation_messages
            .iter()
            .any(|message| message.contains("unsupported by this model")));
    }

    #[test]
    fn existing_targets_are_revalidated_and_archived_or_missing_targets_are_blocked() {
        let mut state = prepared_state();
        state.composer.target_kind = TargetKind::ExistingThread;
        state.composer.selected_thread_id = Some("thread-existing".into());
        state.sessions.value = Some(vec![test_session("thread-existing")]);
        let existing = prepare_task(&state);
        assert_eq!(existing.validation_status, ValidationStatus::Ready);
        assert!(existing
            .validation_messages
            .iter()
            .any(|message| { message.contains("P036 must revalidate availability") }));
        assert_eq!(
            existing.target_thread_id.as_deref(),
            Some("thread-existing")
        );

        state.sessions.value.as_mut().unwrap()[0].archived = Some(true);
        assert_eq!(
            prepare_task(&state).validation_status,
            ValidationStatus::Invalid
        );
        state.sessions.value = Some(Vec::new());
        assert_eq!(
            prepare_task(&state).validation_status,
            ValidationStatus::Invalid
        );
    }

    #[test]
    fn app_server_unavailability_blocks_preflight_and_quota_provenance_is_retained() {
        let mut state = prepared_state();
        state.app_server_status = AppServerStatus::Unavailable;
        assert_eq!(
            prepare_task(&state).validation_status,
            ValidationStatus::Incomplete
        );

        state.app_server_status = AppServerStatus::Connected;
        state.overview.value = Some(crate::tui::backend::OverviewData {
            ledger_status: crate::tui::backend::LedgerStatus::Available,
            token_statistics: None,
            quota: Some(crate::storage::QuotaTimeline {
                five_hour: Some(quota_sample(
                    crate::telemetry::QuotaMeterType::FiveHour,
                    time::OffsetDateTime::now_utc(),
                )),
                weekly: Some(quota_sample(
                    crate::telemetry::QuotaMeterType::Weekly,
                    time::OffsetDateTime::now_utc() - time::Duration::hours(25),
                )),
                history: None,
                evidence_quality: quality_with_partial_coverage(),
            }),
            evidence_quality: None,
            telemetry_version: None,
            schema_version: None,
        });
        let snapshot = preflight_quota_snapshot(&state);
        assert_eq!(snapshot.five_hour.unwrap().freshness, QuotaFreshness::Fresh);
        assert_eq!(snapshot.weekly.unwrap().freshness, QuotaFreshness::Stale);
        assert!(snapshot
            .quota_coverage
            .unwrap()
            .contains("Partial local evidence coverage"));
    }

    #[test]
    fn model_effort_survives_only_when_the_refreshed_model_supports_it() {
        let backend = test_backend();
        let mut state = prepared_state();
        let mut catalog = model_catalog();
        catalog.models.push(crate::codex_app_server::ModelInfo {
            id: "model-other".into(),
            display_name: "Other model".into(),
            is_default: false,
            hidden: true,
            supported_reasoning_efforts: vec![crate::codex_app_server::ReasoningEffortInfo {
                id: "low".into(),
                description: "Lower latency".into(),
            }],
            default_reasoning_effort: "low".into(),
            input_modalities: None,
        });
        state.models.value = Some(catalog);
        state.composer.model_selection = ModelSelection::Explicit("model-visible".into());
        state.composer.reasoning_selection = ReasoningSelection::Explicit("high".into());
        state.composer.model_filter = "Other".into();
        state.composer.model_picker_selected = 1;
        state.overlay = Some(Overlay::ComposerModelPicker);
        handle_composer_overlay(
            &mut state,
            &backend,
            Overlay::ComposerModelPicker,
            key(KeyCode::Enter),
        );
        assert_eq!(
            state.composer.model_selection,
            ModelSelection::Explicit("model-other".into())
        );
        assert_eq!(
            state.composer.reasoning_selection,
            ReasoningSelection::NeedsChoice(Some("high".into()))
        );
        assert!(state
            .composer
            .status
            .as_deref()
            .unwrap()
            .contains("unsupported by this model"));

        state.composer.model_selection = ModelSelection::Explicit("model-other".into());
        state.composer.reasoning_selection = ReasoningSelection::Explicit("high".into());
        let request_id = state.models.request_id;
        apply_backend_event(
            &mut state,
            &backend,
            BackendEvent::Models {
                id: request_id,
                result: Ok(model_catalog()),
            },
        );
        assert_eq!(
            state.composer.reasoning_selection,
            ReasoningSelection::NeedsChoice(Some("high".into()))
        );
    }

    #[test]
    fn model_picker_highlights_the_current_model_and_first_search_result() {
        let backend = test_backend();
        let mut state = prepared_state();
        open_model_picker(&mut state);
        assert_eq!(state.composer.model_picker_selected, 0);
        handle_composer_overlay(
            &mut state,
            &backend,
            Overlay::ComposerModelPicker,
            key(KeyCode::Char('v')),
        );
        assert_eq!(state.composer.model_picker_selected, 1);

        state.composer.model_selection = ModelSelection::Explicit("model-visible".into());
        open_model_picker(&mut state);
        assert_eq!(state.composer.model_picker_selected, 1);
        handle_composer_overlay(
            &mut state,
            &backend,
            Overlay::ComposerModelPicker,
            key(KeyCode::Up),
        );
        handle_composer_overlay(
            &mut state,
            &backend,
            Overlay::ComposerModelPicker,
            key(KeyCode::Enter),
        );
        assert_eq!(state.composer.model_selection, ModelSelection::Inherit);
    }

    #[test]
    fn composer_refresh_uses_the_existing_bounded_metadata_workers() {
        let backend = test_backend();
        let mut state = AppState::default();
        state.active = Screen::Composer;
        handle_key(&mut state, &backend, key(KeyCode::F(5)));
        assert!(state.sessions.loading);
        assert!(state.models.loading);
        assert!(state.overview.loading);
        assert!(!state.mutation_pending);
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

    fn prepared_state() -> AppState {
        let mut state = AppState::default();
        state.active = Screen::Composer;
        state.app_server_status = AppServerStatus::Connected;
        state.models.value = Some(model_catalog());
        state.composer.editor =
            super::super::composer::PromptEditor::from_text("prepared prompt".to_owned()).unwrap();
        state.composer.workspace =
            WorkspaceSelection::CurrentDirectory(std::path::PathBuf::from("/workspace"));
        state
    }

    fn model_catalog() -> crate::codex_app_server::ModelCatalog {
        crate::codex_app_server::ModelCatalog {
            availability: "live",
            models: vec![crate::codex_app_server::ModelInfo {
                id: "model-visible".into(),
                display_name: "Visible model".into(),
                is_default: true,
                hidden: false,
                supported_reasoning_efforts: vec![
                    crate::codex_app_server::ReasoningEffortInfo {
                        id: "low".into(),
                        description: "Lower latency".into(),
                    },
                    crate::codex_app_server::ReasoningEffortInfo {
                        id: "high".into(),
                        description: "More reasoning".into(),
                    },
                ],
                default_reasoning_effort: "low".into(),
                input_modalities: Some(vec!["text".into(), "image".into()]),
            }],
        }
    }

    fn quota_sample(
        meter_type: crate::telemetry::QuotaMeterType,
        observed_at: time::OffsetDateTime,
    ) -> crate::storage::QuotaSnapshotEvidence {
        crate::storage::QuotaSnapshotEvidence {
            sample_id: "sample-fixture".into(),
            source_id: "source-fixture".into(),
            session_id: None,
            observed_at: observed_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
            meter_type,
            used_percent: 38.5,
            reset_at: Some(
                (observed_at + time::Duration::hours(5))
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
            ),
            plan: None,
            codex_version: None,
        }
    }

    fn quality_with_partial_coverage() -> crate::storage::EvidenceQualitySummary {
        crate::storage::EvidenceQualitySummary {
            total_diagnostics: 1,
            unsupported_source_versions: 0,
            compressed_sources_unsupported: 0,
            malformed_or_incomplete_evidence: 0,
            source_continuity_ambiguities: 0,
            known_partial_collection: 1,
            diagnostics_by_code: Vec::new(),
        }
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
