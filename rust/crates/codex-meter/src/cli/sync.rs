//! One-shot native ingestion of privacy-filtered local Codex rollout evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::storage::{
    IngestionDiagnostic, IngestionDiagnosticCode, QuotaSnapshotEvidence, RuntimeRecoveryState,
    SourceBoundaryFingerprint, SqliteStore, StorageError, TokenRecordSemantic, TokenUsageEvidence,
    UsageEvidenceBatch, UsageHistorySummary,
};
use crate::telemetry::source_discovery::{
    discover_local_rollouts, file_identity_sha256, open_verified_rollout,
    verify_committed_boundary, BoundaryCheck, DiscoveredRollout, DiscoveryResult, SourceOpenError,
};
use crate::telemetry::{
    assemble_batch, begin_task_quota_reconciliation, build_observation, extract_token_evidence,
    normalize_quota_item, track_quota_samples, AssembledOutput, AttributedTokenEvent,
    ConfigurationIdentity, ConfigurationValue, EventMessage, IsolationEvidence,
    NormalizedEventType, NormalizedObservation, NormalizedTokenEvent, NormalizedTokenPayload,
    ObservationInput, QuotaNormalizationOutcome, ReadBatch, ReadItem, ReadItemOutcome,
    ReconciliationPolicy, RejectedLineReason, RolloutCursor, RolloutRecordKind, SchemaVersion,
    SourceIdentity, TaskLifecycleTransitionKind, TaskReconciliationTarget, TokenEvidenceExtraction,
    TokenMetric, TokenPayloadKind, TokenTelemetryCompleteness, TokenUsage,
    COMPATIBILITY_CLI_VERSION,
};

use super::args::{CliError, SyncArguments};

