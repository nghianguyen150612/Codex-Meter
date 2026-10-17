//! Bounded, shell-free Python worker invocation.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use super::request::MAXIMUM_REQUEST_BYTES;

pub const ANALYTICS_PROCESS_PROTOCOL_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Copy)]
pub struct ProcessLimits {
    pub maximum_output_bytes: usize,
    pub maximum_diagnostic_bytes: usize,
    pub timeout: Duration,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            maximum_output_bytes: 2 * 1024 * 1024,
            maximum_diagnostic_bytes: 8 * 1024,
            timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessError {
    pub code: &'static str,
    pub message: &'static str,
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ProcessError {}

pub fn invoke_worker(
    python: &OsStr,
    database_path: &Path,
    generated_at: &str,
    five_hour_used_percent: Option<f64>,
    weekly_used_percent: Option<f64>,
    request: &[u8],
    limits: ProcessLimits,
) -> Result<Vec<u8>, ProcessError> {
    if request.len() > MAXIMUM_REQUEST_BYTES {
        return Err(ProcessError {
            code: "invalid_request",
            message: "request exceeds the 64 KiB limit",
        });
    }

    let mut command = Command::new(python);
    command
        .arg("-m")
        .arg("codex_meter.integration.worker")
        .arg("--database")
        .arg(database_path)
        .arg("--runtime-context-version")
        .arg(ANALYTICS_PROCESS_PROTOCOL_VERSION)
        .arg("--generated-at")
        .arg(generated_at);
    if let Some(value) = five_hour_used_percent {
        command
            .arg("--five-hour-used-percent")
            .arg(value.to_string());
    }
    if let Some(value) = weekly_used_percent {
        command.arg("--weekly-used-percent").arg(value.to_string());
    }
    let source_directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../python");
    if source_directory.is_dir() {
        command.current_dir(source_directory);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ProcessError {
            code: "python_unavailable",
            message: "could not start the configured Python executable",
        })?;

    let stdin = child.stdin.take().ok_or(ProcessError {
        code: "python_worker_failed",
        message: "could not open the Python worker request pipe",
    })?;
    let stdout = child.stdout.take().ok_or(ProcessError {
        code: "python_worker_failed",
        message: "could not open the Python worker output pipe",
    })?;
    let stderr = child.stderr.take().ok_or(ProcessError {
        code: "python_worker_failed",
        message: "could not open the Python worker diagnostic pipe",
    })?;

    let stdout_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_overflow_signal = Arc::clone(&stdout_exceeded);
    let stdout_limit = limits.maximum_output_bytes;
    let stdout_reader =
        thread::spawn(move || read_bounded(stdout, stdout_limit, stdout_overflow_signal));
    let stderr_exceeded = Arc::new(AtomicBool::new(false));
    let stderr_overflow_signal = Arc::clone(&stderr_exceeded);
    let stderr_limit = limits.maximum_diagnostic_bytes;
    let stderr_reader =
        thread::spawn(move || read_bounded(stderr, stderr_limit, stderr_overflow_signal));
    let input = request.to_vec();
    let input_writer = thread::spawn(move || {
        let mut stdin = stdin;
        let _ = stdin.write_all(&input);
    });

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if stdout_exceeded.load(Ordering::Acquire) || stderr_exceeded.load(Ordering::Acquire) {
            let _ = child.kill();
            break child.wait().map_err(|_| worker_failed())?;
        }
        if start.elapsed() >= limits.timeout {
            timed_out = true;
            let _ = child.kill();
            break child.wait().map_err(|_| worker_failed())?;
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(worker_failed());
            }
        }
    };
    let _ = child.wait();
    let _ = input_writer.join();
    let stdout = stdout_reader
        .join()
        .map_err(|_| worker_failed())?
        .map_err(|_| worker_failed())?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| worker_failed())?
        .map_err(|_| worker_failed())?;

    if timed_out {
        return Err(ProcessError {
            code: "process_timeout",
            message: "Python analytics worker exceeded the 60 second execution limit",
        });
    }
    if stdout_exceeded.load(Ordering::Acquire) {
        return Err(ProcessError {
            code: "process_output_too_large",
            message: "Python analytics worker exceeded the result output size limit",
        });
    }
    if stderr_exceeded.load(Ordering::Acquire) {
        return Err(ProcessError {
            code: "process_diagnostics_too_large",
            message: "Python analytics worker exceeded the diagnostic output size limit",
        });
    }
    if !status.success() {
        return Err(worker_exit_error(&stderr));
    }
    if stdout.is_empty() {
        return Err(ProcessError {
            code: "invalid_result_json",
            message: "Python analytics worker returned empty output",
        });
    }
    Ok(stdout)
}

