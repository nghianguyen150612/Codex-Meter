//! Native history reporting and privacy-safe presentation.

use std::path::PathBuf;

use serde::Serialize;
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};

use crate::storage::{
    EvidenceDetail, EvidenceQualitySummary, MetricCoverage, MetricTotal, QuotaHistoryCursor,
    QuotaHistoryPage, QuotaSnapshotEvidence, QuotaTimeline, SqliteReadStore, StorageError,
    TokenHistoryCursor, TokenHistoryPage, TokenHistoryQuery, TokenRecordSemantic, TokenStatistics,
    TokenStatisticsQuery,
};
use crate::telemetry::QuotaMeterType;

use super::args::{
    CliError, HistoryArguments, InspectArguments, QuotaArguments, TokenScope, TokensArguments,
};

const REPORT_SCHEMA_VERSION: &str = "1.0.0";
const SOURCE_LABEL: &str = "local_codex_rollout_telemetry";
const FRESHNESS_POLICY_HOURS: i64 = 24;

#[derive(Serialize)]
struct TokenReportJson {
    schema_version: &'static str,
    generated_at: String,
    source: &'static str,
    period: PeriodJson,
    model_filter: Option<String>,
    observation_status: &'static str,
    usage: TokenStatistics,
}

#[derive(Serialize)]
struct PeriodJson {
    name: &'static str,
    since: Option<String>,
    until: Option<String>,
}

#[derive(Serialize)]
struct HistoryReportJson {
    schema_version: &'static str,
    generated_at: String,
    source: &'static str,
    since: Option<String>,
    until: Option<String>,
    model_filter: Option<String>,
    session_filter: Option<String>,
    page_size: u32,
    observation_status: &'static str,
    events: Vec<crate::storage::TokenUsageEvidence>,
    has_more: bool,
    next_cursor: Option<TokenHistoryCursor>,
    evidence_quality: EvidenceQualitySummary,
}

#[derive(Serialize)]
struct InspectReportJson {
    schema_version: &'static str,
    source: &'static str,
    evidence_integrity: &'static str,
    source_compatibility: String,
    evidence: EvidenceDetail,
}

#[derive(Serialize)]
struct QuotaReportJson {
    schema_version: &'static str,
    generated_at: String,
    source: &'static str,
    freshness_policy_hours: i64,
    observation_status: &'static str,
    five_hour: Option<QuotaSampleJson>,
    weekly: Option<QuotaSampleJson>,
    history: Option<QuotaHistoryPage>,
    evidence_quality: EvidenceQualitySummary,
}

#[derive(Serialize)]
struct QuotaSampleJson {
    evidence: QuotaSnapshotEvidence,
    remaining_percent_derived: f64,
    freshness: &'static str,
}

pub(super) fn tokens(arguments: TokensArguments) -> Result<String, CliError> {
    let generated_at = now_utc();
    let (since, until, period_name) = token_period(arguments.scope, &generated_at)?;
    let mut store = open_reporting_store(arguments.database)?;
    let usage = store
        .token_statistics(&TokenStatisticsQuery {
            since: since.clone(),
            until: until.clone(),
            model: arguments.model.clone(),
        })
        .map_err(storage_error)?;
    if arguments.json {
        return encode_json(&TokenReportJson {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at,
            source: SOURCE_LABEL,
            period: PeriodJson {
                name: period_name,
                since,
                until,
            },
            model_filter: arguments.model,
            observation_status: if usage.canonical_response_count == 0 {
                "no_observations"
            } else {
                "observations_available"
            },
            usage,
        });
    }
    Ok(render_tokens(
        period_name,
        since.as_deref(),
        until.as_deref(),
        arguments.model.as_deref(),
        &usage,
    ))
}

pub(super) fn history(arguments: HistoryArguments) -> Result<String, CliError> {
    let generated_at = now_utc();
    let (since, until, period_name) = history_period(&arguments, &generated_at)?;
    let mut store = open_reporting_store(arguments.database)?;
    let cursor = if let Some(event_id) = arguments.after.as_deref() {
        let detail = store.inspect(event_id).map_err(storage_error)?;
        match detail {
            EvidenceDetail::Token { evidence }
                if evidence.record_semantic == TokenRecordSemantic::PerResponse =>
            {
                Some(TokenHistoryCursor {
                    observed_at: evidence.observed_at,
                    event_id: evidence.event_id,
                })
            }
            _ => {
                return Err(CliError {
                    code: "invalid_cursor",
                    message: "--after must identify a per-response event in this history".into(),
                });
            }
        }
    } else {
        None
    };
    let page = store
        .token_history(&TokenHistoryQuery {
            since: since.clone(),
            until: until.clone(),
            model: arguments.model.clone(),
            session_id: arguments.session_id.clone(),
            limit: arguments.limit,
            cursor,
        })
        .map_err(storage_error)?;
    if arguments.json {
        return encode_json(&HistoryReportJson {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at,
            source: SOURCE_LABEL,
            since,
            until,
            model_filter: arguments.model,
            session_filter: arguments.session_id,
            page_size: arguments.limit,
            observation_status: if page.events.is_empty() {
                "no_observations"
            } else {
                "observations_available"
            },
            events: page.events,
            has_more: page.has_more,
            next_cursor: page.next_cursor,
            evidence_quality: page.evidence_quality,
        });
    }
    Ok(render_history(period_name, &page))
}

