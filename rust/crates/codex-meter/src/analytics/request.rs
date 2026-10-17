//! Closed AnalyticsRequest v1 models and P027 execution-policy validation.

use std::collections::HashSet;

use serde::{de::Deserializer, Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub const MAXIMUM_REQUEST_BYTES: usize = 64 * 1024;
pub const MAXIMUM_OBSERVATIONS: u64 = 100_000;
pub const SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;
pub const CONFIGURATION_FIELDS: [&str; 5] = [
    "plan",
    "model",
    "reasoning_level",
    "speed_mode",
    "codex_version",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterType {
    FiveHour,
    Weekly,
}

impl MeterType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FiveHour => "five_hour",
            Self::Weekly => "weekly",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "availability", deny_unknown_fields)]
pub enum ConfigurationValue {
    #[serde(rename = "available")]
    Available {
        value: String,
        provenance: AvailableProvenance,
    },
    #[serde(rename = "unavailable")]
    Unavailable { provenance: UnavailableProvenance },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailableProvenance {
    Observed,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableProvenance {
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationIdentity {
    pub plan: ConfigurationValue,
    pub model: ConfigurationValue,
    pub reasoning_level: ConfigurationValue,
    pub speed_mode: ConfigurationValue,
    pub codex_version: ConfigurationValue,
}

impl ConfigurationIdentity {
    pub fn values(&self) -> [Option<&str>; 5] {
        [
            config_value(&self.plan),
            config_value(&self.model),
            config_value(&self.reasoning_level),
            config_value(&self.speed_mode),
            config_value(&self.codex_version),
        ]
    }
}

fn config_value(value: &ConfigurationValue) -> Option<&str> {
    match value {
        ConfigurationValue::Available { value, .. } => Some(value),
        ConfigurationValue::Unavailable { .. } => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasetSelector {
    pub configuration: ConfigurationIdentity,
    #[serde(default, deserialize_with = "deserialize_nonnull_option")]
    pub observed_from: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nonnull_option")]
    pub observed_through: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum QualityGrade {
    A,
    B,
    C,
    D,
    X,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RequestOptions {
    #[serde(default, deserialize_with = "deserialize_nonnull_option")]
    pub minimum_quality: Option<QualityGrade>,
    #[serde(default, deserialize_with = "deserialize_nonnull_option")]
    pub maximum_observations: Option<u64>,
}

fn deserialize_nonnull_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyticsRequest {
    pub schema_version: String,
    pub request_id: String,
    pub analysis_type: String,
    pub dataset_selector: DatasetSelector,
    pub target_quota_windows: Vec<MeterType>,
    pub options: RequestOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyticsRequestError {
    pub code: &'static str,
    pub message: &'static str,
}

impl std::fmt::Display for AnalyticsRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AnalyticsRequestError {}

impl AnalyticsRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self, AnalyticsRequestError> {
        if bytes.len() > MAXIMUM_REQUEST_BYTES {
            return Err(invalid_request("request exceeds the 64 KiB limit"));
        }
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|_| invalid_request("request is not valid closed AnalyticsRequest v1 JSON"))?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), AnalyticsRequestError> {
        if self.schema_version != "1.0.0" {
            return Err(invalid_request(
                "unsupported AnalyticsRequest schema version",
            ));
        }
        if self.analysis_type != "current_capacity_estimate" {
            return Err(AnalyticsRequestError {
                code: "unsupported_analysis_type",
                message: "only current_capacity_estimate is supported",
            });
        }
        if !opaque_id(&self.request_id) {
            return Err(invalid_request(
                "request_id does not match the v1 opaque ID format",
            ));
        }
        if self
            .dataset_selector
            .configuration
            .values()
            .into_iter()
            .flatten()
            .any(|value| value.is_empty() || value.len() > 128)
        {
            return Err(invalid_request(
                "configuration identity contains an invalid value",
            ));
        }
        for timestamp in [
            self.dataset_selector.observed_from.as_deref(),
            self.dataset_selector.observed_through.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !utc_timestamp(timestamp) {
                return Err(invalid_request(
                    "history bounds must be valid UTC timestamps ending in Z",
                ));
            }
        }
        if let (Some(from), Some(through)) = (
            self.dataset_selector.observed_from.as_deref(),
            self.dataset_selector.observed_through.as_deref(),
        ) {
            if parse_utc_timestamp(from) > parse_utc_timestamp(through) {
                return Err(invalid_request(
                    "observed_from must not be after observed_through",
                ));
            }
        }
        let mut targets = HashSet::new();
        if self
            .target_quota_windows
            .iter()
            .any(|meter| !targets.insert(meter))
        {
            return Err(invalid_request(
                "target_quota_windows must contain unique meters",
            ));
        }
        if self.options.minimum_quality.is_some() {
            return Err(AnalyticsRequestError {
                code: "unsupported_estimator_option",
                message: "minimum_quality is not supported by the P027 P018 eligibility policy",
            });
        }
        if self.options.maximum_observations.is_some_and(|maximum| {
            maximum == 0 || maximum > MAXIMUM_OBSERVATIONS || maximum > SAFE_INTEGER_MAX
        }) {
            return Err(invalid_request(
                "maximum_observations exceeds the supported reader limit",
            ));
        }
        Ok(())
    }
}

pub fn opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

pub fn utc_timestamp(value: &str) -> bool {
    parse_utc_timestamp(value).is_some()
}

fn parse_utc_timestamp(value: &str) -> Option<OffsetDateTime> {
    if !value.ends_with('Z') {
        return None;
    }
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

fn invalid_request(message: &'static str) -> AnalyticsRequestError {
    AnalyticsRequestError {
        code: "invalid_request",
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"{"schema_version":"1.0.0","request_id":"request-1","analysis_type":"current_capacity_estimate","dataset_selector":{"configuration":{"plan":{"availability":"available","value":"Plus","provenance":"observed"},"model":{"availability":"available","value":"gpt-5.6-sol","provenance":"observed"},"reasoning_level":{"availability":"available","value":"high","provenance":"observed"},"speed_mode":{"availability":"unavailable","provenance":"unavailable"},"codex_version":{"availability":"unavailable","provenance":"unavailable"}},"observed_from":"2026-10-01T00:00:00Z","observed_through":"2026-10-02T00:00:00Z"},"target_quota_windows":["five_hour"],"options":{"maximum_observations":10}}"#;

    #[test]
    fn parses_valid_request_without_losing_configuration_provenance() {
        let request = AnalyticsRequest::parse(VALID.as_bytes()).unwrap();
        assert_eq!(request.request_id, "request-1");
        assert!(matches!(
            request.dataset_selector.configuration.speed_mode,
            ConfigurationValue::Unavailable { .. }
        ));
        assert_eq!(request.options.maximum_observations, Some(10));
    }

    #[test]
    fn rejects_unknown_fields_malformed_json_and_unknown_configuration_provenance() {
        for text in [
            "{",
            &VALID.replace("\"options\":{", "\"content\":\"x\",\"options\":{"),
            &VALID.replace("\"provenance\":\"observed\"", "\"provenance\":\"guessed\""),
            &VALID.replace(
                "\"observed_from\":\"2026-10-01T00:00:00Z\"",
                "\"observed_from\":null",
            ),
            &VALID.replace(
                "\"maximum_observations\":10",
                "\"maximum_observations\":null",
            ),
        ] {
            assert!(AnalyticsRequest::parse(text.as_bytes()).is_err());
        }
    }

    #[test]
    fn typed_deserialization_rejects_duplicate_top_level_and_nested_fields() {
        let duplicate_top = VALID.replace(
            "\"request_id\":\"request-1\"",
            "\"request_id\":\"request-1\",\"request_id\":\"request-1\"",
        );
        let duplicate_nested = VALID.replacen(
            "\"availability\":\"available\"",
            "\"availability\":\"available\",\"availability\":\"available\"",
            1,
        );
        assert!(AnalyticsRequest::parse(duplicate_top.as_bytes()).is_err());
        assert!(AnalyticsRequest::parse(duplicate_nested.as_bytes()).is_err());
    }

    #[test]
    fn analytics_request_v1_cross_language_compatibility_corpus() {
        let cases = [
            (VALID.to_owned(), None),
            (VALID.replace("1.0.0", "2.0.0"), Some("invalid_request")),
            (
                VALID.replace("current_capacity_estimate", "history_summary"),
                Some("unsupported_analysis_type"),
            ),
            (
                VALID.replace("\"maximum_observations\":10", "\"minimum_quality\":\"C\""),
                Some("unsupported_estimator_option"),
            ),
            (
                VALID.replace("\"maximum_observations\":10", "\"minimum_quality\":null"),
                Some("invalid_request"),
            ),
            (
                VALID.replace("\"maximum_observations\":10", "\"maximum_observations\":0"),
                Some("invalid_request"),
            ),
            (
                VALID.replace(
                    "\"maximum_observations\":10",
                    "\"maximum_observations\":9007199254740992",
                ),
                Some("invalid_request"),
            ),
            (
                VALID.replace("[\"five_hour\"]", "[\"five_hour\",\"five_hour\"]"),
                Some("invalid_request"),
            ),
            (
                VALID.replace("2026-10-01T00:00:00Z", "2026-10-01T00:00:00+00:00"),
                Some("invalid_request"),
            ),
            (
                VALID.replace("\"value\":\"Plus\"", "\"value\":null"),
                Some("invalid_request"),
            ),
            (
                VALID.replace(
                    "\"maximum_observations\":10",
                    "\"maximum_observations\":NaN",
                ),
                Some("invalid_request"),
            ),
        ];
        for (raw, expected_code) in cases {
            let result = AnalyticsRequest::parse(raw.as_bytes());
            match expected_code {
                None => assert!(result.is_ok()),
                Some(code) => assert_eq!(result.unwrap_err().code, code),
            }
        }
    }

    #[test]
    fn rejects_duplicate_target_and_unsupported_option() {
        let duplicate = VALID.replace("[\"five_hour\"]", "[\"five_hour\",\"five_hour\"]");
        assert_eq!(
            AnalyticsRequest::parse(duplicate.as_bytes())
                .unwrap_err()
                .code,
            "invalid_request"
        );
        let minimum = VALID.replace("\"maximum_observations\":10", "\"minimum_quality\":\"C\"");
        assert_eq!(
            AnalyticsRequest::parse(minimum.as_bytes())
                .unwrap_err()
                .code,
            "unsupported_estimator_option"
        );
    }

    #[test]
    fn rejects_invalid_ranges_versions_and_request_ids() {
        for text in [
            VALID.replace("1.0.0", "2.0.0"),
            VALID.replace("request-1", "has space"),
            VALID.replace("2026-10-01T00:00:00Z", "2026-10-03T00:00:00Z"),
            VALID.replace("current_capacity_estimate", "history_summary"),
        ] {
            assert!(AnalyticsRequest::parse(text.as_bytes()).is_err());
        }
        for maximum in [0, (MAXIMUM_OBSERVATIONS + 1) as i64] {
            let request = VALID.replace(
                "\"maximum_observations\":10",
                &format!("\"maximum_observations\":{maximum}"),
            );
            assert_eq!(
                AnalyticsRequest::parse(request.as_bytes())
                    .unwrap_err()
                    .code,
                "invalid_request"
            );
        }
    }

    #[test]
    fn history_range_comparison_uses_timestamp_values_not_string_order() {
        let forward = VALID.replace("2026-10-02T00:00:00Z", "2026-10-01T00:00:00.1Z");
        assert!(AnalyticsRequest::parse(forward.as_bytes()).is_ok());
        let reversed = VALID
            .replace("2026-10-01T00:00:00Z", "2026-10-01T00:00:00.1Z")
            .replace("2026-10-02T00:00:00Z", "2026-10-01T00:00:00Z");
        assert!(AnalyticsRequest::parse(reversed.as_bytes()).is_err());
    }

    #[test]
    fn rejects_oversized_input() {
        let oversized = vec![b' '; MAXIMUM_REQUEST_BYTES + 1];
        assert_eq!(
            AnalyticsRequest::parse(&oversized).unwrap_err().code,
            "invalid_request"
        );
    }
}
