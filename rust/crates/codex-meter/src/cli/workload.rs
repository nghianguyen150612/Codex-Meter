//! Native workload forecast command and privacy-safe rendering.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::{collections::BTreeMap, fmt::Write as _};

use super::args::{CliError, WorkloadArguments};
use crate::analytics::{
    invoke_worker_with_mode, AnalyticsRequest, ManualMeterPositions, ProcessLimits,
    WorkerInvocation, WorkerOperation, WorkloadResult,
};

pub fn run(arguments: WorkloadArguments) -> Result<String, CliError> {
    if !crate::analytics::utc_timestamp(&arguments.generated_at) {
        return Err(error(
            "invalid_generated_at",
            "--generated-at must be a valid UTC timestamp ending in Z",
        ));
    }
    for percentage in [
        arguments.five_hour_used_percent,
        arguments.weekly_used_percent,
    ]
    .into_iter()
    .flatten()
    {
        if !percentage.is_finite() || !(0.0..=100.0).contains(&percentage) {
            return Err(error(
                "invalid_manual_meter_percentage",
                "meter percentage must be finite and between 0 and 100",
            ));
        }
    }
    let request_bytes = read_bounded_request(&arguments.request)?;
    let request = AnalyticsRequest::parse(&request_bytes).map_err(|reason| CliError {
        code: reason.code,
        message: reason.message.to_owned(),
    })?;
    if (arguments.five_hour_used_percent.is_some()
        && !request
            .target_quota_windows
            .contains(&crate::analytics::MeterType::FiveHour))
        || (arguments.weekly_used_percent.is_some()
            && !request
                .target_quota_windows
                .contains(&crate::analytics::MeterType::Weekly))
    {
        return Err(error(
            "invalid_manual_meter_percentage",
            "a supplied meter position must match a requested target meter",
        ));
    }
    super::estimate::validate_python_executable(&arguments.python)?;
    super::estimate::validate_database_readable(&arguments.database)?;
    let database = super::estimate::absolute_database_path(&arguments.database)?;
    let output = invoke_worker_with_mode(
        WorkerInvocation {
            python: &arguments.python,
            worker_mode: arguments.worker_mode,
            database_path: &database,
            generated_at: &arguments.generated_at,
            five_hour_used_percent: arguments.five_hour_used_percent,
            weekly_used_percent: arguments.weekly_used_percent,
            operation: WorkerOperation::Workload,
        },
        &request_bytes,
        ProcessLimits::default(),
    )
    .map_err(|reason| CliError {
        code: reason.code,
        message: reason.message.to_owned(),
    })?;
    let result = WorkloadResult::parse_and_validate(
        &output,
        &request,
        &arguments.generated_at,
        ManualMeterPositions {
            five_hour_used_percent: arguments.five_hour_used_percent,
            weekly_used_percent: arguments.weekly_used_percent,
        },
    )
    .map_err(|reason| CliError {
        code: reason.code,
        message: reason.message.to_owned(),
    })?;
    if arguments.json {
        let mut json = String::from_utf8(output).map_err(|_| {
            error(
                "invalid_workload_result",
                "Python workload output is not UTF-8",
            )
        })?;
        json.push('\n');
        Ok(json)
    } else {
        Ok(render(&result))
    }
}

