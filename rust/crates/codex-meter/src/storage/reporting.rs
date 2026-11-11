//! Typed read-only queries over P031's checksummed evidence ledger.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, Row};
use serde::Serialize;

use crate::telemetry::QuotaMeterType;

use super::sqlite::StorageError;
use super::usage_evidence::{
    canonical_hash, IngestionDiagnostic, IngestionDiagnosticCode, QuotaSnapshotEvidence,
    TokenRecordSemantic, TokenUsageEvidence,
};

const MAX_REPORT_LIMIT: u32 = 1_000;
const SORTED_TIME_EXPR: &str = "substr(observed_at, 1, 19) || '.' || CASE WHEN substr(observed_at, 20, 1) = '.' THEN replace(printf('%-107s', substr(observed_at, 21, length(observed_at) - 21)), ' ', '0') ELSE replace(printf('%-107s', ''), ' ', '0') END";
const TOKEN_COLUMNS: &str = "event_id, source_id, session_id, task_id, observed_at, record_semantic, model, reasoning_level, service_tier, codex_version, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total, evidence_status, schema_version, evidence_sha256";
const QUOTA_COLUMNS: &str = "sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256";
const DIAGNOSTIC_COLUMNS: &str = "diagnostic_id, source_id, observed_at, record_semantic, diagnostic_code, schema_version, evidence_sha256";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenStatisticsQuery {
    pub since: Option<String>,
    pub until: Option<String>,
    pub model: Option<String>,
}

