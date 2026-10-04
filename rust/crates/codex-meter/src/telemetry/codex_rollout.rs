//! Privacy-filtered source models for the pinned Codex rollout format.
//!
//! This module decodes one complete rollout JSON object. It deliberately does not
//! implement file discovery, JSONL iteration, or normalization into Codex Meter's
//! versioned domain contracts.

use std::fmt;

use serde_json::{Map, Value};

/// Compatibility target used by the P005 evidence audit.
pub const COMPATIBILITY_CLI_VERSION: &str = "0.157.1";

/// Compatibility target used by the P005 evidence audit.
pub const COMPATIBILITY_UPSTREAM_REVISION: &str = "8f7a0f7a878199c6886600370e5be6bd37ca38a3";

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_SAFE_ID_LENGTH: usize = 128;
const MAX_SAFE_TEXT_LENGTH: usize = 256;
const MAX_SAFE_TIMESTAMP_LENGTH: usize = 128;

/// A non-negative token counter bounded by the JSON safe-integer range.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TokenCount(u64);

impl TokenCount {
    /// Constructs a counter when the value is safe to represent exactly in JSON tooling.
    pub const fn new(value: u64) -> Option<Self> {
        if value <= MAX_SAFE_INTEGER {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the counter's integer value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The six upstream token counters, preserving missingness as `None`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenUsage {
    pub input_tokens: Option<TokenCount>,
    pub cached_input_tokens: Option<TokenCount>,
    pub cache_write_input_tokens: Option<TokenCount>,
    pub output_tokens: Option<TokenCount>,
    pub reasoning_output_tokens: Option<TokenCount>,
    pub total_tokens: Option<TokenCount>,
}

/// The generic rollout record envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RolloutRecord {
    pub timestamp: String,
    pub ordinal: Option<u64>,
    pub kind: RolloutRecordKind,
}

/// Supported privacy-filtered rollout record kinds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RolloutRecordKind {
    SessionMeta(SessionMetaRecord),
    TurnContext(TurnContextRecord),
    TokenUsageRecord(TokenUsageRecord),
    EventMsg(EventMessage),
    Unsupported(UnsupportedRecord),
}

/// Privacy-safe session metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionMetaRecord {
    pub session_id: String,
    pub thread_id: String,
    pub cli_version: String,
    pub model_provider: Option<String>,
    pub source: Option<SessionSource>,
}

/// Safe source classification for the upstream `SessionSource` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionSource {
    Cli,
    VsCode,
    Exec,
    Mcp,
    Other,
}

/// Privacy-safe turn configuration evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnContextRecord {
    pub turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

/// Privacy-safe thread configuration evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadSettingsAppliedRecord {
    pub thread_id: Option<String>,
    pub model: String,
    pub model_provider: String,
    pub reasoning_effort: Option<String>,
    pub service_tier: Option<String>,
}

/// A persisted usage record for one completed response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenUsageRecord {
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub root_turn_id: Option<String>,
    /// Usage reported for the one completed response.
    pub usage: TokenUsage,
    /// Cumulative usage within the current turn.
    pub turn_token_usage: TokenUsage,
    /// Cumulative usage within the current thread/session scope.
    pub thread_token_usage: TokenUsage,
}

/// Supported telemetry-safe `event_msg` variants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventMessage {
    TokenCount(TokenCountEvent),
    TaskStarted(TaskStartedEvent),
    TaskCompleted(TaskCompletedEvent),
    TurnAborted(TurnAbortedEvent),
    ContextCompacted(ContextCompactedEvent),
    ThreadSettingsApplied(ThreadSettingsAppliedRecord),
    Unsupported { event_type: String },
}

/// A cumulative token snapshot and the latest appended usage snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenCountEvent {
    pub info: Option<TokenCountInfo>,
    pub rate_limits: Option<RateLimitSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenCountInfo {
    /// Lifetime/session cumulative snapshot, not an event-local delta.
    pub total_token_usage: TokenUsage,
    /// Latest appended usage snapshot, not a lifetime total.
    pub last_token_usage: TokenUsage,
    pub model_context_window: Option<i64>,
}

/// Privacy-safe account quota evidence from one token-count event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitSnapshot {
    pub limit_id: Option<String>,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    pub plan_type: Option<PlanType>,
}

/// One provider-reported quota window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateLimitWindow {
    pub used_percent: QuotaPercent,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

impl Eq for RateLimitWindow {}

/// A provider percentage accepted only when finite and within the inclusive
/// 0..=100 range.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct QuotaPercent(f64);

