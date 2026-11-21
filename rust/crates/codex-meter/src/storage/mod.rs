//! Rust-owned SQLite storage and migration infrastructure.

mod checkpoints;
mod controlled_measurements;
mod live_account_snapshots;
mod migrations;
mod observations;
mod reporting;
mod sampling_finalization;
mod sqlite;
mod usage_evidence;

pub use checkpoints::{
    ActiveReconciliationCheckpoint, QualifiedSamplingCheckpoint, QualifiedSamplingPhase,
    RuntimeCheckpoint, RuntimeRecoveryState, SamplingFinalizationReceipt, STATE_FORMAT_VERSION,
};
pub use controlled_measurements::{
    ActivityDeclaration, ControlledMeasurementSession, MeasurementSessionState,
    CONTROLLED_MEASUREMENT_FORMAT_VERSION, DEFAULT_MEASUREMENT_MINUTES, EVALUATOR_CONTRACT_VERSION,
    MAX_MEASUREMENT_MINUTES,
};
pub use live_account_snapshots::{
    LiveAccountSnapshotHistoryPage, LiveSnapshotSaveDisposition, SavedLiveAccountSnapshot,
    StoredLiveAccountSnapshot, StoredLiveQuotaMeter, LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
    LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION, LIVE_ACCOUNT_QUOTA_PROVENANCE,
    LIVE_ACCOUNT_QUOTA_SOURCE, LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION,
    LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION,
};
pub use observations::{
    ObservationPage, ObservationPageCursor, ObservationQuery, StoredObservation,
};
pub use reporting::{
    ConfigurationBreakdown, DiagnosticCount, EvidenceDetail, EvidenceQualitySummary,
    MetricCoverage, MetricTotal, QuotaHistoryCursor, QuotaHistoryPage, QuotaTimeline,
    ReadinessLedgerCoverage, TaskModelConfiguration, TaskTokenTotals, TaskUsageQuery,
    TaskUsageReport, TaskUsageSummary, TokenHistoryCursor, TokenHistoryPage, TokenHistoryQuery,
    TokenStatistics, TokenStatisticsQuery, ValueCount,
};
pub use sampling_finalization::{SamplingFinalization, SamplingFinalizationRequest};
pub use sqlite::{SqliteReadStore, SqliteStore, StorageError, StorageInfo, DEFAULT_BUSY_TIMEOUT};
pub use usage_evidence::{
    EvidenceInsertCounts, IngestionDiagnostic, IngestionDiagnosticCode, QuotaSnapshotEvidence,
    SourceBoundaryFingerprint, StoredQuotaSnapshot, TokenRecordSemantic, TokenUsageEvidence,
    UsageEvidenceBatch, UsageHistorySummary,
};

#[cfg(test)]
pub(crate) use reporting::tests::{
    insert_diagnostic as insert_test_diagnostic, insert_token as insert_test_token,
    token as test_token,
};
