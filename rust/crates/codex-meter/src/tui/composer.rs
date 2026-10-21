//! Task preparation types and the privacy-safe in-memory prompt editor.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MAX_PROMPT_BYTES: usize = 64 * 1024;
pub const BUILT_IN_TEMPLATES: [(&str, &str); 6] = [
    (
        "Bug Fix",
        "Investigate and fix the issue described below. Explain the cause, make the smallest appropriate change, and add or update focused tests.\n\nIssue:\n",
    ),
    (
        "Feature Implementation",
        "Implement the feature described below. First inspect the relevant design and code, then make a focused change and verify its behavior.\n\nFeature:\n",
    ),
    (
        "Code Review",
        "Review the changes in the current workspace. Prioritize correctness, regressions, security, and missing tests. Report findings with file and line references; do not modify files unless asked.\n",
    ),
    (
        "Testing",
        "Review the current implementation and add focused tests for the behavior described below. Run the narrowest relevant checks and report any remaining gaps.\n\nBehavior:\n",
    ),
    (
        "Documentation",
        "Update the documentation for the behavior described below. Keep it aligned with the current implementation and call out any limitations.\n\nDocumentation task:\n",
    ),
    (
        "Refactoring",
        "Refactor the code described below while preserving its existing behavior. Keep the change focused and add or update tests where the behavior needs protection.\n\nRefactoring task:\n",
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DraftState {
    Empty,
    Editing,
    Modified,
    Validated,
    Saved,
    PreflightReady,
    Invalid,
}

impl DraftState {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Editing => "Editing",
            Self::Modified => "Modified",
            Self::Validated => "Validated",
            Self::Saved => "Saved",
            Self::PreflightReady => "Preflight ready",
            Self::Invalid => "Needs attention",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposerFocus {
    Target,
    Workspace,
    Model,
    Reasoning,
    Prompt,
    Actions,
}

impl ComposerFocus {
    pub const fn next(self) -> Self {
        match self {
            Self::Target => Self::Workspace,
            Self::Workspace => Self::Model,
            Self::Model => Self::Reasoning,
            Self::Reasoning => Self::Prompt,
            Self::Prompt => Self::Actions,
            Self::Actions => Self::Target,
        }
    }

    pub const fn previous(self) -> Self {
        match self {
            Self::Target => Self::Actions,
            Self::Workspace => Self::Target,
            Self::Model => Self::Workspace,
            Self::Reasoning => Self::Model,
            Self::Prompt => Self::Reasoning,
            Self::Actions => Self::Prompt,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    NewThread,
    ExistingThread,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ModelSelection {
    Inherit,
    Explicit(String),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ReasoningSelection {
    Inherit,
    ModelDefault,
    Explicit(String),
    NeedsChoice(Option<String>),
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceSelection {
    Unselected,
    CurrentDirectory(PathBuf),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectAssociation {
    Unavailable,
    /// A P033 response-local label, not a persistent project identifier.
    ResponseLocalAlias(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaFreshness {
    Fresh,
    Stale,
    TimeUnavailable,
    Invalid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuotaObservation {
    pub used_percent: f64,
    pub observed_at: String,
    pub reset_at: Option<String>,
    pub freshness: QuotaFreshness,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreflightSnapshot {
    pub five_hour: Option<QuotaObservation>,
    pub weekly: Option<QuotaObservation>,
    pub quota_coverage: Option<String>,
    pub captured_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationStatus {
    Incomplete,
    Invalid,
    Ready,
}

/// Validated task intent for the future P036 execution controller.
///
/// This value contains prompt text and can identify a private local thread or
/// workspace. It intentionally does not implement `Debug`, `Serialize`, or
/// `Display`; callers should use [`PreparedTask::privacy_safe_summary`].
pub struct PreparedTask {
    pub target_kind: TargetKind,
    pub target_thread_id: Option<String>,
    pub workspace: WorkspaceSelection,
    pub project_association: ProjectAssociation,
    pub model_selection: ModelSelection,
    pub reasoning_selection: ReasoningSelection,
    pub prompt_text: String,
    pub configuration_source: &'static str,
    pub validation_status: ValidationStatus,
    pub validation_messages: Vec<String>,
    pub preflight_snapshot: PreflightSnapshot,
}

impl PreparedTask {
    pub fn privacy_safe_summary(&self) -> String {
        let target = match self.target_kind {
            TargetKind::NewThread => "new thread",
            TargetKind::ExistingThread => "existing thread",
        };
        let model = match &self.model_selection {
            ModelSelection::Inherit => "inherited".to_owned(),
            ModelSelection::Explicit(_) => "explicit".to_owned(),
            ModelSelection::Unavailable => "unavailable".to_owned(),
        };
        let reasoning = match &self.reasoning_selection {
            ReasoningSelection::Inherit => "inherited".to_owned(),
            ReasoningSelection::ModelDefault => "model default".to_owned(),
            ReasoningSelection::Explicit(_) => "explicit".to_owned(),
            ReasoningSelection::NeedsChoice(_) => "choice required".to_owned(),
            ReasoningSelection::Unavailable => "unavailable".to_owned(),
        };
        format!(
            "target={target} model={model} reasoning={reasoning} prompt_bytes={} validation={:?}",
            self.prompt_text.len(),
            self.validation_status
        )
    }
}

/// Prompt editing is byte-indexed only at valid UTF-8 character boundaries.
/// The editor never clips accepted input and never logs or formats its text.
pub struct PromptEditor {
    text: String,
    cursor: usize,
    preferred_column: Option<usize>,
    pub error: Option<EditorError>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorError {
    TooLarge,
    UnsupportedControl,
}

impl PromptEditor {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            preferred_column: None,
            error: None,
        }
    }

    pub fn from_text(text: String) -> Result<Self, EditorError> {
        if text.len() > MAX_PROMPT_BYTES {
            return Err(EditorError::TooLarge);
        }
        if text
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
        {
            return Err(EditorError::UnsupportedControl);
        }
        let mut editor = Self::new();
        editor.cursor = text.len();
        editor.text = text;
        Ok(editor)
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn line_count(&self) -> usize {
        self.text.bytes().filter(|byte| *byte == b'\n').count() + 1
    }

    pub fn cursor_line_column(&self) -> (usize, usize) {
        let prefix = &self.text[..self.cursor];
        let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
        let column = prefix
            .rsplit('\n')
            .next()
            .map_or(0, |line| line.chars().count());
        (line, column)
    }

    pub fn insert_char(&mut self, value: char) -> Result<(), EditorError> {
        if value.is_control() && value != '\n' && value != '\t' {
            return self.reject(EditorError::UnsupportedControl);
        }
        self.insert_str(&value.to_string())
    }

    pub fn insert_paste(&mut self, value: &str) -> Result<(), EditorError> {
        let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
        if normalized
            .chars()
            .any(|character| character.is_control() && character != '\n' && character != '\t')
        {
            return self.reject(EditorError::UnsupportedControl);
        }
        self.insert_str(&normalized)
    }

    fn insert_str(&mut self, value: &str) -> Result<(), EditorError> {
        if self.text.len().saturating_add(value.len()) > MAX_PROMPT_BYTES {
            return self.reject(EditorError::TooLarge);
        }
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        self.preferred_column = None;
        self.error = None;
        Ok(())
    }

    fn reject<T>(&mut self, error: EditorError) -> Result<T, EditorError> {
        self.error = Some(error);
        Err(error)
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let previous = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(self.cursor, |(index, _)| index);
        self.text.drain(previous..self.cursor);
        self.cursor = previous;
        self.preferred_column = None;
        self.error = None;
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        if self.cursor == self.text.len() {
            return false;
        }
        let next = self.text[self.cursor..]
            .char_indices()
            .nth(1)
            .map_or(self.text.len(), |(index, _)| self.cursor + index);
        self.text.drain(self.cursor..next);
        self.preferred_column = None;
        self.error = None;
        true
    }

    pub fn move_left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.preferred_column = None;
    }

    pub fn move_right(&mut self) {
        if let Some((index, _)) = self.text[self.cursor..].char_indices().nth(1) {
            self.cursor += index;
        } else {
            self.cursor = self.text.len();
        }
        self.preferred_column = None;
    }

    pub fn move_home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        self.preferred_column = None;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |index| self.cursor + index);
        self.preferred_column = None;
    }

    pub fn move_vertical(&mut self, delta: isize) {
        let (line_index, column) = self.cursor_line_column();
        let desired_column = *self.preferred_column.get_or_insert(column);
        let lines = self.text.split('\n').collect::<Vec<_>>();
        let target = (line_index as isize + delta).clamp(0, lines.len() as isize - 1) as usize;
        let mut offset = 0;
        for line in lines.iter().take(target) {
            offset += line.len() + 1;
        }
        let target_line = lines[target];
        let byte_column = target_line
            .char_indices()
            .nth(desired_column)
            .map_or(target_line.len(), |(index, _)| index);
        self.cursor = offset + byte_column;
    }
}

impl Default for PromptEditor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_inserts_multiline_unicode_and_combining_text_at_character_boundaries() {
        let mut editor = PromptEditor::new();
        editor.insert_paste("A\n東京 e\u{301}").unwrap();
        assert_eq!(editor.text(), "A\n東京 e\u{301}");
        assert_eq!(editor.cursor_line_column(), (1, 5));
        editor.move_left();
        editor.insert_char('!').unwrap();
        assert_eq!(editor.text(), "A\n東京 e!\u{301}");
    }

    #[test]
    fn editor_moves_across_lines_and_deletes_without_splitting_utf8() {
        let mut editor = PromptEditor::from_text("one\n東京\nthree".into()).unwrap();
        editor.move_home();
        editor.move_vertical(-1);
        assert_eq!(editor.cursor_line_column(), (1, 0));
        editor.move_right();
        editor.delete_forward();
        assert_eq!(editor.text(), "one\n東\nthree");
        editor.backspace();
        assert_eq!(editor.text(), "one\n\nthree");
        editor.move_home();
        editor.backspace();
        assert_eq!(editor.text(), "one\nthree");
    }

    #[test]
    fn editor_rejects_oversized_paste_without_truncating_or_mutating() {
        let mut editor = PromptEditor::new();
        let oversized = "x".repeat(MAX_PROMPT_BYTES + 1);
        assert_eq!(editor.insert_paste(&oversized), Err(EditorError::TooLarge));
        assert!(editor.text().is_empty());
        assert_eq!(editor.error, Some(EditorError::TooLarge));
        editor.insert_paste(&"x".repeat(MAX_PROMPT_BYTES)).unwrap();
        assert_eq!(editor.len_bytes(), MAX_PROMPT_BYTES);
        assert_eq!(editor.insert_char('x'), Err(EditorError::TooLarge));
        assert_eq!(editor.len_bytes(), MAX_PROMPT_BYTES);
    }

    #[test]
    fn editor_normalizes_newlines_and_rejects_unsupported_control_bytes() {
        let mut editor = PromptEditor::new();
        editor.insert_paste("one\r\ntwo\rthree").unwrap();
        assert_eq!(editor.text(), "one\ntwo\nthree");
        assert!(matches!(
            PromptEditor::from_text("malformed\u{1}".to_owned()),
            Err(EditorError::UnsupportedControl)
        ));
        assert_eq!(
            editor.insert_paste("safe\u{1b}[31m"),
            Err(EditorError::UnsupportedControl)
        );
        assert_eq!(editor.text(), "one\ntwo\nthree");
    }

    #[test]
    fn movement_handles_empty_lines_and_vertical_short_lines() {
        let mut editor = PromptEditor::from_text("abcdef\nx\n".into()).unwrap();
        editor.move_home();
        editor.move_vertical(-1);
        assert_eq!(editor.cursor_line_column(), (1, 0));
        editor.move_vertical(-1);
        assert_eq!(editor.cursor_line_column(), (0, 0));
        editor.move_end();
        editor.move_vertical(1);
        assert_eq!(editor.cursor_line_column(), (1, 1));
        editor.move_vertical(1);
        assert_eq!(editor.cursor_line_column(), (2, 0));
    }

    #[test]
    fn privacy_safe_task_summary_omits_prompt_thread_and_workspace_values() {
        let task = PreparedTask {
            target_kind: TargetKind::ExistingThread,
            target_thread_id: Some("private-thread-identifier".to_owned()),
            workspace: WorkspaceSelection::CurrentDirectory(PathBuf::from(
                "/private/workspace/path",
            )),
            project_association: ProjectAssociation::ResponseLocalAlias(
                "private-project-id".to_owned(),
            ),
            model_selection: ModelSelection::Explicit("live-model".to_owned()),
            reasoning_selection: ReasoningSelection::Explicit("high".to_owned()),
            prompt_text: "private prompt canary".to_owned(),
            configuration_source: "Explicitly prepared in Codex Meter",
            validation_status: ValidationStatus::Ready,
            validation_messages: Vec::new(),
            preflight_snapshot: PreflightSnapshot::default(),
        };
        let summary = task.privacy_safe_summary();
        assert!(summary.contains("prompt_bytes=21"));
        assert!(!summary.contains("private prompt canary"));
        assert!(!summary.contains("private-thread-identifier"));
        assert!(!summary.contains("/private/workspace/path"));
        assert!(!summary.contains("private-project-id"));
    }
}