impl QuotaPercent {
    pub const fn new(value: f64) -> Option<Self> {
        if value.is_finite() && (value >= 0.0) && (value <= 100.0) {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Eq for QuotaPercent {}

/// Provider plan values recognized by the pinned Codex source enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanType {
    Free,
    Go,
    Plus,
    Pro,
    ProLite,
    ProMax,
    Team,
    SelfServeBusinessProLite,
    SelfServeBusinessUsageBased,
    Business,
    Ent26,
    EnterpriseCbpAutomation,
    EnterpriseCbpUsageBased,
    Enterprise,
    Edu,
    EduPlus,
    EduPro,
}

impl PlanType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Go => "go",
            Self::Plus => "plus",
            Self::Pro => "pro",
            Self::ProLite => "prolite",
            Self::ProMax => "promax",
            Self::Team => "team",
            Self::SelfServeBusinessProLite => "self_serve_business_prolite",
            Self::SelfServeBusinessUsageBased => "self_serve_business_usage_based",
            Self::Business => "business",
            Self::Ent26 => "ent26",
            Self::EnterpriseCbpAutomation => "enterprise_cbp_automation",
            Self::EnterpriseCbpUsageBased => "enterprise_cbp_usage_based",
            Self::Enterprise => "enterprise",
            Self::Edu => "edu",
            Self::EduPlus => "edu_plus",
            Self::EduPro => "edu_pro",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "free" => Self::Free,
            "go" => Self::Go,
            "plus" => Self::Plus,
            "pro" => Self::Pro,
            "prolite" => Self::ProLite,
            "promax" => Self::ProMax,
            "team" => Self::Team,
            "self_serve_business_prolite" => Self::SelfServeBusinessProLite,
            "self_serve_business_usage_based" => Self::SelfServeBusinessUsageBased,
            "business" => Self::Business,
            "ent26" => Self::Ent26,
            "enterprise_cbp_automation" => Self::EnterpriseCbpAutomation,
            "enterprise_cbp_usage_based" => Self::EnterpriseCbpUsageBased,
            "enterprise" => Self::Enterprise,
            "edu" => Self::Edu,
            "edu_plus" => Self::EduPlus,
            "edu_pro" => Self::EduPro,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskStartedEvent {
    pub turn_id: String,
    pub root_turn_id: Option<String>,
    pub started_at: Option<i64>,
    pub model_context_window: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskCompletedEvent {
    pub turn_id: String,
    pub outcome: TaskCompletion,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub time_to_first_token_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskCompletion {
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnAbortedEvent {
    pub turn_id: Option<String>,
    pub reason: TurnAbortReason,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnAbortReason {
    Interrupted,
    Replaced,
    ReviewEnded,
    BudgetLimited,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextCompactedEvent;

/// Classification for a record whose payload is intentionally not retained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnsupportedRecord {
    pub record_type: String,
    pub classification: UnsupportedRecordClassification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedRecordClassification {
    KnownButIgnored,
    Unknown,
}

/// Structural failures encountered while decoding one complete record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RolloutDecodeError {
    InvalidJson {
        line: usize,
        column: usize,
    },
    TopLevelNotObject,
    MissingField {
        record_type: Option<String>,
        field: &'static str,
    },
    InvalidFieldType {
        record_type: String,
        field: &'static str,
    },
    InvalidFieldValue {
        record_type: String,
        field: &'static str,
        reason: InvalidFieldReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidFieldReason {
    EmptyString,
    UnsafeString,
    NegativeNumber,
    NonIntegerNumber,
    NumberTooLarge,
    NonFiniteNumber,
    PercentageOutOfRange,
    InvalidEnumValue,
}

impl fmt::Display for RolloutDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson { line, column } => {
                write!(
                    formatter,
                    "invalid rollout JSON at line {line}, column {column}"
                )
            }
            Self::TopLevelNotObject => write!(formatter, "rollout record must be a JSON object"),
            Self::MissingField { record_type, field } => match record_type {
                Some(record_type) => {
                    write!(
                        formatter,
                        "record {record_type:?} is missing field {field:?}"
                    )
                }
                None => write!(formatter, "rollout record is missing field {field:?}"),
            },
            Self::InvalidFieldType { record_type, field } => {
                write!(
                    formatter,
                    "record {record_type:?} has an invalid type for {field:?}"
                )
            }
            Self::InvalidFieldValue {
                record_type,
                field,
                reason,
            } => write!(
                formatter,
                "record {record_type:?} has an invalid value for {field:?}: {reason}"
            ),
        }
    }
}

impl fmt::Display for InvalidFieldReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description = match self {
            Self::EmptyString => "empty string",
            Self::UnsafeString => "unsafe string",
            Self::NegativeNumber => "negative number",
            Self::NonIntegerNumber => "non-integer number",
            Self::NumberTooLarge => "number exceeds safe-integer range",
            Self::NonFiniteNumber => "number is not finite",
            Self::PercentageOutOfRange => "percentage is outside 0..=100",
            Self::InvalidEnumValue => "unsupported enum value",
        };
        formatter.write_str(description)
    }
}

impl std::error::Error for RolloutDecodeError {}

/// Decodes one complete JSON rollout record without retaining arbitrary payload content.
pub fn parse_rollout_record(input: &str) -> Result<RolloutRecord, RolloutDecodeError> {
    let value =
        serde_json::from_str::<Value>(input).map_err(|error| RolloutDecodeError::InvalidJson {
            line: error.line(),
            column: error.column(),
        })?;
    decode_rollout_value(value)
}

fn decode_rollout_value(value: Value) -> Result<RolloutRecord, RolloutDecodeError> {
    let object = value
        .as_object()
        .ok_or(RolloutDecodeError::TopLevelNotObject)?;
    let timestamp = required_string(object, None, "timestamp", MAX_SAFE_TIMESTAMP_LENGTH)?;
    let ordinal = optional_u64(object, None, "ordinal")?;
    let record_type = required_string(object, None, "type", MAX_SAFE_TEXT_LENGTH)?;

    let kind = match record_type.as_str() {
        "session_meta" => {
            RolloutRecordKind::SessionMeta(parse_session_meta(object.get("payload"), &record_type)?)
        }
        "turn_context" => {
            RolloutRecordKind::TurnContext(parse_turn_context(object.get("payload"), &record_type)?)
        }
        "token_usage_record" => RolloutRecordKind::TokenUsageRecord(parse_usage_record(
            object.get("payload"),
            &record_type,
        )?),
        "event_msg" => {
            RolloutRecordKind::EventMsg(parse_event_message(object.get("payload"), &record_type)?)
        }
        _ if is_known_ignored_record(&record_type) => {
            RolloutRecordKind::Unsupported(UnsupportedRecord {
                record_type,
                classification: UnsupportedRecordClassification::KnownButIgnored,
            })
        }
        _ => RolloutRecordKind::Unsupported(UnsupportedRecord {
            record_type,
            classification: UnsupportedRecordClassification::Unknown,
        }),
    };

    Ok(RolloutRecord {
        timestamp,
        ordinal,
        kind,
    })
}

fn parse_session_meta(
    payload: Option<&Value>,
    record_type: &str,
) -> Result<SessionMetaRecord, RolloutDecodeError> {
    let object = required_object(payload, record_type, "payload")?;
    Ok(SessionMetaRecord {
        session_id: required_string(object, Some(record_type), "session_id", MAX_SAFE_ID_LENGTH)?,
        thread_id: required_string(object, Some(record_type), "id", MAX_SAFE_ID_LENGTH)?,
        cli_version: required_string(
            object,
            Some(record_type),
            "cli_version",
            MAX_SAFE_TEXT_LENGTH,
        )?,
        model_provider: optional_string(
            object,
            Some(record_type),
            "model_provider",
            MAX_SAFE_TEXT_LENGTH,
        )?,
        source: optional_source(object, Some(record_type))?,
    })
}

fn parse_turn_context(
    payload: Option<&Value>,
    record_type: &str,
) -> Result<TurnContextRecord, RolloutDecodeError> {
    let object = required_object(payload, record_type, "payload")?;
    Ok(TurnContextRecord {
        turn_id: optional_string(object, Some(record_type), "turn_id", MAX_SAFE_ID_LENGTH)?,
        root_turn_id: optional_string(
            object,
            Some(record_type),
            "root_turn_id",
            MAX_SAFE_ID_LENGTH,
        )?,
        model: required_string(object, Some(record_type), "model", MAX_SAFE_TEXT_LENGTH)?,
        reasoning_effort: optional_string(
            object,
            Some(record_type),
            "effort",
            MAX_SAFE_TEXT_LENGTH,
        )?,
    })
}

fn parse_thread_settings(
    object: &Map<String, Value>,
    record_type: &str,
) -> Result<ThreadSettingsAppliedRecord, RolloutDecodeError> {
    let settings = required_object(
        object.get("thread_settings"),
        record_type,
        "thread_settings",
    )?;
    Ok(ThreadSettingsAppliedRecord {
        thread_id: optional_string(object, Some(record_type), "thread_id", MAX_SAFE_ID_LENGTH)?,
        model: required_string(settings, Some(record_type), "model", MAX_SAFE_TEXT_LENGTH)?,
        model_provider: required_string(
            settings,
            Some(record_type),
            "model_provider_id",
            MAX_SAFE_TEXT_LENGTH,
        )?,
        reasoning_effort: optional_string(
            settings,
            Some(record_type),
            "reasoning_effort",
            MAX_SAFE_TEXT_LENGTH,
        )?,
        service_tier: optional_string(
            settings,
            Some(record_type),
            "service_tier",
            MAX_SAFE_TEXT_LENGTH,
        )?,
    })
}

fn parse_usage_record(
    payload: Option<&Value>,
    record_type: &str,
) -> Result<TokenUsageRecord, RolloutDecodeError> {
    let object = required_object(payload, record_type, "payload")?;
    Ok(TokenUsageRecord {
        session_id: required_string(object, Some(record_type), "session_id", MAX_SAFE_ID_LENGTH)?,
        thread_id: required_string(object, Some(record_type), "thread_id", MAX_SAFE_ID_LENGTH)?,
        turn_id: required_string(object, Some(record_type), "turn_id", MAX_SAFE_ID_LENGTH)?,
        root_turn_id: optional_string(
            object,
            Some(record_type),
            "root_turn_id",
            MAX_SAFE_ID_LENGTH,
        )?,
        usage: parse_token_usage(object.get("usage"), record_type, "usage")?,
        turn_token_usage: parse_token_usage(
            object.get("turn_token_usage"),
            record_type,
            "turn_token_usage",
        )?,
        thread_token_usage: parse_token_usage(
            object.get("thread_token_usage"),
            record_type,
            "thread_token_usage",
        )?,
    })
}

fn parse_event_message(
    payload: Option<&Value>,
    record_type: &str,
) -> Result<EventMessage, RolloutDecodeError> {
    let object = required_object(payload, record_type, "payload")?;
    let event_type = required_string(object, Some(record_type), "type", MAX_SAFE_TEXT_LENGTH)?;
    match event_type.as_str() {
        "token_count" => parse_token_count(object, record_type),
        "task_started" | "turn_started" => parse_task_started(object, record_type),
        "task_complete" | "turn_complete" => parse_task_complete(object, record_type),
        "turn_aborted" => parse_turn_aborted(object, record_type),
        "context_compacted" => Ok(EventMessage::ContextCompacted(ContextCompactedEvent)),
        "thread_settings_applied" => Ok(EventMessage::ThreadSettingsApplied(
            parse_thread_settings(object, record_type)?,
        )),
        _ => Ok(EventMessage::Unsupported { event_type }),
    }
}

fn parse_token_count(
    object: &Map<String, Value>,
    record_type: &str,
) -> Result<EventMessage, RolloutDecodeError> {
    let info = match object.get("info") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let info = required_object(Some(value), record_type, "info")?;
            Some(TokenCountInfo {
                total_token_usage: parse_token_usage(
                    info.get("total_token_usage"),
                    record_type,
                    "total_token_usage",
                )?,
                last_token_usage: parse_token_usage(
                    info.get("last_token_usage"),
                    record_type,
                    "last_token_usage",
                )?,
                model_context_window: optional_i64(
                    info,
                    Some(record_type),
                    "model_context_window",
                )?,
            })
        }
    };
    let rate_limits = parse_rate_limits(object.get("rate_limits"), record_type)?;
    Ok(EventMessage::TokenCount(TokenCountEvent {
        info,
        rate_limits,
    }))
}

fn parse_rate_limits(
    value: Option<&Value>,
    record_type: &str,
) -> Result<Option<RateLimitSnapshot>, RolloutDecodeError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = required_object(Some(value), record_type, "rate_limits")?;
    Ok(Some(RateLimitSnapshot {
        limit_id: optional_string(object, Some(record_type), "limit_id", MAX_SAFE_TEXT_LENGTH)?,
        primary: optional_rate_limit_window(object, record_type, "primary")?,
        secondary: optional_rate_limit_window(object, record_type, "secondary")?,
        plan_type: optional_plan_type(object, record_type)?,
    }))
}

