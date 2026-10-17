//! Input preparation, analytics invocation, and result contract enforcement.

use crate::analytics::{
    invoke_worker_with_mode, AnalyticsRequest, AnalyticsResult, ManualMeterPositions,
    ProcessLimits, ResultError, WorkerInvocation,
};
use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use super::args::{CliError, EstimateArguments};
use super::render::render_human;

pub fn run(arguments: EstimateArguments) -> Result<String, CliError> {
    if !crate::analytics::utc_timestamp(&arguments.generated_at) {
        return Err(cli_error(
            "invalid_generated_at",
            "--generated-at must be a valid UTC timestamp ending in Z",
        ));
    }
    validate_percentage(arguments.five_hour_used_percent)?;
    validate_percentage(arguments.weekly_used_percent)?;

    let request_bytes = read_bounded_request(&arguments.request)?;
    let request = AnalyticsRequest::parse(&request_bytes).map_err(|error| CliError {
        code: error.code,
        message: error.message.to_owned(),
    })?;
    if arguments.five_hour_used_percent.is_some()
        && !request
            .target_quota_windows
            .contains(&crate::analytics::MeterType::FiveHour)
        || arguments.weekly_used_percent.is_some()
            && !request
                .target_quota_windows
                .contains(&crate::analytics::MeterType::Weekly)
    {
        return Err(cli_error(
            "invalid_manual_meter_percentage",
            "a supplied meter position must match a requested target meter",
        ));
    }

    validate_python_executable(&arguments.python)?;

    validate_database_readable(&arguments.database)?;
    let database = absolute_database_path(&arguments.database)?;
    let output = invoke_worker_with_mode(
        WorkerInvocation {
            python: &arguments.python,
            worker_mode: arguments.worker_mode,
            database_path: &database,
            generated_at: &arguments.generated_at,
            five_hour_used_percent: arguments.five_hour_used_percent,
            weekly_used_percent: arguments.weekly_used_percent,
        },
        &request_bytes,
        ProcessLimits::default(),
    )
    .map_err(|error| CliError {
        code: error.code,
        message: error.message.to_owned(),
    })?;
    let result = decode_worker_result(
        &output,
        &request,
        &arguments.generated_at,
        ManualMeterPositions {
            five_hour_used_percent: arguments.five_hour_used_percent,
            weekly_used_percent: arguments.weekly_used_percent,
        },
    )?;
    if arguments.json {
        result
            .to_json()
            .map(|json| format!("{json}\n"))
            .map_err(|error| CliError {
                code: error.code,
                message: error.message.to_owned(),
            })
    } else {
        Ok(render_human(
            &result,
            arguments.five_hour_used_percent,
            arguments.weekly_used_percent,
        ))
    }
}

fn absolute_database_path(path: &Path) -> Result<std::path::PathBuf, CliError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|directory| directory.join(path))
        .map_err(|_| cli_error("database_missing", "analytics database does not exist"))
}

fn validate_database_readable(path: &Path) -> Result<(), CliError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            cli_error("database_missing", "analytics database does not exist")
        } else {
            cli_error(
                "database_open_failed",
                "analytics database could not be opened read-only",
            )
        }
    })?;
    if !metadata.is_file() {
        return Err(cli_error(
            "database_missing",
            "analytics database does not exist",
        ));
    }
    File::open(path).map_err(|_| {
        cli_error(
            "database_open_failed",
            "analytics database could not be opened read-only",
        )
    })?;
    Ok(())
}

fn validate_python_executable(executable: &OsStr) -> Result<(), CliError> {
    let configured = Path::new(executable);
    let has_directory = configured.is_absolute() || configured.components().count() > 1;
    let usable = if has_directory {
        executable_file_is_usable(configured)
    } else {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path).any(|directory| {
                    executable_candidates(&directory.join(configured))
                        .any(|candidate| executable_file_is_usable(&candidate))
                })
            })
            .unwrap_or(false)
    };
    if usable {
        Ok(())
    } else {
        Err(cli_error(
            "python_unavailable",
            "could not start the configured Python executable",
        ))
    }
}

fn executable_candidates(path: &Path) -> impl Iterator<Item = std::path::PathBuf> {
    #[cfg(windows)]
    {
        let mut candidates = vec![path.to_path_buf()];
        if path.extension().is_none() {
            let extensions = std::env::var_os("PATHEXT")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned());
            candidates.extend(
                extensions
                    .split(';')
                    .filter(|extension| !extension.is_empty())
                    .map(|extension| path.with_extension(extension.trim_start_matches('.'))),
            );
        }
        candidates.into_iter()
    }
    #[cfg(not(windows))]
    {
        std::iter::once(path.to_path_buf())
    }
}

