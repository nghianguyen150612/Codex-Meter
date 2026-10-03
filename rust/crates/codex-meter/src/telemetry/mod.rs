//! Source adapters for privacy-filtered telemetry inputs.

pub mod codex_rollout;
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

pub use incremental_jsonl::{
    read_available, CursorIdentityError, IncrementalReadError, IoOperation, ReadBatch, ReadItem,
    ReadItemOutcome, RejectedLine, RejectedLineReason, RolloutCursor, SourceIdentity,
};

pub use normalized::{
    MetricAvailability, MetricProvenance, NormalizedEventType, NormalizedTokenEvent,
    NormalizedTokenPayload, SchemaVersion, TokenCounters, TokenMetric, TokenPayloadKind,
};

pub use token_normalization::{
    extract_token_evidence, normalize_token_item, RawTokenEvidence, TimestampErrorReason,
    TokenEvidenceExtraction, TokenEvidenceSemantic, TokenEvidenceSet, TokenNormalizationError,
    TokenNormalizationOutcome,
};