fn optional_rate_limit_window(
    object: &Map<String, Value>,
    record_type: &str,
    field: &'static str,
) -> Result<Option<RateLimitWindow>, RolloutDecodeError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let window = required_object(Some(value), record_type, field)?;
    let used_percent = window
        .get("used_percent")
        .ok_or_else(|| missing_field(record_type, "used_percent"))?;
    let used_percent = used_percent
        .as_f64()
        .ok_or_else(|| invalid_type(record_type, "used_percent"))?;
    let used_percent = QuotaPercent::new(used_percent).ok_or_else(|| {
        let reason = if !used_percent.is_finite() {
            InvalidFieldReason::NonFiniteNumber
        } else {
            InvalidFieldReason::PercentageOutOfRange
        };
        invalid_value(record_type, "used_percent", reason)
    })?;
    Ok(Some(RateLimitWindow {
        used_percent,
        window_minutes: optional_i64(window, Some(record_type), "window_minutes")?,
        resets_at: optional_i64(window, Some(record_type), "resets_at")?,
    }))
}

fn optional_plan_type(
    object: &Map<String, Value>,
    record_type: &str,
) -> Result<Option<PlanType>, RolloutDecodeError> {
    let Some(value) = object.get("plan_type") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value
        .as_str()
        .ok_or_else(|| invalid_type(record_type, "plan_type"))?;
    Ok(PlanType::from_str(value))
}