fn executable_file_is_usable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn decode_worker_result(
    output: &[u8],
    request: &AnalyticsRequest,
    generated_at: &str,
    manual_positions: ManualMeterPositions,
) -> Result<AnalyticsResult, CliError> {
    AnalyticsResult::parse_and_validate(output, request, generated_at, manual_positions).map_err(
        |error: ResultError| CliError {
            code: error.code,
            message: error.message.to_owned(),
        },
    )
}

fn read_bounded_request(path: &Path) -> Result<Vec<u8>, CliError> {
    let file = File::open(path)
        .map_err(|_| cli_error("invalid_request", "request file could not be read"))?;
    let mut bytes = Vec::new();
    file.take((crate::analytics::MAXIMUM_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| cli_error("invalid_request", "request file could not be read"))?;
    if bytes.len() > crate::analytics::MAXIMUM_REQUEST_BYTES {
        return Err(cli_error(
            "invalid_request",
            "request exceeds the 64 KiB limit",
        ));
    }
    Ok(bytes)
}

fn validate_percentage(value: Option<f64>) -> Result<(), CliError> {
    if value.is_some_and(|value| !value.is_finite() || !(0.0..=100.0).contains(&value)) {
        return Err(cli_error(
            "invalid_manual_meter_percentage",
            "meter percentage must be finite and between 0 and 100",
        ));
    }
    Ok(())
}

fn cli_error(code: &'static str, message: &str) -> CliError {
    CliError {
        code,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn rejects_invalid_explicit_timestamp_and_percentages_before_process_launch() {
        let base = EstimateArguments {
            database: PathBuf::from("missing.sqlite"),
            request: PathBuf::from("missing.json"),
            generated_at: "not-a-time".to_owned(),
            five_hour_used_percent: None,
            weekly_used_percent: None,
            python: "missing-python".into(),
            worker_mode: crate::analytics::PythonWorkerMode::Auto,
            json: false,
        };
        assert_eq!(run(base.clone()).unwrap_err().code, "invalid_generated_at");
        let invalid = EstimateArguments {
            generated_at: "2026-10-16T07:00:00Z".to_owned(),
            five_hour_used_percent: Some(f64::INFINITY),
            ..base
        };
        assert_eq!(
            run(invalid).unwrap_err().code,
            "invalid_manual_meter_percentage"
        );
    }

    #[test]
    fn database_path_is_made_absolute_without_canonicalization() {
        let relative = Path::new("data/observations.sqlite");
        let absolute = absolute_database_path(relative).unwrap();
        assert!(absolute.is_absolute());
        assert!(absolute.ends_with(relative));

        let already_absolute = std::env::temp_dir().join("observations.sqlite");
        assert_eq!(
            absolute_database_path(&already_absolute).unwrap(),
            already_absolute
        );
    }

    #[test]
    fn rejects_invalid_utf8_and_json_from_the_worker() {
        let request = AnalyticsRequest::parse(
            br#"{"schema_version":"1.0.0","request_id":"request-1","analysis_type":"current_capacity_estimate","dataset_selector":{"configuration":{"plan":{"availability":"unavailable","provenance":"unavailable"},"model":{"availability":"unavailable","provenance":"unavailable"},"reasoning_level":{"availability":"unavailable","provenance":"unavailable"},"speed_mode":{"availability":"unavailable","provenance":"unavailable"},"codex_version":{"availability":"unavailable","provenance":"unavailable"}}},"target_quota_windows":["five_hour"],"options":{}}"#,
        )
        .unwrap();
        assert_eq!(
            decode_worker_result(
                &[0xff],
                &request,
                "2026-10-16T07:00:00Z",
                ManualMeterPositions::default(),
            )
            .unwrap_err()
            .code,
            "invalid_result_json"
        );
        assert_eq!(
            decode_worker_result(
                b"not-json",
                &request,
                "2026-10-16T07:00:00Z",
                ManualMeterPositions::default(),
            )
            .unwrap_err()
            .code,
            "invalid_result_json"
        );
        assert_eq!(
            decode_worker_result(
                b"{}",
                &request,
                "2026-10-16T07:00:00Z",
                ManualMeterPositions::default(),
            )
            .unwrap_err()
            .code,
            "result_contract_mismatch"
        );
    }
}
