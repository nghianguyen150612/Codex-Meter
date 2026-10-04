//! Typed v1 normalized token-event structures.

use serde::{Deserialize, Serialize};

/// A normalized event emitted by the Codex raw-token adapter.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct NormalizedTokenEvent {
    pub schema_version: SchemaVersion,
    pub event_id: String,
    pub event_type: NormalizedEventType,
    pub source_instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_cursor_id: Option<String>,
    pub event_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub payload: NormalizedTokenPayload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum SchemaVersion {
    #[serde(rename = "1.0.0")]
    V1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum NormalizedEventType {
    #[serde(rename = "session_detected")]
    SessionDetected,
    #[serde(rename = "token_counters_updated")]
    TokenCountersUpdated,
    #[serde(rename = "configuration_evidence_observed")]
    ConfigurationEvidenceObserved,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct NormalizedSessionEvent {
    pub schema_version: SchemaVersion,
    pub event_id: String,
    pub event_type: NormalizedEventType,
    pub source_instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_cursor_id: Option<String>,
    pub event_at: String,
    pub session_id: String,
    pub payload: LifecyclePayload,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct LifecyclePayload {
    pub kind: LifecyclePayloadKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum LifecyclePayloadKind {
    #[serde(rename = "lifecycle")]
    Lifecycle,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct NormalizedConfigurationEvent {
    pub schema_version: SchemaVersion,
    pub event_id: String,
    pub event_type: NormalizedEventType,
    pub source_instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safe_cursor_id: Option<String>,
    pub event_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub payload: ConfigurationPayload,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ConfigurationPayload {
    pub kind: ConfigurationPayloadKind,
    pub configuration: ConfigurationIdentity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum ConfigurationPayloadKind {
    #[serde(rename = "configuration")]
    Configuration,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ConfigurationIdentity {
    pub plan: ConfigurationValue,
    pub model: ConfigurationValue,
    pub reasoning_level: ConfigurationValue,
    pub speed_mode: ConfigurationValue,
    pub codex_version: ConfigurationValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ConfigurationValue {
    pub availability: MetricAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub provenance: MetricProvenance,
}

/// A normalized v1 quota snapshot.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct NormalizedQuotaSample {
    pub schema_version: SchemaVersion,
    pub sample_id: String,
    pub meter_type: QuotaMeterType,
    pub sampled_at: String,
    pub used_percent: PercentageMetric,
    pub remaining_percent: PercentageMetric,
    pub reset_evidence: QuotaWindowIdentity,
    pub configuration: ConfigurationIdentity,
    pub acquisition_status: AcquisitionStatus,
    pub source_kind: QuotaSourceKind,
}

impl Eq for NormalizedQuotaSample {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaMeterType {
    FiveHour,
    Weekly,
}

impl QuotaMeterType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::Weekly => "weekly",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct PercentageMetric {
    pub availability: MetricAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub provenance: MetricProvenance,
}

impl Eq for PercentageMetric {}

impl PercentageMetric {
    pub(crate) fn observed(value: f64) -> Self {
        Self {
            availability: MetricAvailability::Available,
            value: Some(value),
            provenance: MetricProvenance::Observed,
        }
    }

    pub(crate) fn derived(value: f64) -> Self {
        Self {
            availability: MetricAvailability::Available,
            value: Some(value),
            provenance: MetricProvenance::Derived,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub enum QuotaWindowIdentity {
    Unavailable,
    Available {
        meter_type: QuotaMeterType,
        #[serde(skip_serializing_if = "Option::is_none")]
        local_window_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        observed_reset_at: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        evidence_first_sample_at: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        evidence_last_sample_at: Option<String>,
        identity_provenance: QuotaIdentityProvenance,
        identity_confidence: QuotaIdentityConfidence,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaIdentityProvenance {
    ObservedReset,
    LocallyInferred,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaIdentityConfidence {
    High,
    Medium,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionStatus {
    Succeeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum QuotaSourceKind {
    #[serde(rename = "local_meter")]
    LocalMeter,
}

impl ConfigurationValue {
    pub(crate) fn observed(value: impl Into<String>) -> Self {
        Self {
            availability: MetricAvailability::Available,
            value: Some(value.into()),
            provenance: MetricProvenance::Observed,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            availability: MetricAvailability::Unavailable,
            value: None,
            provenance: MetricProvenance::Unavailable,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct NormalizedTokenPayload {
    pub kind: TokenPayloadKind,
    pub token_counters: TokenCounters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum TokenPayloadKind {
    #[serde(rename = "token_counters")]
    TokenCounters,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TokenCounters {
    pub uncached_input: TokenMetric,
    pub cached_input: TokenMetric,
    pub output: TokenMetric,
    pub reasoning_output: TokenMetric,
    pub raw_total: TokenMetric,
}

impl TokenCounters {
    pub(crate) fn from_per_response(usage: &super::codex_rollout::TokenUsage) -> Self {
        Self {
            uncached_input: TokenMetric::unavailable(),
            cached_input: TokenMetric::from_source(usage.cached_input_tokens),
            output: TokenMetric::from_source(usage.output_tokens),
            reasoning_output: TokenMetric::from_source(usage.reasoning_output_tokens),
            raw_total: TokenMetric::from_source(usage.total_tokens),
        }
    }
}

/// Availability of one normalized token metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricAvailability {
    Available,
    Unavailable,
}

/// Provenance of one normalized token metric.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricProvenance {
    Observed,
    Derived,
    Unavailable,
}

/// A v1 token metric. Constructors preserve the schema invariant that only
/// available metrics carry values and unavailable metrics carry no value.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TokenMetric {
    pub availability: MetricAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<u64>,
    pub provenance: MetricProvenance,
}

impl TokenMetric {
    pub(crate) fn from_source(value: Option<super::codex_rollout::TokenCount>) -> Self {
        match value {
            Some(value) => Self::observed(value.get()),
            None => Self::unavailable(),
        }
    }

    pub(crate) fn observed(value: u64) -> Self {
        Self {
            availability: MetricAvailability::Available,
            value: Some(value),
            provenance: MetricProvenance::Observed,
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            availability: MetricAvailability::Unavailable,
            value: None,
            provenance: MetricProvenance::Unavailable,
        }
    }
}
