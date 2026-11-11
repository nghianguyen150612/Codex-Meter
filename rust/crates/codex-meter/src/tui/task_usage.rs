//! Presentation over P039 reports. No SQLite or App Server operations live here.
use super::*;
use crate::storage::{EvidenceQualitySummary, TaskUsageSummary};
use crate::tui::state::TASK_RESULT_LIMIT;
use unicode_width::UnicodeWidthChar;

fn configuration(value: Option<&str>) -> String {
    let value = value.unwrap_or("Unavailable");
    if value.contains('@') || value.contains("://") || value.contains("sk-") {
        "Unavailable".into()
    } else {
        clean_metadata(value, 256)
    }
}

fn compact_identity(task: &TaskUsageSummary) -> String {
    // Length-prefixed hashing retains composite scope in a compact opaque label.
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for value in [&task.source_id, &task.session_id, &task.task_id] {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    format!("task:{:.16}", format!("{:x}", hash.finalize()))
}

fn models(task: &TaskUsageSummary) -> String {
    let mut values = task
        .configurations
        .iter()
        .take(2)
        .map(|c| configuration(c.model.as_deref()))
        .collect::<Vec<_>>();
    if task.configurations.len() > 2 || task.configurations_truncated {
        values.push("more (Enter)".into());
    }
    if values.is_empty() {
        "Unavailable".into()
    } else {
        values.join(", ")
    }
}

fn quality_line(quality: &EvidenceQualitySummary) -> String {
    format!("Source warnings (global): unsupported {} | compressed {} | malformed {} | continuity {} | partial {}",
        quality.unsupported_source_versions, quality.compressed_sources_unsupported,
        quality.malformed_or_incomplete_evidence, quality.source_continuity_ambiguities,
        quality.known_partial_collection)
}

pub(super) fn draw_tasks(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    if area.width < 30 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Task usage: resize to view | t modes | ? help")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    if state.tasks.loading || state.tasks.error.is_some() || state.tasks.value.is_none() {
        render_resource_state(
            frame,
            area,
            &state.tasks,
            "Task usage | read only",
            "No task observations loaded. Press r to refresh local evidence.",
            theme,
        );
        return;
    }
    let report = state.tasks.value.as_ref().expect("loaded task report");
    let mut summary = [
        Line::from(format!(
            "Top {TASK_RESULT_LIMIT} tasks | {} loaded / {} observed | {} | model: {}",
            report.tasks.len(),
            report.task_count,
            state.history_scope.label(),
            clean(&configuration(state.history_model_filter.as_deref()), 32)
        )),
        Line::from(if report.has_more {
            "TRUNCATED: more tasks exist. n/p pages only the loaded top tasks."
        } else {
            "All matching tasks fit within this bounded result."
        }),
        Line::from(format!(
            "Unattributed: {} responses | raw {}",
            report.unattributed.canonical_response_count,
            metric_display(&report.unattributed.raw_total)
        )),
        Line::from(format!(
            "Sources: unsupported {} | partial {}",
            report.evidence_quality.unsupported_source_versions,
            report.evidence_quality.known_partial_collection
        )),
        Line::from(format!(
            "Malformed {} | continuity {} | compressed {}",
            report.evidence_quality.malformed_or_incomplete_evidence,
            report.evidence_quality.source_continuity_ambiguities,
            report.evidence_quality.compressed_sources_unsupported
        )),
        Line::from("Local raw tokens; account quota snapshots do not measure task quota."),
    ];
    if report.tasks.is_empty() {
        summary[1] = Line::from(if report.unattributed.canonical_response_count > 0 {
            "Only unattributed observations: validated task/session identity is missing."
        } else {
            "No task observations for this scope. Usage is not assumed to be zero."
        });
    }
    let summary_text = summary.iter().map(ToString::to_string).collect::<Vec<_>>();
    let summary = wrapped_lines(&summary_text, area.width.saturating_sub(2));
    let summary_height = summary.len() as u16 + 2;
    let row_height = if area.width < 55 {
        6
    } else if area.width < 85 {
        4
    } else {
        1
    };
    if area.height < summary_height + row_height + 3 {
        frame.render_widget(
            Paragraph::new("Task usage: resize to view coverage and tasks | t modes | ? help")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let sections =
        Layout::vertical([Constraint::Length(summary_height), Constraint::Min(0)]).split(area);
    render_panel(
        frame,
        sections[0],
        "History | Task usage | t modes | w scope | m model",
        summary.into_iter().map(Line::from).collect(),
        theme,
    );
    let tasks = state.visible_tasks_page();
    if tasks.is_empty() {
        render_panel(
            frame,
            sections[1],
            "Task usage",
            vec![Line::from(if report.tasks.is_empty() {
                "Run codex-meter sync to collect supported local evidence; r refreshes."
            } else {
                "No loaded tasks match this search. / changes search."
            })],
            theme,
        );
        return;
    }
    let narrow = area.width < 85;
    let rows = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| {
            let raw = match task.raw_total.known_sum {
                Some(value) if task.raw_total.coverage == MetricCoverage::Partial => {
                    format!("{value} partial")
                }
                _ => metric_display(&task.raw_total),
            };
            let coverage = clean(task.evidence_status, 64);
            let row = if area.width < 55 {
                Row::new([Text::from(vec![
                    Line::from(compact_identity(task)),
                    Line::from(format!("Last {}", clean(&task.last_observed_at, 32))),
                    Line::from(format!("Responses: {}", task.canonical_response_count)),
                    Line::from(format!("Raw: {}", metric_field(task.raw_total.known_sum))),
                    Line::from(format!(
                        "Models: {}",
                        clean(&models(task), area.width.saturating_sub(12) as usize)
                    )),
                    Line::from(format!("Coverage: {coverage}")),
                ])])
                .height(6)
            } else if narrow {
                Row::new([Text::from(vec![
                    Line::from(compact_identity(task)),
                    Line::from(format!(
                        "Last {} | {} responses",
                        clean(&task.last_observed_at, 32),
                        task.canonical_response_count
                    )),
                    Line::from(format!("Raw {raw} | {coverage}")),
                    Line::from(format!(
                        "Models: {}",
                        clean(&models(task), area.width.saturating_sub(12) as usize)
                    )),
                ])])
                .height(4)
            } else {
                Row::new(vec![
                    compact_identity(task),
                    clean(&task.last_observed_at, 32),
                    task.canonical_response_count.to_string(),
                    raw,
                    clean(&models(task), 64),
                    coverage,
                ])
            };
            if index == state.selected_task {
                row.style(theme.selected())
            } else {
                row
            }
        })
        .collect::<Vec<_>>();
    let widths = if narrow {
        vec![Constraint::Percentage(100)]
    } else {
        vec![
            Constraint::Percentage(24),
            Constraint::Percentage(22),
            Constraint::Percentage(7),
            Constraint::Percentage(17),
            Constraint::Percentage(16),
            Constraint::Percentage(14),
        ]
    };
    let header = if narrow {
        vec!["Task / last UTC / responses / raw / models / coverage"]
    } else {
        vec![
            "Task (opaque)",
            "Last UTC",
            "Resp.",
            "Raw tokens",
            "Models",
            "Coverage",
        ]
    };
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(theme.title()))
        .column_spacing(1)
        .block(
            Block::default()
                .title(format!(
                    "Loaded tasks | page {} | Enter details | n/p local page",
                    state.task_page + 1
                ))
                .borders(Borders::ALL)
                .border_set(theme.border_set())
                .border_style(theme.border()),
        );
    let mut selection = TableState::default().with_selected(Some(state.selected_task));
    frame.render_stateful_widget(table, sections[1], &mut selection);
}