const MAX_SOURCE_BYTES_PER_SYNC: u64 = 128 * 1024 * 1024;
const MAX_SOURCE_RECORDS_PER_SYNC: usize = 100_000;
const DEFAULT_DATABASE_NAME: &str = "codex-meter.sqlite3";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SyncStatus {
    Completed,
    CompletedWithWarnings,
    CompletedNoNewRecords,
    NoCodexHome,
    ExplicitCodexHomeUnavailable,
    CodexHomeWithoutSessions,
    NoSupportedRollouts,
    UnsupportedCodexVersion,
    NoSupportedTokenEvents,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
enum SyncWarning {
    CodexCliNotDetected,
    CompressedSourcesUnsupported,
    IncompleteSourceMetadata,
    MalformedOrUnsupportedSources,
    UnsupportedCodexVersion,
    UnreadableSource,
    SymlinkEntriesSkipped,
    DiscoveryScanLimited,
    SourceChanged,
    SourceGenerationAmbiguous,
    AmbiguousSourceIdentity,
    SourceTruncated,
    CheckpointAmbiguous,
    CheckpointCorruptOrUnsupported,
    SourceReadLimited,
    SourceReadFailed,
    SourceTransactionConflict,
    QuotaEvidenceIncomplete,
    DataCoverageIncomplete,
    DiagnosticWriteFailed,
}

#[derive(Clone, Debug, Serialize)]
struct MeterSummary {
    used_percent: f64,
    observed_at: String,
    reset_at: Option<String>,
    observed_during_this_sync: bool,
}

#[derive(Clone, Debug, Serialize)]
struct SyncSummary {
    status: SyncStatus,
    source: &'static str,
    supported_codex_cli_version: &'static str,
    codex_cli_detected: bool,
    codex_home_found: bool,
    sessions_seen: usize,
    rollout_sources_seen: usize,
    rollout_sources_processed: usize,
    responses_added: usize,
    snapshots_added: usize,
    quota_snapshots_added: usize,
    observations_added: usize,
    saved_response_events: u64,
    saved_raw_total_known_sum: Option<u64>,
    saved_raw_total_known_events: u64,
    saved_raw_total_missing_events: u64,
    five_hour_meter: Option<MeterSummary>,
    weekly_meter: Option<MeterSummary>,
    skipped_records: usize,
    unsupported_compressed_sources: usize,
    unsupported_version_sources: usize,
    unsupported_or_incomplete_sources: usize,
    ambiguous_source_identities: usize,
    unreadable_sources: usize,
    symlink_entries_skipped: usize,
    scan_limited: bool,
    history_saved: bool,
    database_migration_version: Option<u32>,
    warnings: Vec<SyncWarning>,
}

#[derive(Default)]
struct SourceCounts {
    responses_added: usize,
    snapshots_added: usize,
    quota_snapshots_added: usize,
    observations_added: usize,
    skipped_records: usize,
    source_processed: bool,
    source_warning: Option<SyncWarning>,
    quota_this_sync: BTreeSet<String>,
}

pub(super) fn run(arguments: SyncArguments) -> Result<String, CliError> {
    let discovery = discover_local_rollouts(arguments.codex_home, std::env::var_os("CODEX_HOME"));
    let mut warnings = discovery_warnings(&discovery);
    let mut summary = base_summary(&discovery, &warnings);
    if discovery.explicit_home_invalid {
        summary.status = SyncStatus::ExplicitCodexHomeUnavailable;
        return render_summary(&summary, arguments.json);
    }
    if discovery.codex_home.is_none() {
        summary.status = SyncStatus::NoCodexHome;
        return render_summary(&summary, arguments.json);
    }

    let use_default_database = arguments.database.is_none();
    let database_path = match arguments.database {
        Some(path) => path,
        None => match default_database_path() {
            Ok(path) => path,
            Err(_) => {
                return Err(CliError {
                    code: "application_data_directory_unavailable",
                    message: "could not resolve the per-user Codex Meter data directory".into(),
                });
            }
        },
    };
    if use_default_database {
        let directory = database_path.parent().ok_or_else(|| CliError {
            code: "application_data_directory_unavailable",
            message: "could not resolve the per-user Codex Meter data directory".into(),
        })?;
        prepare_private_data_directory(directory)?;
        prepare_private_database_file(&database_path)?;
    }

    let mut store = SqliteStore::open(&database_path).map_err(storage_cli_error)?;
    summary.history_saved = true;
    summary.database_migration_version = Some(store.info().latest_applied_migration);
    if persist_discovery_diagnostics(&mut store, &discovery).is_err() {
        warnings.insert(SyncWarning::DiagnosticWriteFailed);
    }

    if discovery.rollouts.is_empty() {
        summary.status = if discovery.counts.unsupported_version_sources > 0 {
            SyncStatus::UnsupportedCodexVersion
        } else if discovery.source_files_seen == 0 {
            SyncStatus::CodexHomeWithoutSessions
        } else {
            SyncStatus::NoSupportedRollouts
        };
        if !discovery.roots_present || discovery.source_files_seen > 0 {
            warnings.insert(SyncWarning::DataCoverageIncomplete);
        }
        summary.warnings = warnings.into_iter().collect();
        let history = store.usage_history_summary().map_err(storage_cli_error)?;
        apply_history(&mut summary, &history, &BTreeSet::new());
        return render_summary(&summary, arguments.json);
    }

    let mut added_response_count = 0_usize;
    let mut added_snapshot_count = 0_usize;
    let mut added_quota_count = 0_usize;
    let mut added_observation_count = 0_usize;
    let mut skipped_records = 0_usize;
    let mut processed_sources = 0_usize;
    let mut quota_samples_this_sync = BTreeSet::new();

    for rollout in &discovery.rollouts {
        let counts = ingest_rollout(&mut store, rollout);
        added_response_count += counts.responses_added;
        added_snapshot_count += counts.snapshots_added;
        added_quota_count += counts.quota_snapshots_added;
        added_observation_count += counts.observations_added;
        skipped_records += counts.skipped_records;
        quota_samples_this_sync.extend(counts.quota_this_sync);
        if counts.source_processed {
            processed_sources += 1;
        }
        if let Some(warning) = counts.source_warning {
            if let Some(diagnostic) = source_warning_diagnostic(rollout, warning) {
                if store.save_ingestion_diagnostic(&diagnostic).is_err() {
                    warnings.insert(SyncWarning::DiagnosticWriteFailed);
                }
            }
            warnings.insert(warning);
        }
    }

    let history = store.usage_history_summary().map_err(storage_cli_error)?;
    apply_history(&mut summary, &history, &quota_samples_this_sync);
    summary.sessions_seen = discovery.sessions_seen;
    summary.rollout_sources_seen = discovery.rollouts.len();
    summary.rollout_sources_processed = processed_sources;
    summary.responses_added = added_response_count;
    summary.snapshots_added = added_snapshot_count;
    summary.quota_snapshots_added = added_quota_count;
    summary.observations_added = added_observation_count;
    summary.skipped_records = skipped_records;

    if !codex_cli_detected() {
        warnings.insert(SyncWarning::CodexCliNotDetected);
    }
    if discovery.counts.compressed_sources_skipped > 0 {
        warnings.insert(SyncWarning::CompressedSourcesUnsupported);
    }
    if discovery.counts.incomplete_sources > 0 {
        warnings.insert(SyncWarning::IncompleteSourceMetadata);
    }
    if discovery.counts.unsupported_sources > 0 {
        warnings.insert(SyncWarning::MalformedOrUnsupportedSources);
    }
    if discovery.counts.unsupported_version_sources > 0 {
        warnings.insert(SyncWarning::UnsupportedCodexVersion);
    }
    if discovery.counts.unreadable_sources > 0 || discovery.counts.unreadable_directories > 0 {
        warnings.insert(SyncWarning::UnreadableSource);
    }
    if discovery.counts.symlink_entries_skipped > 0 {
        warnings.insert(SyncWarning::SymlinkEntriesSkipped);
    }
    if discovery.counts.scan_limited {
        warnings.insert(SyncWarning::DiscoveryScanLimited);
    }
    if discovery.counts.ambiguous_source_identities > 0 {
        warnings.insert(SyncWarning::AmbiguousSourceIdentity);
    }
    if added_quota_count == 0
        && history.five_hour_latest.is_none()
        && history.weekly_latest.is_none()
    {
        warnings.insert(SyncWarning::QuotaEvidenceIncomplete);
    }
    if discovery.counts.compressed_sources_skipped > 0
        || discovery.counts.unsupported_version_sources > 0
        || discovery.counts.unsupported_sources > 0
        || discovery.counts.incomplete_sources > 0
        || discovery.counts.ambiguous_source_identities > 0
        || discovery.counts.scan_limited
        || processed_sources < discovery.rollouts.len()
    {
        warnings.insert(SyncWarning::DataCoverageIncomplete);
    }

    summary.status = if !warnings.is_empty() {
        SyncStatus::CompletedWithWarnings
    } else if history.response_count == 0 {
        SyncStatus::NoSupportedTokenEvents
    } else if added_response_count == 0 {
        SyncStatus::CompletedNoNewRecords
    } else {
        SyncStatus::Completed
    };
    summary.warnings = warnings.into_iter().collect();
    render_summary(&summary, arguments.json)
}

fn base_summary(discovery: &DiscoveryResult, warnings: &BTreeSet<SyncWarning>) -> SyncSummary {
    let mut warnings = warnings.clone();
    if !codex_cli_detected() {
        warnings.insert(SyncWarning::CodexCliNotDetected);
    }
    SyncSummary {
        status: SyncStatus::NoCodexHome,
        source: "local_codex",
        supported_codex_cli_version: COMPATIBILITY_CLI_VERSION,
        codex_cli_detected: codex_cli_detected(),
        codex_home_found: discovery.codex_home.is_some(),
        sessions_seen: discovery.sessions_seen,
        rollout_sources_seen: discovery.rollouts.len(),
        rollout_sources_processed: 0,
        responses_added: 0,
        snapshots_added: 0,
        quota_snapshots_added: 0,
        observations_added: 0,
        saved_response_events: 0,
        saved_raw_total_known_sum: None,
        saved_raw_total_known_events: 0,
        saved_raw_total_missing_events: 0,
        five_hour_meter: None,
        weekly_meter: None,
        skipped_records: 0,
        unsupported_compressed_sources: discovery.counts.compressed_sources_skipped,
        unsupported_version_sources: discovery.counts.unsupported_version_sources,
        unsupported_or_incomplete_sources: discovery.counts.unsupported_sources
            + discovery.counts.incomplete_sources,
        ambiguous_source_identities: discovery.counts.ambiguous_source_identities,
        unreadable_sources: discovery.counts.unreadable_sources
            + discovery.counts.unreadable_directories,
        symlink_entries_skipped: discovery.counts.symlink_entries_skipped,
        scan_limited: discovery.counts.scan_limited,
        history_saved: false,
        database_migration_version: None,
        warnings: warnings.into_iter().collect(),
    }
}

fn discovery_warnings(discovery: &DiscoveryResult) -> BTreeSet<SyncWarning> {
    let mut warnings = BTreeSet::new();
    if discovery.counts.compressed_sources_skipped > 0 {
        warnings.insert(SyncWarning::CompressedSourcesUnsupported);
    }
    if discovery.counts.incomplete_sources > 0 {
        warnings.insert(SyncWarning::IncompleteSourceMetadata);
    }
    if discovery.counts.unsupported_sources > 0 {
        warnings.insert(SyncWarning::MalformedOrUnsupportedSources);
    }
    if discovery.counts.unsupported_version_sources > 0 {
        warnings.insert(SyncWarning::UnsupportedCodexVersion);
    }
    if discovery.counts.unreadable_sources > 0 || discovery.counts.unreadable_directories > 0 {
        warnings.insert(SyncWarning::UnreadableSource);
    }
    if discovery.counts.symlink_entries_skipped > 0 {
        warnings.insert(SyncWarning::SymlinkEntriesSkipped);
    }
    if discovery.counts.scan_limited {
        warnings.insert(SyncWarning::DiscoveryScanLimited);
    }
    if discovery.counts.ambiguous_source_identities > 0 {
        warnings.insert(SyncWarning::AmbiguousSourceIdentity);
    }
    warnings
}

fn ingest_rollout(store: &mut SqliteStore, rollout: &DiscoveredRollout) -> SourceCounts {
    let mut counts = SourceCounts::default();
    let mut file = match open_verified_rollout(rollout) {
        Ok(file) => file,
        Err(SourceOpenError::Unreadable) => {
            counts.source_warning = Some(SyncWarning::UnreadableSource);
            return counts;
        }
        Err(SourceOpenError::IdentityChanged) => {
            counts.source_warning = Some(SyncWarning::SourceChanged);
            return counts;
        }
    };

    match store.has_other_rollout_generation(&rollout.source) {
        Ok(false) => {}
        Ok(true) => {
            counts.source_warning = Some(SyncWarning::SourceGenerationAmbiguous);
            return counts;
        }
        Err(_) => {
            counts.source_warning = Some(SyncWarning::CheckpointCorruptOrUnsupported);
            return counts;
        }
    }

    let mut checkpoint = match store.load_runtime_checkpoint(&rollout.source) {
        Ok(checkpoint) => checkpoint,
        Err(_) => {
            counts.source_warning = Some(SyncWarning::CheckpointCorruptOrUnsupported);
            return counts;
        }
    };
    let boundary = match store.load_source_boundary(&rollout.source) {
        Ok(boundary) => boundary,
        Err(_) => {
            counts.source_warning = Some(SyncWarning::CheckpointCorruptOrUnsupported);
            return counts;
        }
    };

    let mut cursor = checkpoint.as_ref().map_or_else(
        || RolloutCursor::at_start(&rollout.source),
        |saved| saved.cursor.clone(),
    );
    let mut state = checkpoint
        .as_ref()
        .map_or_else(RuntimeRecoveryState::default, |saved| saved.state.clone());
    let mut boundary_hash = boundary
        .as_ref()
        .and_then(|value| value.last_line_sha256.clone());
    let current_file_identity = file_identity_sha256(&file);
    if let Some(saved) = &checkpoint {
        if let Some(boundary) = boundary.as_ref() {
            if boundary.committed_offset != saved.cursor.committed_offset() {
                counts.source_warning = Some(SyncWarning::CheckpointAmbiguous);
                return counts;
            }
            if boundary
                .file_identity_sha256
                .as_ref()
                .is_some_and(|identity| current_file_identity.as_ref() != Some(identity))
            {
                counts.source_warning = Some(SyncWarning::SourceChanged);
                return counts;
            }
            match verify_committed_boundary(
                &mut file,
                saved.cursor.committed_offset(),
                boundary.last_line_sha256.as_deref(),
            ) {
                BoundaryCheck::Matches => {}
                BoundaryCheck::MissingFingerprint => {
                    counts.source_warning = Some(SyncWarning::CheckpointAmbiguous);
                    return counts;
                }
                BoundaryCheck::Changed | BoundaryCheck::Unreadable => {
                    counts.source_warning = Some(SyncWarning::SourceChanged);
                    return counts;
                }
                BoundaryCheck::Truncated => {
                    counts.source_warning = Some(SyncWarning::SourceTruncated);
                    return counts;
                }
            }
        } else if saved.cursor.committed_offset() > 0 {
            counts.source_warning = Some(SyncWarning::CheckpointAmbiguous);
            return counts;
        } else {
            boundary_hash = None;
        }
    } else if boundary.is_some() {
        counts.source_warning = Some(SyncWarning::CheckpointAmbiguous);
        return counts;
    }

    if checkpoint.is_none() {
        let initial_boundary = SourceBoundaryFingerprint {
            committed_offset: 0,
            last_line_sha256: None,
            file_identity_sha256: current_file_identity.clone(),
        };
        let initial = store.commit_usage_batch(
            &rollout.source,
            &cursor,
            &state,
            None,
            UsageEvidenceBatch {
                tokens: &[],
                quotas: &[],
                diagnostics: &[],
                observations: &[],
                boundary: &initial_boundary,
            },
        );
        match initial {
            Ok((saved, _)) => checkpoint = Some(saved),
            Err(_) => {
                counts.source_warning = Some(SyncWarning::SourceTransactionConflict);
                return counts;
            }
        }
    }
    let expected_revision = checkpoint.as_ref().map(|value| value.revision);
    let mut expected_revision = expected_revision;
    let mut bytes_scanned = 0_u64;
    let mut records_scanned = 0_usize;
    counts.source_processed = true;

    loop {
        let previous_cursor = cursor.clone();
        let read = match crate::telemetry::read_available(&mut file, &rollout.source, &cursor) {
            Ok(read) => read,
            Err(_) => {
                counts.source_warning = Some(SyncWarning::SourceReadFailed);
                break;
            }
        };
        if read.items.is_empty() {
            break;
        }
        let mut next_state = state.clone();
        let mut token_evidence = Vec::new();
        let mut quota_evidence = Vec::new();
        let mut diagnostics = Vec::new();
        let mut quota_samples = Vec::new();
        let mut terminal_tasks = Vec::new();

        for item in &read.items {
            let Some(record) = decoded_record(item) else {
                if let Some(diagnostic) = diagnostic_for_rejected(&rollout.source, item) {
                    if !is_quota_diagnostic(diagnostic.diagnostic_code) {
                        counts.skipped_records += 1;
                    }
                    diagnostics.push(diagnostic);
                }
                continue;
            };
            let semantic = record_semantic(&record.kind);
            let timestamp =
                crate::telemetry::token_normalization::validate_utc_timestamp(&record.timestamp)
                    .ok()
                    .map(|_| record.timestamp.clone());

            let one_item = ReadBatch {
                items: vec![item.clone()],
                next_cursor: read.next_cursor.clone(),
                has_incomplete_tail: false,
                last_line_sha256: None,
            };
            let assembled =
                match assemble_batch(&next_state.telemetry_state, &rollout.source, &one_item) {
                    Ok(assembled) => assembled,
                    Err(error) => {
                        let code = match error {
                            crate::telemetry::TelemetryAssemblyError::InvalidTimestamp(_) => {
                                IngestionDiagnosticCode::InvalidTimestamp
                            }
                            crate::telemetry::TelemetryAssemblyError::SessionIdentityConflict => {
                                IngestionDiagnosticCode::SessionIdentityConflict
                            }
                            _ => IngestionDiagnosticCode::MalformedRecord,
                        };
                        diagnostics.push(make_diagnostic(
                            &rollout.source,
                            item,
                            source_id(&rollout.source),
                            timestamp,
                            semantic,
                            code,
                        ));
                        counts.skipped_records += 1;
                        continue;
                    }
                };
            for output in &assembled.outputs {
                if let AssembledOutput::TaskLifecycle(transition) = output {
                    if matches!(
                        transition.transition,
                        TaskLifecycleTransitionKind::Completed
                            | TaskLifecycleTransitionKind::Failed
                            | TaskLifecycleTransitionKind::Aborted
                    ) {
                        if let Some(task_id) = transition.task_id.as_ref() {
                            terminal_tasks.push((
                                task_id.clone(),
                                transition.event_at.clone(),
                                transition.transition,
                            ));
                        }
                    }
                }
            }
            next_state.telemetry_state = assembled.next_state;

            if let Some(diagnostic) = diagnostic_for_unsupported(
                &rollout.source,
                item,
                source_id(&rollout.source),
                timestamp.clone(),
                semantic,
            ) {
                counts.skipped_records += 1;
                diagnostics.push(diagnostic);
            }

            let extracted = extract_token_evidence(&record);
            match extracted {
                TokenEvidenceExtraction::PerResponse { evidence, .. } => {
                    let Some(attributed) =
                        assembled.outputs.iter().find_map(|output| match output {
                            AssembledOutput::AttributedToken(event) => Some(event),
                            _ => None,
                        })
                    else {
                        diagnostics.push(make_diagnostic(
                            &rollout.source,
                            item,
                            source_id(&rollout.source),
                            timestamp.clone(),
                            semantic,
                            IngestionDiagnosticCode::MalformedRecord,
                        ));
                        counts.skipped_records += 1;
                        continue;
                    };
                    let response = evidence
                        .per_response
                        .as_ref()
                        .expect("response evidence is present");
                    token_evidence.push(token_row(
                        TokenRowIdentity {
                            event_id: attributed.normalized_event.event_id.clone(),
                            source_id: attributed.normalized_event.source_instance_id.clone(),
                            session_id: attributed.normalized_event.session_id.clone(),
                            task_id: attributed.normalized_event.task_id.clone(),
                            observed_at: record.timestamp.clone(),
                            record_semantic: TokenRecordSemantic::PerResponse,
                            model: attributed.effective_configuration.model.value.clone(),
                            reasoning_level: attributed
                                .effective_configuration
                                .reasoning_level
                                .value
                                .clone(),
                            service_tier: next_state
                                .telemetry_state
                                .thread_configuration
                                .service_tier
                                .clone(),
                            codex_version: attributed
                                .effective_configuration
                                .codex_version
                                .value
                                .clone(),
                        },
                        &response.usage,
                    ));
                    for snapshot in [
                        evidence.turn_cumulative.as_ref(),
                        evidence.thread_cumulative.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        let semantic = match snapshot.semantic {
                            crate::telemetry::TokenEvidenceSemantic::TurnCumulativeSnapshot => {
                                TokenRecordSemantic::TurnCumulativeSnapshot
                            }
                            crate::telemetry::TokenEvidenceSemantic::ThreadCumulativeSnapshot => {
                                TokenRecordSemantic::ThreadCumulativeSnapshot
                            }
                            _ => continue,
                        };
                        token_evidence.push(token_row(
                            TokenRowIdentity {
                                event_id: snapshot_event_id(&rollout.source, item, semantic),
                                source_id: attributed.normalized_event.source_instance_id.clone(),
                                session_id: attributed.normalized_event.session_id.clone(),
                                task_id: attributed.normalized_event.task_id.clone(),
                                observed_at: record.timestamp.clone(),
                                record_semantic: semantic,
                                model: attributed.effective_configuration.model.value.clone(),
                                reasoning_level: attributed
                                    .effective_configuration
                                    .reasoning_level
                                    .value
                                    .clone(),
                                service_tier: next_state
                                    .telemetry_state
                                    .thread_configuration
                                    .service_tier
                                    .clone(),
                                codex_version: attributed
                                    .effective_configuration
                                    .codex_version
                                    .value
                                    .clone(),
                            },
                            &snapshot.usage,
                        ));
                    }
                }
                TokenEvidenceExtraction::TokenCountSnapshot { evidence } => {
                    let session_id = next_state
                        .telemetry_state
                        .session
                        .as_ref()
                        .map(|session| session.session_id.clone());
                    for snapshot in [
                        evidence.total_snapshot.as_ref(),
                        evidence.latest_snapshot.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        let semantic = match snapshot.semantic {
                            crate::telemetry::TokenEvidenceSemantic::TokenCountTotalSnapshot => {
                                TokenRecordSemantic::TokenCountTotalSnapshot
                            }
                            crate::telemetry::TokenEvidenceSemantic::TokenCountLatestSnapshot => {
                                TokenRecordSemantic::TokenCountLatestSnapshot
                            }
                            _ => continue,
                        };
                        token_evidence.push(token_row(
                            TokenRowIdentity {
                                event_id: snapshot_event_id(&rollout.source, item, semantic),
                                source_id: source_id(&rollout.source),
                                session_id: session_id.clone(),
                                task_id: None,
                                observed_at: record.timestamp.clone(),
                                record_semantic: semantic,
                                model: None,
                                reasoning_level: None,
                                service_tier: next_state
                                    .telemetry_state
                                    .thread_configuration
                                    .service_tier
                                    .clone(),
                                codex_version: Some(rollout.cli_version.clone()),
                            },
                            &snapshot.usage,
                        ));
                    }
                }
                TokenEvidenceExtraction::NotTokenBearing => {}
            }

            let quota_outcome = match normalize_quota_item(&rollout.source, item) {
                Ok(outcome) => outcome,
                Err(_) => {
                    diagnostics.push(make_diagnostic(
                        &rollout.source,
                        item,
                        source_id(&rollout.source),
                        timestamp.clone(),
                        "token_count_rate_limits",
                        IngestionDiagnosticCode::InvalidTimestamp,
                    ));
                    continue;
                }
            };
            match quota_outcome {
                QuotaNormalizationOutcome::Samples(samples) => {
                    for sample in samples {
                        let used_percent = sample.used_percent.value.unwrap_or(f64::NAN);
                        let reset_at = match &sample.reset_evidence {
                            crate::telemetry::QuotaWindowIdentity::Unavailable => None,
                            crate::telemetry::QuotaWindowIdentity::Available {
                                observed_reset_at,
                                ..
                            } => observed_reset_at.clone(),
                        };
                        let session_id = next_state
                            .telemetry_state
                            .session
                            .as_ref()
                            .map(|session| session.session_id.clone());
                        quota_samples.push(sample.clone());
                        counts.quota_this_sync.insert(sample.sample_id.clone());
                        quota_evidence.push(QuotaSnapshotEvidence {
                            sample_id: sample.sample_id,
                            source_id: source_id(&rollout.source),
                            session_id,
                            observed_at: sample.sampled_at,
                            meter_type: sample.meter_type,
                            used_percent,
                            reset_at,
                            plan: sample.configuration.plan.value,
                            codex_version: Some(rollout.cli_version.clone()),
                        });
                    }
                }
                QuotaNormalizationOutcome::NoQuotaEvidence => {
                    if matches!(
                        record.kind,
                        RolloutRecordKind::EventMsg(EventMessage::TokenCount(_))
                    ) {
                        diagnostics.push(make_diagnostic(
                            &rollout.source,
                            item,
                            source_id(&rollout.source),
                            timestamp.clone(),
                            "token_count_rate_limits",
                            IngestionDiagnosticCode::QuotaUnavailable,
                        ));
                    }
                }
                QuotaNormalizationOutcome::NonMainLimit => diagnostics.push(make_diagnostic(
                    &rollout.source,
                    item,
                    source_id(&rollout.source),
                    timestamp.clone(),
                    "token_count_rate_limits",
                    IngestionDiagnosticCode::QuotaNonMainLimit,
                )),
                QuotaNormalizationOutcome::UnsupportedWindows => diagnostics.push(make_diagnostic(
                    &rollout.source,
                    item,
                    source_id(&rollout.source),
                    timestamp.clone(),
                    "token_count_rate_limits",
                    IngestionDiagnosticCode::QuotaUnsupportedWindow,
                )),
                QuotaNormalizationOutcome::AmbiguousMeter { .. } => {
                    diagnostics.push(make_diagnostic(
                        &rollout.source,
                        item,
                        source_id(&rollout.source),
                        timestamp.clone(),
                        "token_count_rate_limits",
                        IngestionDiagnosticCode::QuotaAmbiguousWindow,
                    ))
                }
                QuotaNormalizationOutcome::RejectedSourceLine { .. } => {}
            }
        }

        if !quota_samples.is_empty() {
            match track_quota_samples(&next_state.quota_tracking_state, &quota_samples) {
                Ok(batch) => next_state.quota_tracking_state = batch.next_state,
                Err(_) => {
                    counts.source_warning = Some(SyncWarning::QuotaEvidenceIncomplete);
                    break;
                }
            }
        }

        let next_cursor = read.next_cursor.clone();
        let last_line_sha256 = read
            .last_line_sha256
            .clone()
            .or_else(|| boundary_hash.clone());
        let next_boundary = SourceBoundaryFingerprint {
            committed_offset: next_cursor.committed_offset(),
            last_line_sha256,
            file_identity_sha256: current_file_identity.clone(),
        };
        let observations =
            match build_terminal_observations(store, &terminal_tasks, &token_evidence, &next_state)
            {
                Ok(observations) => observations,
                Err(_) => {
                    counts.source_warning = Some(SyncWarning::CheckpointCorruptOrUnsupported);
                    break;
                }
            };
        match store.commit_usage_batch(
            &rollout.source,
            &next_cursor,
            &next_state,
            expected_revision,
            UsageEvidenceBatch {
                tokens: &token_evidence,
                quotas: &quota_evidence,
                diagnostics: &diagnostics,
                observations: &observations,
                boundary: &next_boundary,
            },
        ) {
            Ok((saved, inserted)) => {
                counts.responses_added += inserted.responses;
                counts.snapshots_added += inserted.snapshots;
                counts.quota_snapshots_added += inserted.quota_samples;
                counts.observations_added += inserted.observations;
                state = next_state;
                cursor = saved.cursor.clone();
                boundary_hash = next_boundary.last_line_sha256;
                expected_revision = Some(saved.revision);
                records_scanned += read.items.len();
                bytes_scanned = bytes_scanned.saturating_add(
                    cursor.committed_offset().saturating_sub(
                        checkpoint
                            .as_ref()
                            .map_or(0, |saved| saved.cursor.committed_offset()),
                    ),
                );
                checkpoint = Some(saved);
            }
            Err(_) => {
                counts.source_warning = Some(SyncWarning::SourceTransactionConflict);
                break;
            }
        }
        if records_scanned >= MAX_SOURCE_RECORDS_PER_SYNC
            || bytes_scanned >= MAX_SOURCE_BYTES_PER_SYNC
        {
            counts.source_warning = Some(SyncWarning::SourceReadLimited);
            break;
        }
        if read.has_incomplete_tail || cursor == previous_cursor {
            break;
        }
    }
    counts
}

fn decoded_record(item: &ReadItem) -> Option<crate::telemetry::RolloutRecord> {
    match &item.outcome {
        ReadItemOutcome::Decoded(record) => Some(record.as_ref().clone()),
        ReadItemOutcome::Rejected(_) => None,
    }
}

fn diagnostic_for_rejected(
    source: &SourceIdentity,
    item: &ReadItem,
) -> Option<IngestionDiagnostic> {
    let ReadItemOutcome::Rejected(rejected) = &item.outcome else {
        return None;
    };
    let (code, semantic) = match &rejected.reason {
        RejectedLineReason::EmptyLine => (IngestionDiagnosticCode::EmptyRecord, "empty_record"),
        RejectedLineReason::InvalidUtf8 => {
            (IngestionDiagnosticCode::InvalidUtf8, "malformed_record")
        }
        RejectedLineReason::Decode(_) => {
            (IngestionDiagnosticCode::MalformedRecord, "malformed_record")
        }
    };
    Some(make_diagnostic(
        source,
        item,
        source_id(source),
        None,
        semantic,
        code,
    ))
}

fn diagnostic_for_unsupported(
    source: &SourceIdentity,
    item: &ReadItem,
    source_id: String,
    observed_at: Option<String>,
    semantic: &str,
) -> Option<IngestionDiagnostic> {
    let record = decoded_record(item)?;
    match record.kind {
        RolloutRecordKind::Unsupported(_) => Some(make_diagnostic(
            source,
            item,
            source_id,
            observed_at,
            "unsupported_record",
            IngestionDiagnosticCode::UnsupportedRecord,
        )),
        RolloutRecordKind::EventMsg(EventMessage::Unsupported { .. }) => Some(make_diagnostic(
            source,
            item,
            source_id,
            observed_at,
            semantic,
            IngestionDiagnosticCode::UnsupportedEvent,
        )),
        _ => None,
    }
}

fn make_diagnostic(
    source: &SourceIdentity,
    item: &ReadItem,
    source_id: String,
    observed_at: Option<String>,
    record_semantic: &str,
    diagnostic_code: IngestionDiagnosticCode,
) -> IngestionDiagnostic {
    IngestionDiagnostic {
        diagnostic_id: crate::telemetry::identity::diagnostic_id(
            source,
            item.start_offset,
            item.end_offset,
            item.ordinal,
            diagnostic_code.as_str(),
        ),
        source_id,
        observed_at,
        record_semantic: record_semantic.to_owned(),
        diagnostic_code,
    }
}

struct TokenRowIdentity {
    event_id: String,
    source_id: String,
    session_id: Option<String>,
    task_id: Option<String>,
    observed_at: String,
    record_semantic: TokenRecordSemantic,
    model: Option<String>,
    reasoning_level: Option<String>,
    service_tier: Option<String>,
    codex_version: Option<String>,
}

fn token_row(identity: TokenRowIdentity, usage: &TokenUsage) -> TokenUsageEvidence {
    TokenUsageEvidence {
        event_id: identity.event_id,
        source_id: identity.source_id,
        session_id: identity.session_id,
        task_id: identity.task_id,
        observed_at: identity.observed_at,
        record_semantic: identity.record_semantic,
        model: identity.model,
        reasoning_level: identity.reasoning_level,
        service_tier: identity.service_tier,
        codex_version: identity.codex_version,
        input_tokens: usage.input_tokens.map(crate::telemetry::TokenCount::get),
        cached_input_tokens: usage
            .cached_input_tokens
            .map(crate::telemetry::TokenCount::get),
        cache_write_input_tokens: usage
            .cache_write_input_tokens
            .map(crate::telemetry::TokenCount::get),
        output_tokens: usage.output_tokens.map(crate::telemetry::TokenCount::get),
        reasoning_output_tokens: usage
            .reasoning_output_tokens
            .map(crate::telemetry::TokenCount::get),
        raw_total: usage.total_tokens.map(crate::telemetry::TokenCount::get),
    }
}

fn build_terminal_observations(
    store: &SqliteStore,
    terminal_tasks: &[(String, String, TaskLifecycleTransitionKind)],
    current_tokens: &[TokenUsageEvidence],
    state: &RuntimeRecoveryState,
) -> Result<Vec<NormalizedObservation>, StorageError> {
    let mut observations = BTreeMap::new();
    for (task_id, ended_at, transition) in terminal_tasks {
        let session_id = state
            .telemetry_state
            .session
            .as_ref()
            .map(|session| session.session_id.clone());
        let mut evidence = BTreeMap::new();
        for row in store.task_response_evidence(task_id, session_id.as_deref())? {
            evidence.insert(row.event_id.clone(), row);
        }
        for row in current_tokens.iter().filter(|row| {
            row.task_id.as_deref() == Some(task_id)
                && row.session_id == session_id
                && row.record_semantic == TokenRecordSemantic::PerResponse
        }) {
            if evidence
                .insert(row.event_id.clone(), row.clone())
                .is_some_and(|existing| existing != *row)
            {
                return Err(StorageError::EvidenceConflict);
            }
        }
        let session_id =
            session_id.or_else(|| evidence.values().find_map(|row| row.session_id.clone()));
        let mut target =
            TaskReconciliationTarget::new(task_id.clone(), None, Some(ended_at.clone()));
        target.session_id = session_id;
        let policy = ReconciliationPolicy {
            max_before_sample_age: time::Duration::seconds(0),
            post_task_sample_offsets: vec![time::Duration::seconds(1)],
            stabilization_not_before: time::Duration::seconds(0),
            required_stable_confirmations: 1,
            deadline: time::Duration::seconds(1),
        };
        let reconciliation = begin_task_quota_reconciliation(target, &[], &policy)
            .map_err(|_| StorageError::EvidenceInvalid)?;
        let attributed = evidence
            .values()
            .map(attributed_token_event)
            .collect::<Vec<_>>();
        let completeness = if *transition == TaskLifecycleTransitionKind::Completed {
            TokenTelemetryCompleteness::Complete
        } else {
            TokenTelemetryCompleteness::ProcessInterrupted
        };
        let observation = build_observation(&ObservationInput::new(
            reconciliation,
            attributed,
            completeness,
            IsolationEvidence::UnknownExternalUsage,
            Some(ended_at.clone()),
        ))
        .map_err(|_| StorageError::EvidenceInvalid)?;
        if store
            .load_observation(&observation.observation_id)?
            .is_none()
        {
            observations.insert(observation.observation_id.clone(), observation);
        }
    }
    Ok(observations.into_values().collect())
}

fn attributed_token_event(evidence: &TokenUsageEvidence) -> AttributedTokenEvent {
    let configuration = ConfigurationIdentity {
        plan: ConfigurationValue::unavailable(),
        model: evidence.model.as_deref().map_or_else(
            ConfigurationValue::unavailable,
            ConfigurationValue::observed,
        ),
        reasoning_level: evidence.reasoning_level.as_deref().map_or_else(
            ConfigurationValue::unavailable,
            ConfigurationValue::observed,
        ),
        speed_mode: ConfigurationValue::unavailable(),
        codex_version: evidence.codex_version.as_deref().map_or_else(
            ConfigurationValue::unavailable,
            ConfigurationValue::observed,
        ),
    };
    let counters = crate::telemetry::TokenCounters {
        uncached_input: TokenMetric::unavailable(),
        cached_input: metric_from_raw(evidence.cached_input_tokens),
        output: metric_from_raw(evidence.output_tokens),
        reasoning_output: metric_from_raw(evidence.reasoning_output_tokens),
        raw_total: metric_from_raw(evidence.raw_total),
    };
    AttributedTokenEvent {
        normalized_event: NormalizedTokenEvent {
            schema_version: SchemaVersion::V1,
            event_id: evidence.event_id.clone(),
            event_type: NormalizedEventType::TokenCountersUpdated,
            source_instance_id: evidence.source_id.clone(),
            safe_cursor_id: None,
            event_at: evidence.observed_at.clone(),
            session_id: evidence.session_id.clone(),
            task_id: evidence.task_id.clone(),
            payload: NormalizedTokenPayload {
                kind: TokenPayloadKind::TokenCounters,
                token_counters: counters,
            },
        },
        effective_configuration: configuration,
        configuration_fingerprint: crate::telemetry::identity::configuration_fingerprint(&[
            evidence.model.as_deref(),
            evidence.reasoning_level.as_deref(),
            None,
            evidence.service_tier.as_deref(),
            evidence.codex_version.as_deref(),
        ]),
        task_lifecycle: None,
    }
}

fn metric_from_raw(value: Option<u64>) -> TokenMetric {
    value.map_or_else(TokenMetric::unavailable, TokenMetric::observed)
}

fn snapshot_event_id(
    source: &SourceIdentity,
    item: &ReadItem,
    semantic: TokenRecordSemantic,
) -> String {
    crate::telemetry::identity::token_snapshot_semantic_id(
        source,
        item.start_offset,
        item.end_offset,
        item.ordinal,
        semantic.as_str(),
    )
}

fn record_semantic(kind: &RolloutRecordKind) -> &'static str {
    match kind {
        RolloutRecordKind::SessionMeta(_) => "session_meta",
        RolloutRecordKind::TurnContext(_) => "turn_context",
        RolloutRecordKind::TokenUsageRecord(_) => "token_usage_record",
        RolloutRecordKind::EventMsg(EventMessage::TokenCount(_)) => "token_count",
        RolloutRecordKind::EventMsg(EventMessage::TaskStarted(_)) => "task_started",
        RolloutRecordKind::EventMsg(EventMessage::TaskCompleted(_)) => "task_complete",
        RolloutRecordKind::EventMsg(EventMessage::TurnAborted(_)) => "turn_aborted",
        RolloutRecordKind::EventMsg(EventMessage::ContextCompacted(_)) => "context_compacted",
        RolloutRecordKind::EventMsg(EventMessage::ThreadSettingsApplied(_)) => {
            "thread_settings_applied"
        }
        RolloutRecordKind::EventMsg(EventMessage::Unsupported { .. }) => "unsupported_event",
        RolloutRecordKind::Unsupported(_) => "unsupported_record",
    }
}