pub(super) fn inspect(arguments: InspectArguments) -> Result<String, CliError> {
    let mut store = open_reporting_store(arguments.database)?;
    let detail = store
        .inspect(&arguments.evidence_id)
        .map_err(storage_error)?;
    let compatibility = match &detail {
        EvidenceDetail::Token { evidence } => {
            source_compatibility(evidence.codex_version.as_deref())
        }
        EvidenceDetail::Quota { evidence } => {
            source_compatibility(evidence.codex_version.as_deref())
        }
    };
    if arguments.json {
        return encode_json(&InspectReportJson {
            schema_version: REPORT_SCHEMA_VERSION,
            source: SOURCE_LABEL,
            evidence_integrity: "verified_checksum",
            source_compatibility: compatibility,
            evidence: detail,
        });
    }
    Ok(render_inspect(&detail, &compatibility))
}

pub(super) fn quota(arguments: QuotaArguments) -> Result<String, CliError> {
    let mut store = open_reporting_store(arguments.database)?;
    let cursor = if let Some(sample_id) = arguments.after.as_deref() {
        let detail = store.inspect(sample_id).map_err(storage_error)?;
        match detail {
            EvidenceDetail::Quota { evidence } => Some(QuotaHistoryCursor {
                observed_at: evidence.observed_at,
                sample_id: evidence.sample_id,
            }),
            EvidenceDetail::Token { .. } => {
                return Err(CliError {
                    code: "invalid_cursor",
                    message: "--after must identify a quota sample".into(),
                });
            }
        }
    } else {
        None
    };
    let timeline = store
        .quota_timeline(arguments.history, arguments.limit, cursor.as_ref())
        .map_err(storage_error)?;
    let generated_at = now_utc();
    if arguments.json {
        return encode_json(&QuotaReportJson {
            schema_version: REPORT_SCHEMA_VERSION,
            generated_at: generated_at.clone(),
            source: SOURCE_LABEL,
            freshness_policy_hours: FRESHNESS_POLICY_HOURS,
            observation_status: if timeline.five_hour.is_none() && timeline.weekly.is_none() {
                "no_supported_samples"
            } else {
                "samples_available"
            },
            five_hour: timeline
                .five_hour
                .as_ref()
                .map(|evidence| quota_sample_json(evidence, &generated_at)),
            weekly: timeline
                .weekly
                .as_ref()
                .map(|evidence| quota_sample_json(evidence, &generated_at)),
            history: timeline.history,
            evidence_quality: timeline.evidence_quality,
        });
    }
    Ok(render_quota(&timeline, &generated_at))
}

fn open_reporting_store(database: Option<PathBuf>) -> Result<SqliteReadStore, CliError> {
    let path = match database {
        Some(path) => path,
        None => super::sync::default_database_path().map_err(|_| CliError {
            code: "database_location_unavailable",
            message: "could not determine the default Codex Meter database location".into(),
        })?,
    };
    let metadata = std::fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            no_history_error()
        } else {
            CliError {
                code: "database_open_failed",
                message: "the Codex Meter database could not be opened read-only".into(),
            }
        }
    })?;
    if !metadata.is_file() {
        return Err(no_history_error());
    }
    SqliteReadStore::open(&path).map_err(storage_error)
}

fn no_history_error() -> CliError {
    CliError {
        code: "database_missing",
        message: "No Codex Meter history found. Run `codex-meter sync` to collect supported local Codex usage.".into(),
    }
}

fn storage_error(error: StorageError) -> CliError {
    let code = match &error {
        StorageError::ReportingEvidenceNotFound => "unknown_evidence_id",
        StorageError::ReportingEvidenceAmbiguous => "ambiguous_evidence_id",
        StorageError::ReportingQueryInvalid => "invalid_query",
        StorageError::ReportingTotalOverflow => "token_total_overflow",
        _ => error.code(),
    };
    CliError {
        code,
        message: match &error {
            StorageError::ReportingEvidenceNotFound => "no supported evidence has that ID".into(),
            StorageError::ReportingEvidenceAmbiguous => {
                "the ID matches more than one evidence type".into()
            }
            StorageError::ReportingQueryInvalid => "the reporting query is invalid".into(),
            StorageError::ReportingTotalOverflow => {
                "a token total exceeds the supported integer range".into()
            }
            StorageError::EvidenceCorrupt => "persisted evidence failed its integrity check".into(),
            other => other.to_string(),
        },
    }
}

fn token_period(
    scope: TokenScope,
    generated_at: &str,
) -> Result<(Option<String>, Option<String>, &'static str), CliError> {
    let now = parse_generated_at(generated_at)?;
    let until = Some(format_timestamp(now)?);
    match scope {
        TokenScope::Today => Ok((
            Some(format_timestamp(start_of_utc_day(now))?),
            until,
            "today",
        )),
        TokenScope::Week => Ok((
            Some(format_timestamp(now - Duration::days(7))?),
            until,
            "trailing_7_days",
        )),
        TokenScope::All => Ok((None, None, "all")),
    }
}