pub(in crate::tui) fn task_detail_lines(
    task: &TaskUsageSummary,
    quality: &EvidenceQualitySummary,
) -> Vec<String> {
    let mut lines = vec![
        format!("Source: {}", configuration(Some(&task.source_id))),
        format!("Session: {}", configuration(Some(&task.session_id))),
        format!("Task: {}", configuration(Some(&task.task_id))),
        format!("First UTC: {}", clean(&task.first_observed_at, 64)),
        format!("Last UTC: {}", clean(&task.last_observed_at, 64)),
        format!("Canonical responses: {}", task.canonical_response_count),
    ];
    for (name, metric) in [
        ("Direct raw total", &task.raw_total),
        ("Input", &task.input_tokens),
        ("Cached input", &task.cached_input_tokens),
        ("Cache-write input", &task.cache_write_input_tokens),
        ("Output", &task.output_tokens),
        ("Reasoning output", &task.reasoning_output_tokens),
    ] {
        lines.push(format!(
            "{name}: {} | {:?} | known {} / missing {}",
            metric_display(metric),
            metric.coverage,
            metric.known_event_count,
            metric.missing_event_count
        ));
    }
    lines.push(format!(
        "Evidence coverage: {}",
        clean(task.evidence_status, 64)
    ));
    lines.push(quality_line(quality));
    lines.push(format!(
        "Total ingestion diagnostics (global): {}",
        quality.total_diagnostics
    ));
    lines.push(
        "Only verified per_response evidence counts. Cumulative snapshots are excluded.".into(),
    );
    lines.push(
        "Raw totals are directly observed; components may overlap and are never summed into raw."
            .into(),
    );
    lines.push(
        "Local usage is independent of account-wide five-hour / weekly quota snapshots.".into(),
    );
    lines.push(
        "Configuration observations (model / reasoning / tier / Codex version / responses):".into(),
    );
    for config in task.configurations.iter().take(64) {
        lines.push(format!(
            "{} / {} / {} / {} / {}",
            configuration(config.model.as_deref()),
            configuration(config.reasoning_level.as_deref()),
            configuration(config.service_tier.as_deref()),
            configuration(config.codex_version.as_deref()),
            config.response_count
        ));
    }
    if task.configurations_truncated {
        lines.push("TRUNCATED: only the first 64 configuration tuples are available.".into());
    }
    lines.push(
        "Read only. No conversation content, paths, task launch, or App Server request.".into(),
    );
    lines
}

