//! Strict, versioned decoding and semantic validation for workload worker output.

use std::collections::HashSet;

use serde::Deserialize;

use super::{AnalyticsRequest, AnalyticsResult, Confidence, MeterType, ReasonCode, ResultError};

const SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;
const MAXIMUM_DECISIONS: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadResult {
    pub schema_version: String,
    pub analysis_type: String,
    pub result_status: String,
    pub request_id: String,
    pub generated_at: String,
    pub dataset_configuration: super::ConfigurationIdentity,
    pub workload_method_version: String,
    pub capacity_estimator_method_version: String,
    pub capacity_result: AnalyticsResult,
    pub task_size_distribution: TaskSizeDistribution,
    pub meter_predictions: Vec<MeterPrediction>,
    pub capacity_request_id: String,
    pub capacity_generated_at: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationKey {
    #[serde(deserialize_with = "required_nullable")]
    pub plan: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub model: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub reasoning_level: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub speed_mode: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub codex_version: Option<String>,
}

impl ConfigurationKey {
    fn values(&self) -> [Option<&str>; 5] {
        [
            self.plan.as_deref(),
            self.model.as_deref(),
            self.reasoning_level.as_deref(),
            self.speed_mode.as_deref(),
            self.codex_version.as_deref(),
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSizeDistribution {
    pub method_version: String,
    pub configuration: ConfigurationKey,
    pub status: String,
    #[serde(deserialize_with = "required_nullable")]
    pub p25_raw_tokens: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub p50_raw_tokens: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub p75_raw_tokens: Option<f64>,
    pub accounting: TaskSizeAccounting,
    pub decisions: Vec<TaskSizeDecision>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSizeAccounting {
    pub candidate_observations: u64,
    pub eligible_observations: u64,
    pub excluded_observations: u64,
    pub c_quality_observations: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum TaskSizeDecision {
    #[serde(rename = "sample")]
    Sample {
        observation_id: String,
        configuration: ConfigurationKey,
        raw_total: u64,
        token_quality: String,
    },
    #[serde(rename = "exclusion")]
    Exclusion {
        observation_id: String,
        reasons: Vec<String>,
    },
}

impl TaskSizeDecision {
    fn observation_id(&self) -> &str {
        match self {
            Self::Sample { observation_id, .. } | Self::Exclusion { observation_id, .. } => {
                observation_id
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterPrediction {
    pub meter_type: MeterType,
    pub configuration: ConfigurationKey,
    pub capacity_confidence: Confidence,
    pub capacity_provider_authority: String,
    pub capacity_reason_codes: Vec<ReasonCode>,
    #[serde(deserialize_with = "required_nullable")]
    pub estimated_remaining_raw_tokens: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub light_task_count: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub normal_task_count: Option<u64>,
    #[serde(deserialize_with = "required_nullable")]
    pub heavy_task_count: Option<u64>,
    pub status: String,
    pub insufficiency_reasons: Vec<String>,
}

impl WorkloadResult {
    pub fn parse_and_validate(
        bytes: &[u8],
        request: &AnalyticsRequest,
        expected_generated_at: &str,
        positions: super::ManualMeterPositions,
    ) -> Result<Self, ResultError> {
        let mut result: Self = serde_json::from_slice(bytes).map_err(|_| mismatch())?;
        let capacity_value = serde_json::to_vec(&result.capacity_result).map_err(|_| mismatch())?;
        result.capacity_result = AnalyticsResult::parse_and_validate(
            &capacity_value,
            request,
            expected_generated_at,
            positions,
        )?;
        result.validate_for(request, expected_generated_at)?;
        Ok(result)
    }

    fn validate_for(
        &self,
        request: &AnalyticsRequest,
        expected_generated_at: &str,
    ) -> Result<(), ResultError> {
        if self.schema_version != "1.0.0"
            || self.analysis_type != "remaining_workload_forecast"
            || self.request_id != request.request_id
            || self.generated_at != expected_generated_at
            || self.capacity_result.request_id != request.request_id
            || self.capacity_result.generated_at != expected_generated_at
            || self.capacity_request_id != request.request_id
            || self.capacity_generated_at != expected_generated_at
            || self.capacity_estimator_method_version != "1.0.0"
            || self.workload_method_version != "1.0.0"
            || self.dataset_configuration != request.dataset_selector.configuration
        {
            return Err(mismatch());
        }
        let expected_configuration = request.dataset_selector.configuration.values();
        let distribution = &self.task_size_distribution;
        if distribution.method_version != self.workload_method_version
            || distribution.configuration.values() != expected_configuration
            || distribution.decisions.len() > MAXIMUM_DECISIONS
            || distribution.accounting.candidate_observations as usize
                != distribution.decisions.len()
            || distribution.accounting.candidate_observations
                != distribution.accounting.eligible_observations
                    + distribution.accounting.excluded_observations
            || distribution.accounting.candidate_observations
                > request
                    .options
                    .maximum_observations
                    .unwrap_or(super::MAXIMUM_OBSERVATIONS)
        {
            return Err(mismatch());
        }
        let mut ids = HashSet::new();
        let mut eligible = 0_u64;
        let mut excluded = 0_u64;
        let mut c_quality = 0_u64;
        let mut previous_id = "";
        for decision in &distribution.decisions {
            let id = decision.observation_id();
            if !opaque_id(id) || !ids.insert(id) || id < previous_id {
                return Err(mismatch());
            }
            previous_id = id;
            match decision {
                TaskSizeDecision::Sample {
                    configuration,
                    raw_total,
                    token_quality,
                    ..
                } => {
                    if configuration.values() != expected_configuration
                        || *raw_total == 0
                        || !matches!(token_quality.as_str(), "A" | "B" | "C")
                    {
                        return Err(mismatch());
                    }
                    eligible += 1;
                    c_quality += u64::from(token_quality == "C");
                }
                TaskSizeDecision::Exclusion { reasons, .. } => {
                    let allowed = [
                        "token_not_valid",
                        "token_quality_ineligible",
                        "raw_total_unavailable",
                        "zero_raw_total",
                    ];
                    if reasons.is_empty()
                        || reasons
                            .iter()
                            .any(|reason| !allowed.contains(&reason.as_str()))
                        || reasons.windows(2).any(|pair| {
                            allowed.iter().position(|reason| *reason == pair[0])
                                >= allowed.iter().position(|reason| *reason == pair[1])
                        })
                    {
                        return Err(mismatch());
                    }
                    excluded += 1;
                }
            }
        }
        if eligible != distribution.accounting.eligible_observations
            || excluded != distribution.accounting.excluded_observations
            || c_quality != distribution.accounting.c_quality_observations
        {
            return Err(mismatch());
        }
        let quantiles = [
            distribution.p25_raw_tokens,
            distribution.p50_raw_tokens,
            distribution.p75_raw_tokens,
        ];
        if eligible < 3 {
            if distribution.status != "insufficient" || quantiles.iter().any(Option::is_some) {
                return Err(mismatch());
            }
        } else {
            let [Some(p25), Some(p50), Some(p75)] = quantiles else {
                return Err(mismatch());
            };
            if distribution.status != "available"
                || [p25, p50, p75]
                    .iter()
                    .any(|value| !value.is_finite() || *value <= 0.0)
                || !(p25 <= p50 && p50 <= p75)
            {
                return Err(mismatch());
            }
        }

        if self.meter_predictions.len() != self.capacity_result.quota_capacity_estimates.len() {
            return Err(mismatch());
        }
        for (prediction, capacity) in self
            .meter_predictions
            .iter()
            .zip(&self.capacity_result.quota_capacity_estimates)
        {
            if prediction.meter_type != capacity.meter_type
                || prediction.configuration.values() != expected_configuration
                || prediction.capacity_confidence != capacity.confidence
                || prediction.capacity_provider_authority != capacity.provider_authority
                || prediction.capacity_reason_codes
                    != capacity.reason_codes.clone().unwrap_or_default()
                || prediction.estimated_remaining_raw_tokens
                    != capacity.estimated_remaining_raw_tokens
            {
                return Err(mismatch());
            }
            let history_insufficient = eligible < 3;
            let capacity_insufficient = capacity.confidence == Confidence::Insufficient;
            let (expected_status, expected_reasons) =
                match (history_insufficient, capacity_insufficient) {
                    (true, true) => (
                        "insufficient_task_history_and_capacity",
                        vec!["task_history_insufficient", "capacity_insufficient"],
                    ),
                    (true, false) => (
                        "insufficient_task_history",
                        vec!["task_history_insufficient"],
                    ),
                    (false, true) => ("insufficient_capacity", vec!["capacity_insufficient"]),
                    (false, false) => ("available", vec![]),
                };
            if prediction.status != expected_status
                || prediction
                    .insufficiency_reasons
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    != expected_reasons
            {
                return Err(mismatch());
            }
            let counts = [
                prediction.light_task_count,
                prediction.normal_task_count,
                prediction.heavy_task_count,
            ];
            if expected_status != "available" {
                if counts.iter().any(Option::is_some) {
                    return Err(mismatch());
                }
            } else {
                let Some(remaining) = prediction.estimated_remaining_raw_tokens else {
                    return Err(mismatch());
                };
                let [Some(p25), Some(p50), Some(p75)] = quantiles else {
                    return Err(mismatch());
                };
                let expected = [p25, p50, p75].map(|size| (remaining / size).floor());
                if !remaining.is_finite()
                    || remaining < 0.0
                    || expected
                        .iter()
                        .any(|count| !count.is_finite() || *count > SAFE_INTEGER_MAX as f64)
                    || counts != expected.map(|count| Some(count as u64))
                    || counts.iter().any(|count| count.is_none())
                {
                    return Err(mismatch());
                }
            }
        }
        let expected_status = if self
            .meter_predictions
            .iter()
            .any(|prediction| prediction.status == "available")
        {
            "succeeded"
        } else {
            "insufficient_evidence"
        };
        if self.result_status != expected_status {
            return Err(mismatch());
        }
        Ok(())
    }
}

fn opaque_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn mismatch() -> ResultError {
    ResultError {
        code: "invalid_workload_result",
        message: "Python workload worker output does not match its versioned contract",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> AnalyticsRequest {
        AnalyticsRequest::parse(
            json!({
                "schema_version": "1.0.0",
                "request_id": "worker-test-1",
                "analysis_type": "current_capacity_estimate",
                "dataset_selector": {
                    "configuration": {
                        "plan": {"availability":"available","value":"plus","provenance":"observed"},
                        "model": {"availability":"available","value":"gpt-5.6-sol","provenance":"observed"},
                        "reasoning_level": {"availability":"available","value":"high","provenance":"observed"},
                        "speed_mode": {"availability":"available","value":"standard","provenance":"derived"},
                        "codex_version": {"availability":"unavailable","provenance":"unavailable"}
                    },
                    "observed_from":"2026-10-01T00:00:00Z",
                    "observed_through":"2026-10-02T00:00:00Z"
                },
                "target_quota_windows":["five_hour","weekly"],
                "options":{}
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap()
    }

    fn fixture() -> Vec<u8> {
        include_bytes!(
            "../../../../../fixtures/contracts/v1/workload-result-insufficient-history.json"
        )
        .to_vec()
    }

    #[test]
    fn accepts_the_versioned_worker_fixture_and_rejects_scope_or_version_changes() {
        let request = request();
        let output = fixture();
        let result = WorkloadResult::parse_and_validate(
            &output,
            &request,
            "2026-10-16T07:00:00Z",
            super::super::ManualMeterPositions {
                five_hour_used_percent: Some(24.0),
                weekly_used_percent: Some(12.5),
            },
        )
        .unwrap();
        assert_eq!(
            result
                .task_size_distribution
                .accounting
                .eligible_observations,
            2
        );

        let mut changed: serde_json::Value = serde_json::from_slice(&output).unwrap();
        changed["workload_method_version"] = json!("2.0.0");
        assert!(WorkloadResult::parse_and_validate(
            &changed.to_string().into_bytes(),
            &request,
            "2026-10-16T07:00:00Z",
            super::super::ManualMeterPositions {
                five_hour_used_percent: Some(24.0),
                weekly_used_percent: Some(12.5),
            },
        )
        .is_err());

        changed = serde_json::from_slice(&output).unwrap();
        changed["meter_predictions"][0]["light_task_count"] = json!(0);
        assert!(WorkloadResult::parse_and_validate(
            &changed.to_string().into_bytes(),
            &request,
            "2026-10-16T07:00:00Z",
            super::super::ManualMeterPositions {
                five_hour_used_percent: Some(24.0),
                weekly_used_percent: Some(12.5),
            },
        )
        .is_err());

        changed = serde_json::from_slice(&output).unwrap();
        changed["task_size_distribution"]
            .as_object_mut()
            .unwrap()
            .remove("p25_raw_tokens");
        assert!(WorkloadResult::parse_and_validate(
            &changed.to_string().into_bytes(),
            &request,
            "2026-10-16T07:00:00Z",
            super::super::ManualMeterPositions {
                five_hour_used_percent: Some(24.0),
                weekly_used_percent: Some(12.5),
            },
        )
        .is_err());
    }
}
