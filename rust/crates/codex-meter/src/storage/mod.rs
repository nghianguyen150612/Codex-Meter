//! Rust-owned SQLite storage and migration infrastructure.

mod checkpoints;
mod migrations;
mod observations;
mod reporting;
mod sqlite;
mod usage_evidence;

pub use checkpoints::{
    ActiveReconciliationCheckpoint, RuntimeCheckpoint, RuntimeRecoveryState, STATE_FORMAT_VERSION,
};
pub use observations::{
    ObservationPage, ObservationPageCursor, ObservationQuery, StoredObservation,
};
pub use reporting::{
    ConfigurationBreakdown, DiagnosticCount, EvidenceDetail, EvidenceQualitySummary,
    MetricCoverage, MetricTotal, QuotaHistoryCursor, QuotaHistoryPage, QuotaTimeline,
    TokenHistoryCursor, TokenHistoryPage, TokenHistoryQuery, TokenStatistics, TokenStatisticsQuery,
    ValueCount,
};
pub use sqlite::{SqliteReadStore, SqliteStore, StorageError, StorageInfo, DEFAULT_BUSY_TIMEOUT};
pub use usage_evidence::{
    EvidenceInsertCounts, IngestionDiagnostic, IngestionDiagnosticCode, QuotaSnapshotEvidence,
    SourceBoundaryFingerprint, StoredQuotaSnapshot, TokenRecordSemantic, TokenUsageEvidence,
    UsageEvidenceBatch, UsageHistorySummary,
};
