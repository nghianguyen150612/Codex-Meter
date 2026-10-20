//! Typed, privacy-filtered evidence ledger and atomic cursor commits.

use std::fmt;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::telemetry::{QuotaMeterType, RolloutCursor, SourceIdentity};

use super::checkpoints::{
    encode_state, save_in_transaction_idempotent, RuntimeCheckpoint, RuntimeRecoveryState,
};
use super::observations::{load_observation_from_connection, save_observation_in_transaction};
use super::sqlite::StorageError;
use crate::telemetry::NormalizedObservation;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenRecordSemantic {
    PerResponse,
    TurnCumulativeSnapshot,
    ThreadCumulativeSnapshot,
    TokenCountTotalSnapshot,
    TokenCountLatestSnapshot,
}

impl TokenRecordSemantic {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PerResponse => "per_response",
            Self::TurnCumulativeSnapshot => "turn_cumulative_snapshot",
            Self::ThreadCumulativeSnapshot => "thread_cumulative_snapshot",
            Self::TokenCountTotalSnapshot => "token_count_total_snapshot",
            Self::TokenCountLatestSnapshot => "token_count_latest_snapshot",
        }
    }

    const fn evidence_status(self) -> &'static str {
        match self {
            Self::PerResponse => "observed",
            _ => "snapshot",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TokenUsageEvidence {
    pub event_id: String,
    pub source_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub observed_at: String,
    pub record_semantic: TokenRecordSemantic,
    pub model: Option<String>,
    pub reasoning_level: Option<String>,
    pub service_tier: Option<String>,
    pub codex_version: Option<String>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_output_tokens: Option<u64>,
    pub raw_total: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct QuotaSnapshotEvidence {
    pub sample_id: String,
    pub source_id: String,
    pub session_id: Option<String>,
    pub observed_at: String,
    pub meter_type: QuotaMeterType,
    pub used_percent: f64,
    pub reset_at: Option<String>,
    pub plan: Option<String>,
    pub codex_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestionDiagnosticCode {
    MalformedRecord,
    EmptyRecord,
    InvalidUtf8,
    UnsupportedRecord,
    UnsupportedEvent,
    InvalidTimestamp,
    SessionIdentityConflict,
    QuotaUnavailable,
    QuotaNonMainLimit,
    QuotaUnsupportedWindow,
    QuotaAmbiguousWindow,
    SourceChanged,
    SourceTruncated,
    SourceGenerationAmbiguous,
    CheckpointAmbiguous,
    CheckpointCorrupt,
    SourceReadLimited,
    SourceReadFailed,
    SourceTransactionConflict,
    QuotaEvidenceIncomplete,
    UnsupportedSourceFormat,
    DiscoveryScanLimited,
    SymlinkEntrySkipped,
    DiscoveryUnreadable,
    UnsupportedCodexVersion,
    IncompleteSourceHeader,
    AmbiguousSourceIdentity,
    CompressedSourceUnsupported,
    DataCoverageIncomplete,
}

impl IngestionDiagnosticCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::MalformedRecord => "malformed_record",
            Self::EmptyRecord => "empty_record",
            Self::InvalidUtf8 => "invalid_utf8",
            Self::UnsupportedRecord => "unsupported_record",
            Self::UnsupportedEvent => "unsupported_event",
            Self::InvalidTimestamp => "invalid_timestamp",
            Self::SessionIdentityConflict => "session_identity_conflict",
            Self::QuotaUnavailable => "quota_unavailable",
            Self::QuotaNonMainLimit => "quota_non_main_limit",
            Self::QuotaUnsupportedWindow => "quota_unsupported_window",
            Self::QuotaAmbiguousWindow => "quota_ambiguous_window",
            Self::SourceChanged => "source_changed",
            Self::SourceTruncated => "source_truncated",
            Self::SourceGenerationAmbiguous => "source_generation_ambiguous",
            Self::CheckpointAmbiguous => "checkpoint_ambiguous",
            Self::CheckpointCorrupt => "checkpoint_corrupt",
            Self::SourceReadLimited => "source_read_limited",
            Self::SourceReadFailed => "source_read_failed",
            Self::SourceTransactionConflict => "source_transaction_conflict",
            Self::QuotaEvidenceIncomplete => "quota_evidence_incomplete",
            Self::UnsupportedSourceFormat => "unsupported_source_format",
            Self::DiscoveryScanLimited => "discovery_scan_limited",
            Self::SymlinkEntrySkipped => "symlink_entry_skipped",
            Self::DiscoveryUnreadable => "discovery_unreadable",
            Self::UnsupportedCodexVersion => "unsupported_codex_version",
            Self::IncompleteSourceHeader => "incomplete_source_header",
            Self::AmbiguousSourceIdentity => "ambiguous_source_identity",
            Self::CompressedSourceUnsupported => "compressed_source_unsupported",
            Self::DataCoverageIncomplete => "data_coverage_incomplete",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "malformed_record" => Self::MalformedRecord,
            "empty_record" => Self::EmptyRecord,
            "invalid_utf8" => Self::InvalidUtf8,
            "unsupported_record" => Self::UnsupportedRecord,
            "unsupported_event" => Self::UnsupportedEvent,
            "invalid_timestamp" => Self::InvalidTimestamp,
            "session_identity_conflict" => Self::SessionIdentityConflict,
            "quota_unavailable" => Self::QuotaUnavailable,
            "quota_non_main_limit" => Self::QuotaNonMainLimit,
            "quota_unsupported_window" => Self::QuotaUnsupportedWindow,
            "quota_ambiguous_window" => Self::QuotaAmbiguousWindow,
            "source_changed" => Self::SourceChanged,
            "source_truncated" => Self::SourceTruncated,
            "source_generation_ambiguous" => Self::SourceGenerationAmbiguous,
            "checkpoint_ambiguous" => Self::CheckpointAmbiguous,
            "checkpoint_corrupt" => Self::CheckpointCorrupt,
            "source_read_limited" => Self::SourceReadLimited,
            "source_read_failed" => Self::SourceReadFailed,
            "source_transaction_conflict" => Self::SourceTransactionConflict,
            "quota_evidence_incomplete" => Self::QuotaEvidenceIncomplete,
            "unsupported_source_format" => Self::UnsupportedSourceFormat,
            "discovery_scan_limited" => Self::DiscoveryScanLimited,
            "symlink_entry_skipped" => Self::SymlinkEntrySkipped,
            "discovery_unreadable" => Self::DiscoveryUnreadable,
            "unsupported_codex_version" => Self::UnsupportedCodexVersion,
            "incomplete_source_header" => Self::IncompleteSourceHeader,
            "ambiguous_source_identity" => Self::AmbiguousSourceIdentity,
            "compressed_source_unsupported" => Self::CompressedSourceUnsupported,
            "data_coverage_incomplete" => Self::DataCoverageIncomplete,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IngestionDiagnostic {
    pub diagnostic_id: String,
    pub source_id: String,
    pub observed_at: Option<String>,
    pub record_semantic: String,
    pub diagnostic_code: IngestionDiagnosticCode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceBoundaryFingerprint {
    pub committed_offset: u64,
    pub last_line_sha256: Option<String>,
    pub file_identity_sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StoredQuotaSnapshot {
    pub sample_id: String,
    pub used_percent: f64,
    pub observed_at: String,
    pub reset_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageHistorySummary {
    pub response_count: u64,
    pub raw_total_known_count: u64,
    pub raw_total_missing_count: u64,
    pub raw_total_known_sum: Option<u64>,
    pub five_hour_latest: Option<StoredQuotaSnapshot>,
    pub weekly_latest: Option<StoredQuotaSnapshot>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EvidenceInsertCounts {
    pub responses: usize,
    pub snapshots: usize,
    pub quota_samples: usize,
    pub diagnostics: usize,
    pub observations: usize,
}

pub struct UsageEvidenceBatch<'a> {
    pub tokens: &'a [TokenUsageEvidence],
    pub quotas: &'a [QuotaSnapshotEvidence],
    pub diagnostics: &'a [IngestionDiagnostic],
    pub observations: &'a [NormalizedObservation],
    pub boundary: &'a SourceBoundaryFingerprint,
}

impl TokenUsageEvidence {
    pub(crate) fn validate(&self) -> Result<(), StorageError> {
        validate_identifier(&self.event_id)?;
        validate_identifier(&self.source_id)?;
        validate_optional_identifier(self.session_id.as_deref())?;
        validate_optional_identifier(self.task_id.as_deref())?;
        validate_timestamp(&self.observed_at)?;
        validate_optional_text(self.model.as_deref())?;
        validate_optional_text(self.reasoning_level.as_deref())?;
        validate_optional_text(self.service_tier.as_deref())?;
        validate_optional_text(self.codex_version.as_deref())?;
        for value in [
            self.input_tokens,
            self.cached_input_tokens,
            self.cache_write_input_tokens,
            self.output_tokens,
            self.reasoning_output_tokens,
            self.raw_total,
        ] {
            if value.is_some_and(|count| count > MAX_SAFE_INTEGER) {
                return Err(StorageError::EvidenceInvalid);
            }
        }
        Ok(())
    }
}

impl QuotaSnapshotEvidence {
    pub(crate) fn validate(&self) -> Result<(), StorageError> {
        validate_identifier(&self.sample_id)?;
        validate_identifier(&self.source_id)?;
        validate_optional_identifier(self.session_id.as_deref())?;
        validate_timestamp(&self.observed_at)?;
        if !self.used_percent.is_finite() || !(0.0..=100.0).contains(&self.used_percent) {
            return Err(StorageError::EvidenceInvalid);
        }
        if let Some(reset_at) = self.reset_at.as_deref() {
            validate_timestamp(reset_at)?;
        }
        validate_optional_text(self.plan.as_deref())?;
        validate_optional_text(self.codex_version.as_deref())?;
        Ok(())
    }
}

impl IngestionDiagnostic {
    pub(crate) fn validate(&self) -> Result<(), StorageError> {
        validate_identifier(&self.diagnostic_id)?;
        validate_identifier(&self.source_id)?;
        if let Some(timestamp) = self.observed_at.as_deref() {
            validate_timestamp(timestamp)?;
        }
        if self.record_semantic.is_empty()
            || self.record_semantic.len() > 64
            || self.record_semantic.chars().any(char::is_control)
        {
            return Err(StorageError::EvidenceInvalid);
        }
        Ok(())
    }
}

pub(crate) fn save_ingestion_diagnostic(
    connection: &mut Connection,
    diagnostic: &IngestionDiagnostic,
) -> Result<bool, StorageError> {
    diagnostic.validate()?;
    let checksum = canonical_hash(diagnostic)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(StorageError::sqlite)?;
    let changed = transaction
        .execute(
            "INSERT OR IGNORE INTO ingestion_diagnostics (diagnostic_id, source_id, observed_at, record_semantic, diagnostic_code, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
            params![
                diagnostic.diagnostic_id,
                diagnostic.source_id,
                diagnostic.observed_at,
                diagnostic.record_semantic,
                diagnostic.diagnostic_code.as_str(),
                checksum,
            ],
        )
        .map_err(StorageError::sqlite)?;
    if changed == 0 {
        verify_immutable_hash(
            &transaction,
            "ingestion_diagnostics",
            "diagnostic_id",
            &diagnostic.diagnostic_id,
            &checksum,
        )?;
    }
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok(changed == 1)
}

fn validate_identifier(value: &str) -> Result<(), StorageError> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err(StorageError::EvidenceInvalid)
    } else {
        Ok(())
    }
}

fn validate_optional_identifier(value: Option<&str>) -> Result<(), StorageError> {
    value.map_or(Ok(()), validate_identifier)
}

fn validate_optional_text(value: Option<&str>) -> Result<(), StorageError> {
    if value.is_some_and(|value| {
        value.is_empty() || value.len() > 256 || value.chars().any(char::is_control)
    }) {
        Err(StorageError::EvidenceInvalid)
    } else {
        Ok(())
    }
}

fn validate_timestamp(value: &str) -> Result<(), StorageError> {
    crate::telemetry::token_normalization::validate_utc_timestamp(value)
        .map_err(|_| StorageError::EvidenceInvalid)
}

pub(crate) fn load_source_boundary(
    connection: &Connection,
    source: &SourceIdentity,
) -> Result<Option<SourceBoundaryFingerprint>, StorageError> {
    connection
        .query_row(
            "SELECT committed_offset, last_line_sha256, file_identity_sha256 FROM source_boundary_fingerprints WHERE rollout_id = ?1 AND source_generation = ?2",
            params![source.rollout_id(), source.source_generation()],
            |row| {
                let offset: String = row.get(0)?;
                let hash: Option<String> = row.get(1)?;
                let file_identity_sha256: Option<String> = row.get(2)?;
                Ok((offset, hash, file_identity_sha256))
            },
        )
        .optional()
        .map_err(StorageError::sqlite)?
        .map(|(offset, last_line_sha256, file_identity_sha256)| {
            let committed_offset = offset
                .parse::<u64>()
                .map_err(|_| StorageError::CheckpointCursorInvalid)?;
            if last_line_sha256.as_ref().is_some_and(|hash| !valid_sha256(hash))
                || file_identity_sha256
                    .as_ref()
                    .is_some_and(|hash| !valid_sha256(hash))
            {
                return Err(StorageError::CheckpointCorrupt);
            }
            Ok(SourceBoundaryFingerprint {
                committed_offset,
                last_line_sha256,
                file_identity_sha256,
            })
        })
        .transpose()
}

pub(crate) fn load_task_response_evidence(
    connection: &Connection,
    task_id: &str,
    session_id: Option<&str>,
) -> Result<Vec<TokenUsageEvidence>, StorageError> {
    let mut statement = connection
        .prepare(
            "SELECT event_id, source_id, session_id, task_id, observed_at, model, reasoning_level, service_tier, codex_version, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total, evidence_sha256 FROM token_usage_evidence WHERE task_id = ?1 AND session_id IS ?2 AND record_semantic = 'per_response' ORDER BY observed_at, event_id",
        )
        .map_err(StorageError::sqlite)?;
    let rows = statement
        .query_map(params![task_id, session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, Option<i64>>(14)?,
                row.get::<_, String>(15)?,
            ))
        })
        .map_err(StorageError::sqlite)?;
    rows.map(|row| {
        let (
            event_id,
            source_id,
            session_id,
            stored_task_id,
            observed_at,
            model,
            reasoning_level,
            service_tier,
            codex_version,
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens,
            raw_total,
            expected_hash,
        ) = row.map_err(StorageError::sqlite)?;
        let evidence = TokenUsageEvidence {
            event_id,
            source_id,
            session_id,
            task_id: stored_task_id,
            observed_at,
            record_semantic: TokenRecordSemantic::PerResponse,
            model,
            reasoning_level,
            service_tier,
            codex_version,
            input_tokens: from_sql_token(input_tokens)?,
            cached_input_tokens: from_sql_token(cached_input_tokens)?,
            cache_write_input_tokens: from_sql_token(cache_write_input_tokens)?,
            output_tokens: from_sql_token(output_tokens)?,
            reasoning_output_tokens: from_sql_token(reasoning_output_tokens)?,
            raw_total: from_sql_token(raw_total)?,
        };
        evidence.validate()?;
        if canonical_hash(&evidence)? != expected_hash {
            return Err(StorageError::EvidenceCorrupt);
        }
        Ok(evidence)
    })
    .collect()
}

pub(crate) fn has_other_rollout_generation(
    connection: &Connection,
    source: &SourceIdentity,
) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM runtime_checkpoints WHERE rollout_id = ?1 AND source_generation <> ?2 UNION ALL SELECT 1 FROM source_boundary_fingerprints WHERE rollout_id = ?1 AND source_generation <> ?2)",
            params![source.rollout_id(), source.source_generation()],
            |row| row.get(0),
        )
        .map_err(StorageError::sqlite)
}

