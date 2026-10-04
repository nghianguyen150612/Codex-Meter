//! Source adapters for privacy-filtered telemetry inputs.

pub mod assembly;
pub mod codex_rollout;
mod identity;
pub mod incremental_jsonl;
pub mod normalized;
pub mod quota_normalization;
pub mod quota_reconciliation;
pub mod quota_tracking;
pub mod token_normalization;

pub use codex_rollout::{
    parse_rollout_record, ContextCompactedEvent, EventMessage, PlanType, QuotaPercent,
    RateLimitSnapshot, RateLimitWindow, RolloutDecodeError, RolloutRecord, RolloutRecordKind,
    SessionMetaRecord, SessionSource, TaskCompletedEvent, TaskCompletion, TaskStartedEvent,
    ThreadSettingsAppliedRecord, TokenCount, TokenCountEvent, TokenCountInfo, TokenUsage,
    TokenUsageRecord, TurnAbortReason, TurnAbortedEvent, TurnContextRecord, UnsupportedRecord,
    UnsupportedRecordClassification, COMPATIBILITY_CLI_VERSION, COMPATIBILITY_UPSTREAM_REVISION,
};

pub use assembly::{
    assemble_batch, AssembledBatch, AssembledOutput, AttributedTokenEvent,
    ConfigurationConsistency, ConfigurationState, SessionContext, TaskAnomaly, TaskLifecycle,
    TaskLifecycleTransition, TaskLifecycleTransitionKind, TaskState, TaskTerminalEvidence,
    TelemetryAssemblyError, TelemetryState, TokenSnapshotEvidence,
};

pub use incremental_jsonl::{
    read_available, CursorIdentityError, IncrementalReadError, IoOperation, ReadBatch, ReadItem,
    ReadItemOutcome, RejectedLine, RejectedLineReason, RolloutCursor, SourceIdentity,
};

pub use normalized::{
    AcquisitionStatus, ConfigurationIdentity, ConfigurationPayload, ConfigurationPayloadKind,
    ConfigurationValue, LifecyclePayload, LifecyclePayloadKind, MetricAvailability,
    MetricProvenance, NormalizedConfigurationEvent, NormalizedEventType, NormalizedQuotaSample,
    NormalizedSessionEvent, NormalizedTokenEvent, NormalizedTokenPayload, PercentageMetric,
    QuotaIdentityConfidence, QuotaIdentityProvenance, QuotaMeterType, QuotaSourceKind,
    QuotaWindowIdentity, SchemaVersion, TokenCounters, TokenMetric, TokenPayloadKind,
};

pub use quota_normalization::{
    normalize_quota_item, QuotaNormalizationError, QuotaNormalizationOutcome,
};

pub use quota_reconciliation::{
    begin_task_quota_reconciliation, reconcile_task_quota, reconcile_task_quota_at,
    select_before_sample, AcquisitionFailureReason, AttributionRisk, BaselineUnavailableReason,
    BeforeSampleSelection, MeterReconciliation, MeterReconciliationState, MeterUnstableEvidence,
    PlanDiscontinuityEvidence, PolicyValidationError, QuotaAcquisitionAttempt,
    QuotaAcquisitionResult, ReconciledMeterEvidence, ReconciliationAction, ReconciliationBatch,
    ReconciliationError, ReconciliationInput, ReconciliationOutput, ReconciliationPolicy,
    ResetCrossedEvidence, StableCandidate, TaskQuotaReconciliation, TaskReconciliationTarget,
    TimedOutEvidence,
};

pub use quota_tracking::{
    advance_meter, track_quota_samples, BoundaryEvidence, MeterInstabilityReason,
    MeterTrackingState, NonNegativePercentagePoints, QuotaTrackingBatch, QuotaTrackingError,
    QuotaTrackingOutcome, QuotaTrackingState, SameWindowDelta, TrackedQuotaWindow,
    TrackedWindowIdentity,
};

pub use token_normalization::{
    extract_token_evidence, normalize_token_item, RawTokenEvidence, TimestampErrorReason,
    TokenEvidenceExtraction, TokenEvidenceSemantic, TokenEvidenceSet, TokenNormalizationError,
    TokenNormalizationOutcome,
};
