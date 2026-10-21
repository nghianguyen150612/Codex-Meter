use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Wrap,
};
use ratatui::Frame;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::codex_app_server::{
    HistoryAvailability, ModelInfo, ProjectAssociationKind, ProjectSummary,
};
use crate::storage::{
    EvidenceDetail, MetricCoverage, MetricTotal, QuotaSnapshotEvidence, TokenUsageEvidence,
};

use super::backend::{LedgerStatus, OverviewData};
use super::layout::responsive_layout;
use super::state::{
    looks_like_path, AppServerStatus, AppState, HistoryMode, Overlay, Resource, Screen,
};
use super::theme::Theme;

pub fn draw(frame: &mut Frame<'_>, state: &AppState, theme: Theme) {
    let layout = responsive_layout(frame.area());
    draw_header(frame, layout.header, state, theme);
    if let Some(tabs) = layout.tabs {
        draw_compact_tabs(frame, tabs, state, theme);
    }
    if let Some(sidebar) = layout.sidebar {
        draw_sidebar(frame, sidebar, state, theme);
    }
    draw_screen(frame, layout.content, state, theme, layout.wide);
    draw_footer(frame, layout.footer, state, theme);
    if let Some(overlay) = state.overlay.as_ref() {
        draw_overlay(frame, overlay, state, theme);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let connection = match state.app_server_status {
        AppServerStatus::Unknown => "App Server: not checked",
        AppServerStatus::Connecting => "App Server: connecting",
        AppServerStatus::Connected => "App Server: connected",
        AppServerStatus::Unavailable => "App Server: unavailable",
    };
    let marker_style = match state.app_server_status {
        AppServerStatus::Connected => theme.healthy(),
        AppServerStatus::Unavailable => theme.warning(),
        AppServerStatus::Unknown | AppServerStatus::Connecting => theme.muted(),
    };
    let first = if area.width >= 72 {
        Line::from(vec![
            Span::styled(" CODEX METER ", theme.title()),
            Span::styled(
                format!("v{} | development", env!("CARGO_PKG_VERSION")),
                theme.muted(),
            ),
            Span::raw("   LOCAL CODEX   "),
            Span::styled(connection, marker_style),
        ])
    } else if area.width >= 42 {
        Line::from(vec![
            Span::styled(" Codex Meter", theme.title()),
            Span::raw(" | "),
            Span::raw(state.active.label()),
            Span::raw(" | "),
            Span::styled(
                format!("App: {}", short_status(state.app_server_status.clone())),
                marker_style,
            ),
        ])
    } else if area.width >= 26 {
        Line::from(vec![
            Span::styled(" Codex Meter", theme.title()),
            Span::raw(" | "),
            Span::raw(state.active.label()),
        ])
    } else {
        Line::from(format!(
            " CM {}/5 {}",
            state.active.index() + 1,
            state.active.label()
        ))
    };
    let second = if area.width >= 72 {
        Line::from(" Local telemetry is not complete account usage.")
    } else if area.width >= 42 {
        Line::from(" Local telemetry only; not account-wide")
    } else if area.width >= 30 {
        Line::from(format!(
            " App: {} | local only",
            match state.app_server_status {
                AppServerStatus::Unknown => "unknown",
                AppServerStatus::Connecting => "wait",
                AppServerStatus::Connected => "ready",
                AppServerStatus::Unavailable => "down",
            }
        ))
    } else if area.width >= 26 {
        Line::from(" Local only")
    } else {
        Line::from(" Local telemetry only")
    };
    let lines = if area.height >= 2 {
        vec![first, second]
    } else {
        vec![first]
    };
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .border_set(theme.border_set())
                .borders(Borders::BOTTOM)
                .border_style(theme.border()),
        ),
        area,
    );
}

fn short_status(status: AppServerStatus) -> &'static str {
    match status {
        AppServerStatus::Unknown => "not checked",
        AppServerStatus::Connecting => "connecting",
        AppServerStatus::Connected => "connected",
        AppServerStatus::Unavailable => "unavailable",
    }
}

fn draw_compact_tabs(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    let line = if area.width < 74 {
        Line::from(format!(
            " View {}/5: {}  |  1-5 switch",
            state.active.index() + 1,
            state.active.label()
        ))
    } else {
        Line::from(
            Screen::ALL
                .iter()
                .enumerate()
                .map(|(index, screen)| {
                    let label = format!(" {} {} ", index + 1, screen.label());
                    if *screen == state.active {
                        Span::styled(label, theme.selected())
                    } else {
                        Span::raw(label)
                    }
                })
                .collect::<Vec<_>>(),
        )
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_sidebar(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    let items = Screen::ALL
        .iter()
        .enumerate()
        .map(|(index, screen)| ListItem::new(format!(" {}  {}", index + 1, screen.label())))
        .collect::<Vec<_>>();
    let mut list_state = ListState::default();
    list_state.select(Some(state.active.index()));
    let list = List::new(items)
        .block(
            Block::default()
                .title("Workspace")
                .border_set(theme.border_set())
                .borders(Borders::ALL)
                .border_style(theme.border()),
        )
        .highlight_style(theme.selected())
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_screen(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme, wide: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    match state.active {
        Screen::Overview => draw_overview(frame, area, state, theme),
        Screen::Sessions => draw_sessions(frame, area, state, theme, wide),
        Screen::Projects => draw_projects(frame, area, state, theme, wide),
        Screen::Models => draw_models(frame, area, state, theme, wide),
        Screen::History => draw_history(frame, area, state, theme, wide),
    }
}

fn draw_overview(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    let detailed = area.width >= 84 && area.height >= 30;
    let (quota_height, token_height, source_height) = if detailed {
        (8, 11, area.height.saturating_sub(19))
    } else {
        let quota = area.height.min(7);
        let token = area.height.saturating_sub(quota).min(8);
        (quota, token, area.height.saturating_sub(quota + token))
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(quota_height),
            Constraint::Length(token_height),
            Constraint::Length(source_height),
        ])
        .split(area);
    let data = state.overview.value.as_ref();
    let quota_lines = data.map_or_else(
        || loading_or_unavailable(&state.overview, "Quota evidence is not loaded."),
        |overview| quota_overview_lines(overview, area.width),
    );
    render_panel(
        frame,
        chunks[0],
        "QUOTA | observed only",
        quota_lines,
        theme,
    );

    let token_lines = data.map_or_else(
        || loading_or_unavailable(&state.overview, "Token evidence is not loaded."),
        |overview| token_overview_lines(overview, !detailed),
    );
    render_panel(
        frame,
        chunks[1],
        "TOKEN STATISTICS | canonical responses",
        token_lines,
        theme,
    );

    let health = data.map_or_else(
        || loading_or_unavailable(&state.overview, "Local source health is not loaded."),
        |overview| {
            if detailed {
                source_health_lines(overview, state)
            } else {
                compact_source_health_lines(overview, state)
            }
        },
    );
    render_panel(frame, chunks[2], "SOURCE HEALTH", health, theme);
}

fn quota_overview_lines(data: &OverviewData, width: u16) -> Vec<Line<'static>> {
    let Some(timeline) = data.quota.as_ref() else {
        return vec![Line::from("Unavailable | no local quota sample")];
    };
    let bar_width = width.saturating_sub(66).clamp(8, 30) as usize;
    let mut lines = Vec::new();
    lines.extend(quota_sample_lines(
        "5-hour",
        timeline.five_hour.as_ref(),
        bar_width,
    ));
    lines.extend(quota_sample_lines(
        "Weekly",
        timeline.weekly.as_ref(),
        bar_width,
    ));
    let quality = &timeline.evidence_quality;
    let warnings = quality.total_diagnostics;
    let coverage = if quality.unsupported_source_versions > 0 {
        "Unsupported source version observed"
    } else if quality.known_partial_collection > 0
        || quality.malformed_or_incomplete_evidence > 0
        || quality.source_continuity_ambiguities > 0
    {
        "Partial local evidence coverage"
    } else if warnings > 0 {
        "Evidence diagnostics present"
    } else {
        "Local evidence only | account-wide coverage unavailable"
    };
    lines.push(Line::from(if warnings > 0 {
        format!("{coverage} | {warnings} coverage diagnostic(s)")
    } else {
        coverage.to_owned()
    }));
    lines
}

fn quota_sample_lines(
    label: &str,
    sample: Option<&QuotaSnapshotEvidence>,
    bar_width: usize,
) -> Vec<Line<'static>> {
    let Some(sample) = sample else {
        return vec![Line::from(format!(
            "{label:<8} Unavailable | no observed local sample"
        ))];
    };
    let value = sample.used_percent;
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return vec![
            Line::from(format!(
                "{label:<8} Invalid measurement | {}%",
                number(value)
            )),
            Line::from(format!(
                "         Observed: {}",
                clean(&sample.observed_at, 80)
            )),
        ];
    }
    let freshness = freshness(&sample.observed_at);
    let filled = ((bar_width as f64 * value / 100.0).round() as usize).min(bar_width);
    let bar = format!(
        "[{}{}]",
        "#".repeat(filled),
        "-".repeat(bar_width.saturating_sub(filled))
    );
    let remaining = 100.0 - value;
    let used = number(value);
    let remaining_text = number(remaining);
    vec![
        Line::from(format!(
            "{label:<8} {bar} {used}% used | {remaining_text}% remaining | {freshness}"
        )),
        Line::from(format!(
            "         Observed: {} | Reset: {}",
            clean(&sample.observed_at, 40),
            sample
                .reset_at
                .as_deref()
                .map(|v| clean(v, 40))
                .unwrap_or_else(|| "Unavailable | not observed".to_owned())
        )),
    ]
}