fn history_period(
    arguments: &HistoryArguments,
    generated_at: &str,
) -> Result<(Option<String>, Option<String>, &'static str), CliError> {
    let now = parse_generated_at(generated_at)?;
    if arguments.today {
        return Ok((
            Some(format_timestamp(start_of_utc_day(now))?),
            Some(format_timestamp(now)?),
            "today",
        ));
    }
    if arguments.week {
        return Ok((
            Some(format_timestamp(now - Duration::days(7))?),
            Some(format_timestamp(now)?),
            "trailing_7_days",
        ));
    }
    let name = match (arguments.since.is_some(), arguments.until.is_some()) {
        (true, true) => "explicit_range",
        (true, false) => "since_time",
        (false, true) => "until_time",
        (false, false) => "all_recent",
    };
    Ok((arguments.since.clone(), arguments.until.clone(), name))
}

fn parse_generated_at(value: &str) -> Result<OffsetDateTime, CliError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| CliError {
        code: "invalid_timestamp",
        message: "could not interpret the UTC reporting timestamp".into(),
    })
}

fn start_of_utc_day(value: OffsetDateTime) -> OffsetDateTime {
    value.date().midnight().assume_utc()
}

fn now_utc() -> String {
    format_timestamp(OffsetDateTime::now_utc()).unwrap_or_else(|_| "unavailable".to_owned())
}

fn format_timestamp(value: OffsetDateTime) -> Result<String, CliError> {
    value
        .to_offset(time::UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| CliError {
            code: "timestamp_format_failed",
            message: "could not format a UTC reporting timestamp".into(),
        })
}

fn encode_json<T: Serialize>(value: &T) -> Result<String, CliError> {
    serde_json::to_string_pretty(value)
        .map(|mut json| {
            json.push('\n');
            json
        })
        .map_err(|_| CliError {
            code: "report_encoding_failed",
            message: "could not encode the privacy-safe reporting result".into(),
        })
}

fn render_tokens(
    period_name: &str,
    since: Option<&str>,
    until: Option<&str>,
    model: Option<&str>,
    usage: &TokenStatistics,
) -> String {
    let period_label = match period_name {
        "today" => "Today (UTC)",
        "trailing_7_days" => "Trailing 7 days (UTC)",
        "all" => "All recorded history",
        _ => period_name,
    };
    let mut output = String::from("Codex Meter — Token Usage\n");
    output.push_str(&format!("Period: {period_label}\n"));
    if since.is_some() || until.is_some() {
        output.push_str(&format!(
            "Range:  {} to {}\n",
            since.unwrap_or("beginning of recorded history"),
            until.unwrap_or("no upper bound")
        ));
    }
    if let Some(model) = model {
        output.push_str(&format!("Model:  {}\n", escape_terminal(model)));
    }
    if usage.canonical_response_count == 0 {
        output.push_str("\nNo observations yet for this period.\n");
    } else {
        output.push('\n');
        render_metric(
            &mut output,
            "Raw tokens recorded",
            &usage.raw_total,
            usage.canonical_response_count,
        );
        render_metric(
            &mut output,
            "Input tokens",
            &usage.input_tokens,
            usage.canonical_response_count,
        );
        render_metric(
            &mut output,
            "Cached input tokens",
            &usage.cached_input_tokens,
            usage.canonical_response_count,
        );
        render_metric(
            &mut output,
            "Cache-write input tokens",
            &usage.cache_write_input_tokens,
            usage.canonical_response_count,
        );
        render_metric(
            &mut output,
            "Output tokens",
            &usage.output_tokens,
            usage.canonical_response_count,
        );
        render_metric(
            &mut output,
            "Reasoning output tokens",
            &usage.reasoning_output_tokens,
            usage.canonical_response_count,
        );
        output.push('\n');
        output.push_str(&format!(
            "Canonical responses          {}\n",
            number(usage.canonical_response_count)
        ));
        output.push_str(&format!(
            "Distinct observed sessions    {}\n",
            number(usage.distinct_session_count)
        ));
        output.push_str(&format!(
            "Distinct observed tasks       {}\n",
            number(usage.distinct_task_count)
        ));
        output.push_str(&format!(
            "Events missing raw totals    {}\n",
            number(usage.raw_total.missing_event_count)
        ));
        output.push_str("\nConfiguration counts (Unavailable is kept separate)\n");
        render_breakdown(
            &mut output,
            "Model",
            &usage.configuration_breakdown.model,
            usage.configuration_breakdown.model_truncated,
        );
        render_breakdown(
            &mut output,
            "Reasoning",
            &usage.configuration_breakdown.reasoning_level,
            usage.configuration_breakdown.reasoning_level_truncated,
        );
        render_breakdown(
            &mut output,
            "Service tier",
            &usage.configuration_breakdown.service_tier,
            usage.configuration_breakdown.service_tier_truncated,
        );
    }
    output.push_str("\nSemantics: source input may include cached input; categories can overlap. Reasoning output is reported separately and is not added to output. Raw totals are source-reported and are not reconstructed from components.\n");
    render_quality(&mut output, &usage.evidence_quality);
    output.push_str("Coverage: supported local Codex rollout evidence only; this is not complete ChatGPT account usage.\n");
    output
}