fn detail_area(width: u16, height: u16) -> Rect {
    centered(
        Rect::new(0, 0, width, height),
        width.saturating_sub(2).min(100),
        height.saturating_sub(2),
    )
}

// Pre-wrap by display columns, including long identifiers, so scroll limits and
// rendering agree on every resize. Input is bounded by typed reporting limits.
fn wrapped_lines(lines: &[String], width: u16) -> Vec<String> {
    let width = usize::from(width.max(2));
    let mut output = Vec::new();
    for line in lines {
        let mut current = String::new();
        let mut columns = 0;
        for ch in line.chars() {
            let size = ch.width().unwrap_or(0);
            if columns + size > width && !current.is_empty() {
                output.push(std::mem::take(&mut current));
                columns = 0;
            }
            current.push(ch);
            columns += size;
        }
        output.push(current);
    }
    output
}

pub(in crate::tui) fn update_task_detail_scroll(state: &mut AppState, width: u16, height: u16) {
    let area = detail_area(width, height);
    let lines = wrapped_lines(&state.task_detail_lines, area.width.saturating_sub(2));
    state.task_detail_max_scroll = lines
        .len()
        .saturating_sub(area.height.saturating_sub(3) as usize)
        .min(u16::MAX as usize) as u16;
    state.task_detail_scroll = state.task_detail_scroll.min(state.task_detail_max_scroll);
}

