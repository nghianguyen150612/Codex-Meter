//! Strict AnalyticsResult v1 decoding and request-scope semantic validation.

use std::collections::HashSet;

use serde::{de::Deserializer, Deserialize, Serialize};

use super::request::{
    opaque_id, utc_timestamp, AnalyticsRequest, ConfigurationIdentity, MeterType, SAFE_INTEGER_MAX,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultError {
    pub code: &'static str,
    pub message: &'static str,
}

impl std::fmt::Display for ResultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ResultError {}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Succeeded,
    InsufficientEvidence,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Insufficient,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    QuotaResetCrossed,
    MeterUnavailable,
    MeterUnstable,
    TelemetryIncomplete,
    ConcurrentUsagePossible,
    ProcessInterrupted,
    SourceAcquisitionFailure,
    UnknownReason,
}

impl ReasonCode {
    fn order(&self) -> u8 {
        match self {
            Self::QuotaResetCrossed => 0,
            Self::MeterUnavailable => 1,
            Self::MeterUnstable => 2,
            Self::TelemetryIncomplete => 3,
            Self::ConcurrentUsagePossible => 4,
            Self::ProcessInterrupted => 5,
            Self::SourceAcquisitionFailure => 6,
            Self::UnknownReason => 7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyticsResult {
    pub schema_version: String,
    pub request_id: String,
    pub analysis_type: String,
    pub result_status: ResultStatus,
    pub dataset_configuration: ConfigurationIdentity,
    pub generated_at: String,
    pub estimator_method_version: String,
    pub quota_capacity_estimates: Vec<QuotaCapacityEstimate>,
    pub warnings: Vec<ReasonCode>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaCapacityEstimate {
    pub meter_type: MeterType,
    pub metric_kind: String,
    pub provider_authority: String,
    pub confidence: Confidence,
    pub sample_accounting: SampleAccounting,
    #[serde(
        default,
        deserialize_with = "deserialize_nonnull_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub estimated_full_capacity_raw_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_nonnull_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub estimated_used_raw_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_nonnull_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub estimated_remaining_raw_tokens: Option<f64>,
    #[serde(
        default,
        deserialize_with = "deserialize_nonnull_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub capacity_percentiles_raw_tokens: Option<CapacityPercentiles>,
    #[serde(
        default,
        deserialize_with = "deserialize_nonnull_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub reason_codes: Option<Vec<ReasonCode>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SampleAccounting {
    pub candidate_observations: u64,
    pub valid_observations: u64,
    pub excluded_observations: u64,
    pub outliers_removed: u64,
    pub used_observations: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CapacityPercentiles {
    pub p25: f64,
    pub p50: f64,
    pub p75: f64,
}

fn deserialize_nonnull_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl AnalyticsResult {
    pub fn parse_and_validate(
        bytes: &[u8],
        request: &AnalyticsRequest,
        expected_generated_at: &str,
    ) -> Result<Self, ResultError> {
        let _: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ResultError {
            code: "invalid_result_json",
            message: "Python analytics worker output is not one valid JSON document",
        })?;
        let result: Self = serde_json::from_slice(bytes).map_err(|_| result_mismatch())?;
        result.validate_for(request, expected_generated_at)?;
        Ok(result)
    }

    pub fn validate_for(
        &self,
        request: &AnalyticsRequest,
        expected_generated_at: &str,
    ) -> Result<(), ResultError> {
        if self.schema_version != "1.0.0"
            || self.request_id != request.request_id
            || self.analysis_type != "current_capacity_estimate"
            || self.dataset_configuration != request.dataset_selector.configuration
            || self.generated_at != expected_generated_at
            || !utc_timestamp(&self.generated_at)
            || self.estimator_method_version != "1.0.0"
        {
            return Err(result_mismatch());
        }
        if !opaque_id(&self.request_id) || self.result_status == ResultStatus::Failed {
            return Err(result_mismatch());
        }
        validate_configuration(&self.dataset_configuration)?;

        let expected_meters: HashSet<_> = request.target_quota_windows.iter().collect();
        let mut seen_meters = HashSet::new();
        if self.quota_capacity_estimates.len() != expected_meters.len() {
            return Err(result_mismatch());
        }
        let mut reason_union = HashSet::new();
        let mut any_sufficient = false;
        for estimate in &self.quota_capacity_estimates {
            if !expected_meters.contains(&estimate.meter_type)
                || !seen_meters.insert(&estimate.meter_type)
            {
                return Err(result_mismatch());
            }
            if estimate.metric_kind != "estimated_raw_tokens"
                || estimate.provider_authority != "codex_meter_empirical_estimate"
            {
                return Err(result_mismatch());
            }
            validate_accounting(&estimate.sample_accounting)?;
            if request
                .options
                .maximum_observations
                .is_some_and(|maximum| estimate.sample_accounting.candidate_observations > maximum)
            {
                return Err(result_mismatch());
            }
            let reasons = estimate.reason_codes.as_deref().unwrap_or_default();
            validate_unique_reasons(reasons)?;
            reason_union.extend(reasons.iter().cloned());
            validate_estimate(estimate)?;
            any_sufficient |= estimate.confidence != Confidence::Insufficient;
        }
        if self.quota_capacity_estimates.is_empty() {
            reason_union.insert(ReasonCode::UnknownReason);
        }
        let expected_status = if any_sufficient {
            ResultStatus::Succeeded
        } else {
            ResultStatus::InsufficientEvidence
        };
        if self.result_status != expected_status {
            return Err(result_mismatch());
        }
        validate_unique_reasons(&self.warnings)?;
        let expected_warnings = ordered_reasons(reason_union);
        if self.warnings != expected_warnings {
            return Err(result_mismatch());
        }
        Ok(())
    }

    pub fn to_json(&self) -> Result<String, ResultError> {
        serde_json::to_string(self).map_err(|_| result_mismatch())
    }
}

fn validate_configuration(configuration: &ConfigurationIdentity) -> Result<(), ResultError> {
    for value in [
        &configuration.plan,
        &configuration.model,
        &configuration.reasoning_level,
        &configuration.speed_mode,
        &configuration.codex_version,
    ] {
        if let super::request::ConfigurationValue::Available { value, .. } = value {
            if value.is_empty() || value.len() > 128 {
                return Err(result_mismatch());
            }
        }
    }
    Ok(())
}

fn validate_accounting(accounting: &SampleAccounting) -> Result<(), ResultError> {
    let counts = [
        accounting.candidate_observations,
        accounting.valid_observations,
        accounting.excluded_observations,
        accounting.outliers_removed,
        accounting.used_observations,
    ];
    if counts.iter().any(|count| *count > SAFE_INTEGER_MAX)
        || accounting
            .valid_observations
            .checked_add(accounting.excluded_observations)
            != Some(accounting.candidate_observations)
        || accounting.outliers_removed > accounting.valid_observations
        || accounting.valid_observations - accounting.outliers_removed
            != accounting.used_observations
    {
        return Err(result_mismatch());
    }
    Ok(())
}

fn validate_estimate(estimate: &QuotaCapacityEstimate) -> Result<(), ResultError> {
    let numeric_values = [
        estimate.estimated_full_capacity_raw_tokens,
        estimate.estimated_used_raw_tokens,
        estimate.estimated_remaining_raw_tokens,
    ];
    match estimate.confidence {
        Confidence::Insufficient => {
            if numeric_values.iter().any(Option::is_some)
                || estimate.capacity_percentiles_raw_tokens.is_some()
                || estimate.sample_accounting.used_observations >= 2
                || estimate
                    .reason_codes
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
            {
                return Err(result_mismatch());
            }
        }
        Confidence::Low | Confidence::Medium | Confidence::High => {
            let [Some(full), Some(used), Some(remaining)] = numeric_values else {
                return Err(result_mismatch());
            };
            let Some(percentiles) = &estimate.capacity_percentiles_raw_tokens else {
                return Err(result_mismatch());
            };
            if estimate.sample_accounting.used_observations < 2
                || ![
                    full,
                    used,
                    remaining,
                    percentiles.p25,
                    percentiles.p50,
                    percentiles.p75,
                ]
                .iter()
                .all(|value| value.is_finite())
                || full <= 0.0
                || used < 0.0
                || remaining < 0.0
                || !nearly_equal(used + remaining, full)
                || percentiles.p25 <= 0.0
                || percentiles.p50 <= 0.0
                || percentiles.p75 <= 0.0
                || !(percentiles.p25 <= percentiles.p50 && percentiles.p50 <= percentiles.p75)
            {
                return Err(result_mismatch());
            }
        }
    }
    Ok(())
}

fn nearly_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= right.abs() * 1e-12
}

fn validate_unique_reasons(reasons: &[ReasonCode]) -> Result<(), ResultError> {
    if reasons.iter().collect::<HashSet<_>>().len() != reasons.len()
        || reasons
            .windows(2)
            .any(|pair| pair[0].order() >= pair[1].order())
    {
        return Err(result_mismatch());
    }
    Ok(())
}

fn ordered_reasons(reasons: HashSet<ReasonCode>) -> Vec<ReasonCode> {
    let mut ordered: Vec<_> = reasons.into_iter().collect();
    ordered.sort_by_key(ReasonCode::order);
    ordered
}

fn result_mismatch() -> ResultError {
    ResultError {
        code: "result_contract_mismatch",
        message: "Python worker output did not satisfy AnalyticsResult v1 and request scope",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::AnalyticsRequest;

    const REQUEST: &str = r#"{"schema_version":"1.0.0","request_id":"request-1","analysis_type":"current_capacity_estimate","dataset_selector":{"configuration":{"plan":{"availability":"available","value":"Plus","provenance":"observed"},"model":{"availability":"available","value":"gpt-5.6-sol","provenance":"observed"},"reasoning_level":{"availability":"available","value":"high","provenance":"observed"},"speed_mode":{"availability":"unavailable","provenance":"unavailable"},"codex_version":{"availability":"unavailable","provenance":"unavailable"}}},"target_quota_windows":["five_hour"],"options":{}}"#;

    fn request() -> AnalyticsRequest {
        AnalyticsRequest::parse(REQUEST.as_bytes()).unwrap()
    }

    fn insufficient_result() -> String {
        r#"{"schema_version":"1.0.0","request_id":"request-1","analysis_type":"current_capacity_estimate","result_status":"insufficient_evidence","dataset_configuration":{"plan":{"availability":"available","value":"Plus","provenance":"observed"},"model":{"availability":"available","value":"gpt-5.6-sol","provenance":"observed"},"reasoning_level":{"availability":"available","value":"high","provenance":"observed"},"speed_mode":{"availability":"unavailable","provenance":"unavailable"},"codex_version":{"availability":"unavailable","provenance":"unavailable"}},"generated_at":"2026-10-16T07:00:00Z","estimator_method_version":"1.0.0","quota_capacity_estimates":[{"meter_type":"five_hour","metric_kind":"estimated_raw_tokens","provider_authority":"codex_meter_empirical_estimate","confidence":"insufficient","sample_accounting":{"candidate_observations":0,"valid_observations":0,"excluded_observations":0,"outliers_removed":0,"used_observations":0},"reason_codes":["unknown_reason"]}],"warnings":["unknown_reason"]}"#.to_owned()
    }

    fn succeeded_result(request: &AnalyticsRequest) -> serde_json::Value {
        serde_json::json!({
            "schema_version": "1.0.0",
            "request_id": request.request_id,
            "analysis_type": "current_capacity_estimate",
            "result_status": "succeeded",
            "dataset_configuration": request.dataset_selector.configuration,
            "generated_at": "2026-10-16T07:00:00Z",
            "estimator_method_version": "1.0.0",
            "quota_capacity_estimates": [{
                "meter_type": "five_hour",
                "metric_kind": "estimated_raw_tokens",
                "provider_authority": "codex_meter_empirical_estimate",
                "confidence": "medium",
                "sample_accounting": {
                    "candidate_observations": 4,
                    "valid_observations": 4,
                    "excluded_observations": 0,
                    "outliers_removed": 0,
                    "used_observations": 4
                },
                "estimated_full_capacity_raw_tokens": 362500.0,
                "estimated_used_raw_tokens": 87000.0,
                "estimated_remaining_raw_tokens": 275500.0,
                "capacity_percentiles_raw_tokens": {
                    "p25": 350000.0,
                    "p50": 362500.0,
                    "p75": 375000.0
                }
            }],
            "warnings": []
        })
    }

    fn assert_result_rejected(result: &serde_json::Value, request: &AnalyticsRequest) {
        assert!(
            AnalyticsResult::parse_and_validate(
                result.to_string().as_bytes(),
                request,
                "2026-10-16T07:00:00Z"
            )
            .is_err(),
            "result unexpectedly passed validation: {result}"
        );
    }

    #[test]
    fn accepts_a_valid_insufficient_result_and_rejects_scope_or_semantic_changes() {
        let valid = insufficient_result();
        assert!(AnalyticsResult::parse_and_validate(
            valid.as_bytes(),
            &request(),
            "2026-10-16T07:00:00Z"
        )
        .is_ok());
        for invalid in [
            valid.replace("request-1", "request-2"),
            valid.replace("insufficient_evidence", "succeeded"),
            valid.replace(
                "\"used_observations\":0",
                "\"used_observations\":9007199254740992",
            ),
            valid.replace(
                "\"unknown_reason\"]}],\"warnings\":[\"unknown_reason\"]",
                "\"unknown_reason\"],\"extra\":true}],\"warnings\":[\"unknown_reason\"]",
            ),
            valid.replace("\"warnings\":[\"unknown_reason\"]", "\"warnings\":[]"),
            valid.replace(
                "\"request_id\":\"request-1\"",
                "\"request_id\":\"request-1\",\"request_id\":\"request-1\"",
            ),
        ] {
            assert!(AnalyticsResult::parse_and_validate(
                invalid.as_bytes(),
                &request(),
                "2026-10-16T07:00:00Z"
            )
            .is_err());
        }

        let constrained_request = AnalyticsRequest::parse(
            REQUEST
                .replace(
                    "\"options\":{}",
                    "\"options\":{\"maximum_observations\":10}",
                )
                .as_bytes(),
        )
        .unwrap();
        let over_limit = valid
            .replace(
                "\"candidate_observations\":0",
                "\"candidate_observations\":11",
            )
            .replace(
                "\"excluded_observations\":0",
                "\"excluded_observations\":11",
            );
        assert!(AnalyticsResult::parse_and_validate(
            over_limit.as_bytes(),
            &constrained_request,
            "2026-10-16T07:00:00Z"
        )
        .is_err());
    }

    #[test]
    fn rejects_explicit_null_and_unknown_fields() {
        let valid = insufficient_result();
        let explicit_null = valid.replace(
            "\"confidence\":\"insufficient\"",
            "\"confidence\":\"insufficient\",\"estimated_full_capacity_raw_tokens\":null",
        );
        assert!(AnalyticsResult::parse_and_validate(
            explicit_null.as_bytes(),
            &request(),
            "2026-10-16T07:00:00Z"
        )
        .is_err());
        assert_eq!(
            AnalyticsResult::parse_and_validate(b"not-json", &request(), "2026-10-16T07:00:00Z")
                .unwrap_err()
                .code,
            "invalid_result_json"
        );
    }

    #[test]
    fn accepts_succeeded_and_mixed_meter_results() {
        let one_meter_request = request();
        let succeeded = succeeded_result(&one_meter_request);
        assert!(AnalyticsResult::parse_and_validate(
            succeeded.to_string().as_bytes(),
            &one_meter_request,
            "2026-10-16T07:00:00Z"
        )
        .is_ok());

        let two_meter_request = AnalyticsRequest::parse(
            REQUEST
                .replace(
                    "\"target_quota_windows\":[\"five_hour\"]",
                    "\"target_quota_windows\":[\"five_hour\",\"weekly\"]",
                )
                .as_bytes(),
        )
        .unwrap();
        let mut mixed = succeeded_result(&two_meter_request);
        mixed["quota_capacity_estimates"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
                "meter_type": "weekly",
                "metric_kind": "estimated_raw_tokens",
                "provider_authority": "codex_meter_empirical_estimate",
                "confidence": "insufficient",
                "sample_accounting": {
                    "candidate_observations": 1,
                    "valid_observations": 0,
                    "excluded_observations": 1,
                    "outliers_removed": 0,
                    "used_observations": 0
                },
                "reason_codes": ["meter_unavailable"]
            }));
        mixed["warnings"] = serde_json::json!(["meter_unavailable"]);
        assert!(AnalyticsResult::parse_and_validate(
            mixed.to_string().as_bytes(),
            &two_meter_request,
            "2026-10-16T07:00:00Z"
        )
        .is_ok());
    }

    #[test]
    fn rejects_result_scope_and_authority_changes() {
        let request = request();
        let valid = succeeded_result(&request);
        let mut cases = Vec::new();

        let mut changed = valid.clone();
        changed["request_id"] = serde_json::json!("another-request");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["analysis_type"] = serde_json::json!("history_summary");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["dataset_configuration"]["plan"]["value"] = serde_json::json!("Pro");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["meter_type"] = serde_json::json!("weekly");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["metric_kind"] = serde_json::json!("tokens");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["provider_authority"] =
            serde_json::json!("provider_official");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["estimator_method_version"] = serde_json::json!("2.0.0");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["schema_version"] = serde_json::json!("2.0.0");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["generated_at"] = serde_json::json!("2026-10-16T07:00:01Z");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["confidence"] = serde_json::json!("unrecognized");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["result_status"] = serde_json::json!("failed");
        cases.push(changed);
        for invalid in cases {
            assert_result_rejected(&invalid, &request);
        }

        let two_meter_request = AnalyticsRequest::parse(
            REQUEST
                .replace(
                    "\"target_quota_windows\":[\"five_hour\"]",
                    "\"target_quota_windows\":[\"five_hour\",\"weekly\"]",
                )
                .as_bytes(),
        )
        .unwrap();
        let mut missing = succeeded_result(&two_meter_request);
        missing["quota_capacity_estimates"] = serde_json::json!([]);
        assert_result_rejected(&missing, &two_meter_request);
        let mut fabricated = succeeded_result(&two_meter_request);
        fabricated["quota_capacity_estimates"][0]["meter_type"] = serde_json::json!("weekly");
        let duplicate = fabricated["quota_capacity_estimates"][0].clone();
        fabricated["quota_capacity_estimates"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_result_rejected(&fabricated, &two_meter_request);
    }

    #[test]
    fn rejects_invalid_numeric_and_sample_accounting_semantics() {
        let request = request();
        let valid = succeeded_result(&request);
        let mut cases = Vec::new();

        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["estimated_full_capacity_raw_tokens"] =
            serde_json::json!(0.0);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["estimated_used_raw_tokens"] =
            serde_json::json!(-1.0);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["estimated_remaining_raw_tokens"] =
            serde_json::json!(275499.0);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["capacity_percentiles_raw_tokens"] =
            serde_json::json!({"p25": 380000.0, "p50": 362500.0, "p75": 375000.0});
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["capacity_percentiles_raw_tokens"] =
            serde_json::json!({"p25": 0.0, "p50": 362500.0, "p75": 375000.0});
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]
            .as_object_mut()
            .unwrap()
            .remove("capacity_percentiles_raw_tokens");
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["sample_accounting"]["candidate_observations"] =
            serde_json::json!(9_007_199_254_740_992_u64);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["sample_accounting"]["excluded_observations"] =
            serde_json::json!(1);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]["sample_accounting"]["outliers_removed"] =
            serde_json::json!(1);
        cases.push(changed);
        let mut changed = valid.clone();
        changed["quota_capacity_estimates"][0]
            .as_object_mut()
            .unwrap()
            .remove("estimated_full_capacity_raw_tokens");
        cases.push(changed);
        for invalid in cases {
            assert_result_rejected(&invalid, &request);
        }

        let mut non_finite = valid.to_string();
        non_finite = non_finite.replace(
            "\"estimated_full_capacity_raw_tokens\":362500.0",
            "\"estimated_full_capacity_raw_tokens\":Infinity",
        );
        assert_eq!(
            AnalyticsResult::parse_and_validate(
                non_finite.as_bytes(),
                &request,
                "2026-10-16T07:00:00Z"
            )
            .unwrap_err()
            .code,
            "invalid_result_json"
        );

        let mut insufficient =
            serde_json::from_str::<serde_json::Value>(&insufficient_result()).unwrap();
        insufficient["quota_capacity_estimates"][0]["estimated_full_capacity_raw_tokens"] =
            serde_json::json!(0.0);
        assert_result_rejected(&insufficient, &request);
    }

    #[test]
    fn rejects_invalid_reason_warning_and_unknown_field_semantics() {
        let request = request();
        let valid = succeeded_result(&request);
        let mut invalid_reason = valid.clone();
        invalid_reason["quota_capacity_estimates"][0]["reason_codes"] =
            serde_json::json!(["made_up"]);
        assert_result_rejected(&invalid_reason, &request);

        let mut inconsistent_warnings = valid.clone();
        inconsistent_warnings["quota_capacity_estimates"][0]["reason_codes"] =
            serde_json::json!(["meter_unavailable"]);
        inconsistent_warnings["warnings"] = serde_json::json!([]);
        assert_result_rejected(&inconsistent_warnings, &request);

        let mut unordered = valid.clone();
        unordered["quota_capacity_estimates"][0]["reason_codes"] =
            serde_json::json!(["meter_unstable", "quota_reset_crossed"]);
        unordered["warnings"] = serde_json::json!(["quota_reset_crossed", "meter_unstable"]);
        assert_result_rejected(&unordered, &request);

        let mut unknown = valid;
        unknown["untrusted_extra"] = serde_json::json!(true);
        assert_result_rejected(&unknown, &request);
    }
}