fn render_metric(output: &mut String, label: &str, metric: &MetricTotal, response_count: u64) {
    let value = metric
        .known_sum
        .map(number)
        .unwrap_or_else(|| match metric.coverage {
            MetricCoverage::NoObservations => "No observations".into(),
            _ => "Unavailable".into(),
        });
    let qualifier = match metric.coverage {
        MetricCoverage::Partial => format!(
            "partial; known for {} of {} responses",
            number(metric.known_event_count),
            number(response_count)
        ),
        MetricCoverage::Unavailable => {
            format!("unavailable for {} responses", number(response_count))
        }
        MetricCoverage::NoObservations | MetricCoverage::Complete => String::new(),
    };
    output.push_str(&format!("{label:<29}{value:>16}"));
    if !qualifier.is_empty() {
        output.push_str(&format!("  ({qualifier})"));
    }
    output.push('\n');
}

fn render_breakdown(
    output: &mut String,
    label: &str,
    counts: &[crate::storage::ValueCount],
    truncated: bool,
) {
    if counts.is_empty() {
        output.push_str(&format!("  {label:<14}Unavailable\n"));
        return;
    }
    output.push_str(&format!("  {label}\n"));
    for count in counts {
        let value = count
            .value
            .as_deref()
            .map(|value| short_cell(value, 40))
            .unwrap_or_else(|| "Unavailable".to_owned());
        output.push_str(&format!(
            "    {value:<40} {} responses\n",
            number(count.event_count)
        ));
    }
    if truncated {
        output.push_str("  Additional configuration groups omitted after the first 1,000.\n");
    }
}

fn render_history(period_name: &str, page: &TokenHistoryPage) -> String {
    let mut output = format!(
        "Codex Meter — Token Usage History\nPeriod: {}\n",
        history_period_label(period_name)
    );
    if page.events.is_empty() {
        output.push_str("\nNo observations yet.\n");
    } else {
        output.push_str("\nTime (UTC)              | Session   | Model            | Reasoning    | Raw total          | Evidence\n");
        output.push_str("------------------------|-----------|------------------|--------------|--------------------|------------------\n");
        for event in &page.events {
            let session = event
                .session_id
                .as_deref()
                .map(|value| short_id(value, 9))
                .unwrap_or_else(|| "Unavailable".into());
            let model = event
                .model
                .as_deref()
                .map(|value| short_cell(value, 16))
                .unwrap_or_else(|| "Unavailable".into());
            let reasoning = event
                .reasoning_level
                .as_deref()
                .map(|value| short_cell(value, 12))
                .unwrap_or_else(|| "Unavailable".into());
            let raw = event
                .raw_total
                .map(number)
                .unwrap_or_else(|| "Unavailable".into());
            let evidence = if event.raw_total.is_some() {
                "Observed"
            } else {
                "Raw total missing"
            };
            output.push_str(&format!(
                "{:<24} | {:<9} | {:<16} | {:<12} | {:>18} | {}\n",
                short_timestamp(&event.observed_at),
                session,
                model,
                reasoning,
                raw,
                evidence
            ));
        }
        output.push_str(&format!(
            "\nShowing {} canonical response events.\n",
            page.events.len()
        ));
        if let Some(cursor) = &page.next_cursor {
            output.push_str(&format!(
                "More events are available. Continue with --after {}.\n",
                cursor.event_id
            ));
        }
    }
    render_quality(&mut output, &page.evidence_quality);
    output.push_str("Coverage: supported local Codex rollout evidence only; concurrent clients and cloud usage may not appear here.\n");
    output
}