fn token_overview_lines(data: &OverviewData, compact: bool) -> Vec<Line<'static>> {
    let Some(stats) = data.token_statistics.as_ref() else {
        let mut lines = vec![Line::from("Unavailable | no local token evidence")];
        if data.ledger_status == LedgerStatus::Missing {
            lines.push(Line::from(
                "Press s, then confirm, to run one-shot local telemetry sync.",
            ));
            lines.push(Line::from("No sync runs automatically."));
        }
        return lines;
    };
    if compact {
        return vec![
            Line::from(format!(
                "Raw total                 {}",
                metric_display(&stats.raw_total)
            )),
            Line::from(format!(
                "Input {} | Cached input {}",
                metric_display(&stats.input_tokens),
                metric_display(&stats.cached_input_tokens)
            )),
            Line::from(format!(
                "Cache-write input         {}",
                metric_display(&stats.cache_write_input_tokens)
            )),
            Line::from(format!(
                "Output {} | Reasoning {}",
                metric_display(&stats.output_tokens),
                metric_display(&stats.reasoning_output_tokens)
            )),
            Line::from(format!(
                "Responses {} | Raw total missing {}",
                stats.canonical_response_count, stats.raw_total.missing_event_count
            )),
        ];
    }
    vec![
        Line::from(format!(
            "Raw total                 {}",
            metric_display(&stats.raw_total)
        )),
        Line::from(format!(
            "Input tokens              {}",
            metric_display(&stats.input_tokens)
        )),
        Line::from(format!(
            "Cached input              {}",
            metric_display(&stats.cached_input_tokens)
        )),
        Line::from(format!(
            "Cache-write input         {}",
            metric_display(&stats.cache_write_input_tokens)
        )),
        Line::from(format!(
            "Output                    {}",
            metric_display(&stats.output_tokens)
        )),
        Line::from(format!(
            "Reasoning output          {}",
            metric_display(&stats.reasoning_output_tokens)
        )),
        Line::from(format!(
            "Canonical responses       {}",
            stats.canonical_response_count
        )),
        Line::from(format!(
            "Raw total missing         {}",
            stats.raw_total.missing_event_count
        )),
        Line::from("Snapshot records are excluded from consumption totals."),
    ]
}

fn compact_source_health_lines(data: &OverviewData, state: &AppState) -> Vec<Line<'static>> {
    let ledger = match data.ledger_status {
        LedgerStatus::Available => "readable".to_owned(),
        LedgerStatus::Missing => "unavailable".to_owned(),
    };
    let schema = data
        .schema_version
        .map_or_else(|| "unavailable".to_owned(), |value| format!("v{value}"));
    let telemetry = data.telemetry_version.as_deref().map_or_else(
        || "unavailable".to_owned(),
        |value| {
            let support = if value == crate::telemetry::COMPATIBILITY_CLI_VERSION {
                "supported"
            } else {
                "unverified"
            };
            format!("{} {support}", clean(value, 24))
        },
    );
    let (session_count, active, idle, not_loaded, last_update) =
        state.sessions.value.as_deref().map_or_else(
            || ("unavailable".to_owned(), 0, 0, 0, "unavailable".to_owned()),
            |sessions| {
                let active = sessions
                    .iter()
                    .filter(|session| session.runtime_status.as_str() == "active")
                    .count();
                let idle = sessions
                    .iter()
                    .filter(|session| session.runtime_status.as_str() == "idle")
                    .count();
                let not_loaded = sessions
                    .iter()
                    .filter(|session| session.runtime_status.as_str() == "not_loaded")
                    .count();
                let last_update = sessions
                    .iter()
                    .filter_map(|session| session.updated_at)
                    .max()
                    .map(format_unix)
                    .unwrap_or_else(|| "unavailable".to_owned());
                (
                    sessions.len().to_string(),
                    active,
                    idle,
                    not_loaded,
                    last_update,
                )
            },
        );
    vec![
        Line::from(format!(
            "SQLite: {ledger} | schema {schema} | telemetry {telemetry}"
        )),
        Line::from(format!(
            "Sessions {session_count} | active {active} | idle {idle} | not loaded {not_loaded} | updated {last_update}"
        )),
    ]
}