fn is_quota_diagnostic(code: IngestionDiagnosticCode) -> bool {
    matches!(
        code,
        IngestionDiagnosticCode::QuotaUnavailable
            | IngestionDiagnosticCode::QuotaNonMainLimit
            | IngestionDiagnosticCode::QuotaUnsupportedWindow
            | IngestionDiagnosticCode::QuotaAmbiguousWindow
    )
}

fn source_id(source: &SourceIdentity) -> String {
    crate::telemetry::identity::source_instance_id(source)
}

fn apply_history(
    summary: &mut SyncSummary,
    history: &UsageHistorySummary,
    quota_samples_this_sync: &BTreeSet<String>,
) {
    summary.saved_response_events = history.response_count;
    summary.saved_raw_total_known_sum = history.raw_total_known_sum;
    summary.saved_raw_total_known_events = history.raw_total_known_count;
    summary.saved_raw_total_missing_events = history.raw_total_missing_count;
    summary.five_hour_meter = history
        .five_hour_latest
        .as_ref()
        .map(|sample| MeterSummary {
            used_percent: sample.used_percent,
            observed_at: sample.observed_at.clone(),
            reset_at: sample.reset_at.clone(),
            observed_during_this_sync: quota_samples_this_sync.contains(&sample.sample_id),
        });
    summary.weekly_meter = history.weekly_latest.as_ref().map(|sample| MeterSummary {
        used_percent: sample.used_percent,
        observed_at: sample.observed_at.clone(),
        reset_at: sample.reset_at.clone(),
        observed_during_this_sync: quota_samples_this_sync.contains(&sample.sample_id),
    });
}