fn read_bounded_request(path: &Path) -> Result<Vec<u8>, CliError> {
    let file =
        File::open(path).map_err(|_| error("invalid_request", "request file could not be read"))?;
    let mut bytes = Vec::new();
    file.take((crate::analytics::MAXIMUM_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| error("invalid_request", "request file could not be read"))?;
    if bytes.len() > crate::analytics::MAXIMUM_REQUEST_BYTES {
        return Err(error("invalid_request", "request exceeds the 64 KiB limit"));
    }
    Ok(bytes)
}

fn render(result: &WorkloadResult) -> String {
    let distribution = &result.task_size_distribution;
    let mut output = String::from("Codex Meter — Workload Forecast\n");
    let _ = writeln!(output, "Status: {}", result.result_status.replace('_', " "));
    output.push_str("\nSource: Local empirical observations\nConfiguration:\n");
    for (label, value) in [
        ("Plan", &result.dataset_configuration.plan),
        ("Model", &result.dataset_configuration.model),
        ("Reasoning", &result.dataset_configuration.reasoning_level),
        ("Speed mode", &result.dataset_configuration.speed_mode),
        ("Codex version", &result.dataset_configuration.codex_version),
    ] {
        use crate::analytics::ConfigurationValue;
        let (displayed, provenance) = match value {
            ConfigurationValue::Available { value, provenance } => {
                (value.as_str(), format!("{provenance:?}").to_lowercase())
            }
            ConfigurationValue::Unavailable { .. } => ("unknown", "unavailable".to_owned()),
        };
        let _ = writeln!(output, "  {label}: {displayed} ({provenance})");
    }
    output.push_str("\nHistorical task sizes\n");
    output.push_str(&format!(
        "  Eligible samples:  {}\n  Excluded samples:  {}\n",
        distribution.accounting.eligible_observations,
        distribution.accounting.excluded_observations
    ));
    output.push_str(&format!(
        "  C-quality samples: {}\n",
        distribution.accounting.c_quality_observations
    ));
    let mut exclusion_reasons = BTreeMap::<&str, u64>::new();
    for decision in &distribution.decisions {
        if let crate::analytics::TaskSizeDecision::Exclusion { reasons, .. } = decision {
            for reason in reasons {
                *exclusion_reasons.entry(reason).or_default() += 1;
            }
        }
    }
    if !exclusion_reasons.is_empty() {
        output.push_str("  Exclusion reasons:\n");
        for (reason, count) in exclusion_reasons {
            let _ = writeln!(output, "    {reason}: {count}");
        }
    }
    for (name, value) in [
        ("Light (P25)", distribution.p25_raw_tokens),
        ("Normal (P50)", distribution.p50_raw_tokens),
        ("Heavy (P75)", distribution.p75_raw_tokens),
    ] {
        output.push_str(&format!(
            "  {name}: {}\n",
            value
                .map(|number| format!("{number:.0} raw tokens"))
                .unwrap_or_else(|| "unavailable".to_owned())
        ));
    }
    for prediction in &result.meter_predictions {
        output.push_str(&format!(
            "\n{} meter\n",
            match prediction.meter_type {
                crate::analytics::MeterType::FiveHour => "5-hour",
                crate::analytics::MeterType::Weekly => "Weekly",
            }
        ));
        if prediction.status == "available" {
            output.push_str(&format!(
                "  Light:  {} tasks\n  Normal: {} tasks\n  Heavy:  {} tasks\n",
                display_count(prediction.light_task_count),
                display_count(prediction.normal_task_count),
                display_count(prediction.heavy_task_count)
            ));
        } else {
            output.push_str(&format!(
                "  Status: {}\n",
                prediction.status.replace('_', " ")
            ));
        }
        output.push_str(&format!(
            "  Confidence: {:?}\n",
            prediction.capacity_confidence
        ));
        if !prediction.capacity_reason_codes.is_empty() {
            output.push_str(&format!(
                "  Capacity reasons: {}\n",
                prediction
                    .capacity_reason_codes
                    .iter()
                    .map(|reason| format!("{reason:?}").to_lowercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if result.meter_predictions.is_empty() {
        output.push_str("\nNo quota meters were selected in the request.\n");
    }
    output.push_str("\nWarning: Estimates are empirical and unofficial. They do not guarantee future task counts or represent an official allowance.\n");
    output
}

fn display_count(count: Option<u64>) -> String {
    count
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_owned())
}

fn error(code: &'static str, message: &str) -> CliError {
    CliError {
        code,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::PythonWorkerMode;
    use std::ffi::OsString;

    fn arguments() -> WorkloadArguments {
        WorkloadArguments {
            database: "missing.sqlite".into(),
            request: "missing.json".into(),
            generated_at: "2026-10-16T07:00:00Z".to_owned(),
            five_hour_used_percent: None,
            weekly_used_percent: None,
            python: OsString::from("python"),
            worker_mode: PythonWorkerMode::Auto,
            json: false,
        }
    }

    #[test]
    fn invalid_timestamp_and_meter_positions_fail_before_file_or_worker_access() {
        let mut invalid_time = arguments();
        invalid_time.generated_at = "not-a-timestamp".to_owned();
        assert_eq!(run(invalid_time).unwrap_err().code, "invalid_generated_at");

        let mut invalid_percentage = arguments();
        invalid_percentage.weekly_used_percent = Some(f64::NAN);
        assert_eq!(
            run(invalid_percentage).unwrap_err().code,
            "invalid_manual_meter_percentage"
        );
    }
}