fn source_health_lines(data: &OverviewData, state: &AppState) -> Vec<Line<'static>> {
    let ledger = match data.ledger_status {
        LedgerStatus::Available => "SQLite evidence ledger: readable",
        LedgerStatus::Missing => "SQLite evidence ledger: unavailable | no database found",
    };
    let app_server = match state.app_server_status {
        AppServerStatus::Connected => "Codex App Server: connected",
        AppServerStatus::Unavailable => "Codex App Server: unavailable",
        AppServerStatus::Unknown => "Codex App Server: not checked",
        AppServerStatus::Connecting => "Codex App Server: connecting",
    };
    let version = match data.telemetry_version.as_deref() {
        Some(value) if value == crate::telemetry::COMPATIBILITY_CLI_VERSION => {
            format!("Telemetry version: supported ({})", clean(value, 40))
        }
        Some(value) => format!(
            "Telemetry version: unsupported/unverified ({})",
            clean(value, 40)
        ),
        None => "Telemetry version: unavailable".to_owned(),
    };
    let coverage = data.evidence_quality.as_ref().map_or_else(
        || "Source coverage: unavailable | no local evidence".to_owned(),
        |quality| {
            if quality.unsupported_source_versions > 0 {
                "Source coverage: unsupported source version recorded".to_owned()
            } else if quality.known_partial_collection > 0
                || quality.malformed_or_incomplete_evidence > 0
                || quality.source_continuity_ambiguities > 0
            {
                "Source coverage: partial local collection".to_owned()
            } else {
                "Source coverage: local telemetry | not account-wide".to_owned()
            }
        },
    );
    let last_sample = data.quota.as_ref().and_then(|quota| {
        [quota.five_hour.as_ref(), quota.weekly.as_ref()]
            .into_iter()
            .flatten()
            .map(|sample| sample.observed_at.as_str())
            .max_by_key(|observed_at| OffsetDateTime::parse(observed_at, &Rfc3339).ok())
    });
    let last_sample = last_sample.map_or_else(
        || "Last locally observed quota sample: unavailable".to_owned(),
        |value| format!("Last locally observed quota sample: {}", clean(value, 72)),
    );
    let (session_summary, last_session_update) = match state.sessions.value.as_deref() {
        Some(sessions) => {
            let active = sessions
                .iter()
                .filter(|session| session.runtime_status.as_str() == "active")
                .count();
            let idle = sessions
                .iter()
                .filter(|session| session.runtime_status.as_str() == "idle")
                .count();
            let not_loaded = sessions
                .iter()
                .filter(|session| session.runtime_status.as_str() == "not_loaded")
                .count();
            let last_update = sessions
                .iter()
                .filter_map(|session| session.updated_at)
                .max()
                .map(format_unix);
            (
                format!("Sessions: {} known | active {active} | idle {idle} | not loaded {not_loaded} | other {}", sessions.len(), sessions.len().saturating_sub(active + idle + not_loaded)),
                format!("Last session update: {}", last_update.unwrap_or_else(|| "Unavailable".to_owned())),
            )
        }
        None if state.sessions.loading => (
            "Sessions: loading App Server metadata".to_owned(),
            "Last session update: unavailable".to_owned(),
        ),
        None if state.sessions.error.is_some() => (
            "Sessions: unavailable | App Server request failed".to_owned(),
            "Last session update: unavailable".to_owned(),
        ),
        None => (
            "Sessions: unavailable | not loaded".to_owned(),
            "Last session update: unavailable".to_owned(),
        ),
    };
    vec![
        Line::from(ledger),
        Line::from(app_server),
        Line::from(version),
        Line::from(coverage),
        Line::from(last_sample),
        Line::from(session_summary),
        Line::from(last_session_update),
        Line::from(format!(
            "SQLite migration schema: {}",
            data.schema_version
                .map_or_else(|| "unavailable".to_owned(), |version| version.to_string())
        )),
    ]
}

fn draw_sessions(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme, wide: bool) {
    let title = format!(
        "Sessions | {} | {}",
        archive_filter_label(state),
        if state.sessions_active_only {
            "active runtime"
        } else {
            "all runtime states"
        }
    );
    let Some(sessions) = state.sessions.value.as_ref() else {
        render_resource_state(
            frame,
            area,
            &state.sessions,
            "Sessions",
            "Connect to local Codex App Server to list session metadata.",
            theme,
        );
        return;
    };
    let visible = state.visible_sessions();
    if visible.is_empty() {
        let detail = if sessions.is_empty() {
            "No local sessions are currently reported by Codex App Server."
        } else {
            "No sessions match the current filter or search."
        };
        render_panel(frame, area, title, vec![Line::from(detail)], theme);
        return;
    }
    let query = state
        .searches
        .get(&Screen::Sessions)
        .map(String::as_str)
        .unwrap_or("");
    let page = state.visible_sessions_page();
    let mut rows = Vec::with_capacity(page.len());
    for (index, session) in page.iter().enumerate() {
        let source = session
            .source
            .as_ref()
            .map_or("Unavailable", |value| value.as_str());
        let model = session.model.as_deref().unwrap_or("Unavailable");
        let updated = session
            .updated_at
            .map_or_else(|| "Unavailable".to_owned(), format_unix);
        let columns = if wide {
            vec![
                clean_metadata(&session.thread_id, 72),
                clean(session.runtime_status.as_str(), 20),
                clean(source, 24),
                clean_metadata(model, 44),
                updated,
            ]
        } else {
            vec![
                clean_metadata(&session.thread_id, 42),
                clean(session.runtime_status.as_str(), 20),
                clean(source, 22),
                clean_metadata(model, 38),
                updated,
            ]
        };
        let row = Row::new(columns).style(if index == state.selected_session {
            theme.selected()
        } else {
            Style::default()
        });
        rows.push(row);
    }
    let filter_note = if query.is_empty() {
        ""
    } else {
        " | search active"
    };
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(if wide { 30 } else { 28 }),
            Constraint::Percentage(13),
            Constraint::Percentage(15),
            Constraint::Percentage(22),
            Constraint::Percentage(20),
        ],
    )
    .header(
        Row::new(["Session / Thread", "Status", "Source", "Model", "Updated"]).style(theme.title()),
    )
    .block(
        Block::default()
            .title(format!(
                "{title} | page {}/{}{filter_note}",
                state.session_page + 1,
                state.page_count()
            ))
            .border_set(theme.border_set())
            .borders(Borders::ALL)
            .border_style(theme.border()),
    )
    .column_spacing(1)
    .highlight_symbol("> ");
    let mut table_state = TableState::default();
    table_state.select(Some(state.selected_session));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn draw_projects(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme, _wide: bool) {
    let Some(projects) = state.projects.value.as_ref() else {
        render_resource_state(
            frame,
            area,
            &state.projects,
            "Projects",
            "Project grouping uses current local session metadata. Paths and remotes are hidden.",
            theme,
        );
        return;
    };
    let visible = state.visible_projects();
    if visible.is_empty() {
        render_panel(
            frame,
            area,
            "Projects | response-local aliases",
            vec![Line::from(if projects.is_empty() {
                "No project associations were reported."
            } else {
                "No projects match the current search."
            })],
            theme,
        );
        return;
    }
    let page = state.visible_projects_page();
    let rows = page
        .iter()
        .enumerate()
        .map(|(index, project)| project_row(project, index == state.selected_project, theme))
        .collect::<Vec<_>>();
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(27),
            Constraint::Percentage(15),
            Constraint::Percentage(25),
            Constraint::Percentage(16),
            Constraint::Percentage(17),
        ],
    )
    .header(
        Row::new([
            "Project",
            "Sessions",
            "Association",
            "Repositories",
            "Worktrees",
        ])
        .style(theme.title()),
    )
    .block(
        Block::default()
            .title(format!(
                "Projects | aliases last only for this response | page {}/{}",
                state.project_page + 1,
                state.page_count()
            ))
            .border_set(theme.border_set())
            .borders(Borders::ALL)
            .border_style(theme.border()),
    )
    .column_spacing(1);
    let mut table_state = TableState::default();
    table_state.select(Some(state.selected_project));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn project_row(project: &ProjectSummary, selected: bool, theme: Theme) -> Row<'static> {
    let association = association_label(project.association);
    let rows = Row::new([
        clean(&project.project_alias, 48),
        project.session_count.to_string(),
        association.to_owned(),
        project
            .repository_count
            .map_or_else(|| "Unavailable".to_owned(), |value| value.to_string()),
        project
            .worktree_count
            .map_or_else(|| "Unavailable".to_owned(), |value| value.to_string()),
    ]);
    if selected {
        rows.style(theme.selected())
    } else {
        rows
    }
}

