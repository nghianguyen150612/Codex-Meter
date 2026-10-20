//! Rust-owned SQLite storage and migration infrastructure.

mod checkpoints;
mod migrations;
mod observations;
mod sqlite;
mod usage_evidence;

pub use checkpoints::{
    ActiveReconciliationCheckpoint, RuntimeCheckpoint, RuntimeRecoveryState, STATE_FORMAT_VERSION,
};
pub use observations::{
    ObservationPage, ObservationPageCursor, ObservationQuery, StoredObservation,
};
pub use sqlite::{SqliteStore, StorageError, StorageInfo, DEFAULT_BUSY_TIMEOUT};
pub use usage_evidence::{
    EvidenceInsertCounts, IngestionDiagnostic, IngestionDiagnosticCode, QuotaSnapshotEvidence,
    SourceBoundaryFingerprint, StoredQuotaSnapshot, TokenRecordSemantic, TokenUsageEvidence,
    UsageEvidenceBatch, UsageHistorySummary,
};
