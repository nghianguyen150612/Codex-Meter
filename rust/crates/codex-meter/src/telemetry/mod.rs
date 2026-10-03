//! Source adapters for privacy-filtered telemetry inputs.

pub mod assembly;
pub mod codex_rollout;
mod identity;
pub mod incremental_jsonl;
pub mod normalized;
pub mod token_normalization;

pub use codex_rollout::{
    parse_rollout_record, ContextCompactedEvent, EventMessage, RolloutDecodeError, RolloutRecord,
    RolloutRecordKind, SessionMetaRecord, SessionSource, TaskCompletedEvent, TaskCompletion,
    TaskStartedEvent, ThreadSettingsAppliedRecord, TokenCount, TokenCountEvent, TokenCountInfo,
    TokenUsage, TokenUsageRecord, TurnAbortReason, TurnAbortedEvent, TurnContextRecord,
    UnsupportedRecord, UnsupportedRecordClassification, COMPATIBILITY_CLI_VERSION,
    COMPATIBILITY_UPSTREAM_REVISION,
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
    ConfigurationIdentity, ConfigurationPayload, ConfigurationPayloadKind, ConfigurationValue,
    LifecyclePayload, LifecyclePayloadKind, MetricAvailability, MetricProvenance,
    NormalizedConfigurationEvent, NormalizedEventType, NormalizedSessionEvent,
    NormalizedTokenEvent, NormalizedTokenPayload, SchemaVersion, TokenCounters, TokenMetric,
    TokenPayloadKind,
};

pub use token_normalization::{
    extract_token_evidence, normalize_token_item, RawTokenEvidence, TimestampErrorReason,
    TokenEvidenceExtraction, TokenEvidenceSemantic, TokenEvidenceSet, TokenNormalizationError,
    TokenNormalizationOutcome,
};