fn default_database_path() -> Result<PathBuf, ()> {
    let data_directory = if cfg!(target_os = "linux") {
        let xdg = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        match xdg.filter(|path| path.is_absolute()) {
            Some(path) => path.join("codex-meter"),
            None => user_home()?.join(".local/share/codex-meter"),
        }
    } else if cfg!(target_os = "macos") {
        user_home()?.join("Library/Application Support/codex-meter")
    } else if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or(())?
            .join("codex-meter")
    } else {
        user_home()?.join(".local/share/codex-meter")
    };
    Ok(data_directory.join(DEFAULT_DATABASE_NAME))
}

fn user_home() -> Result<PathBuf, ()> {
    #[cfg(target_os = "windows")]
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    #[cfg(not(target_os = "windows"))]
    let home = std::env::var_os("HOME");
    home.map(PathBuf::from).ok_or(())
}

fn prepare_private_data_directory(directory: &Path) -> Result<(), CliError> {
    fs::create_dir_all(directory).map_err(|_| CliError {
        code: "application_data_directory_unavailable",
        message: "could not create the per-user Codex Meter data directory".into(),
    })?;
    let metadata = fs::symlink_metadata(directory).map_err(|_| CliError {
        code: "application_data_directory_unavailable",
        message: "could not verify the per-user Codex Meter data directory".into(),
    })?;
    if !metadata.file_type().is_dir() {
        return Err(CliError {
            code: "application_data_directory_unavailable",
            message: "could not verify the per-user Codex Meter data directory".into(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(|_| {
            CliError {
                code: "application_data_directory_unavailable",
                message: "could not secure the per-user Codex Meter data directory".into(),
            }
        })?;
    }
    Ok(())
}

fn prepare_private_database_file(path: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|_| {
                    CliError {
                        code: "database_permissions_unavailable",
                        message: "could not secure the Codex Meter database file".into(),
                    }
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true).mode(0o600);
                match options.open(path) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(path).map_err(|_| CliError {
                            code: "database_permissions_unavailable",
                            message: "could not secure the Codex Meter database file".into(),
                        })?;
                        if !metadata.file_type().is_file() {
                            return Err(CliError {
                                code: "database_permissions_unavailable",
                                message: "could not secure the Codex Meter database file".into(),
                            });
                        }
                        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(
                            |_| CliError {
                                code: "database_permissions_unavailable",
                                message: "could not secure the Codex Meter database file".into(),
                            },
                        )?;
                    }
                    Err(_) => {
                        return Err(CliError {
                            code: "database_permissions_unavailable",
                            message: "could not secure the Codex Meter database file".into(),
                        });
                    }
                }
            }
            _ => {
                return Err(CliError {
                    code: "database_permissions_unavailable",
                    message: "could not secure the Codex Meter database file".into(),
                });
            }
        }
    }
    Ok(())
}