fn draw_models(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme, _wide: bool) {
    let Some(catalog) = state.models.value.as_ref() else {
        render_resource_state(
            frame,
            area,
            &state.models,
            "Models",
            "Model metadata comes from the live local Codex App Server catalog.",
            theme,
        );
        return;
    };
    let visible = state.visible_models();
    if visible.is_empty() {
        render_panel(
            frame,
            area,
            "Models | live local catalog",
            vec![
                Line::from(if catalog.models.is_empty() {
                    "The current App Server reported no model entries."
                } else {
                    "No models match the current search."
                }),
                Line::from(
                    "Catalog availability does not prove account entitlement or execution access.",
                ),
            ],
            theme,
        );
        return;
    }
    let page = state.visible_models_page();
    let rows = page
        .iter()
        .enumerate()
        .map(|(index, model)| model_row(model, index == state.selected_model, theme))
        .collect::<Vec<_>>();
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(28),
            Constraint::Percentage(30),
            Constraint::Percentage(12),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
        ],
    )
    .header(
        Row::new([
            "Model ID",
            "Display name",
            "Default",
            "Visibility",
            "Default effort",
        ])
        .style(theme.title()),
    )
    .block(
        Block::default()
            .title(format!(
                "Models | local App Server catalog | account access not guaranteed | page {}/{}",
                state.model_page + 1,
                state.page_count()
            ))
            .border_set(theme.border_set())
            .borders(Borders::ALL)
            .border_style(theme.border()),
    )
    .column_spacing(1);
    let mut table_state = TableState::default();
    table_state.select(Some(state.selected_model));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn model_row(model: &ModelInfo, selected: bool, theme: Theme) -> Row<'static> {
    let row = Row::new([
        clean_metadata(&model.id, 80),
        clean_metadata(&model.display_name, 80),
        if model.is_default {
            "Yes".to_owned()
        } else {
            "No".to_owned()
        },
        if model.hidden {
            "Hidden".to_owned()
        } else {
            "Visible".to_owned()
        },
        clean_metadata(&model.default_reasoning_effort, 64),
    ]);
    if selected {
        row.style(theme.selected())
    } else {
        row
    }
}

fn draw_history(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme, _wide: bool) {
    if state.history_mode == HistoryMode::Quota {
        draw_quota_history(frame, area, state, theme);
        return;
    }
    let Some(page) = state.history.value.as_ref() else {
        render_resource_state(frame, area, &state.history, "History", "Canonical per-response token evidence. Snapshots are inspected separately and never summed.", theme);
        return;
    };
    if page.events.is_empty() {
        render_panel(
            frame,
            area,
            "History | canonical per-response evidence",
            vec![
                Line::from("No canonical token events were observed for this scope."),
                Line::from("This does not mean account usage was zero."),
            ],
            theme,
        );
        return;
    }
    let events = state.visible_token_events();
    if events.is_empty() {
        render_panel(
            frame,
            area,
            "History | canonical per-response evidence",
            vec![Line::from(
                "No events match this search on the current page.",
            )],
            theme,
        );
        return;
    }
    let rows = events
        .iter()
        .enumerate()
        .map(|(index, event)| history_row(event, index == state.selected_history, theme))
        .collect::<Vec<_>>();
    let scope = state.history_scope.label();
    let model = state
        .history_model_filter
        .as_deref()
        .map(|value| format!(" | model: {}", clean(value, 48)))
        .unwrap_or_default();
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(22),
            Constraint::Percentage(16),
            Constraint::Percentage(19),
            Constraint::Percentage(18),
        ],
    )
    .header(Row::new(["Time", "Model", "Reasoning", "Raw tokens", "Evidence"]).style(theme.title()))
    .block(
        Block::default()
            .title(format!(
                "History | {scope}{model} | page {}",
                state.history_page_cursors.len()
            ))
            .border_set(theme.border_set())
            .borders(Borders::ALL)
            .border_style(theme.border()),
    )
    .column_spacing(1);
    let mut table_state = TableState::default();
    table_state.select(Some(state.selected_history));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn history_row(event: &TokenUsageEvidence, selected: bool, theme: Theme) -> Row<'static> {
    let evidence = match (event.raw_total, event.input_tokens, event.output_tokens) {
        (Some(_), Some(_), Some(_)) => "complete fields".to_owned(),
        _ => "incomplete fields".to_owned(),
    };
    let row = Row::new([
        clean(&event.observed_at, 72),
        clean_metadata(event.model.as_deref().unwrap_or("Unavailable"), 72),
        clean_metadata(
            event.reasoning_level.as_deref().unwrap_or("Unavailable"),
            48,
        ),
        event
            .raw_total
            .map_or_else(|| "Unavailable".to_owned(), |value| value.to_string()),
        evidence,
    ]);
    if selected {
        row.style(theme.selected())
    } else {
        row
    }
}