fn render_inspect(detail: &EvidenceDetail, compatibility: &str) -> String {
    let mut output =
        String::from("Codex Meter — Evidence Inspector\nIntegrity: checksum verified\n");
    match detail {
        EvidenceDetail::Token { evidence } => {
            output.push_str("Evidence type: Token usage\n");
            output.push_str(&format!("Event ID:       {}\n", evidence.event_id));
            output.push_str(&format!("Timestamp:      {}\n", evidence.observed_at));
            output.push_str(&format!(
                "Semantic:       {}\n",
                semantic_label(evidence.record_semantic)
            ));
            output.push_str(&format!(
                "Evidence state: {}\n",
                if evidence.record_semantic == TokenRecordSemantic::PerResponse {
                    "observed"
                } else {
                    "snapshot (not canonical consumption)"
                }
            ));
            output.push_str(&format!(
                "Session ID:     {}\n",
                evidence.session_id.as_deref().unwrap_or("Unavailable")
            ));
            output.push_str(&format!(
                "Task ID:        {}\n",
                evidence.task_id.as_deref().unwrap_or("Unavailable")
            ));
            output.push_str(&format!(
                "Model:          {}\n",
                value_or_unavailable(evidence.model.as_deref())
            ));
            output.push_str(&format!(
                "Reasoning:      {}\n",
                value_or_unavailable(evidence.reasoning_level.as_deref())
            ));
            output.push_str(&format!(
                "Service tier:   {}\n",
                value_or_unavailable(evidence.service_tier.as_deref())
            ));
            output.push_str(&format!(
                "Codex version:  {}\n",
                value_or_unavailable(evidence.codex_version.as_deref())
            ));
            output.push_str("\nDirect counters\n");
            for (label, value) in [
                ("Input tokens", evidence.input_tokens),
                ("Cached input tokens", evidence.cached_input_tokens),
                (
                    "Cache-write input tokens",
                    evidence.cache_write_input_tokens,
                ),
                ("Output tokens", evidence.output_tokens),
                ("Reasoning output tokens", evidence.reasoning_output_tokens),
                ("Raw total", evidence.raw_total),
            ] {
                output.push_str(&format!(
                    "  {label:<28}{}\n",
                    value.map(number).unwrap_or_else(|| "Unavailable".into())
                ));
            }
        }
        EvidenceDetail::Quota { evidence } => {
            output.push_str("Evidence type: Quota sample\n");
            output.push_str(&format!("Sample ID:      {}\n", evidence.sample_id));
            output.push_str(&format!(
                "Meter:          {}\n",
                quota_meter_label(evidence.meter_type)
            ));
            output.push_str(&format!(
                "Used:           {}% (observed)\n",
                percent(evidence.used_percent)
            ));
            output.push_str(&format!("Observed:       {}\n", evidence.observed_at));
            output.push_str(&format!(
                "Reset:          {}\n",
                evidence.reset_at.as_deref().unwrap_or("Unavailable")
            ));
            output.push_str(&format!(
                "Plan:           {}\n",
                value_or_unavailable(evidence.plan.as_deref())
            ));
            output.push_str(&format!(
                "Codex version:  {}\n",
                value_or_unavailable(evidence.codex_version.as_deref())
            ));
            output.push_str("Provenance:     Local Codex rollout account-meter evidence\n");
            output
                .push_str("Attribution:    Does not establish which task caused meter movement\n");
        }
    }
    output.push_str(&format!("\nSource compatibility: {compatibility}\n"));
    output
}

fn render_quota(timeline: &QuotaTimeline, generated_at: &str) -> String {
    let now = parse_generated_at(generated_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let mut output = String::from("Codex Meter — Quota Evidence\n\n");
    render_quota_meter(
        &mut output,
        "Five-hour meter",
        timeline.five_hour.as_ref(),
        now,
    );
    output.push('\n');
    render_quota_meter(&mut output, "Weekly meter", timeline.weekly.as_ref(), now);
    if let Some(history) = &timeline.history {
        output.push_str("\nQuota history\n");
        render_quota_history(&mut output, history);
    }
    output.push_str("\nSource: Local Codex rollout account-meter evidence. This is not live quota, official raw-token allowance, or task-specific attribution.\n");
    output.push_str(&format!("Freshness label policy: samples older than {FRESHNESS_POLICY_HOURS} hours are marked potentially stale; this is a presentation threshold, not a provider guarantee.\n"));
    render_quality(&mut output, &timeline.evidence_quality);
    output
}

fn render_quota_meter(
    output: &mut String,
    label: &str,
    sample: Option<&QuotaSnapshotEvidence>,
    now: OffsetDateTime,
) {
    output.push_str(&format!("{label}\n"));
    let Some(sample) = sample else {
        output.push_str("  Used:      Unavailable\n  Observed:  No supported sample\n");
        return;
    };
    output.push_str(&format!(
        "  {} {}%\n",
        percentage_bar(sample.used_percent),
        percent(sample.used_percent)
    ));
    output.push_str(&format!(
        "  Used:      {}% observed\n",
        percent(sample.used_percent)
    ));
    output.push_str(&format!(
        "  Remaining: {}% (derived)\n",
        percent(round_percent(100.0 - sample.used_percent))
    ));
    output.push_str(&format!(
        "  Observed:  {} ({})\n",
        sample.observed_at,
        freshness_label(sample, now)
    ));
    output.push_str(&format!(
        "  Reset:     {}\n",
        sample.reset_at.as_deref().unwrap_or("Unavailable")
    ));
}

fn render_quota_history(output: &mut String, page: &QuotaHistoryPage) {
    if page.samples.is_empty() {
        output.push_str("  No supported samples.\n");
        return;
    }
    output.push_str("  Time | Meter | Used | Sample ID\n");
    for sample in &page.samples {
        output.push_str(&format!(
            "  {} | {} | {}% | {}\n",
            short_timestamp(&sample.observed_at),
            quota_meter_label(sample.meter_type),
            percent(sample.used_percent),
            short_id(&sample.sample_id, 12)
        ));
    }
    if let Some(cursor) = &page.next_cursor {
        output.push_str(&format!(
            "  More samples are available. Continue with --after {}.\n",
            cursor.sample_id
        ));
    }
}

fn quota_sample_json(evidence: &QuotaSnapshotEvidence, generated_at: &str) -> QuotaSampleJson {
    let now = parse_generated_at(generated_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
    QuotaSampleJson {
        evidence: evidence.clone(),
        remaining_percent_derived: round_percent(100.0 - evidence.used_percent),
        freshness: freshness_label(evidence, now),
    }
}

fn freshness_label(sample: &QuotaSnapshotEvidence, now: OffsetDateTime) -> &'static str {
    let Ok(observed) = OffsetDateTime::parse(&sample.observed_at, &Rfc3339) else {
        return "unknown_freshness";
    };
    let age = now - observed;
    if age.is_negative() {
        "unknown_freshness_clock_skew"
    } else if age <= Duration::hours(FRESHNESS_POLICY_HOURS) {
        "recent_by_24_hour_policy"
    } else {
        "potentially_stale"
    }
}

fn render_quality(output: &mut String, quality: &EvidenceQualitySummary) {
    output.push_str(&format!(
        "Evidence quality: {} persisted ingestion diagnostics",
        number(quality.total_diagnostics)
    ));
    if quality.total_diagnostics == 0 {
        output.push_str("; no recorded ingestion diagnostics.\n");
        return;
    }
    output.push_str(&format!("; unsupported source versions {}; compressed sources unsupported {}; malformed/incomplete evidence {}; continuity ambiguity {}; known partial collection {}.\n",
        number(quality.unsupported_source_versions),
        number(quality.compressed_sources_unsupported),
        number(quality.malformed_or_incomplete_evidence),
        number(quality.source_continuity_ambiguities),
        number(quality.known_partial_collection)));
}

fn source_compatibility(version: Option<&str>) -> String {
    match version {
        Some("0.157.1") => "Codex CLI 0.157.1 is recorded; P031 parsing is verified only for that rollout format.".into(),
        Some(version) => format!("Codex CLI {version} is recorded; parsing compatibility is verified only for 0.157.1."),
        None => "Codex version is unavailable; parsing compatibility is verified only for Codex CLI 0.157.1.".into(),
    }
}

fn semantic_label(semantic: TokenRecordSemantic) -> &'static str {
    match semantic {
        TokenRecordSemantic::PerResponse => "per_response",
        TokenRecordSemantic::TurnCumulativeSnapshot => "turn_cumulative_snapshot",
        TokenRecordSemantic::ThreadCumulativeSnapshot => "thread_cumulative_snapshot",
        TokenRecordSemantic::TokenCountTotalSnapshot => "token_count_total_snapshot",
        TokenRecordSemantic::TokenCountLatestSnapshot => "token_count_latest_snapshot",
    }
}