fn codex_cli_detected() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    for directory in std::env::split_paths(&path) {
        let candidates = if cfg!(target_os = "windows") {
            vec!["codex.exe", "codex.cmd", "codex.bat"]
        } else {
            vec!["codex"]
        };
        for candidate in candidates {
            let path = directory.join(candidate);
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_file() {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 != 0 {
                    return true;
                }
            }
            #[cfg(not(unix))]
            return true;
        }
    }
    false
}

fn storage_cli_error(error: StorageError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}

fn render_summary(summary: &SyncSummary, json: bool) -> Result<String, CliError> {
    if json {
        return serde_json::to_string_pretty(summary)
            .map(|mut value| {
                value.push('\n');
                value
            })
            .map_err(|_| CliError {
                code: "summary_encoding_failed",
                message: "could not encode the privacy-safe sync summary".into(),
            });
    }
    let mut output = String::from("Codex Meter — Local Sync\n\n");
    output.push_str("Source:          Local Codex\n");
    if summary.codex_home_found {
        output.push_str("Codex home:      Found\n");
    } else if summary.status == SyncStatus::ExplicitCodexHomeUnavailable {
        output.push_str("Codex home:      Explicit source unavailable\n");
    } else {
        output.push_str("Codex home:      Not found\n");
    }
    output.push_str(&format!(
        "Codex CLI:       {}\n",
        if summary.codex_cli_detected {
            "Detected"
        } else {
            "Not detected"
        }
    ));
    output.push_str(&format!("Sessions seen:   {}\n", summary.sessions_seen));
    output.push_str(&format!(
        "Sources read:    {} / {}\n",
        summary.rollout_sources_processed, summary.rollout_sources_seen
    ));
    output.push_str(&format!("Responses added: {}\n", summary.responses_added));
    output.push_str(&format!(
        "Observations:    {} newly saved\n",
        summary.observations_added
    ));
    if let Some(raw_total) = summary.saved_raw_total_known_sum {
        output.push_str(&format!(
            "Raw tokens:      {raw_total} directly observed in saved response records"
        ));
        if summary.saved_raw_total_missing_events > 0 {
            output.push_str(" (partial; some raw totals are unavailable)");
        }
        output.push('\n');
    } else {
        output.push_str("Raw tokens:      Unavailable\n");
    }
    output.push_str(&format!(
        "5-hour meter:    {}\n",
        render_meter(summary.five_hour_meter.as_ref())
    ));
    output.push_str(&format!(
        "Weekly meter:    {}\n",
        render_meter(summary.weekly_meter.as_ref())
    ));
    output.push_str(&format!("Skipped records: {}\n", summary.skipped_records));
    output.push_str(&format!(
        "Compressed skipped: {}\n",
        summary.unsupported_compressed_sources
    ));
    output.push_str(&format!(
        "History:         {}\n",
        if summary.history_saved {
            "Saved locally"
        } else {
            "Not opened"
        }
    ));
    if !summary.warnings.is_empty() {
        output.push_str("Coverage notes:  ");
        output.push_str(
            &summary
                .warnings
                .iter()
                .map(|warning| warning_label(*warning))
                .collect::<Vec<_>>()
                .join("; "),
        );
        output.push('\n');
    }
    output.push_str(&format!(
        "\nSync result:     {}\n",
        status_label(summary.status)
    ));
    Ok(output)
}

