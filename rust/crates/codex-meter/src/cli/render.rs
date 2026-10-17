//! Human-readable current-capacity presentation.

use crate::analytics::{AnalyticsResult, Confidence, MeterType, QuotaCapacityEstimate, ReasonCode};

pub fn render_human(
    result: &AnalyticsResult,
    five_hour_position: Option<f64>,
    weekly_position: Option<f64>,
) -> String {
    let mut output = String::from("Codex Meter — Empirical Capacity Estimate\n\nConfiguration\n");
    render_configuration(&mut output, &result.dataset_configuration);
    for estimate in &result.quota_capacity_estimates {
        output.push('\n');
        render_estimate(
            &mut output,
            estimate,
            match estimate.meter_type {
                MeterType::FiveHour => five_hour_position,
                MeterType::Weekly => weekly_position,
            },
        );
    }
    output.push_str(
        "\nSource: Codex Meter empirical estimate\nCodex Meter empirical estimates are not official provider quota allowances.\n",
    );
    output
}

fn render_configuration(
    output: &mut String,
    configuration: &crate::analytics::ConfigurationIdentity,
) {
    for (label, value) in [
        ("Plan", &configuration.plan),
        ("Model", &configuration.model),
        ("Reasoning", &configuration.reasoning_level),
        ("Speed", &configuration.speed_mode),
        ("Codex version", &configuration.codex_version),
    ] {
        let description = match value {
            crate::analytics::ConfigurationValue::Available { value, provenance } => {
                format!("{} ({provenance:?})", escape_terminal(value))
            }
            crate::analytics::ConfigurationValue::Unavailable { .. } => "Unavailable".to_owned(),
        };
        output.push_str(&format!("  {label:<16}{description}\n"));
    }
}

fn render_estimate(output: &mut String, estimate: &QuotaCapacityEstimate, position: Option<f64>) {
    output.push_str(match estimate.meter_type {
        MeterType::FiveHour => "Five-hour window\n",
        MeterType::Weekly => "Weekly window\n",
    });
    if estimate.confidence == Confidence::Insufficient {
        output.push_str("  Evidence:                       insufficient\n");
    } else {
        output.push_str(&format!(
            "  Estimated full capacity:       {} raw tokens\n",
            number(estimate.estimated_full_capacity_raw_tokens)
        ));
        output.push_str(&format!(
            "  Estimated used:                {} raw tokens\n",
            number(estimate.estimated_used_raw_tokens)
        ));
        output.push_str(&format!(
            "  Estimated remaining:           {} raw tokens\n",
            number(estimate.estimated_remaining_raw_tokens)
        ));
        let percentiles = estimate
            .capacity_percentiles_raw_tokens
            .as_ref()
            .expect("validated sufficient result includes percentiles");
        output.push_str(&format!(
            "  P25 / P50 / P75:               {} / {} / {} raw tokens\n",
            percentiles.p25, percentiles.p50, percentiles.p75
        ));
    }
    if let Some(position) = position {
        output.push_str(&format!(
            "  Current meter position:        {}% used (manually supplied, unverified)\n",
            position
        ));
    }
    output.push_str(&format!(
        "  Confidence:                    {}\n",
        confidence_name(&estimate.confidence)
    ));
    output.push_str(&format!(
        "  Observations:                  {} candidates, {} valid, {} excluded, {} outliers removed, {} used\n",
        estimate.sample_accounting.candidate_observations,
        estimate.sample_accounting.valid_observations,
        estimate.sample_accounting.excluded_observations,
        estimate.sample_accounting.outliers_removed,
        estimate.sample_accounting.used_observations
    ));
    if let Some(reasons) = &estimate.reason_codes {
        if !reasons.is_empty() {
            output.push_str(&format!(
                "  Reasons:                       {}\n",
                reasons
                    .iter()
                    .map(reason_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
}

fn number(value: Option<f64>) -> String {
    value
        .map(|number| number.to_string())
        .unwrap_or_else(|| "Unavailable".to_owned())
}

fn escape_terminal(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                format!("\\u{{{:x}}}", u32::from(character))
            } else {
                character.to_string()
            }
        })
        .collect()
}

fn confidence_name(confidence: &Confidence) -> &'static str {
    match confidence {
        Confidence::Insufficient => "insufficient",
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

fn reason_name(reason: &ReasonCode) -> &'static str {
    match reason {
        ReasonCode::QuotaResetCrossed => "quota_reset_crossed",
        ReasonCode::MeterUnavailable => "meter_unavailable",
        ReasonCode::MeterUnstable => "meter_unstable",
        ReasonCode::TelemetryIncomplete => "telemetry_incomplete",
        ReasonCode::ConcurrentUsagePossible => "concurrent_usage_possible",
        ReasonCode::ProcessInterrupted => "process_interrupted",
        ReasonCode::SourceAcquisitionFailure => "source_acquisition_failure",
        ReasonCode::UnknownReason => "unknown_reason",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::{AnalyticsResult, ManualMeterPositions};

    #[test]
    fn manual_input_is_labeled_unverified_and_configuration_missingness_is_preserved() {
        let json = r#"{"schema_version":"1.0.0","request_id":"request-1","analysis_type":"current_capacity_estimate","result_status":"insufficient_evidence","dataset_configuration":{"plan":{"availability":"unavailable","provenance":"unavailable"},"model":{"availability":"unavailable","provenance":"unavailable"},"reasoning_level":{"availability":"unavailable","provenance":"unavailable"},"speed_mode":{"availability":"unavailable","provenance":"unavailable"},"codex_version":{"availability":"unavailable","provenance":"unavailable"}},"generated_at":"2026-10-16T07:00:00Z","estimator_method_version":"1.0.0","quota_capacity_estimates":[{"meter_type":"five_hour","metric_kind":"estimated_raw_tokens","provider_authority":"codex_meter_empirical_estimate","confidence":"insufficient","sample_accounting":{"candidate_observations":0,"valid_observations":0,"excluded_observations":0,"outliers_removed":0,"used_observations":0},"reason_codes":["unknown_reason"]}],"warnings":["unknown_reason"]}"#;
        let request = crate::analytics::AnalyticsRequest::parse(r#"{"schema_version":"1.0.0","request_id":"request-1","analysis_type":"current_capacity_estimate","dataset_selector":{"configuration":{"plan":{"availability":"unavailable","provenance":"unavailable"},"model":{"availability":"unavailable","provenance":"unavailable"},"reasoning_level":{"availability":"unavailable","provenance":"unavailable"},"speed_mode":{"availability":"unavailable","provenance":"unavailable"},"codex_version":{"availability":"unavailable","provenance":"unavailable"}}},"target_quota_windows":["five_hour"],"options":{}}"#.as_bytes()).unwrap();
        let result = AnalyticsResult::parse_and_validate(
            json.as_bytes(),
            &request,
            "2026-10-16T07:00:00Z",
            ManualMeterPositions {
                five_hour_used_percent: Some(24.0),
                weekly_used_percent: None,
            },
        )
        .unwrap();
        let rendered = render_human(&result, Some(24.0), None);
        assert!(rendered.contains("24% used (manually supplied, unverified)"));
        assert!(rendered.contains("Plan            Unavailable"));
        assert!(rendered.contains(
            "Codex Meter empirical estimates are not official provider quota allowances."
        ));
    }

    #[test]
    fn control_characters_in_configuration_values_cannot_write_terminal_controls() {
        assert_eq!(escape_terminal("Plus\n\u{1b}[31m"), "Plus\\u{a}\\u{1b}[31m");
    }
}