fn draw_quota_history(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    let Some(page) = state.quota_history.value.as_ref() else {
        render_resource_state(
            frame,
            area,
            &state.quota_history,
            "Quota history",
            "Quota snapshots are observations; they are not task-level consumption.",
            theme,
        );
        return;
    };
    if page.samples.is_empty() {
        render_panel(
            frame,
            area,
            "Quota history | observed snapshots",
            vec![
                Line::from("No quota snapshots are available."),
                Line::from("No account capacity or token limit is inferred."),
            ],
            theme,
        );
        return;
    }
    let samples = state.visible_quota_samples();
    if samples.is_empty() {
        render_panel(
            frame,
            area,
            "Quota history | observed snapshots",
            vec![Line::from(
                "No quota samples match this search on the current page.",
            )],
            theme,
        );
        return;
    }
    let rows = samples
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let row = Row::new([
                clean(&sample.observed_at, 72),
                match sample.meter_type {
                    crate::telemetry::QuotaMeterType::FiveHour => "5-hour",
                    crate::telemetry::QuotaMeterType::Weekly => "Weekly",
                }
                .to_owned(),
                if sample.used_percent.is_finite() && (0.0..=100.0).contains(&sample.used_percent) {
                    format!("{}% used", number(sample.used_percent))
                } else {
                    "Invalid measurement".to_owned()
                },
                sample
                    .reset_at
                    .as_deref()
                    .map(|value| clean(value, 72))
                    .unwrap_or_else(|| "Unavailable".to_owned()),
                clean_metadata(sample.codex_version.as_deref().unwrap_or("Unavailable"), 48),
            ]);
            if index == state.selected_history {
                row.style(theme.selected())
            } else {
                row
            }
        })
        .collect::<Vec<_>>();
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(25),
            Constraint::Percentage(14),
            Constraint::Percentage(18),
            Constraint::Percentage(25),
            Constraint::Percentage(18),
        ],
    )
    .header(
        Row::new(["Observed", "Window", "Used", "Reset", "Telemetry version"]).style(theme.title()),
    )
    .block(
        Block::default()
            .title(format!(
                "Quota history | page {}",
                state.quota_page_cursors.len()
            ))
            .border_set(theme.border_set())
            .borders(Borders::ALL)
            .border_style(theme.border()),
    )
    .column_spacing(1);
    let mut table_state = TableState::default();
    table_state.select(Some(state.selected_history));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_resource_state<T>(
    frame: &mut Frame<'_>,
    area: Rect,
    resource: &Resource<T>,
    title: &str,
    hint: &str,
    theme: Theme,
) {
    let (text, style) = if resource.loading {
        ("Loading local evidence...".to_owned(), theme.muted())
    } else if let Some(error) = resource.error.as_deref() {
        (clean(error, 240), theme.error())
    } else {
        (hint.to_owned(), theme.unavailable())
    };
    let paragraph = Paragraph::new(text)
        .style(style)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .title(title)
                .border_set(theme.border_set())
                .borders(Borders::ALL)
                .border_style(theme.border()),
        );
    frame.render_widget(paragraph, area);
}

fn loading_or_unavailable<T>(resource: &Resource<T>, fallback: &str) -> Vec<Line<'static>> {
    if resource.loading {
        vec![Line::from("Loading local evidence...")]
    } else if let Some(error) = resource.error.as_deref() {
        vec![Line::from(clean(error, 200))]
    } else {
        vec![Line::from(fallback.to_owned())]
    }
}

fn render_panel(
    frame: &mut Frame<'_>,
    area: Rect,
    title: impl Into<String>,
    lines: Vec<Line<'static>>,
    theme: Theme,
) {
    let paragraph = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .title(title.into())
                .border_set(theme.border_set())
                .borders(Borders::ALL)
                .border_style(theme.border()),
        );
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let keys = match state.active {
        Screen::Sessions => "Tab switch | Up/Down/jk move | n/p page | Enter details | / search | f archive | a active | L load | F fork | r refresh | s sync | ? help | q quit",
        Screen::History if state.history_mode == HistoryMode::Tokens => "Tab switch | Up/Down/jk move | n/p page | Enter inspect | / search | m model | w scope | t token/quota | r refresh | s sync | ? help | q quit",
        Screen::History => "Tab switch | Up/Down/jk move | n/p page | Enter inspect | / search | w scope | t token/quota | r refresh | s sync | ? help | q quit",
        Screen::Projects | Screen::Models => "Tab switch | Up/Down/jk move | n/p page | Enter details | / search | r refresh | s sync | ? help | q quit",
        Screen::Overview => "Tab switch | 1-5 views | r refresh | s sync | ? help | q quit",
    };
    let mut lines = Vec::new();
    if let Some(search) = state.search.as_ref() {
        lines.push(Line::from(format!(
            "{}: {}_",
            if search.kind == super::state::SearchKind::Model {
                "Model filter"
            } else {
                "Search"
            },
            clean(&search.value, 80)
        )));
    } else if let Some(status) = state.status.as_deref() {
        lines.push(Line::from(clean(status, 240)));
    } else {
        lines.push(Line::from(""));
    }
    if area.height > 1 {
        if state.search.is_some() {
            lines.push(Line::from("Printable keys edit | Enter apply | Esc cancel"));
        } else if state.overlay.is_some() {
            lines.push(Line::from("Esc close | q quit"));
        } else {
            lines.push(Line::from(keys));
        }
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .border_set(theme.border_set())
                .borders(Borders::TOP)
                .border_style(theme.border()),
        ),
        area,
    );
}

fn draw_overlay(frame: &mut Frame<'_>, overlay: &Overlay, state: &AppState, theme: Theme) {
    let area = frame.area();
    if area.width < 20 || area.height < 5 {
        return;
    }
    let (title, lines) = match overlay {
        Overlay::Help => (
            "Keyboard help",
            vec![
                "Tab / Shift+Tab or 1-5  Switch views".to_owned(),
                "Up/Down or j/k              Move selection".to_owned(),
                "Enter                  Open safe metadata details".to_owned(),
                "/                      Search displayed metadata".to_owned(),
                "n/p or PageUp/Down     Change bounded list page".to_owned(),
                "r                      Refresh the active view".to_owned(),
                "s                      Ask before one-shot local telemetry sync".to_owned(),
                "f/a (Sessions)         Archive filter / active runtime".to_owned(),
                "L                      Load selected session; no prompt or turn".to_owned(),
                "F                      Confirm before cloning a session history".to_owned(),
                "m (token history) / w scope / t token-quota".to_owned(),
                "Esc                    Close dialog or cancel search".to_owned(),
                "q or Ctrl+C            Exit and restore terminal".to_owned(),
                "All usage data is local; quota is observed account-meter evidence.".to_owned(),
            ],
        ),
        Overlay::ForkConfirm { thread_id } => (
            "Confirm session fork",
            vec![
                format!("Source thread: {}", clean_metadata(thread_id, 120)),
                "This clones the existing conversation history into a new thread.".to_owned(),
                "No prompt will be sent and no turn will be started.".to_owned(),
                "Press y or Enter to fork | n or Esc to cancel".to_owned(),
            ],
        ),
        Overlay::SyncConfirm => (
            "Confirm local telemetry sync",
            vec![
                "This invokes the existing one-shot P031 local ingestion service.".to_owned(),
                "It reads supported local rollout metadata and writes the evidence ledger."
                    .to_owned(),
                "No network upload is performed. Coverage warnings will be reported.".to_owned(),
                "Press y or Enter to start | n or Esc to cancel".to_owned(),
            ],
        ),
        Overlay::SessionDetail => ("Session details", session_detail_lines(state)),
        Overlay::ModelDetail => ("Model details", model_detail_lines(state)),
        Overlay::ProjectDetail => ("Project details", project_detail_lines(state)),
        Overlay::EvidenceDetail => ("Evidence details", evidence_detail_lines(state)),
    };
    let width = area.width.saturating_sub(6).clamp(14, 84);
    let height = area
        .height
        .saturating_sub(4)
        .min((lines.len() as u16).saturating_add(4))
        .max(5);
    let modal = centered(area, width, height);
    frame.render_widget(Clear, modal);
    let text = Paragraph::new(lines.join("\n"))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .title(title)
                .border_set(theme.border_set())
                .borders(Borders::ALL)
                .border_style(theme.title()),
        );
    frame.render_widget(text, modal);
}