pub(super) fn draw_task_detail(frame: &mut Frame<'_>, state: &AppState, theme: Theme) {
    let area = frame.area();
    if area.width < 16 || area.height < 6 {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new("Esc close | resize for task details").wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let modal = detail_area(area.width, area.height);
    frame.render_widget(Clear, modal);
    let body = Rect::new(modal.x + 1, modal.y + 1, modal.width - 2, modal.height - 3);
    let lines = wrapped_lines(&state.task_detail_lines, body.width);
    let max_scroll = lines
        .len()
        .saturating_sub(body.height as usize)
        .min(u16::MAX as usize) as u16;
    let scroll = state.task_detail_scroll.min(max_scroll);
    frame.render_widget(
        Block::default()
            .title("Task details | read only")
            .borders(Borders::ALL)
            .border_set(theme.border_set())
            .border_style(theme.border()),
        modal,
    );
    frame.render_widget(
        Paragraph::new(lines.into_iter().map(Line::from).collect::<Vec<_>>()).scroll((scroll, 0)),
        body,
    );
    frame.render_widget(
        Paragraph::new(format!(
            "Esc close | j/k scroll | n/p page | Home/End | {scroll}/{max_scroll}"
        )),
        Rect::new(body.x, modal.bottom() - 2, body.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::{event, report, Fixture};
    use ratatui::{backend::TestBackend, Terminal};

    fn state() -> AppState {
        let mut state = AppState::default();
        state.active = Screen::History;
        state.history_mode = HistoryMode::Tasks;
        state.tasks.value = Some(report(3));
        state
    }
    fn render(state: &mut AppState, width: u16, height: u16) -> String {
        update_task_detail_scroll(state, width, height);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, state, Theme::detect()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn task_table_renders_wide_standard_narrow_and_tiny_with_explicit_coverage() {
        let mut state = state();
        for (width, height) in [(180, 42), (100, 36), (80, 30), (40, 40)] {
            let text = render(&mut state, width, height);
            assert!(text.contains("Task usage"), "{width}x{height}");
            assert!(text.contains("Unattributed"), "{width}x{height}");
            assert!(text.contains("unsupported 1"), "{width}x{height}");
            assert!(text.contains("task:"), "{width}x{height}");
            assert!(text.contains("model-fixture"), "{width}x{height}");
        }
        for (width, height) in [(20, 8), (1, 1), (0, 0), (80, 10)] {
            let _ = render(&mut state, width, height);
        }
    }

    #[test]
    fn table_loading_error_empty_and_only_unattributed_are_distinct() {
        let mut state = state();
        state.tasks.loading = true;
        assert!(render(&mut state, 120, 35).contains("Loading local evidence"));
        state.tasks.loading = false;
        state.tasks.error =
            Some("No local telemetry database is available. Run `codex-meter sync`".into());
        assert!(render(&mut state, 120, 35).contains("No local telemetry database"));
        state.tasks.error = Some("Local SQLite reporting failed (storage_evidence_corrupt)".into());
        assert!(render(&mut state, 120, 35).contains("evidence_corrupt"));
        state.tasks.error = None;
        state.tasks.value = Some(report(0));
        assert!(render(&mut state, 120, 35).contains("No task observations"));
        let mut orphan = event(1);
        orphan.task_id = None;
        state.tasks.value = Some(Fixture::new(&[orphan]).report());
        assert!(render(&mut state, 120, 35).contains("Only unattributed observations"));
    }

    #[test]
    fn truncation_and_missing_partial_zero_are_never_presented_as_complete_usage() {
        let mut a = event(0);
        let mut b = event(1);
        b.task_id = a.task_id.clone();
        b.raw_total = None;
        a.input_tokens = None;
        b.input_tokens = None;
        let fixture = Fixture::new(&[a, b]);
        let mut state = state();
        let mut report = fixture.report();
        report.has_more = true;
        report.tasks[0].configurations_truncated = true;
        state.task_detail_lines = task_detail_lines(&report.tasks[0], &report.evidence_quality);
        state.tasks.value = Some(report);
        let text = render(&mut state, 180, 42);
        assert!(text.contains("TRUNCATED"));
        assert!(text.contains("0 partial"));
        let lines = state.task_detail_lines.join("\n");
        assert!(lines.contains("Input: Unavailable"));
        assert!(lines.contains("Raw total") || lines.contains("raw total"));
        assert!(lines.contains("TRUNCATED"));
        assert!(lines.contains("known 1 / missing 1"));
    }

    #[test]
    fn overlay_scroll_resize_and_unicode_long_identifiers_remain_bounded() {
        let mut state = state();
        let report = state.tasks.value.as_mut().unwrap();
        let task = &mut report.tasks[0];
        task.task_id = format!("task:{}", "界λ".repeat(20));
        task.configurations[0].model = Some("long-model-".repeat(20));
        state.task_detail_lines = task_detail_lines(task, &report.evidence_quality);
        state.task_detail = Some(task.clone());
        state.overlay = Some(Overlay::TaskDetail);
        for (width, height) in [(140, 40), (80, 24), (32, 12), (12, 4), (100, 50)] {
            let text = render(&mut state, width, height);
            assert!(text.contains("Esc"));
            assert!(state.task_detail_scroll <= state.task_detail_max_scroll);
            state.task_detail_scroll = u16::MAX;
        }
        let text = render(&mut state, 80, 24);
        assert!(text.contains("No conversation content"));
        assert!(!wrapped_lines(&["界界λa".into()], 3)
            .iter()
            .any(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) > 3));
    }

    #[test]
    fn task_presentation_hides_sensitive_metadata_and_uses_full_composite_display_hash() {
        let mut state = state();
        let report = state.tasks.value.as_mut().unwrap();
        let a = report.tasks[0].clone();
        let mut b = a.clone();
        b.source_id = "src:other".into();
        assert_ne!(compact_identity(&a), compact_identity(&b));
        b = a.clone();
        b.session_id = "session:other".into();
        assert_ne!(compact_identity(&a), compact_identity(&b));
        let task = &mut report.tasks[0];
        task.configurations[0].model = Some("/private/repository/PROMPT_SENTINEL".into());
        task.configurations[0].service_tier = Some("user@example.invalid".into());
        task.configurations[0].reasoning_level = Some("sk-SENSITIVE_SENTINEL".into());
        state
            .composer
            .editor
            .insert_paste("PROMPT_CONTENT_SENTINEL")
            .unwrap();
        state.execution.output = "TRANSCRIPT_SENTINEL".into();
        state.task_detail_lines = task_detail_lines(task, &report.evidence_quality);
        state.overlay = Some(Overlay::TaskDetail);
        let text = render(&mut state, 120, 70);
        for sensitive in [
            "private/repository",
            "PROMPT_SENTINEL",
            "user@example",
            "sk-SENSITIVE",
            "PROMPT_CONTENT",
            "TRANSCRIPT_SENTINEL",
        ] {
            assert!(!text.contains(sensitive), "{sensitive}");
        }
    }
}