fn from_sql_token(value: Option<i64>) -> Result<Option<u64>, StorageError> {
    value
        .map(|value| u64::try_from(value).map_err(|_| StorageError::EvidenceCorrupt))
        .transpose()
}

pub(crate) fn commit_usage_batch(
    connection: &mut Connection,
    source: &SourceIdentity,
    cursor: &RolloutCursor,
    state: &RuntimeRecoveryState,
    expected_checkpoint_revision: Option<u64>,
    batch: UsageEvidenceBatch<'_>,
) -> Result<(RuntimeCheckpoint, EvidenceInsertCounts), StorageError> {
    let UsageEvidenceBatch {
        tokens,
        quotas,
        diagnostics,
        observations,
        boundary,
    } = batch;
    if boundary.committed_offset != cursor.committed_offset()
        || (boundary.committed_offset == 0) != boundary.last_line_sha256.is_none()
        || boundary
            .last_line_sha256
            .as_ref()
            .is_some_and(|hash| !valid_sha256(hash))
        || boundary
            .file_identity_sha256
            .as_ref()
            .is_some_and(|hash| !valid_sha256(hash))
    {
        return Err(StorageError::CheckpointCursorInvalid);
    }
    let (state_json, state_sha256) = encode_state(state)?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(StorageError::sqlite)?;
    let mut inserted = EvidenceInsertCounts::default();
    for evidence in tokens {
        evidence.validate()?;
        let checksum = canonical_hash(evidence)?;
        let changed = transaction
            .execute(
                "INSERT OR IGNORE INTO token_usage_evidence (event_id, source_id, session_id, task_id, observed_at, record_semantic, model, reasoning_level, service_tier, codex_version, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total, evidence_status, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 1, ?18)",
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
                    sql_token(evidence.input_tokens),
                    sql_token(evidence.cached_input_tokens),
                    sql_token(evidence.cache_write_input_tokens),
                    sql_token(evidence.output_tokens),
                    sql_token(evidence.reasoning_output_tokens),
                    sql_token(evidence.raw_total),
                    evidence.record_semantic.evidence_status(),
                    checksum,
                ],
            )
            .map_err(StorageError::sqlite)?;
        if changed == 1 {
            match evidence.record_semantic {
                TokenRecordSemantic::PerResponse => inserted.responses += 1,
                _ => inserted.snapshots += 1,
            }
        } else {
            verify_immutable_hash(
                &transaction,
                "token_usage_evidence",
                "event_id",
                &evidence.event_id,
                &checksum,
            )?;
        }
    }
    for evidence in quotas {
        evidence.validate()?;
        let checksum = canonical_hash(evidence)?;
        let changed = transaction
            .execute(
                "INSERT OR IGNORE INTO quota_snapshot_evidence (sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'observed', 1, ?10)",
                params![
                    evidence.sample_id,
                    evidence.source_id,
                    evidence.session_id,
                    evidence.observed_at,
                    meter_name(evidence.meter_type),
                    evidence.used_percent,
                    evidence.reset_at,
                    evidence.plan,
                    evidence.codex_version,
                    checksum,
                ],
            )
            .map_err(StorageError::sqlite)?;
        if changed == 1 {
            inserted.quota_samples += 1;
        } else {
            verify_immutable_hash(
                &transaction,
                "quota_snapshot_evidence",
                "sample_id",
                &evidence.sample_id,
                &checksum,
            )?;
        }
    }
    for diagnostic in diagnostics {
        diagnostic.validate()?;
        let checksum = canonical_hash(diagnostic)?;
        let changed = transaction
            .execute(
                "INSERT OR IGNORE INTO ingestion_diagnostics (diagnostic_id, source_id, observed_at, record_semantic, diagnostic_code, schema_version, evidence_sha256) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                params![
                    diagnostic.diagnostic_id,
                    diagnostic.source_id,
                    diagnostic.observed_at,
                    diagnostic.record_semantic,
                    diagnostic.diagnostic_code.as_str(),
                    checksum,
                ],
            )
            .map_err(StorageError::sqlite)?;
        if changed == 1 {
            inserted.diagnostics += 1;
        } else {
            verify_immutable_hash(
                &transaction,
                "ingestion_diagnostics",
                "diagnostic_id",
                &diagnostic.diagnostic_id,
                &checksum,
            )?;
        }
    }

    for observation in observations {
        let was_missing =
            load_observation_from_connection(&transaction, &observation.observation_id)?.is_none();
        save_observation_in_transaction(&transaction, observation, None)?;
        if was_missing {
            inserted.observations += 1;
        }
    }

    let previous_boundary = transaction
        .query_row(
            "SELECT committed_offset, last_line_sha256, file_identity_sha256 FROM source_boundary_fingerprints WHERE rollout_id = ?1 AND source_generation = ?2",
            params![source.rollout_id(), source.source_generation()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(StorageError::sqlite)?;
    if let Some((previous_offset, previous_hash, previous_file_identity)) = previous_boundary {
        let previous_offset = previous_offset
            .parse::<u64>()
            .map_err(|_| StorageError::CheckpointCursorInvalid)?;
        if boundary.committed_offset < previous_offset
            || (boundary.committed_offset == previous_offset
                && (boundary.last_line_sha256 != previous_hash
                    || boundary.file_identity_sha256 != previous_file_identity))
            || previous_file_identity
                .as_ref()
                .is_some_and(|identity| boundary.file_identity_sha256.as_ref() != Some(identity))
        {
            return Err(StorageError::SourceBoundaryConflict);
        }
    }
    transaction
        .execute(
            "INSERT INTO source_boundary_fingerprints (rollout_id, source_generation, committed_offset, last_line_sha256, file_identity_sha256) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (rollout_id, source_generation) DO UPDATE SET committed_offset = excluded.committed_offset, last_line_sha256 = excluded.last_line_sha256, file_identity_sha256 = excluded.file_identity_sha256",
            params![
                source.rollout_id(),
                source.source_generation(),
                boundary.committed_offset.to_string(),
                boundary.last_line_sha256,
                boundary.file_identity_sha256,
            ],
        )
        .map_err(StorageError::sqlite)?;
    let checkpoint = save_in_transaction_idempotent(
        &transaction,
        source,
        cursor,
        state,
        state_json,
        state_sha256,
        expected_checkpoint_revision,
    )?;
    transaction.commit().map_err(StorageError::sqlite)?;
    Ok((checkpoint, inserted))
}

fn verify_immutable_hash(
    transaction: &Transaction<'_>,
    table: &str,
    key_column: &str,
    key: &str,
    expected_hash: &str,
) -> Result<(), StorageError> {
    let query = format!("SELECT evidence_sha256 FROM {table} WHERE {key_column} = ?1");
    let actual: Option<String> = transaction
        .query_row(&query, [key], |row| row.get(0))
        .optional()
        .map_err(StorageError::sqlite)?;
    if actual.as_deref() == Some(expected_hash) {
        Ok(())
    } else {
        Err(StorageError::EvidenceConflict)
    }
}

pub(crate) fn canonical_hash<T: Serialize>(value: &T) -> Result<String, StorageError> {
    let encoded = serde_json::to_vec(value).map_err(|_| StorageError::EvidenceInvalid)?;
    Ok(Sha256::digest(encoded)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn sql_token(value: Option<u64>) -> Option<i64> {
    value.map(|value| i64::try_from(value).expect("source token count is validated"))
}

fn meter_name(value: QuotaMeterType) -> &'static str {
    match value {
        QuotaMeterType::FiveHour => "five_hour",
        QuotaMeterType::Weekly => "weekly",
    }
}

pub(crate) fn history_summary(
    connection: &Connection,
) -> Result<UsageHistorySummary, StorageError> {
    validate_evidence_checksums(connection)?;
    let response_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic = 'per_response' AND evidence_status = 'observed'",
            [],
            |row| row.get(0),
        )
        .map_err(StorageError::sqlite)?;
    let raw_total_known_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic = 'per_response' AND raw_total IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(StorageError::sqlite)?;
    let raw_total_missing_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic = 'per_response' AND raw_total IS NULL",
            [],
            |row| row.get(0),
        )
        .map_err(StorageError::sqlite)?;
    let mut raw_total_known_sum = Some(0_u64);
    let mut statement = connection
        .prepare("SELECT raw_total FROM token_usage_evidence WHERE record_semantic = 'per_response' AND raw_total IS NOT NULL")
        .map_err(StorageError::sqlite)?;
    let mut rows = statement.query([]).map_err(StorageError::sqlite)?;
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let raw_total: i64 = row.get(0).map_err(StorageError::sqlite)?;
        raw_total_known_sum = raw_total_known_sum.and_then(|sum| {
            u64::try_from(raw_total)
                .ok()
                .and_then(|value| sum.checked_add(value))
                .filter(|value| *value <= MAX_SAFE_INTEGER)
        });
    }
    if raw_total_known_count == 0 {
        raw_total_known_sum = None;
    }
    Ok(UsageHistorySummary {
        response_count: u64::try_from(response_count).map_err(|_| StorageError::EvidenceCorrupt)?,
        raw_total_known_count: u64::try_from(raw_total_known_count)
            .map_err(|_| StorageError::EvidenceCorrupt)?,
        raw_total_missing_count: u64::try_from(raw_total_missing_count)
            .map_err(|_| StorageError::EvidenceCorrupt)?,
        raw_total_known_sum,
        five_hour_latest: latest_quota(connection, "five_hour")?,
        weekly_latest: latest_quota(connection, "weekly")?,
    })
}