fn quota_meter_label(meter: QuotaMeterType) -> &'static str {
    match meter {
        QuotaMeterType::FiveHour => "5-hour",
        QuotaMeterType::Weekly => "weekly",
    }
}

fn percentage_bar(used: f64) -> String {
    let filled = ((used / 100.0) * 20.0).round() as usize;
    format!(
        "[{}{}]",
        "#".repeat(filled.min(20)),
        ".".repeat(20_usize.saturating_sub(filled.min(20)))
    )
}

fn percent(value: f64) -> String {
    let value = round_percent(value);
    let mut rendered = format!("{value:.2}");
    while rendered.contains('.') && rendered.ends_with('0') {
        rendered.pop();
    }
    if rendered.ends_with('.') {
        rendered.pop();
    }
    rendered
}

fn round_percent(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn number(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(character);
    }
    result
}

fn short_id(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn short_timestamp(value: &str) -> String {
    let Some(seconds) = value.get(..19) else {
        return short_cell(value, 24);
    };
    let fraction = value
        .strip_prefix(seconds)
        .and_then(|rest| rest.strip_prefix('.'))
        .map(|rest| {
            rest.trim_end_matches('Z')
                .chars()
                .take(3)
                .collect::<String>()
        })
        .unwrap_or_default();
    if fraction.is_empty() {
        format!("{seconds}Z")
    } else {
        format!("{seconds}.{fraction}Z")
    }
}

fn short_cell(value: &str, max_chars: usize) -> String {
    let safe = escape_terminal(value);
    if safe.chars().count() <= max_chars {
        safe
    } else {
        format!(
            "{}…",
            safe.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn escape_terminal(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                format!("\\u{{{:x}}}", u32::from(character))
            } else {
                character.to_string()
            }
        })
        .collect()
}

fn value_or_unavailable(value: Option<&str>) -> String {
    value
        .map(escape_terminal)
        .unwrap_or_else(|| "Unavailable".into())
}

fn history_period_label(value: &str) -> &str {
    match value {
        "today" => "Today (UTC)",
        "trailing_7_days" => "Trailing 7 days (UTC)",
        "all_recent" => "All recorded history",
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::params;
    use sha2::{Digest, Sha256};

    use crate::storage::{QuotaSnapshotEvidence, SqliteStore, TokenUsageEvidence};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn database_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-cli-reporting-{}-{}.sqlite",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove_database(path: &std::path::Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    fn hash<T: Serialize>(value: &T) -> String {
        Sha256::digest(serde_json::to_vec(value).unwrap())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn seed(database: &std::path::Path) -> (String, String) {
        let now = OffsetDateTime::now_utc();
        let timestamp = format_timestamp(now).unwrap();
        let event = TokenUsageEvidence {
            event_id: "evt:opaque001".into(),
            source_id: "src:opaque001".into(),
            session_id: Some("sess:hashed001".into()),
            task_id: Some("task:hashed001".into()),
            observed_at: timestamp.clone(),
            record_semantic: TokenRecordSemantic::PerResponse,
            model: Some("gpt-5.6-sol".into()),
            reasoning_level: Some("high".into()),
            service_tier: Some("priority".into()),
            codex_version: Some("0.157.1".into()),
            input_tokens: Some(10),
            cached_input_tokens: Some(4),
            cache_write_input_tokens: None,
            output_tokens: Some(5),
            reasoning_output_tokens: Some(3),
            raw_total: Some(15),
        };
        let quota = QuotaSnapshotEvidence {
            sample_id: "quota:opaque001".into(),
            source_id: "src:opaque001".into(),
            session_id: Some("sess:hashed001".into()),
            observed_at: timestamp.clone(),
            meter_type: QuotaMeterType::FiveHour,
            used_percent: 42.0,
            reset_at: None,
            plan: Some("Plus".into()),
            codex_version: Some("0.157.1".into()),
        };
        let store = SqliteStore::open(database).unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO token_usage_evidence (event_id, source_id, session_id, task_id, observed_at, record_semantic, model, reasoning_level, service_tier, codex_version, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total, evidence_status, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, 'per_response', ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14, 'observed', 1, ?15)",
                params![
                    event.event_id,
                    event.source_id,
                    event.session_id,
                    event.task_id,
                    event.observed_at,
                    event.model,
                    event.reasoning_level,
                    event.service_tier,
                    event.codex_version,
                    event.input_tokens.map(|value| value as i64),
                    event.cached_input_tokens.map(|value| value as i64),
                    event.output_tokens.map(|value| value as i64),
                    event.reasoning_output_tokens.map(|value| value as i64),
                    event.raw_total.map(|value| value as i64),
                    hash(&event),
                ],
            )
            .unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO quota_snapshot_evidence (sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, 'five_hour', ?5, ?6, ?7, ?8, 'observed', 1, ?9)",
                params![
                    quota.sample_id,
                    quota.source_id,
                    quota.session_id,
                    quota.observed_at,
                    quota.used_percent,
                    quota.reset_at,
                    quota.plan,
                    quota.codex_version,
                    hash(&quota),
                ],
            )
            .unwrap();
        drop(store);
        (event.event_id, quota.sample_id)
    }

    fn command(args: &[&str], database: &std::path::Path) -> Result<String, CliError> {
        let mut values = args
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        values.splice(
            1..1,
            [
                "--database".to_owned(),
                database.to_string_lossy().into_owned(),
            ],
        );
        crate::cli::run(values)
    }

    #[test]
    fn json_commands_are_versioned_clean_and_include_exact_opaque_ids() {
        let database = database_path();
        let (event_id, sample_id) = seed(&database);

        let tokens = command(&["tokens", "--all", "--json"], &database).unwrap();
        assert!(tokens.starts_with('{'));
        assert!(!tokens.contains("Codex Meter —"));
        let tokens: serde_json::Value = serde_json::from_str(&tokens).unwrap();
        assert_eq!(tokens["schema_version"], REPORT_SCHEMA_VERSION);
        assert_eq!(tokens["usage"]["canonical_response_count"], 1);
        assert_eq!(tokens["usage"]["raw_total"]["known_sum"], 15);
        assert_eq!(tokens["usage"]["reasoning_output_tokens"]["known_sum"], 3);

        let history = command(&["history", "--json"], &database).unwrap();
        let history: serde_json::Value = serde_json::from_str(&history).unwrap();
        assert_eq!(history["events"][0]["event_id"], event_id);
        assert_eq!(history["events"][0]["session_id"], "sess:hashed001");

        let inspect = command(&["inspect", &event_id, "--json"], &database).unwrap();
        let inspect: serde_json::Value = serde_json::from_str(&inspect).unwrap();
        assert_eq!(inspect["evidence_integrity"], "verified_checksum");
        assert_eq!(inspect["evidence"]["evidence"]["event_id"], event_id);
        assert!(inspect.to_string().find("/workspace/").is_none());
        assert!(inspect.to_string().find("prompt").is_none());

        let quota = command(&["quota", "--json"], &database).unwrap();
        let quota: serde_json::Value = serde_json::from_str(&quota).unwrap();
        assert_eq!(quota["five_hour"]["evidence"]["sample_id"], sample_id);
        assert_eq!(quota["five_hour"]["evidence"]["used_percent"], 42.0);
        assert_eq!(quota["five_hour"]["remaining_percent_derived"], 58.0);
        remove_database(&database);
    }

    #[test]
    fn empty_reporting_outputs_say_no_observations_and_never_create_missing_databases() {
        let database = database_path();
        {
            let _store = SqliteStore::open(&database).unwrap();
        }
        let tokens = command(&["tokens", "--json"], &database).unwrap();
        let value: serde_json::Value = serde_json::from_str(&tokens).unwrap();
        assert_eq!(value["observation_status"], "no_observations");
        assert_eq!(
            value["usage"]["raw_total"]["known_sum"],
            serde_json::Value::Null
        );
        assert!(command(&["history"], &database)
            .unwrap()
            .contains("No observations yet"));
        assert!(command(&["quota"], &database)
            .unwrap()
            .contains("No supported sample"));
        remove_database(&database);

        let missing = database_path();
        remove_database(&missing);
        let error = command(&["tokens"], &missing).unwrap_err();
        assert_eq!(error.code, "database_missing");
        assert!(error.message.contains("Run `codex-meter sync`"));
        assert!(!missing.exists());
    }

    #[test]
    fn human_reports_are_plain_text_and_explicit_about_partial_counter_semantics() {
        let mut usage = TokenStatistics {
            canonical_response_count: 2,
            raw_total: MetricTotal {
                known_sum: Some(20),
                known_event_count: 1,
                missing_event_count: 1,
                coverage: MetricCoverage::Partial,
            },
            input_tokens: MetricTotal {
                known_sum: Some(10),
                known_event_count: 2,
                missing_event_count: 0,
                coverage: MetricCoverage::Complete,
            },
            cached_input_tokens: MetricTotal::default(),
            cache_write_input_tokens: MetricTotal::default(),
            output_tokens: MetricTotal::default(),
            reasoning_output_tokens: MetricTotal::default(),
            distinct_session_count: 1,
            distinct_task_count: 1,
            configuration_breakdown: crate::storage::ConfigurationBreakdown {
                model: vec![],
                model_truncated: false,
                reasoning_level: vec![],
                reasoning_level_truncated: false,
                service_tier: vec![],
                service_tier_truncated: false,
            },
            evidence_quality: EvidenceQualitySummary {
                total_diagnostics: 0,
                unsupported_source_versions: 0,
                compressed_sources_unsupported: 0,
                malformed_or_incomplete_evidence: 0,
                source_continuity_ambiguities: 0,
                known_partial_collection: 0,
                diagnostics_by_code: vec![],
            },
        };
        let output = render_tokens("today", None, None, None, &usage);
        assert!(output.contains("partial; known for 1 of 2 responses"));
        assert!(output.contains("source input may include cached input"));
        assert!(!output.contains("\u{1b}"));
        usage.canonical_response_count = 0;
        assert!(render_tokens("today", None, None, None, &usage).contains("No observations yet"));
    }

    #[test]
    fn quota_freshness_is_labeled_by_policy_and_future_samples_are_unknown() {
        let sample = |observed_at: &str| QuotaSnapshotEvidence {
            sample_id: "quota:opaque".into(),
            source_id: "src:opaque".into(),
            session_id: None,
            observed_at: observed_at.into(),
            meter_type: QuotaMeterType::FiveHour,
            used_percent: 25.0,
            reset_at: None,
            plan: None,
            codex_version: None,
        };
        let now = OffsetDateTime::parse("2026-10-20T12:00:00Z", &Rfc3339).unwrap();
        assert_eq!(
            freshness_label(&sample("2026-10-20T11:00:00Z"), now),
            "recent_by_24_hour_policy"
        );
        assert_eq!(
            freshness_label(&sample("2026-10-19T11:59:59Z"), now),
            "potentially_stale"
        );
        assert_eq!(
            freshness_label(&sample("2026-10-20T12:00:01Z"), now),
            "unknown_freshness_clock_skew"
        );
    }

    #[test]
    fn history_table_bounds_long_cells_and_keeps_full_ids_in_structured_output() {
        let event = TokenUsageEvidence {
            event_id: "evt:exact-opaque-id".into(),
            source_id: "src:opaque".into(),
            session_id: Some("sess:hashed-id-that-is-long".into()),
            task_id: None,
            observed_at: "2026-10-20T12:00:00.123456789Z".into(),
            record_semantic: TokenRecordSemantic::PerResponse,
            model: Some("m".repeat(256)),
            reasoning_level: Some("r".repeat(64)),
            service_tier: None,
            codex_version: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            output_tokens: None,
            reasoning_output_tokens: None,
            raw_total: None,
        };
        let page = TokenHistoryPage {
            events: vec![event.clone()],
            next_cursor: None,
            has_more: false,
            evidence_quality: EvidenceQualitySummary {
                total_diagnostics: 0,
                unsupported_source_versions: 0,
                compressed_sources_unsupported: 0,
                malformed_or_incomplete_evidence: 0,
                source_continuity_ambiguities: 0,
                known_partial_collection: 0,
                diagnostics_by_code: vec![],
            },
        };
        let output = render_history("all_recent", &page);
        let row = output
            .lines()
            .find(|line| line.starts_with("2026-10-20T12:00:00"))
            .unwrap();
        assert!(row.chars().count() <= 120);
        assert!(!row.contains(&"m".repeat(256)));
        let json = serde_json::to_string(&page).unwrap();
        assert!(json.contains("evt:exact-opaque-id"));
        assert!(json.contains(&"m".repeat(256)));
    }
}