fn parse_task_started(
    object: &Map<String, Value>,
    record_type: &str,
) -> Result<EventMessage, RolloutDecodeError> {
    Ok(EventMessage::TaskStarted(TaskStartedEvent {
        turn_id: required_string(object, Some(record_type), "turn_id", MAX_SAFE_ID_LENGTH)?,
        root_turn_id: optional_string(
            object,
            Some(record_type),
            "root_turn_id",
            MAX_SAFE_ID_LENGTH,
        )?,
        started_at: optional_i64(object, Some(record_type), "started_at")?,
        model_context_window: optional_i64(object, Some(record_type), "model_context_window")?,
    }))
}

fn parse_task_complete(
    object: &Map<String, Value>,
    record_type: &str,
) -> Result<EventMessage, RolloutDecodeError> {
    let outcome = match object.get("error") {
        None | Some(Value::Null) => TaskCompletion::Completed,
        Some(Value::Object(_)) => TaskCompletion::Failed,
        Some(_) => {
            return Err(invalid_type(record_type, "error"));
        }
    };
    Ok(EventMessage::TaskCompleted(TaskCompletedEvent {
        turn_id: required_string(object, Some(record_type), "turn_id", MAX_SAFE_ID_LENGTH)?,
        outcome,
        started_at: optional_i64(object, Some(record_type), "started_at")?,
        completed_at: optional_i64(object, Some(record_type), "completed_at")?,
        duration_ms: optional_i64(object, Some(record_type), "duration_ms")?,
        time_to_first_token_ms: optional_i64(object, Some(record_type), "time_to_first_token_ms")?,
    }))
}