fn render_meter(meter: Option<&MeterSummary>) -> String {
    meter.map_or_else(
        || "Unavailable (no supported local snapshot)".to_owned(),
        |sample| {
            let suffix = if sample.observed_during_this_sync {
                "observed in this sync"
            } else {
                "latest saved local observation"
            };
            format!(
                "{:.2}% used ({suffix}, {})",
                sample.used_percent, sample.observed_at
            )
        },
    )
}

fn status_label(status: SyncStatus) -> &'static str {
    match status {
        SyncStatus::Completed => "Completed",
        SyncStatus::CompletedWithWarnings => "Completed with coverage notes",
        SyncStatus::CompletedNoNewRecords => "Completed; no new response records",
        SyncStatus::NoCodexHome => "Codex home not found",
        SyncStatus::ExplicitCodexHomeUnavailable => "Explicit Codex home is unavailable",
        SyncStatus::CodexHomeWithoutSessions => "Codex home has no rollout sessions",
        SyncStatus::NoSupportedRollouts => "No supported rollout files found",
        SyncStatus::UnsupportedCodexVersion => "Codex telemetry version is unsupported",
        SyncStatus::NoSupportedTokenEvents => "No supported per-response token events found",
    }
}

fn warning_label(warning: SyncWarning) -> &'static str {
    match warning {
        SyncWarning::CodexCliNotDetected => "Codex CLI executable was not detected",
        SyncWarning::CompressedSourcesUnsupported => "compressed rollout files were skipped",
        SyncWarning::IncompleteSourceMetadata => "some rollout headers are incomplete",
        SyncWarning::MalformedOrUnsupportedSources => {
            "some source records or formats were unsupported"
        }
        SyncWarning::UnsupportedCodexVersion => "some Codex CLI versions were not verified",
        SyncWarning::UnreadableSource => "some source files could not be read",
        SyncWarning::SymlinkEntriesSkipped => "symlinked source entries were skipped",
        SyncWarning::DiscoveryScanLimited => "source discovery reached its scan limit",
        SyncWarning::SourceChanged => "a source generation changed and was not resumed",
        SyncWarning::SourceGenerationAmbiguous => {
            "a prior generation exists for this session identity; the new source was skipped"
        }
        SyncWarning::AmbiguousSourceIdentity => {
            "multiple source files share an identity and were skipped as ambiguous"
        }
        SyncWarning::SourceTruncated => "a truncated source was not rewound",
        SyncWarning::CheckpointAmbiguous => "a source checkpoint could not be matched safely",
        SyncWarning::CheckpointCorruptOrUnsupported => {
            "a source checkpoint was corrupt or unsupported"
        }
        SyncWarning::SourceReadLimited => "a source reached the per-sync processing limit",
        SyncWarning::SourceReadFailed => "a source could not be read completely",
        SyncWarning::SourceTransactionConflict => "a source batch was not committed",
        SyncWarning::QuotaEvidenceIncomplete => "supported local quota snapshots were unavailable",
        SyncWarning::DataCoverageIncomplete => "history coverage is incomplete",
        SyncWarning::DiagnosticWriteFailed => "a sanitized ingestion diagnostic could not be saved",
    }
}