fn read_bounded<R: Read>(
    mut reader: R,
    limit: usize,
    exceeded_bound: Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::with_capacity(limit.min(16 * 1024));
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&buffer[..read.min(remaining)]);
        if read > remaining {
            exceeded_bound.store(true, Ordering::Release);
        }
    }
    Ok(retained)
}

fn worker_exit_error(stderr: &[u8]) -> ProcessError {
    let prefix = stderr
        .split(|byte| *byte == b':' || *byte == b'\n')
        .next()
        .unwrap_or_default();
    let code = std::str::from_utf8(prefix).ok().unwrap_or_default();
    let (code, message) = match code {
        "invalid_request" => (
            "invalid_request",
            "Python worker rejected the analytics request",
        ),
        "unsupported_analysis_type" => (
            "unsupported_analysis_type",
            "requested analysis type is not supported",
        ),
        "unsupported_estimator_option" => (
            "unsupported_estimator_option",
            "requested estimator option is not supported",
        ),
        "invalid_manual_meter_percentage" => (
            "invalid_manual_meter_percentage",
            "manual meter percentage is invalid",
        ),
        "unrequested_meter_position" => (
            "invalid_manual_meter_percentage",
            "manual meter position targets an unrequested meter",
        ),
        "unsupported_protocol_version" => (
            "python_worker_failed",
            "Python worker protocol version is incompatible",
        ),
        "database_missing" => ("database_missing", "analytics database does not exist"),
        "database_schema_incompatible" => (
            "database_schema_incompatible",
            "analytics database schema is incompatible",
        ),
        "migration_checksum_drift" => (
            "migration_checksum_drift",
            "analytics database migration checksums are incompatible",
        ),
        "corrupt_observation" => (
            "corrupt_observation",
            "analytics database contains a corrupt observation",
        ),
        "database_open_failed" => (
            "database_open_failed",
            "analytics database could not be opened read-only",
        ),
        "database_read_failed" => (
            "database_read_failed",
            "analytics database could not be read",
        ),
        "current_quota_position_required" => (
            "current_quota_position_required",
            "a current meter percentage is required for sufficient historical evidence",
        ),
        "analytics_failed" => ("python_worker_failed", "Python capacity analysis failed"),
        _ => (
            "python_worker_failed",
            "Python analytics worker exited with an error",
        ),
    };
    ProcessError { code, message }
}

fn worker_failed() -> ProcessError {
    ProcessError {
        code: "python_worker_failed",
        message: "Python analytics worker process failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn bounded_reader_retains_only_the_configured_limit() {
        let exceeded = Arc::new(AtomicBool::new(false));
        let output = read_bounded(Cursor::new(b"abcdefgh"), 5, Arc::clone(&exceeded)).unwrap();
        assert_eq!(output, b"abcde");
        assert!(exceeded.load(Ordering::Acquire));
    }

    #[test]
    fn worker_diagnostics_map_only_known_categories_and_never_echo_tracebacks() {
        assert_eq!(
            worker_exit_error(b"database_missing: path").code,
            "database_missing"
        );
        let traceback = worker_exit_error(b"Traceback (most recent call last): secret");
        assert_eq!(traceback.code, "python_worker_failed");
        assert!(!traceback.message.contains("secret"));
    }

    #[test]
    fn real_worker_rejects_invalid_requests_and_missing_python_is_typed() {
        let error = invoke_worker(
            OsStr::new("python"),
            Path::new("missing-database.sqlite"),
            "2026-10-16T07:00:00Z",
            None,
            None,
            b"{}",
            ProcessLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, "invalid_request");

        let unavailable = invoke_worker(
            OsStr::new("codex-meter-python-executable-does-not-exist"),
            Path::new("missing-database.sqlite"),
            "2026-10-16T07:00:00Z",
            None,
            None,
            b"{}",
            ProcessLimits::default(),
        )
        .unwrap_err();
        assert_eq!(unavailable.code, "python_unavailable");
    }

    #[test]
    fn timeout_kills_and_reaps_a_live_worker() {
        let error = invoke_worker(
            OsStr::new("python"),
            Path::new("missing-database.sqlite"),
            "2026-10-16T07:00:00Z",
            None,
            None,
            b"{}",
            ProcessLimits {
                timeout: Duration::from_nanos(1),
                ..ProcessLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "process_timeout");
    }
}