fn parse_turn_aborted(
    object: &Map<String, Value>,
    record_type: &str,
) -> Result<EventMessage, RolloutDecodeError> {
    let reason = required_string(object, Some(record_type), "reason", MAX_SAFE_TEXT_LENGTH)?;
    let reason = match reason.as_str() {
        "interrupted" => TurnAbortReason::Interrupted,
        "replaced" => TurnAbortReason::Replaced,
        "review_ended" => TurnAbortReason::ReviewEnded,
        "budget_limited" => TurnAbortReason::BudgetLimited,
        _ => TurnAbortReason::Unknown,
    };
    Ok(EventMessage::TurnAborted(TurnAbortedEvent {
        turn_id: optional_string(object, Some(record_type), "turn_id", MAX_SAFE_ID_LENGTH)?,
        reason,
        started_at: optional_i64(object, Some(record_type), "started_at")?,
        completed_at: optional_i64(object, Some(record_type), "completed_at")?,
        duration_ms: optional_i64(object, Some(record_type), "duration_ms")?,
    }))
}

fn parse_token_usage(
    value: Option<&Value>,
    record_type: &str,
    field: &'static str,
) -> Result<TokenUsage, RolloutDecodeError> {
    let object = required_object(value, record_type, field)?;
    Ok(TokenUsage {
        input_tokens: optional_token(object, record_type, field, "input_tokens")?,
        cached_input_tokens: optional_token(object, record_type, field, "cached_input_tokens")?,
        cache_write_input_tokens: optional_token(
            object,
            record_type,
            field,
            "cache_write_input_tokens",
        )?,
        output_tokens: optional_token(object, record_type, field, "output_tokens")?,
        reasoning_output_tokens: optional_token(
            object,
            record_type,
            field,
            "reasoning_output_tokens",
        )?,
        total_tokens: optional_token(object, record_type, field, "total_tokens")?,
    })
}

fn optional_token(
    object: &Map<String, Value>,
    record_type: &str,
    parent_field: &'static str,
    field: &'static str,
) -> Result<Option<TokenCount>, RolloutDecodeError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let number = value
        .as_number()
        .ok_or_else(|| invalid_type(record_type, parent_field))?;
    let value = number.as_u64().ok_or_else(|| {
        let reason = if number.as_i64().is_some() {
            InvalidFieldReason::NegativeNumber
        } else {
            InvalidFieldReason::NonIntegerNumber
        };
        invalid_value(record_type, field, reason)
    })?;
    TokenCount::new(value)
        .ok_or_else(|| invalid_value(record_type, field, InvalidFieldReason::NumberTooLarge))
        .map(Some)
}

fn optional_u64(
    object: &Map<String, Value>,
    record_type: Option<&str>,
    field: &'static str,
) -> Result<Option<u64>, RolloutDecodeError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let record_type = record_type.unwrap_or("rollout");
    let number = value
        .as_number()
        .ok_or_else(|| invalid_type(record_type, field))?;
    number
        .as_u64()
        .ok_or_else(|| invalid_value(record_type, field, InvalidFieldReason::NonIntegerNumber))
        .map(Some)
}

fn optional_i64(
    object: &Map<String, Value>,
    record_type: Option<&str>,
    field: &'static str,
) -> Result<Option<i64>, RolloutDecodeError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let record_type = record_type.unwrap_or("event_msg");
    let number = value
        .as_number()
        .ok_or_else(|| invalid_type(record_type, field))?;
    number
        .as_i64()
        .ok_or_else(|| invalid_value(record_type, field, InvalidFieldReason::NonIntegerNumber))
        .map(Some)
}