fn persist_discovery_diagnostics(
    store: &mut SqliteStore,
    discovery: &DiscoveryResult,
) -> Result<(), StorageError> {
    let mut findings = Vec::new();
    if discovery.counts.compressed_sources_skipped > 0 {
        findings.push(IngestionDiagnosticCode::CompressedSourceUnsupported);
    }
    if discovery.counts.incomplete_sources > 0 {
        findings.push(IngestionDiagnosticCode::IncompleteSourceHeader);
    }
    if discovery.counts.unsupported_sources > 0 {
        findings.push(IngestionDiagnosticCode::UnsupportedSourceFormat);
    }
    if discovery.counts.unsupported_version_sources > 0 {
        findings.push(IngestionDiagnosticCode::UnsupportedCodexVersion);
    }
    if discovery.counts.unreadable_sources > 0 || discovery.counts.unreadable_directories > 0 {
        findings.push(IngestionDiagnosticCode::DiscoveryUnreadable);
    }
    if discovery.counts.symlink_entries_skipped > 0 {
        findings.push(IngestionDiagnosticCode::SymlinkEntrySkipped);
    }
    if discovery.counts.scan_limited {
        findings.push(IngestionDiagnosticCode::DiscoveryScanLimited);
    }
    if discovery.counts.ambiguous_source_identities > 0 {
        findings.push(IngestionDiagnosticCode::AmbiguousSourceIdentity);
    }
    let source = SourceIdentity::new("local-discovery", "local-discovery-v1")
        .expect("static discovery identity should be valid");
    let source_id = source_id(&source);
    for code in findings {
        let diagnostic = IngestionDiagnostic {
            diagnostic_id: crate::telemetry::identity::diagnostic_id(
                &source,
                0,
                0,
                None,
                code.as_str(),
            ),
            source_id: source_id.clone(),
            observed_at: None,
            record_semantic: "source_discovery".into(),
            diagnostic_code: code,
        };
        store.save_ingestion_diagnostic(&diagnostic)?;
    }
    Ok(())
}