impl TokenStatisticsQuery {
    fn validate(&self) -> Result<(), StorageError> {
        validate_time_range(self.since.as_deref(), self.until.as_deref())?;
        validate_optional_filter(self.model.as_deref())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TokenHistoryCursor {
    pub observed_at: String,
    pub event_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenHistoryQuery {
    pub since: Option<String>,
    pub until: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub limit: u32,
    pub cursor: Option<TokenHistoryCursor>,
}

impl Default for TokenHistoryQuery {
    fn default() -> Self {
        Self {
            since: None,
            until: None,
            model: None,
            session_id: None,
            limit: 20,
            cursor: None,
        }
    }
}

impl TokenHistoryQuery {
    fn validate(&self) -> Result<(), StorageError> {
        validate_time_range(self.since.as_deref(), self.until.as_deref())?;
        validate_optional_filter(self.model.as_deref())?;
        validate_optional_filter(self.session_id.as_deref())?;
        validate_limit(self.limit)?;
        if let Some(cursor) = &self.cursor {
            validate_time_range(Some(cursor.observed_at.as_str()), None)?;
            validate_identifier(&cursor.event_id)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricCoverage {
    NoObservations,
    Complete,
    Partial,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MetricTotal {
    pub known_sum: Option<u64>,
    pub known_event_count: u64,
    pub missing_event_count: u64,
    pub coverage: MetricCoverage,
}

impl Default for MetricTotal {
    fn default() -> Self {
        Self {
            known_sum: None,
            known_event_count: 0,
            missing_event_count: 0,
            coverage: MetricCoverage::NoObservations,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConfigurationBreakdown {
    pub model: Vec<ValueCount>,
    pub model_truncated: bool,
    pub reasoning_level: Vec<ValueCount>,
    pub reasoning_level_truncated: bool,
    pub service_tier: Vec<ValueCount>,
    pub service_tier_truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticCount {
    pub code: String,
    pub diagnostic_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValueCount {
    pub value: Option<String>,
    pub event_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceQualitySummary {
    pub total_diagnostics: u64,
    pub unsupported_source_versions: u64,
    pub compressed_sources_unsupported: u64,
    pub malformed_or_incomplete_evidence: u64,
    pub source_continuity_ambiguities: u64,
    pub known_partial_collection: u64,
    pub diagnostics_by_code: Vec<DiagnosticCount>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TokenStatistics {
    pub canonical_response_count: u64,
    pub raw_total: MetricTotal,
    pub input_tokens: MetricTotal,
    pub cached_input_tokens: MetricTotal,
    pub cache_write_input_tokens: MetricTotal,
    pub output_tokens: MetricTotal,
    pub reasoning_output_tokens: MetricTotal,
    pub distinct_session_count: u64,
    pub distinct_task_count: u64,
    pub configuration_breakdown: ConfigurationBreakdown,
    pub evidence_quality: EvidenceQualitySummary,
}

/// A task identity is scoped by all three persisted opaque IDs. Task IDs alone
/// are not globally unique and are never joined to App Server thread IDs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskUsageQuery {
    pub since: Option<String>,
    pub until: Option<String>,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskModelConfiguration {
    pub model: Option<String>,
    pub reasoning_level: Option<String>,
    pub service_tier: Option<String>,
    pub codex_version: Option<String>,
    pub response_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskUsageSummary {
    pub source_id: String,
    pub session_id: String,
    pub task_id: String,
    pub first_observed_at: String,
    pub last_observed_at: String,
    pub canonical_response_count: u64,
    pub raw_total: MetricTotal,
    pub input_tokens: MetricTotal,
    pub cached_input_tokens: MetricTotal,
    pub cache_write_input_tokens: MetricTotal,
    pub output_tokens: MetricTotal,
    pub reasoning_output_tokens: MetricTotal,
    pub configurations: Vec<TaskModelConfiguration>,
    pub configurations_truncated: bool,
    pub evidence_status: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskUsageReport {
    pub task_count: u64,
    pub canonical_response_count: u64,
    pub tasks: Vec<TaskUsageSummary>,
    pub has_more: bool,
    pub attributed: TaskTokenTotals,
    pub unattributed: TaskTokenTotals,
    pub evidence_quality: EvidenceQualitySummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskTokenTotals {
    pub canonical_response_count: u64,
    pub raw_total: MetricTotal,
    pub input_tokens: MetricTotal,
    pub cached_input_tokens: MetricTotal,
    pub cache_write_input_tokens: MetricTotal,
    pub output_tokens: MetricTotal,
    pub reasoning_output_tokens: MetricTotal,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TokenHistoryPage {
    pub events: Vec<TokenUsageEvidence>,
    pub next_cursor: Option<TokenHistoryCursor>,
    pub has_more: bool,
    pub evidence_quality: EvidenceQualitySummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QuotaHistoryCursor {
    pub observed_at: String,
    pub sample_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotaHistoryPage {
    pub samples: Vec<QuotaSnapshotEvidence>,
    pub next_cursor: Option<QuotaHistoryCursor>,
    pub has_more: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotaTimeline {
    pub five_hour: Option<QuotaSnapshotEvidence>,
    pub weekly: Option<QuotaSnapshotEvidence>,
    pub history: Option<QuotaHistoryPage>,
    pub evidence_quality: EvidenceQualitySummary,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "evidence_type", rename_all = "snake_case")]
pub enum EvidenceDetail {
    Token { evidence: TokenUsageEvidence },
    Quota { evidence: QuotaSnapshotEvidence },
}

pub(super) fn token_statistics(
    connection: &mut Connection,
    query: &TokenStatisticsQuery,
) -> Result<TokenStatistics, StorageError> {
    query.validate()?;
    let transaction = connection.transaction().map_err(StorageError::sqlite)?;
    let mut result = TokenStatistics {
        canonical_response_count: 0,
        raw_total: MetricTotal::default(),
        input_tokens: MetricTotal::default(),
        cached_input_tokens: MetricTotal::default(),
        cache_write_input_tokens: MetricTotal::default(),
        output_tokens: MetricTotal::default(),
        reasoning_output_tokens: MetricTotal::default(),
        distinct_session_count: 0,
        distinct_task_count: 0,
        configuration_breakdown: ConfigurationBreakdown {
            model: Vec::new(),
            model_truncated: false,
            reasoning_level: Vec::new(),
            reasoning_level_truncated: false,
            service_tier: Vec::new(),
            service_tier_truncated: false,
        },
        evidence_quality: EvidenceQualitySummary {
            total_diagnostics: 0,
            unsupported_source_versions: 0,
            compressed_sources_unsupported: 0,
            malformed_or_incomplete_evidence: 0,
            source_continuity_ambiguities: 0,
            known_partial_collection: 0,
            diagnostics_by_code: Vec::new(),
        },
    };
    let sql = format!(
        "SELECT {TOKEN_COLUMNS} FROM token_usage_evidence \
         WHERE record_semantic = 'per_response' \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3) \
         ORDER BY ({SORTED_TIME_EXPR}), event_id"
    );
    let since = query.since.as_deref().map(timestamp_sort_key);
    let until = query.until.as_deref().map(timestamp_sort_key);
    let mut statement = transaction.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement
        .query(params![since, until, query.model])
        .map_err(StorageError::sqlite)?;
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let evidence = read_token_row(row)?;
        result.canonical_response_count = checked_increment(result.canonical_response_count)?;
        accumulate_metric(&mut result.raw_total, evidence.raw_total)?;
        accumulate_metric(&mut result.input_tokens, evidence.input_tokens)?;
        accumulate_metric(
            &mut result.cached_input_tokens,
            evidence.cached_input_tokens,
        )?;
        accumulate_metric(
            &mut result.cache_write_input_tokens,
            evidence.cache_write_input_tokens,
        )?;
        accumulate_metric(&mut result.output_tokens, evidence.output_tokens)?;
        accumulate_metric(
            &mut result.reasoning_output_tokens,
            evidence.reasoning_output_tokens,
        )?;
        // Identifiers and configuration are queried below with bounded SQL aggregates.
        let _ = (evidence.session_id, evidence.task_id);
    }
    drop(rows);
    drop(statement);
    finish_metric(&mut result.raw_total, result.canonical_response_count);
    finish_metric(&mut result.input_tokens, result.canonical_response_count);
    finish_metric(
        &mut result.cached_input_tokens,
        result.canonical_response_count,
    );
    finish_metric(
        &mut result.cache_write_input_tokens,
        result.canonical_response_count,
    );
    finish_metric(&mut result.output_tokens, result.canonical_response_count);
    finish_metric(
        &mut result.reasoning_output_tokens,
        result.canonical_response_count,
    );
    let (session_count, task_count) = distinct_identifier_counts(&transaction, query)?;
    result.distinct_session_count = session_count;
    result.distinct_task_count = task_count;
    let (model, model_truncated) = configuration_counts(&transaction, query, "model")?;
    let (reasoning_level, reasoning_level_truncated) =
        configuration_counts(&transaction, query, "reasoning_level")?;
    let (service_tier, service_tier_truncated) =
        configuration_counts(&transaction, query, "service_tier")?;
    result.configuration_breakdown = ConfigurationBreakdown {
        model,
        model_truncated,
        reasoning_level,
        reasoning_level_truncated,
        service_tier,
        service_tier_truncated,
    };
    result.evidence_quality = evidence_quality_in_transaction(&transaction)?;
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(result)
}

struct TaskKey {
    source_id: String,
    session_id: String,
    task_id: String,
}

type TaskConfigurationKey = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Default)]
struct TaskAccumulator {
    first_key: Option<String>,
    first_observed_at: Option<String>,
    last_key: Option<String>,
    last_observed_at: Option<String>,
    response_count: u64,
    raw_total: MetricTotal,
    input_tokens: MetricTotal,
    cached_input_tokens: MetricTotal,
    cache_write_input_tokens: MetricTotal,
    output_tokens: MetricTotal,
    reasoning_output_tokens: MetricTotal,
    configurations: BTreeMap<TaskConfigurationKey, u64>,
    configurations_truncated: bool,
}

impl TaskAccumulator {
    fn add(&mut self, event: &TokenUsageEvidence) -> Result<(), StorageError> {
        self.response_count = checked_increment(self.response_count)?;
        accumulate_task_metrics(self, event)?;
        let time_key = timestamp_sort_key(&event.observed_at);
        if self
            .first_key
            .as_ref()
            .is_none_or(|key| time_key.as_str() < key.as_str())
        {
            self.first_key = Some(time_key.clone());
            self.first_observed_at = Some(event.observed_at.clone());
        }
        if self
            .last_key
            .as_ref()
            .is_none_or(|key| time_key.as_str() > key.as_str())
        {
            self.last_key = Some(time_key);
            self.last_observed_at = Some(event.observed_at.clone());
        }
        let configuration = (
            event.model.clone(),
            event.reasoning_level.clone(),
            event.service_tier.clone(),
            event.codex_version.clone(),
        );
        if let Some(count) = self.configurations.get_mut(&configuration) {
            *count = checked_increment(*count)?;
        } else if self.configurations.len() < 64 {
            self.configurations.insert(configuration, 1);
        } else {
            self.configurations_truncated = true;
        }
        Ok(())
    }

    fn finish_metrics(&mut self) {
        for metric in [
            &mut self.raw_total,
            &mut self.input_tokens,
            &mut self.cached_input_tokens,
            &mut self.cache_write_input_tokens,
            &mut self.output_tokens,
            &mut self.reasoning_output_tokens,
        ] {
            finish_metric(metric, self.response_count);
        }
    }
}

fn accumulate_task_metrics(
    accumulator: &mut TaskAccumulator,
    event: &TokenUsageEvidence,
) -> Result<(), StorageError> {
    accumulate_metric(&mut accumulator.raw_total, event.raw_total)?;
    accumulate_metric(&mut accumulator.input_tokens, event.input_tokens)?;
    accumulate_metric(
        &mut accumulator.cached_input_tokens,
        event.cached_input_tokens,
    )?;
    accumulate_metric(
        &mut accumulator.cache_write_input_tokens,
        event.cache_write_input_tokens,
    )?;
    accumulate_metric(&mut accumulator.output_tokens, event.output_tokens)?;
    accumulate_metric(
        &mut accumulator.reasoning_output_tokens,
        event.reasoning_output_tokens,
    )?;
    Ok(())
}

pub(super) fn task_usage(
    connection: &mut Connection,
    query: &TaskUsageQuery,
) -> Result<TaskUsageReport, StorageError> {
    validate_time_range(query.since.as_deref(), query.until.as_deref())?;
    validate_optional_filter(query.model.as_deref())?;
    validate_optional_filter(query.session_id.as_deref())?;
    validate_limit(query.limit)?;

    let transaction = connection.transaction().map_err(StorageError::sqlite)?;
    let since = query.since.as_deref().map(timestamp_sort_key);
    let until = query.until.as_deref().map(timestamp_sort_key);

    // Verify every in-scope canonical row while keeping memory bounded. This
    // also builds the unattributed totals without consulting snapshots.
    let scan_sql = format!(
        "SELECT {TOKEN_COLUMNS} FROM token_usage_evidence \
         WHERE record_semantic = 'per_response' \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3) \
         AND (?4 IS NULL OR session_id = ?4) \
         ORDER BY ({SORTED_TIME_EXPR}), event_id"
    );
    let mut unattributed = TaskAccumulator::default();
    let mut attributed = TaskAccumulator::default();
    let mut canonical_response_count = 0_u64;
    {
        let mut statement = transaction
            .prepare(&scan_sql)
            .map_err(StorageError::sqlite)?;
        let mut rows = statement
            .query(params![since, until, query.model, query.session_id])
            .map_err(StorageError::sqlite)?;
        while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
            let event = read_token_row(row)?;
            canonical_response_count = checked_increment(canonical_response_count)?;
            if event.task_id.is_none() || event.session_id.is_none() {
                unattributed.add(&event)?;
            } else {
                attributed.add(&event)?;
            }
        }
    }
    unattributed.finish_metrics();
    attributed.finish_metrics();

    let task_count_sql = format!(
        "SELECT COUNT(*) FROM (SELECT 1 FROM token_usage_evidence \
         WHERE record_semantic = 'per_response' AND task_id IS NOT NULL AND session_id IS NOT NULL \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3) AND (?4 IS NULL OR session_id = ?4) \
         GROUP BY source_id, session_id, task_id)"
    );
    let task_count: i64 = transaction
        .query_row(
            &task_count_sql,
            params![since, until, query.model, query.session_id],
            |row| row.get(0),
        )
        .map_err(StorageError::sqlite)?;
    let task_count = u64::try_from(task_count).map_err(|_| StorageError::EvidenceCorrupt)?;

    let fetch_limit = query
        .limit
        .checked_add(1)
        .ok_or(StorageError::ReportingQueryInvalid)?;
    let candidate_sql = format!(
        "SELECT source_id, session_id, task_id FROM token_usage_evidence \
         WHERE record_semantic = 'per_response' AND task_id IS NOT NULL AND session_id IS NOT NULL \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3) AND (?4 IS NULL OR session_id = ?4) \
         GROUP BY source_id, session_id, task_id \
         ORDER BY SUM(raw_total) DESC, MAX({SORTED_TIME_EXPR}) DESC, source_id, session_id, task_id LIMIT ?5"
    );
    let mut keys = Vec::with_capacity(fetch_limit as usize);
    {
        let mut statement = transaction
            .prepare(&candidate_sql)
            .map_err(StorageError::sqlite)?;
        let mut rows = statement
            .query(params![
                since,
                until,
                query.model,
                query.session_id,
                i64::from(fetch_limit)
            ])
            .map_err(reporting_sqlite_error)?;
        while let Some(row) = rows.next().map_err(reporting_sqlite_error)? {
            keys.push(TaskKey {
                source_id: get_corrupt(row, 0)?,
                session_id: get_corrupt(row, 1)?,
                task_id: get_corrupt(row, 2)?,
            });
        }
    }
    let has_more = keys.len() > query.limit as usize;
    if has_more {
        keys.pop();
    }
    let mut tasks = Vec::with_capacity(keys.len());
    for key in keys {
        let event_sql = format!(
            "SELECT {TOKEN_COLUMNS} FROM token_usage_evidence \
             WHERE record_semantic = 'per_response' AND source_id = ?1 AND session_id = ?2 AND task_id = ?3 \
             AND (?4 IS NULL OR ({SORTED_TIME_EXPR}) >= ?4) \
             AND (?5 IS NULL OR ({SORTED_TIME_EXPR}) < ?5) \
             AND (?6 IS NULL OR model = ?6) ORDER BY ({SORTED_TIME_EXPR}), event_id"
        );
        let mut accumulator = TaskAccumulator::default();
        {
            let mut statement = transaction
                .prepare(&event_sql)
                .map_err(StorageError::sqlite)?;
            let mut rows = statement
                .query(params![
                    key.source_id,
                    key.session_id,
                    key.task_id,
                    since,
                    until,
                    query.model,
                ])
                .map_err(StorageError::sqlite)?;
            while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
                accumulator.add(&read_token_row(row)?)?;
            }
        }
        accumulator.finish_metrics();
        let configurations: Vec<_> = accumulator
            .configurations
            .into_iter()
            .map(
                |((model, reasoning_level, service_tier, codex_version), response_count)| {
                    TaskModelConfiguration {
                        model,
                        reasoning_level,
                        service_tier,
                        codex_version,
                        response_count,
                    }
                },
            )
            .collect();
        let evidence_status = if accumulator.raw_total.coverage == MetricCoverage::Unavailable {
            "usage_unavailable"
        } else if [
            accumulator.raw_total.coverage,
            accumulator.input_tokens.coverage,
            accumulator.cached_input_tokens.coverage,
            accumulator.cache_write_input_tokens.coverage,
            accumulator.output_tokens.coverage,
            accumulator.reasoning_output_tokens.coverage,
        ]
        .contains(&MetricCoverage::Partial)
            || [
                accumulator.raw_total.coverage,
                accumulator.input_tokens.coverage,
                accumulator.cached_input_tokens.coverage,
                accumulator.cache_write_input_tokens.coverage,
                accumulator.output_tokens.coverage,
                accumulator.reasoning_output_tokens.coverage,
            ]
            .contains(&MetricCoverage::Unavailable)
        {
            "partial"
        } else {
            "complete"
        };
        tasks.push(TaskUsageSummary {
            source_id: key.source_id,
            session_id: key.session_id,
            task_id: key.task_id,
            first_observed_at: accumulator.first_observed_at.unwrap_or_default(),
            last_observed_at: accumulator.last_observed_at.unwrap_or_default(),
            canonical_response_count: accumulator.response_count,
            raw_total: accumulator.raw_total,
            input_tokens: accumulator.input_tokens,
            cached_input_tokens: accumulator.cached_input_tokens,
            cache_write_input_tokens: accumulator.cache_write_input_tokens,
            output_tokens: accumulator.output_tokens,
            reasoning_output_tokens: accumulator.reasoning_output_tokens,
            configurations,
            configurations_truncated: accumulator.configurations_truncated,
            evidence_status,
        });
    }
    let evidence_quality = evidence_quality_in_transaction(&transaction)?;
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(TaskUsageReport {
        task_count,
        canonical_response_count,
        tasks,
        has_more,
        attributed: TaskTokenTotals {
            canonical_response_count: attributed.response_count,
            raw_total: attributed.raw_total,
            input_tokens: attributed.input_tokens,
            cached_input_tokens: attributed.cached_input_tokens,
            cache_write_input_tokens: attributed.cache_write_input_tokens,
            output_tokens: attributed.output_tokens,
            reasoning_output_tokens: attributed.reasoning_output_tokens,
        },
        unattributed: TaskTokenTotals {
            canonical_response_count: unattributed.response_count,
            raw_total: unattributed.raw_total,
            input_tokens: unattributed.input_tokens,
            cached_input_tokens: unattributed.cached_input_tokens,
            cache_write_input_tokens: unattributed.cache_write_input_tokens,
            output_tokens: unattributed.output_tokens,
            reasoning_output_tokens: unattributed.reasoning_output_tokens,
        },
        evidence_quality,
    })
}

pub(super) fn token_history(
    connection: &mut Connection,
    query: &TokenHistoryQuery,
) -> Result<TokenHistoryPage, StorageError> {
    query.validate()?;
    let transaction = connection.transaction().map_err(StorageError::sqlite)?;
    let cursor_time = query
        .cursor
        .as_ref()
        .map(|cursor| timestamp_sort_key(&cursor.observed_at));
    let cursor_id = query.cursor.as_ref().map(|cursor| cursor.event_id.as_str());
    if let Some(cursor) = &query.cursor {
        let event = load_token_by_id(&transaction, &cursor.event_id)?
            .ok_or(StorageError::ReportingEvidenceNotFound)?;
        if event.observed_at != cursor.observed_at
            || event.record_semantic != TokenRecordSemantic::PerResponse
            || !matches_history_filters(&event, query)
        {
            return Err(StorageError::ReportingQueryInvalid);
        }
    }
    let sql = format!(
        "SELECT {TOKEN_COLUMNS} FROM token_usage_evidence \
         WHERE record_semantic = 'per_response' \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3) \
         AND (?4 IS NULL OR session_id = ?4) \
         AND (?5 IS NULL OR ({SORTED_TIME_EXPR}) < ?5 OR (({SORTED_TIME_EXPR}) = ?5 AND event_id < ?6)) \
         ORDER BY ({SORTED_TIME_EXPR}) DESC, event_id DESC LIMIT ?7"
    );
    let since = query.since.as_deref().map(timestamp_sort_key);
    let until = query.until.as_deref().map(timestamp_sort_key);
    let fetch_limit = query
        .limit
        .checked_add(1)
        .ok_or(StorageError::ReportingQueryInvalid)?;
    let mut statement = transaction.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement
        .query(params![
            since,
            until,
            query.model,
            query.session_id,
            cursor_time,
            cursor_id,
            i64::from(fetch_limit),
        ])
        .map_err(StorageError::sqlite)?;
    let mut events = Vec::with_capacity(query.limit as usize + 1);
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        events.push(read_token_row(row)?);
    }
    drop(rows);
    drop(statement);
    let has_more = events.len() > query.limit as usize;
    if has_more {
        events.pop();
    }
    let next_cursor = has_more
        .then(|| {
            events.last().map(|event| TokenHistoryCursor {
                observed_at: event.observed_at.clone(),
                event_id: event.event_id.clone(),
            })
        })
        .flatten();
    let evidence_quality = evidence_quality_in_transaction(&transaction)?;
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(TokenHistoryPage {
        events,
        next_cursor,
        has_more,
        evidence_quality,
    })
}

pub(super) fn quota_timeline(
    connection: &mut Connection,
    include_history: bool,
    limit: u32,
    cursor: Option<&QuotaHistoryCursor>,
) -> Result<QuotaTimeline, StorageError> {
    validate_limit(limit)?;
    let transaction = connection.transaction().map_err(StorageError::sqlite)?;
    let five_hour = load_latest_quota(&transaction, "five_hour")?;
    let weekly = load_latest_quota(&transaction, "weekly")?;
    let history = if include_history {
        Some(quota_history_in_transaction(&transaction, limit, cursor)?)
    } else if cursor.is_some() {
        return Err(StorageError::ReportingQueryInvalid);
    } else {
        None
    };
    let evidence_quality = evidence_quality_in_transaction(&transaction)?;
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(QuotaTimeline {
        five_hour,
        weekly,
        history,
        evidence_quality,
    })
}

pub(super) fn inspect(
    connection: &mut Connection,
    evidence_id: &str,
) -> Result<EvidenceDetail, StorageError> {
    validate_identifier(evidence_id)?;
    let transaction = connection.transaction().map_err(StorageError::sqlite)?;
    let token = load_token_by_id(&transaction, evidence_id)?;
    let quota = load_quota_by_id(&transaction, evidence_id)?;
    let detail = match (token, quota) {
        (Some(_), Some(_)) => return Err(StorageError::ReportingEvidenceAmbiguous),
        (Some(evidence), None) => EvidenceDetail::Token { evidence },
        (None, Some(evidence)) => EvidenceDetail::Quota { evidence },
        (None, None) => return Err(StorageError::ReportingEvidenceNotFound),
    };
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(detail)
}

pub(super) fn evidence_quality(
    connection: &mut Connection,
) -> Result<EvidenceQualitySummary, StorageError> {
    let transaction = connection.transaction().map_err(StorageError::sqlite)?;
    let summary = evidence_quality_in_transaction(&transaction)?;
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(summary)
}

fn quota_history_in_transaction(
    connection: &Connection,
    limit: u32,
    cursor: Option<&QuotaHistoryCursor>,
) -> Result<QuotaHistoryPage, StorageError> {
    if let Some(cursor) = cursor {
        validate_time_range(Some(&cursor.observed_at), None)?;
        validate_identifier(&cursor.sample_id)?;
        let sample = load_quota_by_id(connection, &cursor.sample_id)?
            .ok_or(StorageError::ReportingEvidenceNotFound)?;
        if sample.observed_at != cursor.observed_at {
            return Err(StorageError::ReportingQueryInvalid);
        }
    }
    let cursor_time = cursor.map(|value| timestamp_sort_key(&value.observed_at));
    let cursor_id = cursor.map(|value| value.sample_id.as_str());
    let sql = format!(
        "SELECT {QUOTA_COLUMNS} FROM quota_snapshot_evidence \
         WHERE (?1 IS NULL OR ({SORTED_TIME_EXPR}) < ?1 OR (({SORTED_TIME_EXPR}) = ?1 AND sample_id < ?2)) \
         ORDER BY ({SORTED_TIME_EXPR}) DESC, sample_id DESC LIMIT ?3"
    );
    let fetch_limit = limit
        .checked_add(1)
        .ok_or(StorageError::ReportingQueryInvalid)?;
    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement
        .query(params![cursor_time, cursor_id, i64::from(fetch_limit)])
        .map_err(StorageError::sqlite)?;
    let mut samples = Vec::with_capacity(limit as usize + 1);
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        samples.push(read_quota_row(row)?);
    }
    let has_more = samples.len() > limit as usize;
    if has_more {
        samples.pop();
    }
    let next_cursor = if has_more {
        samples.last().map(|sample| QuotaHistoryCursor {
            observed_at: sample.observed_at.clone(),
            sample_id: sample.sample_id.clone(),
        })
    } else {
        None
    };
    Ok(QuotaHistoryPage {
        samples,
        next_cursor,
        has_more,
    })
}

fn load_latest_quota(
    connection: &Connection,
    meter_type: &str,
) -> Result<Option<QuotaSnapshotEvidence>, StorageError> {
    let sql = format!(
        "SELECT {QUOTA_COLUMNS} FROM quota_snapshot_evidence WHERE meter_type = ?1 \
         ORDER BY ({SORTED_TIME_EXPR}) DESC, sample_id DESC LIMIT 1"
    );
    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement
        .query([meter_type])
        .map_err(StorageError::sqlite)?;
    rows.next()
        .map_err(StorageError::sqlite)?
        .map(read_quota_row)
        .transpose()
}

fn load_token_by_id(
    connection: &Connection,
    event_id: &str,
) -> Result<Option<TokenUsageEvidence>, StorageError> {
    let sql = format!("SELECT {TOKEN_COLUMNS} FROM token_usage_evidence WHERE event_id = ?1");
    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement.query([event_id]).map_err(StorageError::sqlite)?;
    rows.next()
        .map_err(StorageError::sqlite)?
        .map(read_token_row)
        .transpose()
}

fn load_quota_by_id(
    connection: &Connection,
    sample_id: &str,
) -> Result<Option<QuotaSnapshotEvidence>, StorageError> {
    let sql = format!("SELECT {QUOTA_COLUMNS} FROM quota_snapshot_evidence WHERE sample_id = ?1");
    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement.query([sample_id]).map_err(StorageError::sqlite)?;
    rows.next()
        .map_err(StorageError::sqlite)?
        .map(read_quota_row)
        .transpose()
}

fn read_token_row(row: &Row<'_>) -> Result<TokenUsageEvidence, StorageError> {
    let semantic: String = get_corrupt(row, 5)?;
    let record_semantic = match semantic.as_str() {
        "per_response" => TokenRecordSemantic::PerResponse,
        "turn_cumulative_snapshot" => TokenRecordSemantic::TurnCumulativeSnapshot,
        "thread_cumulative_snapshot" => TokenRecordSemantic::ThreadCumulativeSnapshot,
        "token_count_total_snapshot" => TokenRecordSemantic::TokenCountTotalSnapshot,
        "token_count_latest_snapshot" => TokenRecordSemantic::TokenCountLatestSnapshot,
        _ => return Err(StorageError::EvidenceCorrupt),
    };
    let evidence = TokenUsageEvidence {
        event_id: get_corrupt(row, 0)?,
        source_id: get_corrupt(row, 1)?,
        session_id: get_corrupt(row, 2)?,
        task_id: get_corrupt(row, 3)?,
        observed_at: get_corrupt(row, 4)?,
        record_semantic,
        model: get_corrupt(row, 6)?,
        reasoning_level: get_corrupt(row, 7)?,
        service_tier: get_corrupt(row, 8)?,
        codex_version: get_corrupt(row, 9)?,
        input_tokens: read_sql_token(row, 10)?,
        cached_input_tokens: read_sql_token(row, 11)?,
        cache_write_input_tokens: read_sql_token(row, 12)?,
        output_tokens: read_sql_token(row, 13)?,
        reasoning_output_tokens: read_sql_token(row, 14)?,
        raw_total: read_sql_token(row, 15)?,
    };
    let expected_status: String = get_corrupt(row, 16)?;
    let schema_version: i64 = get_corrupt(row, 17)?;
    let expected_hash: String = get_corrupt(row, 18)?;
    let status = if record_semantic == TokenRecordSemantic::PerResponse {
        "observed"
    } else {
        "snapshot"
    };
    if expected_status != status || schema_version != 1 {
        return Err(StorageError::EvidenceCorrupt);
    }
    evidence
        .validate()
        .map_err(|_| StorageError::EvidenceCorrupt)?;
    verify_checksum(&evidence, &expected_hash)?;
    Ok(evidence)
}

fn read_quota_row(row: &Row<'_>) -> Result<QuotaSnapshotEvidence, StorageError> {
    let meter: String = get_corrupt(row, 4)?;
    let meter_type = match meter.as_str() {
        "five_hour" => QuotaMeterType::FiveHour,
        "weekly" => QuotaMeterType::Weekly,
        _ => return Err(StorageError::EvidenceCorrupt),
    };
    let evidence = QuotaSnapshotEvidence {
        sample_id: get_corrupt(row, 0)?,
        source_id: get_corrupt(row, 1)?,
        session_id: get_corrupt(row, 2)?,
        observed_at: get_corrupt(row, 3)?,
        meter_type,
        used_percent: get_corrupt(row, 5)?,
        reset_at: get_corrupt(row, 6)?,
        plan: get_corrupt(row, 7)?,
        codex_version: get_corrupt(row, 8)?,
    };
    let status: String = get_corrupt(row, 9)?;
    let schema_version: i64 = get_corrupt(row, 10)?;
    let expected_hash: String = get_corrupt(row, 11)?;
    if status != "observed" || schema_version != 1 {
        return Err(StorageError::EvidenceCorrupt);
    }
    evidence
        .validate()
        .map_err(|_| StorageError::EvidenceCorrupt)?;
    verify_checksum(&evidence, &expected_hash)?;
    Ok(evidence)
}

fn evidence_quality_in_transaction(
    connection: &Connection,
) -> Result<EvidenceQualitySummary, StorageError> {
    let sql =
        format!("SELECT {DIAGNOSTIC_COLUMNS} FROM ingestion_diagnostics ORDER BY diagnostic_id");
    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement.query([]).map_err(StorageError::sqlite)?;
    let mut counts = BTreeMap::<String, u64>::new();
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let diagnostic = read_diagnostic_row(row)?;
        let count = counts
            .entry(diagnostic.diagnostic_code.as_str().to_owned())
            .or_default();
        *count = checked_increment(*count)?;
    }
    let total_diagnostics = counts
        .values()
        .try_fold(0_u64, |total, count| total.checked_add(*count))
        .ok_or(StorageError::ReportingTotalOverflow)?;
    let count_code = |code: &str| counts.get(code).copied().unwrap_or(0);
    let sum_codes = |codes: &[&str]| -> Result<u64, StorageError> {
        codes.iter().try_fold(0_u64, |sum, code| {
            sum.checked_add(count_code(code))
                .ok_or(StorageError::ReportingTotalOverflow)
        })
    };
    let unsupported_source_versions = count_code("unsupported_codex_version");
    let compressed_sources_unsupported = count_code("compressed_source_unsupported");
    let malformed_or_incomplete_evidence = sum_codes(&[
        "malformed_record",
        "empty_record",
        "invalid_utf8",
        "invalid_timestamp",
        "incomplete_source_header",
        "unsupported_record",
        "unsupported_event",
    ])?;
    let source_continuity_ambiguities = sum_codes(&[
        "session_identity_conflict",
        "source_changed",
        "source_truncated",
        "source_generation_ambiguous",
        "checkpoint_ambiguous",
        "checkpoint_corrupt",
        "ambiguous_source_identity",
    ])?;
    let known_partial_collection = sum_codes(&[
        "source_read_limited",
        "source_read_failed",
        "source_transaction_conflict",
        "quota_evidence_incomplete",
        "unsupported_source_format",
        "discovery_scan_limited",
        "symlink_entry_skipped",
        "discovery_unreadable",
        "data_coverage_incomplete",
        "unsupported_codex_version",
        "compressed_source_unsupported",
    ])?;
    let diagnostics_by_code = counts
        .into_iter()
        .map(|(code, diagnostic_count)| DiagnosticCount {
            code,
            diagnostic_count,
        })
        .collect();
    Ok(EvidenceQualitySummary {
        total_diagnostics,
        unsupported_source_versions,
        compressed_sources_unsupported,
        malformed_or_incomplete_evidence,
        source_continuity_ambiguities,
        known_partial_collection,
        diagnostics_by_code,
    })
}

fn read_diagnostic_row(row: &Row<'_>) -> Result<IngestionDiagnostic, StorageError> {
    let code: String = get_corrupt(row, 4)?;
    let diagnostic = IngestionDiagnostic {
        diagnostic_id: get_corrupt(row, 0)?,
        source_id: get_corrupt(row, 1)?,
        observed_at: get_corrupt(row, 2)?,
        record_semantic: get_corrupt(row, 3)?,
        diagnostic_code: IngestionDiagnosticCode::parse(&code)
            .ok_or(StorageError::EvidenceCorrupt)?,
    };
    let schema_version: i64 = get_corrupt(row, 5)?;
    let expected_hash: String = get_corrupt(row, 6)?;
    if schema_version != 1 {
        return Err(StorageError::EvidenceCorrupt);
    }
    diagnostic
        .validate()
        .map_err(|_| StorageError::EvidenceCorrupt)?;
    verify_checksum(&diagnostic, &expected_hash)?;
    Ok(diagnostic)
}

fn verify_checksum<T: Serialize>(value: &T, expected: &str) -> Result<(), StorageError> {
    if canonical_hash(value)? == expected {
        Ok(())
    } else {
        Err(StorageError::EvidenceCorrupt)
    }
}

fn read_sql_token(row: &Row<'_>, index: usize) -> Result<Option<u64>, StorageError> {
    let value: Option<i64> = get_corrupt(row, index)?;
    value
        .map(|value| u64::try_from(value).map_err(|_| StorageError::EvidenceCorrupt))
        .transpose()
}

fn get_corrupt<T: rusqlite::types::FromSql>(
    row: &Row<'_>,
    index: usize,
) -> Result<T, StorageError> {
    row.get(index).map_err(|_| StorageError::EvidenceCorrupt)
}

fn accumulate_metric(total: &mut MetricTotal, value: Option<u64>) -> Result<(), StorageError> {
    match value {
        Some(value) => {
            total.known_sum = Some(
                total
                    .known_sum
                    .unwrap_or(0)
                    .checked_add(value)
                    .ok_or(StorageError::ReportingTotalOverflow)?,
            );
            total.known_event_count = checked_increment(total.known_event_count)?;
        }
        None => total.missing_event_count = checked_increment(total.missing_event_count)?,
    }
    Ok(())
}

fn reporting_sqlite_error(error: rusqlite::Error) -> StorageError {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(_, Some(message)) if message == "integer overflow"
    ) {
        StorageError::ReportingTotalOverflow
    } else {
        StorageError::sqlite(error)
    }
}

fn finish_metric(metric: &mut MetricTotal, response_count: u64) {
    metric.coverage = if response_count == 0 {
        MetricCoverage::NoObservations
    } else if metric.known_event_count == 0 {
        MetricCoverage::Unavailable
    } else if metric.missing_event_count > 0 {
        MetricCoverage::Partial
    } else {
        MetricCoverage::Complete
    };
}

fn checked_increment(value: u64) -> Result<u64, StorageError> {
    value
        .checked_add(1)
        .ok_or(StorageError::ReportingTotalOverflow)
}

fn distinct_identifier_counts(
    connection: &Connection,
    query: &TokenStatisticsQuery,
) -> Result<(u64, u64), StorageError> {
    let sql = format!(
        "SELECT COUNT(DISTINCT session_id), COUNT(DISTINCT task_id) \
         FROM token_usage_evidence WHERE record_semantic = 'per_response' \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3)"
    );
    let since = query.since.as_deref().map(timestamp_sort_key);
    let until = query.until.as_deref().map(timestamp_sort_key);
    let (sessions, tasks): (i64, i64) = connection
        .query_row(&sql, params![since, until, query.model], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(StorageError::sqlite)?;
    Ok((
        u64::try_from(sessions).map_err(|_| StorageError::EvidenceCorrupt)?,
        u64::try_from(tasks).map_err(|_| StorageError::EvidenceCorrupt)?,
    ))
}

fn configuration_counts(
    connection: &Connection,
    query: &TokenStatisticsQuery,
    column: &'static str,
) -> Result<(Vec<ValueCount>, bool), StorageError> {
    let column = match column {
        "model" => "model",
        "reasoning_level" => "reasoning_level",
        "service_tier" => "service_tier",
        _ => return Err(StorageError::ReportingQueryInvalid),
    };
    let sql = format!(
        "SELECT {column}, COUNT(*) FROM token_usage_evidence \
         WHERE record_semantic = 'per_response' \
         AND (?1 IS NULL OR ({SORTED_TIME_EXPR}) >= ?1) \
         AND (?2 IS NULL OR ({SORTED_TIME_EXPR}) < ?2) \
         AND (?3 IS NULL OR model = ?3) \
         GROUP BY {column} ORDER BY {column} LIMIT ?4"
    );
    let since = query.since.as_deref().map(timestamp_sort_key);
    let until = query.until.as_deref().map(timestamp_sort_key);
    let mut statement = connection.prepare(&sql).map_err(StorageError::sqlite)?;
    let mut rows = statement
        .query(params![
            since,
            until,
            query.model,
            i64::from(MAX_REPORT_LIMIT + 1)
        ])
        .map_err(StorageError::sqlite)?;
    let mut values = Vec::new();
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let value: Option<String> = row.get(0).map_err(|_| StorageError::EvidenceCorrupt)?;
        let count: i64 = row.get(1).map_err(|_| StorageError::EvidenceCorrupt)?;
        values.push(ValueCount {
            value,
            event_count: u64::try_from(count).map_err(|_| StorageError::EvidenceCorrupt)?,
        });
    }
    let truncated = values.len() > MAX_REPORT_LIMIT as usize;
    if truncated {
        values.pop();
    }
    Ok((values, truncated))
}

fn validate_time_range(since: Option<&str>, until: Option<&str>) -> Result<(), StorageError> {
    use time::{format_description::well_known::Rfc3339, OffsetDateTime};
    let mut parsed = [None, None];
    for timestamp in [since, until].into_iter().flatten() {
        if timestamp.len() > 128 {
            return Err(StorageError::ReportingQueryInvalid);
        }
        crate::telemetry::token_normalization::validate_utc_timestamp(timestamp)
            .map_err(|_| StorageError::ReportingQueryInvalid)?;
    }
    if let Some(since) = since {
        parsed[0] = Some(
            OffsetDateTime::parse(since, &Rfc3339)
                .map_err(|_| StorageError::ReportingQueryInvalid)?,
        );
    }
    if let Some(until) = until {
        parsed[1] = Some(
            OffsetDateTime::parse(until, &Rfc3339)
                .map_err(|_| StorageError::ReportingQueryInvalid)?,
        );
    }
    if parsed[0]
        .zip(parsed[1])
        .is_some_and(|(since, until)| since >= until)
    {
        return Err(StorageError::ReportingQueryInvalid);
    }
    Ok(())
}

fn timestamp_sort_key(value: &str) -> String {
    let timestamp = value.strip_suffix('Z').unwrap_or(value);
    let (seconds, fraction) = timestamp
        .split_once('.')
        .map_or((timestamp, ""), |(seconds, fraction)| (seconds, fraction));
    let padded_fraction = format!("{fraction:0<107}");
    format!("{seconds}.{padded_fraction}")
}

fn validate_optional_filter(value: Option<&str>) -> Result<(), StorageError> {
    if value.is_some_and(|value| {
        value.is_empty() || value.len() > 256 || value.chars().any(char::is_control)
    }) {
        Err(StorageError::ReportingQueryInvalid)
    } else {
        Ok(())
    }
}

fn validate_identifier(value: &str) -> Result<(), StorageError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err(StorageError::ReportingQueryInvalid)
    } else {
        Ok(())
    }
}

fn validate_limit(limit: u32) -> Result<(), StorageError> {
    if (1..=MAX_REPORT_LIMIT).contains(&limit) {
        Ok(())
    } else {
        Err(StorageError::ReportingQueryInvalid)
    }
}

fn matches_history_filters(event: &TokenUsageEvidence, query: &TokenHistoryQuery) -> bool {
    query
        .since
        .as_ref()
        .is_none_or(|since| timestamp_sort_key(&event.observed_at) >= timestamp_sort_key(since))
        && query
            .until
            .as_ref()
            .is_none_or(|until| timestamp_sort_key(&event.observed_at) < timestamp_sort_key(until))
        && query
            .model
            .as_ref()
            .is_none_or(|model| event.model.as_ref() == Some(model))
        && query
            .session_id
            .as_ref()
            .is_none_or(|session| event.session_id.as_ref() == Some(session))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::storage::{SqliteReadStore, SqliteStore};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-reporting-{}-{}.sqlite",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove(path: &std::path::Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[allow(clippy::too_many_arguments)] // Fixture fields mirror the typed evidence row.
    pub(crate) fn token(
        id: &str,
        timestamp: &str,
        model: Option<&str>,
        session: Option<&str>,
        semantic: TokenRecordSemantic,
        raw: Option<u64>,
        input: Option<u64>,
        output: Option<u64>,
        reasoning: Option<u64>,
    ) -> TokenUsageEvidence {
        TokenUsageEvidence {
            event_id: id.to_owned(),
            source_id: "src:opaque".to_owned(),
            session_id: session.map(str::to_owned),
            task_id: Some(format!("task:{id}")),
            observed_at: timestamp.to_owned(),
            record_semantic: semantic,
            model: model.map(str::to_owned),
            reasoning_level: Some("high".to_owned()),
            service_tier: None,
            codex_version: Some("0.157.1".to_owned()),
            input_tokens: input,
            cached_input_tokens: Some(2),
            cache_write_input_tokens: None,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            raw_total: raw,
        }
    }

    pub(crate) fn insert_token(store: &SqliteStore, evidence: &TokenUsageEvidence) {
        let status = if evidence.record_semantic == TokenRecordSemantic::PerResponse {
            "observed"
        } else {
            "snapshot"
        };
        let checksum = canonical_hash(evidence).unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO token_usage_evidence (event_id, source_id, session_id, task_id, observed_at, record_semantic, model, reasoning_level, service_tier, codex_version, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total, evidence_status, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 1, ?18)",
                params![
                    evidence.event_id,
                    evidence.source_id,
                    evidence.session_id,
                    evidence.task_id,
                    evidence.observed_at,
                    evidence.record_semantic.as_str(),
                    evidence.model,
                    evidence.reasoning_level,
                    evidence.service_tier,
                    evidence.codex_version,
                    evidence.input_tokens.map(|v| v as i64),
                    evidence.cached_input_tokens.map(|v| v as i64),
                    evidence.cache_write_input_tokens.map(|v| v as i64),
                    evidence.output_tokens.map(|v| v as i64),
                    evidence.reasoning_output_tokens.map(|v| v as i64),
                    evidence.raw_total.map(|v| v as i64),
                    status,
                    checksum,
                ],
            )
            .unwrap();
    }

    fn quota(
        id: &str,
        meter: QuotaMeterType,
        timestamp: &str,
        used: f64,
        reset: Option<&str>,
    ) -> QuotaSnapshotEvidence {
        QuotaSnapshotEvidence {
            sample_id: id.to_owned(),
            source_id: "src:opaque".to_owned(),
            session_id: Some("session:hashed".to_owned()),
            observed_at: timestamp.to_owned(),
            meter_type: meter,
            used_percent: used,
            reset_at: reset.map(str::to_owned),
            plan: Some("Plus".to_owned()),
            codex_version: Some("0.157.1".to_owned()),
        }
    }

    fn insert_quota(store: &SqliteStore, evidence: &QuotaSnapshotEvidence) {
        let checksum = canonical_hash(evidence).unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO quota_snapshot_evidence (sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'observed', 1, ?10)",
                params![
                    evidence.sample_id,
                    evidence.source_id,
                    evidence.session_id,
                    evidence.observed_at,
                    evidence.meter_type.as_str(),
                    evidence.used_percent,
                    evidence.reset_at,
                    evidence.plan,
                    evidence.codex_version,
                    checksum,
                ],
            )
            .unwrap();
    }

    fn diagnostic(id: &str, code: IngestionDiagnosticCode) -> IngestionDiagnostic {
        IngestionDiagnostic {
            diagnostic_id: id.to_owned(),
            source_id: "src:opaque".to_owned(),
            observed_at: Some("2026-10-20T12:00:00Z".to_owned()),
            record_semantic: "source".to_owned(),
            diagnostic_code: code,
        }
    }

    pub(crate) fn insert_diagnostic(store: &SqliteStore, value: &IngestionDiagnostic) {
        let checksum = canonical_hash(value).unwrap();
        store
            .connection()
            .execute(
                "INSERT INTO ingestion_diagnostics (diagnostic_id, source_id, observed_at, record_semantic, diagnostic_code, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![
                    value.diagnostic_id,
                    value.source_id,
                    value.observed_at,
                    value.record_semantic,
                    value.diagnostic_code.as_str(),
                    checksum,
                ],
            )
            .unwrap();
    }

    #[test]
    fn statistics_count_only_per_response_and_keep_missing_zero_and_overlapping_fields_distinct() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            let mut first = token(
                "evt-a",
                "2026-10-20T12:00:00.1Z",
                Some("gpt-x"),
                Some("session:a"),
                TokenRecordSemantic::PerResponse,
                Some(30),
                Some(10),
                Some(20),
                Some(5),
            );
            first.cache_write_input_tokens = Some(7);
            insert_token(&store, &first);
            insert_token(
                &store,
                &token(
                    "evt-b",
                    "2026-10-20T12:00:00.12Z",
                    Some("gpt-x"),
                    Some("session:a"),
                    TokenRecordSemantic::PerResponse,
                    None,
                    Some(0),
                    Some(5),
                    Some(3),
                ),
            );
            insert_token(
                &store,
                &token(
                    "evt-c",
                    "2026-10-20T12:00:00Z",
                    None,
                    None,
                    TokenRecordSemantic::PerResponse,
                    Some(50),
                    None,
                    None,
                    None,
                ),
            );
            insert_token(
                &store,
                &token(
                    "snapshot",
                    "2026-10-20T12:00:01Z",
                    Some("gpt-x"),
                    Some("session:a"),
                    TokenRecordSemantic::TurnCumulativeSnapshot,
                    Some(99_999),
                    Some(99_999),
                    Some(99_999),
                    Some(99_999),
                ),
            );
            insert_diagnostic(
                &store,
                &diagnostic(
                    "diag-unsupported",
                    IngestionDiagnosticCode::UnsupportedCodexVersion,
                ),
            );
            insert_diagnostic(
                &store,
                &diagnostic(
                    "diag-compressed",
                    IngestionDiagnosticCode::CompressedSourceUnsupported,
                ),
            );
        }
        {
            let mut store = SqliteReadStore::open(&database).unwrap();
            let stats = store
                .token_statistics(&TokenStatisticsQuery::default())
                .unwrap();
            assert_eq!(stats.canonical_response_count, 3);
            assert_eq!(stats.raw_total.known_sum, Some(80));
            assert_eq!(stats.raw_total.missing_event_count, 1);
            assert_eq!(stats.raw_total.coverage, MetricCoverage::Partial);
            assert_eq!(stats.input_tokens.known_sum, Some(10));
            assert_eq!(stats.input_tokens.known_event_count, 2);
            assert_eq!(stats.input_tokens.missing_event_count, 1);
            assert_eq!(stats.cached_input_tokens.known_sum, Some(6));
            assert_eq!(stats.cache_write_input_tokens.known_sum, Some(7));
            assert_eq!(stats.cache_write_input_tokens.missing_event_count, 2);
            assert_eq!(stats.output_tokens.known_sum, Some(25));
            assert_eq!(stats.reasoning_output_tokens.known_sum, Some(8));
            assert_eq!(stats.distinct_session_count, 1);
            assert_eq!(stats.distinct_task_count, 3);
            assert_eq!(stats.evidence_quality.unsupported_source_versions, 1);
            assert_eq!(stats.evidence_quality.compressed_sources_unsupported, 1);
            assert_eq!(stats.evidence_quality.known_partial_collection, 2);

            let filtered = store
                .token_statistics(&TokenStatisticsQuery {
                    model: Some("gpt-x".into()),
                    ..TokenStatisticsQuery::default()
                })
                .unwrap();
            assert_eq!(filtered.canonical_response_count, 2);
            assert_eq!(filtered.raw_total.known_sum, Some(30));
            assert_eq!(filtered.raw_total.missing_event_count, 1);
        }
        remove(&database);
    }

    #[test]
    fn task_usage_scopes_ids_aggregates_responses_and_keeps_unknowns_unattributed() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            let mut first = token(
                "evt-task-a",
                "2026-10-20T12:00:00.12Z",
                Some("gpt-x"),
                Some("session:a"),
                TokenRecordSemantic::PerResponse,
                Some(10),
                Some(0),
                Some(3),
                Some(2),
            );
            first.task_id = Some("task:shared".into());
            first.cache_write_input_tokens = Some(5);
            insert_token(&store, &first);
            // The evidence ledger's event ID primary key makes replayed events idempotent.
            store
                .connection()
                .execute(
                    "INSERT OR IGNORE INTO token_usage_evidence SELECT * FROM token_usage_evidence WHERE event_id = ?1",
                    [&first.event_id],
                )
                .unwrap();

            let mut second = token(
                "evt-task-b",
                "2026-10-20T12:00:00.1Z",
                Some("gpt-y"),
                Some("session:a"),
                TokenRecordSemantic::PerResponse,
                Some(20),
                None,
                Some(4),
                Some(1),
            );
            second.task_id = Some("task:shared".into());
            second.reasoning_level = None;
            insert_token(&store, &second);

            let mut other_source = token(
                "evt-other-source",
                "2026-10-20T12:00:01Z",
                Some("gpt-z"),
                Some("session:a"),
                TokenRecordSemantic::PerResponse,
                Some(7),
                Some(8),
                Some(1),
                Some(0),
            );
            other_source.source_id = "src:second".into();
            other_source.task_id = Some("task:shared".into());
            insert_token(&store, &other_source);

            let mut another_task = token(
                "evt-another-task",
                "2026-10-20T12:00:01.5Z",
                Some("gpt-x"),
                Some("session:a"),
                TokenRecordSemantic::PerResponse,
                Some(4),
                Some(3),
                Some(1),
                Some(0),
            );
            another_task.task_id = Some("task:other".into());
            insert_token(&store, &another_task);

            let mut no_task = token(
                "evt-no-task",
                "2026-10-20T12:00:02Z",
                Some("gpt-x"),
                Some("session:a"),
                TokenRecordSemantic::PerResponse,
                Some(6),
                Some(0),
                None,
                None,
            );
            no_task.task_id = None;
            insert_token(&store, &no_task);

            let mut no_session = token(
                "evt-no-session",
                "2026-10-20T12:00:03Z",
                Some("gpt-x"),
                None,
                TokenRecordSemantic::PerResponse,
                None,
                None,
                None,
                None,
            );
            no_session.task_id = Some("task:ambiguous".into());
            insert_token(&store, &no_session);

            let mut snapshot = token(
                "evt-snapshot",
                "2026-10-20T12:00:04Z",
                Some("gpt-x"),
                Some("session:a"),
                TokenRecordSemantic::ThreadCumulativeSnapshot,
                Some(999),
                Some(999),
                Some(999),
                Some(999),
            );
            snapshot.task_id = Some("task:shared".into());
            insert_token(&store, &snapshot);
            insert_diagnostic(
                &store,
                &diagnostic(
                    "diag-task-coverage",
                    IngestionDiagnosticCode::DataCoverageIncomplete,
                ),
            );
        }

        let before_read = fs::read(&database).unwrap();
        let mut store = SqliteReadStore::open(&database).unwrap();
        let report = store
            .task_usage(&TaskUsageQuery {
                since: None,
                until: None,
                model: None,
                session_id: None,
                limit: 2,
            })
            .unwrap();
        let repeated = store
            .task_usage(&TaskUsageQuery {
                since: None,
                until: None,
                model: None,
                session_id: None,
                limit: 2,
            })
            .unwrap();
        assert_eq!(report, repeated);
        assert_eq!(report.task_count, 3);
        assert_eq!(report.canonical_response_count, 6);
        assert_eq!(report.attributed.canonical_response_count, 4);
        assert_eq!(report.attributed.raw_total.known_sum, Some(41));
        assert_eq!(report.tasks.len(), 2);
        assert!(report.has_more);
        let shared = report
            .tasks
            .iter()
            .find(|task| task.source_id == "src:opaque")
            .unwrap();
        assert_eq!(shared.task_id, "task:shared");
        assert_eq!(shared.canonical_response_count, 2);
        assert_eq!(shared.first_observed_at, "2026-10-20T12:00:00.1Z");
        assert_eq!(shared.last_observed_at, "2026-10-20T12:00:00.12Z");
        assert_eq!(shared.raw_total.known_sum, Some(30));
        assert_eq!(shared.input_tokens.known_sum, Some(0));
        assert_eq!(shared.input_tokens.missing_event_count, 1);
        assert_eq!(shared.input_tokens.coverage, MetricCoverage::Partial);
        assert_eq!(shared.cache_write_input_tokens.known_sum, Some(5));
        assert_eq!(shared.output_tokens.known_sum, Some(7));
        assert_eq!(shared.reasoning_output_tokens.known_sum, Some(3));
        assert_eq!(shared.evidence_status, "partial");
        assert_eq!(shared.configurations.len(), 2);
        assert!(shared
            .configurations
            .iter()
            .any(
                |configuration| configuration.model.as_deref() == Some("gpt-y")
                    && configuration.reasoning_level.is_none()
            ));
        assert_eq!(report.unattributed.canonical_response_count, 2);
        assert_eq!(report.unattributed.raw_total.known_sum, Some(6));
        assert_eq!(report.unattributed.raw_total.missing_event_count, 1);
        assert_eq!(report.unattributed.input_tokens.known_sum, Some(0));
        assert_eq!(report.evidence_quality.known_partial_collection, 1);

        let filtered = store
            .task_usage(&TaskUsageQuery {
                since: None,
                until: None,
                model: Some("gpt-y".into()),
                session_id: Some("session:a".into()),
                limit: 20,
            })
            .unwrap();
        assert_eq!(filtered.canonical_response_count, 1);
        assert_eq!(filtered.task_count, 1);
        assert_eq!(filtered.tasks[0].raw_total.known_sum, Some(20));
        let bounded = store
            .task_usage(&TaskUsageQuery {
                since: Some("2026-10-20T12:00:00Z".into()),
                until: Some("2026-10-20T12:00:02Z".into()),
                model: None,
                session_id: None,
                limit: 20,
            })
            .unwrap();
        assert_eq!(bounded.canonical_response_count, 4);
        assert_eq!(bounded.task_count, 3);
        drop(store);
        assert_eq!(fs::read(&database).unwrap(), before_read);
        remove(&database);
    }

    #[test]
    fn task_usage_rejects_corrupt_in_scope_evidence_even_when_task_page_is_bounded() {
        let database = path();
        {
            let writer = SqliteStore::open(&database).unwrap();
            insert_token(
                &writer,
                &token(
                    "evt-corrupt-task",
                    "2026-10-20T12:00:00Z",
                    Some("gpt-x"),
                    Some("session:a"),
                    TokenRecordSemantic::PerResponse,
                    Some(10),
                    Some(5),
                    Some(5),
                    Some(1),
                ),
            );
            writer
                .connection()
                .execute(
                    "UPDATE token_usage_evidence SET evidence_sha256 = ?1 WHERE event_id = 'evt-corrupt-task'",
                    ["0000000000000000000000000000000000000000000000000000000000000000"],
                )
                .unwrap();
        }
        let mut reader = SqliteReadStore::open(&database).unwrap();
        assert!(matches!(
            reader.task_usage(&TaskUsageQuery {
                since: None,
                until: None,
                model: None,
                session_id: None,
                limit: 1,
            }),
            Err(StorageError::EvidenceCorrupt)
        ));
        drop(reader);
        remove(&database);
    }

    #[test]
    fn history_keyset_orders_fractional_timestamps_and_equal_timestamp_ids_stably() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            for (id, time, model, session) in [
                (
                    "evt-a",
                    "2026-10-20T12:00:00.1Z",
                    Some("gpt-x"),
                    Some("session:a"),
                ),
                (
                    "evt-z",
                    "2026-10-20T12:00:00.10Z",
                    Some("gpt-x"),
                    Some("session:a"),
                ),
                (
                    "evt-b",
                    "2026-10-20T12:00:00.12Z",
                    Some("gpt-y"),
                    Some("session:b"),
                ),
                ("evt-c", "2026-10-20T12:00:00Z", Some("gpt-x"), None),
            ] {
                insert_token(
                    &store,
                    &token(
                        id,
                        time,
                        model,
                        session,
                        TokenRecordSemantic::PerResponse,
                        Some(1),
                        Some(1),
                        Some(1),
                        Some(1),
                    ),
                );
            }
        }
        {
            let mut store = SqliteReadStore::open(&database).unwrap();
            let mut cursor = None;
            let mut ids = Vec::new();
            for _ in 0..4 {
                let page = store
                    .token_history(&TokenHistoryQuery {
                        limit: 1,
                        cursor,
                        ..TokenHistoryQuery::default()
                    })
                    .unwrap();
                ids.extend(page.events.iter().map(|event| event.event_id.clone()));
                cursor = page.next_cursor;
            }
            assert_eq!(ids, ["evt-b", "evt-z", "evt-a", "evt-c"]);

            let filtered = store
                .token_history(&TokenHistoryQuery {
                    model: Some("gpt-x".into()),
                    session_id: Some("session:a".into()),
                    limit: 20,
                    cursor: None,
                    since: Some("2026-10-20T12:00:00.1Z".into()),
                    until: Some("2026-10-20T12:00:00.11Z".into()),
                })
                .unwrap();
            assert_eq!(
                filtered
                    .events
                    .iter()
                    .map(|event| event.event_id.as_str())
                    .collect::<Vec<_>>(),
                ["evt-z", "evt-a"]
            );
        }
        remove(&database);
    }