fn validate_evidence_checksums(connection: &Connection) -> Result<(), StorageError> {
    let mut statement = connection
        .prepare("SELECT event_id, source_id, session_id, task_id, observed_at, record_semantic, model, reasoning_level, service_tier, codex_version, input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total, evidence_sha256 FROM token_usage_evidence ORDER BY event_id")
        .map_err(StorageError::sqlite)?;
    let mut rows = statement.query([]).map_err(StorageError::sqlite)?;
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let record_semantic: String = row.get(5).map_err(StorageError::sqlite)?;
        let record_semantic = match record_semantic.as_str() {
            "per_response" => TokenRecordSemantic::PerResponse,
            "turn_cumulative_snapshot" => TokenRecordSemantic::TurnCumulativeSnapshot,
            "thread_cumulative_snapshot" => TokenRecordSemantic::ThreadCumulativeSnapshot,
            "token_count_total_snapshot" => TokenRecordSemantic::TokenCountTotalSnapshot,
            "token_count_latest_snapshot" => TokenRecordSemantic::TokenCountLatestSnapshot,
            _ => return Err(StorageError::EvidenceCorrupt),
        };
        let evidence = TokenUsageEvidence {
            event_id: row.get(0).map_err(StorageError::sqlite)?,
            source_id: row.get(1).map_err(StorageError::sqlite)?,
            session_id: row.get(2).map_err(StorageError::sqlite)?,
            task_id: row.get(3).map_err(StorageError::sqlite)?,
            observed_at: row.get(4).map_err(StorageError::sqlite)?,
            record_semantic,
            model: row.get(6).map_err(StorageError::sqlite)?,
            reasoning_level: row.get(7).map_err(StorageError::sqlite)?,
            service_tier: row.get(8).map_err(StorageError::sqlite)?,
            codex_version: row.get(9).map_err(StorageError::sqlite)?,
            input_tokens: from_sql_token(row.get(10).map_err(StorageError::sqlite)?)?,
            cached_input_tokens: from_sql_token(row.get(11).map_err(StorageError::sqlite)?)?,
            cache_write_input_tokens: from_sql_token(row.get(12).map_err(StorageError::sqlite)?)?,
            output_tokens: from_sql_token(row.get(13).map_err(StorageError::sqlite)?)?,
            reasoning_output_tokens: from_sql_token(row.get(14).map_err(StorageError::sqlite)?)?,
            raw_total: from_sql_token(row.get(15).map_err(StorageError::sqlite)?)?,
        };
        let expected_hash: String = row.get(16).map_err(StorageError::sqlite)?;
        evidence
            .validate()
            .map_err(|_| StorageError::EvidenceCorrupt)?;
        if canonical_hash(&evidence)? != expected_hash {
            return Err(StorageError::EvidenceCorrupt);
        }
    }

    let mut statement = connection
        .prepare("SELECT sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_sha256 FROM quota_snapshot_evidence ORDER BY sample_id")
        .map_err(StorageError::sqlite)?;
    let mut rows = statement.query([]).map_err(StorageError::sqlite)?;
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let meter_type: String = row.get(4).map_err(StorageError::sqlite)?;
        let meter_type = match meter_type.as_str() {
            "five_hour" => QuotaMeterType::FiveHour,
            "weekly" => QuotaMeterType::Weekly,
            _ => return Err(StorageError::EvidenceCorrupt),
        };
        let evidence = QuotaSnapshotEvidence {
            sample_id: row.get(0).map_err(StorageError::sqlite)?,
            source_id: row.get(1).map_err(StorageError::sqlite)?,
            session_id: row.get(2).map_err(StorageError::sqlite)?,
            observed_at: row.get(3).map_err(StorageError::sqlite)?,
            meter_type,
            used_percent: row.get(5).map_err(StorageError::sqlite)?,
            reset_at: row.get(6).map_err(StorageError::sqlite)?,
            plan: row.get(7).map_err(StorageError::sqlite)?,
            codex_version: row.get(8).map_err(StorageError::sqlite)?,
        };
        let expected_hash: String = row.get(9).map_err(StorageError::sqlite)?;
        evidence
            .validate()
            .map_err(|_| StorageError::EvidenceCorrupt)?;
        if canonical_hash(&evidence)? != expected_hash {
            return Err(StorageError::EvidenceCorrupt);
        }
    }

    let mut statement = connection
        .prepare("SELECT diagnostic_id, source_id, observed_at, record_semantic, diagnostic_code, evidence_sha256 FROM ingestion_diagnostics ORDER BY diagnostic_id")
        .map_err(StorageError::sqlite)?;
    let mut rows = statement.query([]).map_err(StorageError::sqlite)?;
    while let Some(row) = rows.next().map_err(StorageError::sqlite)? {
        let diagnostic_code: String = row.get(4).map_err(StorageError::sqlite)?;
        let diagnostic = IngestionDiagnostic {
            diagnostic_id: row.get(0).map_err(StorageError::sqlite)?,
            source_id: row.get(1).map_err(StorageError::sqlite)?,
            observed_at: row.get(2).map_err(StorageError::sqlite)?,
            record_semantic: row.get(3).map_err(StorageError::sqlite)?,
            diagnostic_code: IngestionDiagnosticCode::parse(&diagnostic_code)
                .ok_or(StorageError::EvidenceCorrupt)?,
        };
        let expected_hash: String = row.get(5).map_err(StorageError::sqlite)?;
        diagnostic
            .validate()
            .map_err(|_| StorageError::EvidenceCorrupt)?;
        if canonical_hash(&diagnostic)? != expected_hash {
            return Err(StorageError::EvidenceCorrupt);
        }
    }
    Ok(())
}

fn latest_quota(
    connection: &rusqlite::Connection,
    meter_type: &str,
) -> Result<Option<StoredQuotaSnapshot>, StorageError> {
    connection
        .query_row(
            "SELECT sample_id, used_percent, observed_at, reset_at FROM quota_snapshot_evidence WHERE meter_type = ?1 ORDER BY observed_at DESC, sample_id DESC LIMIT 1",
            [meter_type],
            |row| {
                Ok(StoredQuotaSnapshot {
                    sample_id: row.get(0)?,
                    used_percent: row.get(1)?,
                    observed_at: row.get(2)?,
                    reset_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StorageError::sqlite)
}

impl fmt::Display for IngestionDiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