fn source_warning_diagnostic(
    rollout: &DiscoveredRollout,
    warning: SyncWarning,
) -> Option<IngestionDiagnostic> {
    let code = match warning {
        SyncWarning::UnreadableSource => IngestionDiagnosticCode::SourceReadFailed,
        SyncWarning::SourceChanged => IngestionDiagnosticCode::SourceChanged,
        SyncWarning::SourceGenerationAmbiguous => {
            IngestionDiagnosticCode::SourceGenerationAmbiguous
        }
        SyncWarning::SourceTruncated => IngestionDiagnosticCode::SourceTruncated,
        SyncWarning::CheckpointAmbiguous => IngestionDiagnosticCode::CheckpointAmbiguous,
        SyncWarning::CheckpointCorruptOrUnsupported => IngestionDiagnosticCode::CheckpointCorrupt,
        SyncWarning::SourceReadLimited => IngestionDiagnosticCode::SourceReadLimited,
        SyncWarning::SourceReadFailed => IngestionDiagnosticCode::SourceReadFailed,
        SyncWarning::SourceTransactionConflict => {
            IngestionDiagnosticCode::SourceTransactionConflict
        }
        SyncWarning::QuotaEvidenceIncomplete => IngestionDiagnosticCode::QuotaEvidenceIncomplete,
        _ => return None,
    };
    Some(IngestionDiagnostic {
        diagnostic_id: crate::telemetry::identity::diagnostic_id(
            &rollout.source,
            0,
            0,
            None,
            code.as_str(),
        ),
        source_id: source_id(&rollout.source),
        observed_at: None,
        record_semantic: "source_boundary".into(),
        diagnostic_code: code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_database_filename_is_canonical() {
        assert_eq!(DEFAULT_DATABASE_NAME, "codex-meter.sqlite3");
    }
}