fn optional_source(
    object: &Map<String, Value>,
    record_type: Option<&str>,
) -> Result<Option<SessionSource>, RolloutDecodeError> {
    let Some(value) = object.get("source") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let record_type = record_type.unwrap_or("session_meta");
    let source = value
        .as_str()
        .ok_or_else(|| invalid_type(record_type, "source"))?;
    let source = match source {
        "cli" => SessionSource::Cli,
        "vscode" => SessionSource::VsCode,
        "exec" => SessionSource::Exec,
        "mcp" => SessionSource::Mcp,
        _ => SessionSource::Other,
    };
    Ok(Some(source))
}

fn required_object<'a>(
    value: Option<&'a Value>,
    record_type: &str,
    field: &'static str,
) -> Result<&'a Map<String, Value>, RolloutDecodeError> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| invalid_type(record_type, field))
}

fn required_string(
    object: &Map<String, Value>,
    record_type: Option<&str>,
    field: &'static str,
    max_length: usize,
) -> Result<String, RolloutDecodeError> {
    let record_type = record_type.unwrap_or("rollout");
    let value = object
        .get(field)
        .ok_or_else(|| missing_field(record_type, field))?;
    parse_string(value, record_type, field, max_length, true)
}

fn optional_string(
    object: &Map<String, Value>,
    record_type: Option<&str>,
    field: &'static str,
    max_length: usize,
) -> Result<Option<String>, RolloutDecodeError> {
    let Some(value) = object.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let record_type = record_type.unwrap_or("rollout");
    parse_string(value, record_type, field, max_length, false).map(Some)
}

fn parse_string(
    value: &Value,
    record_type: &str,
    field: &'static str,
    max_length: usize,
    require_non_empty: bool,
) -> Result<String, RolloutDecodeError> {
    let value = value
        .as_str()
        .ok_or_else(|| invalid_type(record_type, field))?;
    if require_non_empty && value.is_empty() {
        return Err(invalid_value(
            record_type,
            field,
            InvalidFieldReason::EmptyString,
        ));
    }
    if value.len() > max_length || value.chars().any(char::is_control) {
        return Err(invalid_value(
            record_type,
            field,
            InvalidFieldReason::UnsafeString,
        ));
    }
    Ok(value.to_owned())
}

fn is_known_ignored_record(record_type: &str) -> bool {
    matches!(
        record_type,
        "response_item"
            | "inter_agent_communication"
            | "inter_agent_communication_metadata"
            | "compacted"
            | "world_state"
            | "retained_context"
            | "security_risk_score"
            | "realtime_item"
    )
}

fn missing_field(record_type: &str, field: &'static str) -> RolloutDecodeError {
    RolloutDecodeError::MissingField {
        record_type: Some(record_type.to_owned()),
        field,
    }
}

fn invalid_type(record_type: &str, field: &'static str) -> RolloutDecodeError {
    RolloutDecodeError::InvalidFieldType {
        record_type: record_type.to_owned(),
        field,
    }
}