fn session_detail_lines(state: &AppState) -> Vec<String> {
    let Some(detail) = state.session_detail.value.as_ref() else {
        return vec![
            state
                .session_detail
                .error
                .clone()
                .unwrap_or_else(|| "Loading safe session metadata...".to_owned()),
            "Session token attribution: Unavailable".to_owned(),
        ];
    };
    let session = &detail.session;
    vec![
        format!("Thread ID: {}", clean_metadata(&session.thread_id, 128)),
        format!(
            "Session-tree ID: {}",
            session
                .session_id
                .as_deref()
                .map(|v| clean_metadata(v, 128))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        format!(
            "Runtime status: {}",
            clean(session.runtime_status.as_str(), 64)
        ),
        format!("Archive state: {}", option_bool(session.archived)),
        format!(
            "Source: {}",
            session
                .source
                .as_ref()
                .map_or("Unavailable".to_owned(), |v| clean(v.as_str(), 64))
        ),
        format!(
            "Model: {}",
            session
                .model
                .as_deref()
                .map(|v| clean_metadata(v, 128))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        format!(
            "Reasoning evidence: {}",
            session
                .reasoning_effort
                .as_deref()
                .map(|v| clean_metadata(v, 64))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        format!(
            "Project association: {}",
            association_label(detail.project.kind)
        ),
        format!(
            "Codex project ID: {}",
            detail
                .project
                .codex_project_id
                .as_deref()
                .map(|v| clean_metadata(v, 128))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        format!(
            "Parent/fork: {}",
            session
                .parent_thread_id
                .as_deref()
                .or(session.forked_from_id.as_deref())
                .map(|v| clean_metadata(v, 128))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        format!(
            "History metadata: {}",
            history_availability(session.history_availability)
        ),
        "Session token attribution: Unavailable".to_owned(),
        "No transcript content or filesystem path is loaded.".to_owned(),
    ]
}

fn model_detail_lines(state: &AppState) -> Vec<String> {
    let Some(model) = state.selected_model_info() else {
        return vec!["No model is selected.".to_owned()];
    };
    let efforts = model
        .supported_reasoning_efforts
        .iter()
        .map(|value| clean_metadata(&value.id, 64))
        .collect::<Vec<_>>();
    vec![
        format!("Model ID: {}", clean_metadata(&model.id, 160)),
        format!("Display name: {}", clean_metadata(&model.display_name, 160)),
        format!("Default: {}", if model.is_default { "yes" } else { "no" }),
        format!(
            "Visibility: {}",
            if model.hidden { "hidden" } else { "visible" }
        ),
        format!(
            "Supported reasoning efforts: {}",
            if efforts.is_empty() {
                "Unavailable".to_owned()
            } else {
                efforts.join(", ")
            }
        ),
        format!(
            "Default reasoning effort: {}",
            clean_metadata(&model.default_reasoning_effort, 64)
        ),
        format!(
            "Input modalities: {}",
            model
                .input_modalities
                .as_ref()
                .map(|values| values
                    .iter()
                    .map(|value| clean_metadata(value, 40))
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        "Catalog metadata is reported by this local App Server.".to_owned(),
        "It does not guarantee current account entitlement or execution access.".to_owned(),
    ]
}

fn project_detail_lines(state: &AppState) -> Vec<String> {
    let Some(project) = state.selected_project_summary() else {
        return vec!["No project is selected.".to_owned()];
    };
    vec![
        format!(
            "Alias: {} (this response only)",
            clean(&project.project_alias, 64)
        ),
        format!(
            "Codex project ID: {}",
            project
                .codex_project_id
                .as_deref()
                .map(|v| clean_metadata(v, 128))
                .unwrap_or_else(|| "Unavailable".to_owned())
        ),
        format!("Association: {}", association_label(project.association)),
        format!("Sessions: {}", project.session_count),
        format!("Archived sessions: {}", project.archived_session_count),
        format!(
            "Known repositories: {}",
            project
                .repository_count
                .map_or_else(|| "Unavailable".to_owned(), |v| v.to_string())
        ),
        format!(
            "Known worktrees: {}",
            project
                .worktree_count
                .map_or_else(|| "Unavailable".to_owned(), |v| v.to_string())
        ),
        "Paths and Git remotes are hidden and are not stored in the usage ledger.".to_owned(),
    ]
}

fn evidence_detail_lines(state: &AppState) -> Vec<String> {
    let Some(detail) = state.evidence_detail.value.as_ref() else {
        return vec![state
            .evidence_detail
            .error
            .clone()
            .unwrap_or_else(|| "Loading read-only evidence details...".to_owned())];
    };
    match detail {
        EvidenceDetail::Token { evidence } => vec![
            format!(
                "Evidence: token event | {}",
                clean_metadata(&evidence.event_id, 128)
            ),
            format!("Observed: {}", clean(&evidence.observed_at, 80)),
            format!("Semantic: {:?}", evidence.record_semantic),
            format!(
                "Model: {}",
                evidence
                    .model
                    .as_deref()
                    .map(|v| clean_metadata(v, 128))
                    .unwrap_or_else(|| "Unavailable".to_owned())
            ),
            format!(
                "Reasoning: {}",
                evidence
                    .reasoning_level
                    .as_deref()
                    .map(|v| clean_metadata(v, 64))
                    .unwrap_or_else(|| "Unavailable".to_owned())
            ),
            format!(
                "Raw total: {}",
                evidence
                    .raw_total
                    .map_or_else(|| "Unavailable".to_owned(), |v| v.to_string())
            ),
            format!(
                "Input / cached / cache-write: {} / {} / {}",
                metric_field(evidence.input_tokens),
                metric_field(evidence.cached_input_tokens),
                metric_field(evidence.cache_write_input_tokens)
            ),
            format!(
                "Output / reasoning output: {} / {}",
                metric_field(evidence.output_tokens),
                metric_field(evidence.reasoning_output_tokens)
            ),
            "Only canonical per-response rows contribute to token totals.".to_owned(),
        ],
        EvidenceDetail::Quota { evidence } => vec![
            format!(
                "Evidence: quota sample | {}",
                clean_metadata(&evidence.sample_id, 128)
            ),
            format!("Observed: {}", clean(&evidence.observed_at, 80)),
            format!("Window: {:?}", evidence.meter_type),
            format!("Used: {}%", number(evidence.used_percent)),
            format!(
                "Reset: {}",
                evidence
                    .reset_at
                    .as_deref()
                    .map(|v| clean_metadata(v, 80))
                    .unwrap_or_else(|| "Unavailable".to_owned())
            ),
            "This is an account-meter snapshot, not task-level consumption.".to_owned(),
        ],
    }
}

fn metric_display(metric: &MetricTotal) -> String {
    match metric.coverage {
        MetricCoverage::NoObservations | MetricCoverage::Unavailable => "Unavailable".to_owned(),
        MetricCoverage::Complete => metric
            .known_sum
            .map_or_else(|| "Unavailable".to_owned(), |value| value.to_string()),
        MetricCoverage::Partial => match metric.known_sum {
            Some(value) => format!(
                "{value} known | partial | {} missing",
                metric.missing_event_count
            ),
            None => format!(
                "Unavailable | partial | {} missing",
                metric.missing_event_count
            ),
        },
    }
}

fn freshness(observed_at: &str) -> &'static str {
    let Ok(observed) = OffsetDateTime::parse(observed_at, &Rfc3339) else {
        return "Time unavailable";
    };
    let age = OffsetDateTime::now_utc() - observed;
    if age < time::Duration::minutes(-5) {
        "Time inconsistent"
    } else if age > time::Duration::hours(24) {
        "Stale"
    } else {
        "Fresh"
    }
}

fn number(value: f64) -> String {
    if value.is_finite() {
        value.to_string()
    } else {
        "invalid".to_owned()
    }
}

fn metric_field(value: Option<u64>) -> String {
    value.map_or_else(|| "Unavailable".to_owned(), |v| v.to_string())
}

fn option_bool(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "Unavailable",
    }
}

fn archive_filter_label(state: &AppState) -> &'static str {
    match state.session_archive_filter {
        crate::codex_app_server::ArchiveFilter::NonArchived => "active/non-archived filter",
        crate::codex_app_server::ArchiveFilter::Archived => "archived filter",
        crate::codex_app_server::ArchiveFilter::All => "all sessions",
    }
}

fn association_label(kind: ProjectAssociationKind) -> &'static str {
    match kind {
        ProjectAssociationKind::CodexProjectId => "Codex project ID",
        ProjectAssociationKind::LocalGitRepository => "Local Git repository",
        ProjectAssociationKind::WorkingDirectory => "Working directory",
        ProjectAssociationKind::Unavailable => "Unavailable / ambiguous",
    }
}

fn history_availability(value: HistoryAvailability) -> &'static str {
    match value {
        HistoryAvailability::MetadataAvailable => "metadata available",
        HistoryAvailability::Ephemeral => "ephemeral",
        HistoryAvailability::Unknown => "Unavailable",
    }
}

fn format_unix(timestamp: i64) -> String {
    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|value| value.format(&Rfc3339).ok())
        .unwrap_or_else(|| "Unavailable".to_owned())
}

fn clean(value: &str, max_chars: usize) -> String {
    clean_with_unicode(value, max_chars, Theme::detect().supports_unicode())
}

fn clean_metadata(value: &str, max_chars: usize) -> String {
    if looks_like_path(value) {
        "Unavailable".to_owned()
    } else {
        clean(value, max_chars)
    }
}

fn clean_with_unicode(value: &str, max_chars: usize, unicode: bool) -> String {
    let output = value
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if unicode || character.is_ascii() {
                character
            } else {
                '?'
            }
        })
        .take(max_chars)
        .collect::<String>();
    if output.is_empty() {
        "Unavailable".to_owned()
    } else {
        output
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width.min(area.width),
        height.min(area.height),
    )
}