    #[test]
    fn quota_meters_have_independent_latest_samples_and_paged_history() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            insert_quota(
                &store,
                &quota(
                    "five-old",
                    QuotaMeterType::FiveHour,
                    "2026-10-19T12:00:00Z",
                    20.0,
                    None,
                ),
            );
            insert_quota(
                &store,
                &quota(
                    "five-new",
                    QuotaMeterType::FiveHour,
                    "2026-10-20T12:00:00Z",
                    42.0,
                    Some("2026-10-20T15:00:00Z"),
                ),
            );
            insert_quota(
                &store,
                &quota(
                    "weekly-old",
                    QuotaMeterType::Weekly,
                    "2026-10-19T11:00:00Z",
                    61.0,
                    None,
                ),
            );
        }
        {
            let mut store = SqliteReadStore::open(&database).unwrap();
            let timeline = store.quota_timeline(true, 1, None).unwrap();
            assert_eq!(timeline.five_hour.as_ref().unwrap().sample_id, "five-new");
            assert_eq!(timeline.weekly.as_ref().unwrap().sample_id, "weekly-old");
            assert_eq!(timeline.five_hour.as_ref().unwrap().used_percent, 42.0);
            assert_eq!(
                timeline.five_hour.as_ref().unwrap().reset_at.as_deref(),
                Some("2026-10-20T15:00:00Z")
            );
            let page = timeline.history.unwrap();
            assert_eq!(page.samples[0].sample_id, "five-new");
            assert!(page.has_more);
            let second = store
                .quota_timeline(true, 1, page.next_cursor.as_ref())
                .unwrap()
                .history
                .unwrap();
            assert_eq!(second.samples[0].sample_id, "five-old");
        }
        remove(&database);
    }

    #[test]
    fn inspector_distinguishes_unknown_ids_from_corrupt_checksums() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            insert_token(
                &store,
                &token(
                    "known-event",
                    "2026-10-20T12:00:00Z",
                    Some("gpt-x"),
                    None,
                    TokenRecordSemantic::PerResponse,
                    Some(10),
                    None,
                    None,
                    None,
                ),
            );
            store
                .connection()
                .execute(
                    "UPDATE token_usage_evidence SET evidence_sha256 = ?1 WHERE event_id = ?2",
                    params!["0".repeat(64), "known-event"],
                )
                .unwrap();
        }
        let mut reader = SqliteReadStore::open(&database).unwrap();
        assert!(matches!(
            reader.inspect("absent-event"),
            Err(StorageError::ReportingEvidenceNotFound)
        ));
        assert!(matches!(
            reader.inspect("known-event"),
            Err(StorageError::EvidenceCorrupt)
        ));
        drop(reader);
        remove(&database);
    }

    #[test]
    fn aggregation_overflow_returns_an_error_instead_of_wrapping() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            for index in 0..2_050 {
                insert_token(
                    &store,
                    &token(
                        &format!("evt-{index:04}"),
                        "2026-10-20T12:00:00Z",
                        Some("gpt-x"),
                        None,
                        TokenRecordSemantic::PerResponse,
                        Some(9_007_199_254_740_991),
                        Some(9_007_199_254_740_991),
                        None,
                        None,
                    ),
                );
            }
        }
        let mut store = SqliteReadStore::open(&database).unwrap();
        assert!(matches!(
            store.token_statistics(&TokenStatisticsQuery::default()),
            Err(StorageError::ReportingTotalOverflow)
        ));
        assert!(matches!(
            store.task_usage(&TaskUsageQuery {
                since: None,
                until: None,
                model: None,
                session_id: None,
                limit: 1,
            }),
            Err(StorageError::ReportingTotalOverflow)
        ));
        drop(store);
        remove(&database);
    }

    #[test]
    fn read_only_reports_preserve_database_bytes_and_do_not_upgrade_a_v5_schema() {
        let database = path();
        {
            let mut connection = rusqlite::Connection::open(&database).unwrap();
            super::super::migrations::migrate(
                &mut connection,
                &super::super::migrations::MIGRATIONS[..5],
            )
            .unwrap();
        }
        let before = fs::read(&database).unwrap();
        {
            let mut store = SqliteReadStore::open(&database).unwrap();
            assert_eq!(store.info().latest_applied_migration, 5);
            assert_eq!(
                store
                    .token_statistics(&TokenStatisticsQuery::default())
                    .unwrap()
                    .canonical_response_count,
                0
            );
            assert_eq!(
                store
                    .task_usage(&TaskUsageQuery {
                        since: None,
                        until: None,
                        model: None,
                        session_id: None,
                        limit: 10,
                    })
                    .unwrap()
                    .task_count,
                0
            );
        }
        let after = fs::read(&database).unwrap();
        assert_eq!(before, after);
        let connection = rusqlite::Connection::open(&database).unwrap();
        let migrations: i64 = connection
            .query_row("SELECT count(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(migrations, 5);
        drop(connection);
        remove(&database);
    }

    #[test]
    fn large_history_uses_bounded_keyset_pages_without_duplicates_or_gaps() {
        let database = path();
        {
            let store = SqliteStore::open(&database).unwrap();
            for index in 0..1_005 {
                insert_token(
                    &store,
                    &token(
                        &format!("evt-{index:04}"),
                        &format!("2026-10-20T12:{:02}:{:02}Z", (index / 60) % 60, index % 60),
                        Some("gpt-x"),
                        Some("session:large"),
                        TokenRecordSemantic::PerResponse,
                        Some(1),
                        Some(1),
                        Some(1),
                        Some(1),
                    ),
                );
            }
        }
        {
            let mut store = SqliteReadStore::open(&database).unwrap();
            let first = store
                .token_history(&TokenHistoryQuery {
                    limit: 1_000,
                    ..TokenHistoryQuery::default()
                })
                .unwrap();
            assert_eq!(first.events.len(), 1_000);
            assert!(first.has_more);
            let second = store
                .token_history(&TokenHistoryQuery {
                    limit: 1_000,
                    cursor: first.next_cursor,
                    ..TokenHistoryQuery::default()
                })
                .unwrap();
            assert_eq!(second.events.len(), 5);
            assert!(!second.has_more);
            let ids = first
                .events
                .iter()
                .chain(second.events.iter())
                .map(|event| event.event_id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(ids.len(), 1_005);
        }
        remove(&database);
    }

    #[test]
    fn global_time_indexes_support_the_history_ordering_expression() {
        let store = SqliteStore::open_in_memory().unwrap();
        let token_plan = store
            .connection()
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT event_id FROM token_usage_evidence ORDER BY ({SORTED_TIME_EXPR}) DESC, event_id DESC LIMIT 20"
            ))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" ");
        let quota_plan = store
            .connection()
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT sample_id FROM quota_snapshot_evidence ORDER BY ({SORTED_TIME_EXPR}) DESC, sample_id DESC LIMIT 20"
            ))
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" ");
        assert!(
            token_plan.contains("token_usage_global_time_idx"),
            "{token_plan}"
        );
        assert!(
            quota_plan.contains("quota_snapshot_global_time_idx"),
            "{quota_plan}"
        );
    }

    #[test]
    fn old_unsupported_source_format_fails_read_only_schema_validation() {
        let database = path();
        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute("CREATE TABLE schema_migrations (version INTEGER)", [])
                .unwrap();
        }
        assert!(matches!(
            SqliteReadStore::open(&database),
            Err(StorageError::MigrationHistoryCorrupt { .. })
        ));
        remove(&database);
    }
}