fn invalid_value(
    record_type: &str,
    field: &'static str,
    reason: InvalidFieldReason,
) -> RolloutDecodeError {
    RolloutDecodeError::InvalidFieldValue {
        record_type: record_type.to_owned(),
        field,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_META: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/session-meta.json"
    ));
    const TURN_CONTEXT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/turn-context.json"
    ));
    const TOKEN_USAGE_RECORD: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-record.json"
    ));
    const TOKEN_USAGE_MISSING_REASONING: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-missing-reasoning.json"
    ));
    const TOKEN_USAGE_ZERO_REASONING: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-usage-zero-reasoning.json"
    ));
    const TOKEN_COUNT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/token-count.json"
    ));
    const TASK_STARTED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/task-started.json"
    ));
    const TURN_STARTED_ALIAS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/turn-started-alias.json"
    ));
    const TASK_COMPLETE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/task-complete.json"
    ));
    const TURN_COMPLETE_ALIAS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/turn-complete-alias.json"
    ));
    const TURN_ABORTED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/turn-aborted.json"
    ));
    const CONTEXT_COMPACTED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/context-compacted.json"
    ));
    const THREAD_SETTINGS_APPLIED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/thread-settings-applied.json"
    ));
    const UNSUPPORTED_RESPONSE_ITEM: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/unsupported-response-item.json"
    ));
    const UNKNOWN_FUTURE_RECORD: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/unknown-future-record.json"
    ));
    const UNKNOWN_EVENT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/unknown-event.json"
    ));

    fn parse_fixture(input: &str) -> RolloutRecord {
        parse_rollout_record(input).expect("synthetic rollout fixture should decode")
    }

    fn token_value(value: Option<TokenCount>) -> u64 {
        value
            .expect("fixture token counter should be present")
            .get()
    }

    #[test]
    fn decodes_session_metadata_and_optional_ordinal() {
        let record = parse_fixture(SESSION_META);

        assert_eq!(record.timestamp, "2026-10-03T10:00:00Z");
        assert_eq!(record.ordinal, Some(0));
        let RolloutRecordKind::SessionMeta(meta) = record.kind else {
            panic!("expected session metadata");
        };
        assert_eq!(meta.session_id, "session-synthetic-001");
        assert_eq!(meta.thread_id, "thread-synthetic-001");
        assert_eq!(meta.cli_version, COMPATIBILITY_CLI_VERSION);
        assert_eq!(meta.model_provider.as_deref(), Some("synthetic-provider"));
        assert_eq!(meta.source, Some(SessionSource::Cli));
    }

    #[test]
    fn decodes_turn_context_without_retaining_unallowlisted_fields() {
        let record = parse_fixture(TURN_CONTEXT);
        assert_eq!(record.ordinal, None);
        let RolloutRecordKind::TurnContext(context) = record.kind else {
            panic!("expected turn context");
        };
        assert_eq!(context.turn_id.as_deref(), Some("turn-synthetic-001"));
        assert_eq!(
            context.root_turn_id.as_deref(),
            Some("root-turn-synthetic-001")
        );
        assert_eq!(context.model, "gpt-synthetic");
        assert_eq!(context.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn keeps_per_response_and_cumulative_usage_separate() {
        let record = parse_fixture(TOKEN_USAGE_RECORD);
        let RolloutRecordKind::TokenUsageRecord(usage) = record.kind else {
            panic!("expected token usage record");
        };

        assert_eq!(token_value(usage.usage.total_tokens), 100);
        assert_eq!(token_value(usage.turn_token_usage.total_tokens), 300);
        assert_eq!(token_value(usage.thread_token_usage.total_tokens), 900);
        assert_ne!(usage.usage, usage.turn_token_usage);
        assert_ne!(usage.turn_token_usage, usage.thread_token_usage);
    }

    #[test]
    fn preserves_missing_token_counters_as_missing_and_zero_as_zero() {
        let missing = parse_fixture(TOKEN_USAGE_MISSING_REASONING);
        let zero = parse_fixture(TOKEN_USAGE_ZERO_REASONING);
        let RolloutRecordKind::TokenUsageRecord(missing) = missing.kind else {
            panic!("expected missing-reasoning token usage record");
        };
        let RolloutRecordKind::TokenUsageRecord(zero) = zero.kind else {
            panic!("expected zero-reasoning token usage record");
        };

        assert_eq!(missing.usage.reasoning_output_tokens, None);
        assert_eq!(
            zero.usage.reasoning_output_tokens.map(TokenCount::get),
            Some(0)
        );
    }

    #[test]
    fn decodes_token_count_as_cumulative_and_latest_snapshots() {
        let record = parse_fixture(TOKEN_COUNT);
        let RolloutRecordKind::EventMsg(EventMessage::TokenCount(event)) = record.kind else {
            panic!("expected token count event");
        };
        let info = event.info.expect("token count info should be present");

        assert_eq!(token_value(info.total_token_usage.total_tokens), 900);
        assert_eq!(token_value(info.last_token_usage.total_tokens), 100);
        assert_eq!(info.model_context_window, Some(200_000));
        assert_eq!(event.rate_limits.as_ref().unwrap().limit_id, None);
        assert_eq!(
            event
                .rate_limits
                .as_ref()
                .unwrap()
                .primary
                .unwrap()
                .used_percent
                .get(),
            42.0
        );
        assert_eq!(event.rate_limits.unwrap().plan_type, None);
    }

    #[test]
    fn decodes_lifecycle_events_and_aliases() {
        let RolloutRecordKind::EventMsg(EventMessage::TaskStarted(started)) =
            parse_fixture(TASK_STARTED).kind
        else {
            panic!("expected task-started event");
        };
        assert_eq!(started.turn_id, "turn-synthetic-001");
        assert_eq!(
            started.root_turn_id.as_deref(),
            Some("root-turn-synthetic-001")
        );

        let RolloutRecordKind::EventMsg(EventMessage::TaskStarted(alias)) =
            parse_fixture(TURN_STARTED_ALIAS).kind
        else {
            panic!("expected turn-started alias");
        };
        assert_eq!(alias.turn_id, "turn-synthetic-002");

        let RolloutRecordKind::EventMsg(EventMessage::TaskCompleted(completed)) =
            parse_fixture(TASK_COMPLETE).kind
        else {
            panic!("expected task-complete event");
        };
        assert_eq!(completed.outcome, TaskCompletion::Failed);
        assert_eq!(completed.duration_ms, Some(58_000));

        let RolloutRecordKind::EventMsg(EventMessage::TaskCompleted(alias)) =
            parse_fixture(TURN_COMPLETE_ALIAS).kind
        else {
            panic!("expected turn-complete alias");
        };
        assert_eq!(alias.outcome, TaskCompletion::Completed);
    }

    #[test]
    fn decodes_abort_compaction_and_thread_configuration() {
        let RolloutRecordKind::EventMsg(EventMessage::TurnAborted(aborted)) =
            parse_fixture(TURN_ABORTED).kind
        else {
            panic!("expected turn-aborted event");
        };
        assert_eq!(aborted.turn_id.as_deref(), Some("turn-synthetic-003"));
        assert_eq!(aborted.reason, TurnAbortReason::Interrupted);

        let RolloutRecordKind::EventMsg(EventMessage::ContextCompacted(_)) =
            parse_fixture(CONTEXT_COMPACTED).kind
        else {
            panic!("expected context-compacted event");
        };

        let RolloutRecordKind::EventMsg(EventMessage::ThreadSettingsApplied(settings)) =
            parse_fixture(THREAD_SETTINGS_APPLIED).kind
        else {
            panic!("expected thread settings event");
        };
        assert_eq!(settings.model, "gpt-synthetic");
        assert_eq!(settings.model_provider, "synthetic-provider");
        assert_eq!(settings.reasoning_effort.as_deref(), Some("medium"));
        assert_eq!(settings.service_tier.as_deref(), Some("synthetic-tier"));
    }

    #[test]
    fn unknown_and_known_ignored_records_discard_payloads() {
        for (fixture, expected_type, classification) in [
            (
                UNSUPPORTED_RESPONSE_ITEM,
                "response_item",
                UnsupportedRecordClassification::KnownButIgnored,
            ),
            (
                UNKNOWN_FUTURE_RECORD,
                "future_telemetry_v99",
                UnsupportedRecordClassification::Unknown,
            ),
        ] {
            let RolloutRecordKind::Unsupported(record) = parse_fixture(fixture).kind else {
                panic!("expected unsupported record");
            };
            assert_eq!(record.record_type, expected_type);
            assert_eq!(record.classification, classification);
            assert!(!format!("{record:?}").contains("synthetic secret"));
        }
    }

    #[test]
    fn unknown_event_discards_payload() {
        let RolloutRecordKind::EventMsg(EventMessage::Unsupported { event_type }) =
            parse_fixture(UNKNOWN_EVENT).kind
        else {
            panic!("expected unsupported event");
        };
        assert_eq!(event_type, "future_event_v99");
        assert!(!format!("{event_type:?}").contains("synthetic secret"));
    }

    #[test]
    fn invalid_json_and_known_shape_errors_are_structural_only() {
        let invalid_json = parse_rollout_record(
            r#"{"timestamp":"2026-10-03T10:00:00Z","type":"session_meta","payload": "secret payload""#,
        )
        .expect_err("invalid JSON should fail");
        assert!(matches!(
            invalid_json,
            RolloutDecodeError::InvalidJson { .. }
        ));
        assert!(!invalid_json.to_string().contains("secret payload"));

        let wrong_shape = parse_rollout_record(
            r#"{"timestamp":"2026-10-03T10:00:00Z","type":"token_usage_record","payload":{"usage":[]}}"#,
        )
        .expect_err("known record with invalid shape should fail");
        assert!(matches!(
            wrong_shape,
            RolloutDecodeError::InvalidFieldType { .. } | RolloutDecodeError::MissingField { .. }
        ));
        assert!(!wrong_shape.to_string().contains("secret payload"));
    }

    #[test]
    fn rejects_negative_fractional_and_oversized_token_counts() {
        for (value, expected_reason) in [
            ("-1", InvalidFieldReason::NegativeNumber),
            ("1.5", InvalidFieldReason::NonIntegerNumber),
            ("9007199254740992", InvalidFieldReason::NumberTooLarge),
        ] {
            let input = r#"{"timestamp":"2026-10-03T10:00:00Z","type":"token_usage_record","payload":{"session_id":"s","thread_id":"t","turn_id":"u","usage":{"input_tokens":0},"turn_token_usage":{"input_tokens":0},"thread_token_usage":{"input_tokens":0}}}"#
                .replace("\"input_tokens\":0", &format!("\"input_tokens\":{value}"));
            let error = parse_rollout_record(&input).expect_err("unsafe token count should fail");
            assert!(matches!(
                error,
                RolloutDecodeError::InvalidFieldValue { reason, .. } if reason == expected_reason
            ));
        }
    }

    #[test]
    fn compatibility_metadata_is_pinned() {
        assert_eq!(COMPATIBILITY_CLI_VERSION, "0.157.1");
        assert_eq!(
            COMPATIBILITY_UPSTREAM_REVISION,
            "8f7a0f7a878199c6886600370e5be6bd37ca38a3"
        );
    }
}