#[cfg(test)]
mod tests {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use super::*;
    use crate::codex_app_server::{ModelCatalog, RuntimeStatus, SessionMetadata, SessionOrigin};
    use crate::storage::{
        ConfigurationBreakdown, EvidenceQualitySummary, MetricCoverage, MetricTotal,
        QuotaHistoryPage, QuotaTimeline, TokenHistoryPage, TokenStatistics,
    };

    #[test]
    fn renders_wide_medium_narrow_and_low_height_without_panicking() {
        for (width, height) in [(150, 42), (90, 26), (50, 18), (36, 7), (18, 3)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw(frame, &AppState::default(), Theme::detect()))
                .unwrap();
        }
    }

    #[test]
    fn narrow_header_keeps_the_product_and_active_view_readable() {
        let backend = TestBackend::new(18, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        state.active = Screen::History;
        terminal
            .draw(|frame| draw(frame, &state, Theme::detect()))
            .unwrap();
        let line = (0..18)
            .map(|x| terminal.backend().buffer()[(x, 0)].symbol())
            .collect::<Vec<_>>()
            .concat();
        assert!(line.contains("CM 5/5 History"));
    }

    #[test]
    fn standard_terminal_overview_keeps_quota_coverage_tokens_and_source_summary_visible() {
        let now = OffsetDateTime::now_utc();
        let mut quality = empty_quality();
        quality.total_diagnostics = 1;
        quality.known_partial_collection = 1;
        let metric = |value| MetricTotal {
            known_sum: Some(value),
            known_event_count: 1,
            missing_event_count: 0,
            coverage: MetricCoverage::Complete,
        };
        let statistics = TokenStatistics {
            canonical_response_count: 1,
            raw_total: metric(1000),
            input_tokens: metric(800),
            cached_input_tokens: metric(400),
            cache_write_input_tokens: metric(50),
            output_tokens: metric(200),
            reasoning_output_tokens: metric(25),
            distinct_session_count: 1,
            distinct_task_count: 1,
            configuration_breakdown: ConfigurationBreakdown {
                model: Vec::new(),
                model_truncated: false,
                reasoning_level: Vec::new(),
                reasoning_level_truncated: false,
                service_tier: Vec::new(),
                service_tier_truncated: false,
            },
            evidence_quality: quality.clone(),
        };
        let mut weekly = quota_sample(21.5, now);
        weekly.meter_type = crate::telemetry::QuotaMeterType::Weekly;
        let mut state = AppState::default();
        state.overview.value = Some(OverviewData {
            ledger_status: LedgerStatus::Available,
            token_statistics: Some(statistics),
            quota: Some(QuotaTimeline {
                five_hour: Some(quota_sample(42.125, now)),
                weekly: Some(weekly),
                history: None,
                evidence_quality: quality,
            }),
            evidence_quality: None,
            telemetry_version: Some(crate::telemetry::COMPATIBILITY_CLI_VERSION.to_owned()),
            schema_version: Some(9),
        });
        state.sessions.value = Some(Vec::new());

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| draw(frame, &state, Theme::detect()))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .concat();
        assert!(text.contains("Partial local evidence coverage"));
        assert!(text.contains("42.125% used"));
        assert!(text.contains("Raw total"));
        assert!(text.contains("Responses 1"));
        assert!(text.contains("SQLite: readable"));
        assert!(text.contains("Sessions 0"));
    }

    #[test]
    fn empty_views_and_overlays_render_without_invalid_buffer_writes() {
        let backend = TestBackend::new(52, 18);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = AppState::default();
        for (screen, overlay) in [
            (Screen::Sessions, None),
            (Screen::Projects, None),
            (Screen::Models, None),
            (Screen::History, None),
            (Screen::Overview, Some(Overlay::Help)),
            (
                Screen::Sessions,
                Some(Overlay::ForkConfirm {
                    thread_id: "thread-1".to_owned(),
                }),
            ),
            (Screen::Overview, Some(Overlay::SyncConfirm)),
        ] {
            state.active = screen;
            state.overlay = overlay;
            state.sessions.value = Some(Vec::new());
            state.projects.value = Some(Vec::new());
            state.models.value = Some(ModelCatalog {
                availability: "live",
                models: Vec::new(),
            });
            state.history.value = Some(TokenHistoryPage {
                events: Vec::new(),
                next_cursor: None,
                has_more: false,
                evidence_quality: empty_quality(),
            });
            state.quota_history.value = Some(QuotaHistoryPage {
                samples: Vec::new(),
                next_cursor: None,
                has_more: false,
            });
            terminal
                .draw(|frame| draw(frame, &state, Theme::detect()))
                .unwrap();
        }
    }

    #[test]
    fn quota_rendering_keeps_stale_invalid_and_unavailable_distinct() {
        let now = OffsetDateTime::now_utc();
        let mut stale = quota_sample(42.125, now - time::Duration::hours(25));
        stale.reset_at = None;
        let stale_lines = quota_sample_lines("5-hour", Some(&stale), 20);
        assert!(stale_lines[0].to_string().contains("5-hour"));
        assert!(stale_lines[0].to_string().contains("Stale"));
        assert!(stale_lines[0].to_string().contains("42.125"));
        assert!(stale_lines[1].to_string().contains("Unavailable"));
        let invalid = quota_sample(120.0, now);
        let invalid_lines = quota_sample_lines("Weekly", Some(&invalid), 20);
        assert!(invalid_lines[0].to_string().contains("Invalid measurement"));
        assert!(!invalid_lines[0].to_string().contains("["));
        assert!(quota_sample_lines("Weekly", None, 20)[0]
            .to_string()
            .contains("Unavailable"));
    }

    #[test]
    fn partial_token_totals_are_labeled_and_long_unicode_is_safe() {
        let partial = MetricTotal {
            known_sum: Some(1234),
            known_event_count: 1,
            missing_event_count: 2,
            coverage: MetricCoverage::Partial,
        };
        assert_eq!(metric_display(&partial), "1234 known | partial | 2 missing");
        assert_eq!(metric_display(&MetricTotal::default()), "Unavailable");
        let long = format!("{}-{}", "東京🙂".repeat(80), "thread-".repeat(50));
        assert_ne!(clean(&long, 70), "Unavailable");
        let ascii = clean_with_unicode(&long, 70, false);
        assert!(ascii.is_ascii());
        assert!(ascii.contains('?'));
        assert_eq!(
            clean_metadata("/private/path/to/session", 80),
            "Unavailable"
        );
        assert_eq!(clean_metadata("repo/src/file.rs", 80), "Unavailable");
        assert_eq!(clean_metadata("thread-opaque-id", 80), "thread-opaque-id");
        let mut state = AppState::default();
        state.active = Screen::Sessions;
        state.sessions.value = Some(vec![SessionMetadata {
            thread_id: long.clone(),
            session_id: None,
            name: Some("/private/path/private-session-title".to_owned()),
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
            history_availability: crate::codex_app_server::HistoryAvailability::Unknown,
            origin: SessionOrigin::LocalCodex,
            usage_link: crate::codex_app_server::UsageLinkAvailability::Unavailable,
        }]);
        let backend = TestBackend::new(36, 9);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw(frame, &state, Theme::detect()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .concat();
        assert!(!rendered.contains("/private/path/"));
    }

    #[test]
    fn unavailable_and_stale_quota_samples_are_not_rendered_as_zero() {
        let missing = quota_sample_lines("Weekly", None, 20);
        assert!(missing[0].to_string().contains("Unavailable"));
        assert!(!missing[0].to_string().contains("0%"));

        let sample = QuotaSnapshotEvidence {
            sample_id: "stale-sample".into(),
            source_id: "source".into(),
            session_id: None,
            observed_at: "2000-01-01T00:00:00Z".into(),
            meter_type: crate::telemetry::QuotaMeterType::Weekly,
            used_percent: 12.5,
            reset_at: None,
            plan: None,
            codex_version: None,
        };
        let stale = quota_sample_lines("Weekly", Some(&sample), 20);
        assert!(stale[0].to_string().contains("Stale"));
        assert!(stale[0].to_string().contains("12.5% used"));
        assert!(stale[0].to_string().contains("87.5% remaining"));
    }

    #[test]
    fn ascii_quota_bars_cover_zero_full_and_fractional_values() {
        let now = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let sample = |used_percent| QuotaSnapshotEvidence {
            sample_id: "sample".into(),
            source_id: "source".into(),
            session_id: None,
            observed_at: now.clone(),
            meter_type: crate::telemetry::QuotaMeterType::FiveHour,
            used_percent,
            reset_at: None,
            plan: None,
            codex_version: None,
        };
        for value in [0.0, 0.25, 100.0] {
            let lines = quota_sample_lines("5-hour", Some(&sample(value)), 20);
            assert_eq!(lines.len(), 2);
            assert!(lines[0].to_string().contains("% used"));
            assert!(lines[1].to_string().contains("Reset: Unavailable"));
        }
        assert!(quota_sample_lines("5-hour", None, 20)[0]
            .to_string()
            .contains("Unavailable"));
    }

    #[test]
    fn layout_collapses_navigation_for_medium_and_low_terminals() {
        assert!(responsive_layout(Rect::new(0, 0, 120, 30))
            .sidebar
            .is_some());
        assert!(responsive_layout(Rect::new(0, 0, 80, 24)).tabs.is_some());
        assert!(responsive_layout(Rect::new(0, 0, 36, 7)).tabs.is_some());
        assert!(responsive_layout(Rect::new(0, 0, 36, 5)).tabs.is_none());
    }

    fn quota_sample(used_percent: f64, observed: OffsetDateTime) -> QuotaSnapshotEvidence {
        QuotaSnapshotEvidence {
            sample_id: "sample-1".into(),
            source_id: "source-1".into(),
            session_id: None,
            observed_at: observed.format(&Rfc3339).unwrap(),
            meter_type: crate::telemetry::QuotaMeterType::FiveHour,
            used_percent,
            reset_at: None,
            plan: None,
            codex_version: None,
        }
    }

    fn empty_quality() -> EvidenceQualitySummary {
        EvidenceQualitySummary {
            total_diagnostics: 0,
            unsupported_source_versions: 0,
            compressed_sources_unsupported: 0,
            malformed_or_incomplete_evidence: 0,
            source_continuity_ambiguities: 0,
            known_partial_collection: 0,
            diagnostics_by_code: Vec::new(),
        }
    }
}
