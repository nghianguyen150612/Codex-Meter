//! Isolated, bounded Python worker invocation.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::request::MAXIMUM_REQUEST_BYTES;

pub const ANALYTICS_PROCESS_PROTOCOL_VERSION: &str = "1.0.0";
const MAXIMUM_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAXIMUM_DIAGNOSTIC_BYTES: usize = 8 * 1024;
const MAXIMUM_TIMEOUT: Duration = Duration::from_secs(60);
const MINIMUM_TIMEOUT: Duration = Duration::from_millis(20);
const CLEANUP_RESERVE: Duration = Duration::from_millis(250);
const EXIT_PIPE_GRACE: Duration = Duration::from_millis(50);
const CLEANUP_POLL_INTERVAL: Duration = Duration::from_millis(5);
const SUCCESS_MARKER: &[u8] = b"codex_meter_worker_v1 input_complete\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonWorkerMode {
    Auto,
    Source,
    Installed,
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerInvocation<'a> {
    pub python: &'a OsStr,
    pub worker_mode: PythonWorkerMode,
    pub database_path: &'a Path,
    pub generated_at: &'a str,
    pub five_hour_used_percent: Option<f64>,
    pub weekly_used_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessLimits {
    pub maximum_output_bytes: usize,
    pub maximum_diagnostic_bytes: usize,
    pub timeout: Duration,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            maximum_output_bytes: MAXIMUM_OUTPUT_BYTES,
            maximum_diagnostic_bytes: MAXIMUM_DIAGNOSTIC_BYTES,
            timeout: MAXIMUM_TIMEOUT,
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

#[derive(Debug)]
struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub fn invoke_worker(
    python: &OsStr,
    database_path: &Path,
    generated_at: &str,
    five_hour_used_percent: Option<f64>,
    weekly_used_percent: Option<f64>,
    request: &[u8],
    limits: ProcessLimits,
) -> Result<Vec<u8>, ProcessError> {
    invoke_worker_with_mode(
        WorkerInvocation {
            python,
            worker_mode: PythonWorkerMode::Auto,
            database_path,
            generated_at,
            five_hour_used_percent,
            weekly_used_percent,
        },
        request,
        limits,
    )
}

pub fn invoke_worker_with_mode(
    invocation: WorkerInvocation<'_>,
    request: &[u8],
    limits: ProcessLimits,
) -> Result<Vec<u8>, ProcessError> {
    validate_limits(limits)?;
    if request.len() > MAXIMUM_REQUEST_BYTES {
        return Err(ProcessError {
            code: "invalid_request",
            message: "request exceeds the 64 KiB limit",
        });
    }

    let mut command = Command::new(invocation.python);
    command.arg("-I").arg("-S").arg("-c").arg(WORKER_BOOTSTRAP);
    let source_root = match invocation.worker_mode {
        PythonWorkerMode::Auto => source_tree_root()?,
        PythonWorkerMode::Source => Some(source_tree_root()?.ok_or_else(worker_resolution_error)?),
        PythonWorkerMode::Installed => None,
    };
    if let Some(root) = source_root {
        command.arg("source").arg(root);
    } else {
        command.arg("installed");
    }
    command
        .arg("--database")
        .arg(invocation.database_path)
        .arg("--runtime-context-version")
        .arg(ANALYTICS_PROCESS_PROTOCOL_VERSION)
        .arg("--generated-at")
        .arg(invocation.generated_at);
    if let Some(value) = invocation.five_hour_used_percent {
        command
            .arg("--five-hour-used-percent")
            .arg(value.to_string());
    }
    if let Some(value) = invocation.weekly_used_percent {
        command.arg("--weekly-used-percent").arg(value.to_string());
    }
    let output = run_command(command, request, limits)?;
    validate_success_marker(&output.stderr)?;
    Ok(output.stdout)
}

fn validate_success_marker(stderr: &[u8]) -> Result<(), ProcessError> {
    if stderr == SUCCESS_MARKER {
        Ok(())
    } else {
        Err(ProcessError {
            code: "python_worker_protocol_failed",
            message: "Python analytics worker did not complete the input protocol",
        })
    }
}

fn validate_limits(limits: ProcessLimits) -> Result<(), ProcessError> {
    if limits.maximum_output_bytes == 0
        || limits.maximum_output_bytes > MAXIMUM_OUTPUT_BYTES
        || limits.maximum_diagnostic_bytes == 0
        || limits.maximum_diagnostic_bytes > MAXIMUM_DIAGNOSTIC_BYTES
        || limits.timeout < MINIMUM_TIMEOUT
        || limits.timeout > MAXIMUM_TIMEOUT
    {
        return Err(ProcessError {
            code: "invalid_process_limits",
            message: "analytics process limits are outside the supported bounds",
        });
    }
    Ok(())
}

fn apply_minimal_environment(command: &mut Command) {
    command.env_clear();
    // PATH is needed for the default `python` executable. The remaining entries
    // preserve Windows process startup and standard temporary-file behavior.
    for name in [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "WINDIR",
        "TEMP",
        "TMP",
        "TMPDIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn source_tree_root() -> Result<Option<PathBuf>, ProcessError> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../python");
    if !source.is_dir() {
        return Ok(None);
    }
    let root = source
        .canonicalize()
        .map_err(|_| worker_resolution_error())?;
    validate_worker_files(&root).map_err(|_| worker_resolution_error())?;
    Ok(Some(root))
}

fn validate_worker_files(root: &Path) -> std::io::Result<()> {
    for relative in [
        "codex_meter/__init__.py",
        "codex_meter/integration/__init__.py",
        "codex_meter/integration/worker.py",
    ] {
        let path = root.join(relative).canonicalize()?;
        if !path.is_file() || !path.starts_with(root) {
            return Err(std::io::Error::other(
                "worker path is outside the trusted root",
            ));
        }
    }
    Ok(())
}

fn run_command(
    mut command: Command,
    request: &[u8],
    limits: ProcessLimits,
) -> Result<ProcessOutput, ProcessError> {
    validate_limits(limits)?;
    if request.len() > MAXIMUM_REQUEST_BYTES {
        return Err(ProcessError {
            code: "invalid_request",
            message: "request exceeds the 64 KiB limit",
        });
    }

    let started = Instant::now();
    let deadline = started + limits.timeout;
    let reserve = CLEANUP_RESERVE.min(limits.timeout / 4);
    let execution_deadline = deadline - reserve;

    apply_minimal_environment(&mut command);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(windows)]
    let process_job = windows_job::ProcessJob::new().map_err(|_| ProcessError {
        code: "process_cleanup_failed",
        message: "could not establish bounded worker process cleanup",
    })?;

    let mut child = Some(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| ProcessError {
                code: "python_unavailable",
                message: "could not start the configured Python executable",
            })?,
    );
    let child_id = child.as_ref().expect("spawn returned a child").id();

    #[cfg(windows)]
    if process_job
        .assign(child.as_ref().expect("spawn returned a child"))
        .is_err()
    {
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
        }
        if reap_direct_child(&mut child, deadline).is_none() {
            if let Some(mut unreaped_child) = child.take() {
                thread::spawn(move || {
                    let _ = unreaped_child.wait();
                });
            }
        }
        return Err(ProcessError {
            code: "process_cleanup_failed",
            message: "could not establish bounded worker process cleanup",
        });
    }

    let child_ref = child.as_mut().expect("spawn returned a child");
    let stdin = child_ref.stdin.take().ok_or_else(worker_failed)?;
    let stdout = child_ref.stdout.take().ok_or_else(worker_failed)?;
    let stderr = child_ref.stderr.take().ok_or_else(worker_failed)?;

    let stdout_exceeded = Arc::new(AtomicBool::new(false));
    let stderr_exceeded = Arc::new(AtomicBool::new(false));
    let stdout_reader = {
        let signal = Arc::clone(&stdout_exceeded);
        thread::spawn(move || read_bounded(stdout, limits.maximum_output_bytes, signal))
    };
    let stderr_reader = {
        let signal = Arc::clone(&stderr_exceeded);
        thread::spawn(move || read_bounded(stderr, limits.maximum_diagnostic_bytes, signal))
    };
    let input = request.to_vec();
    let (input_sender, input_receiver) = mpsc::sync_channel(1);
    let input_writer = thread::spawn(move || {
        let mut stdin = stdin;
        let result = stdin.write_all(&input);
        let _ = input_sender.send(result.is_err());
        result
    });

    let mut status = None;
    let mut timed_out = false;
    let mut input_failed = false;
    let mut input_status_checked = false;
    let mut failure = None;
    loop {
        if stdout_exceeded.load(Ordering::Acquire) {
            failure = Some(output_too_large());
            break;
        }
        if stderr_exceeded.load(Ordering::Acquire) {
            failure = Some(diagnostics_too_large());
            break;
        }
        if input_writer.is_finished() && !input_status_checked {
            match input_receiver.try_recv() {
                Ok(error) => {
                    input_status_checked = true;
                    if error {
                        match child.as_mut().expect("child is not reaped").try_wait() {
                            Ok(Some(exit_status)) => {
                                input_failed = exit_status.success();
                                status = Some(exit_status);
                            }
                            Ok(None) => input_failed = true,
                            Err(_) => failure = Some(worker_failed()),
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    input_status_checked = true;
                    input_failed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            if status.is_some() || input_failed || failure.is_some() {
                break;
            }
        }
        match child.as_mut().expect("child is not reaped").try_wait() {
            Ok(Some(exit_status)) => {
                status = Some(exit_status);
                break;
            }
            Ok(None) => {}
            Err(_) => {
                failure = Some(worker_failed());
                break;
            }
        }
        if Instant::now() >= execution_deadline {
            timed_out = true;
            break;
        }
        thread::sleep(
            CLEANUP_POLL_INTERVAL.min(execution_deadline.saturating_duration_since(Instant::now())),
        );
    }

    let mut tree_terminated = false;
    if failure.is_some() || timed_out || input_failed {
        tree_terminated = true;
        terminate_process_tree(
            child_id,
            &mut child,
            #[cfg(windows)]
            &process_job,
        );
    }

    if status.is_some() && !all_pipes_finished(&stdout_reader, &stderr_reader, &input_writer) {
        let drain_deadline = (Instant::now() + EXIT_PIPE_GRACE).min(execution_deadline);
        while !all_pipes_finished(&stdout_reader, &stderr_reader, &input_writer)
            && Instant::now() < drain_deadline
        {
            if stdout_exceeded.load(Ordering::Acquire) {
                failure = Some(output_too_large());
                break;
            }
            if stderr_exceeded.load(Ordering::Acquire) {
                failure = Some(diagnostics_too_large());
                break;
            }
            thread::sleep(
                CLEANUP_POLL_INTERVAL.min(drain_deadline.saturating_duration_since(Instant::now())),
            );
        }
        if !all_pipes_finished(&stdout_reader, &stderr_reader, &input_writer) {
            if !tree_terminated {
                tree_terminated = true;
                terminate_process_tree(
                    child_id,
                    &mut child,
                    #[cfg(windows)]
                    &process_job,
                );
            }
            if failure.is_none() {
                failure = Some(ProcessError {
                    code: "process_lifecycle_failed",
                    message: "Python worker left a pipe open after process exit",
                });
            }
        }
    }

    if status.is_none() {
        let Some(reaped) = reap_direct_child(&mut child, deadline) else {
            if let Some(mut unreaped_child) = child.take() {
                thread::spawn(move || {
                    let _ = unreaped_child.wait();
                });
            }
            return Err(ProcessError {
                code: "process_cleanup_failed",
                message: "Python worker could not be reaped within the cleanup bound",
            });
        };
        status = Some(reaped);
    }

    if tree_terminated && !wait_for_pipes(&stdout_reader, &stderr_reader, &input_writer, deadline) {
        return Err(ProcessError {
            code: "process_cleanup_failed",
            message: "Python worker pipes did not close within the cleanup bound",
        });
    }

    if failure.is_none() && stdout_exceeded.load(Ordering::Acquire) {
        failure = Some(output_too_large());
    }
    if failure.is_none() && stderr_exceeded.load(Ordering::Acquire) {
        failure = Some(diagnostics_too_large());
    }

    if let Some(error) = failure {
        return Err(error);
    }
    if timed_out {
        return Err(ProcessError {
            code: "process_timeout",
            message: "Python analytics worker exceeded the execution time limit",
        });
    }

    let input_result = join_input_writer(input_writer)?;
    let stdout = join_reader(stdout_reader)?;
    let stderr = join_reader(stderr_reader)?;
    let status = status.ok_or_else(worker_failed)?;
    if input_failed {
        return Err(input_delivery_failed());
    }
    if !status.success() {
        return Err(worker_exit_error(&stderr));
    }
    if input_failed || input_result.is_err() {
        return Err(input_delivery_failed());
    }
    if Instant::now() >= deadline {
        return Err(ProcessError {
            code: "process_timeout",
            message: "Python analytics worker exceeded the execution time limit",
        });
    }
    if stdout.is_empty() {
        return Err(ProcessError {
            code: "invalid_result_json",
            message: "Python analytics worker returned empty output",
        });
    }
    Ok(ProcessOutput { stdout, stderr })
}

fn all_pipes_finished(
    stdout: &JoinHandle<std::io::Result<Vec<u8>>>,
    stderr: &JoinHandle<std::io::Result<Vec<u8>>>,
    input: &JoinHandle<std::io::Result<()>>,
) -> bool {
    stdout.is_finished() && stderr.is_finished() && input.is_finished()
}

fn wait_for_pipes(
    stdout: &JoinHandle<std::io::Result<Vec<u8>>>,
    stderr: &JoinHandle<std::io::Result<Vec<u8>>>,
    input: &JoinHandle<std::io::Result<()>>,
    deadline: Instant,
) -> bool {
    while !all_pipes_finished(stdout, stderr, input) && Instant::now() < deadline {
        thread::sleep(
            CLEANUP_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    all_pipes_finished(stdout, stderr, input)
}

fn reap_direct_child(child: &mut Option<Child>, deadline: Instant) -> Option<ExitStatus> {
    loop {
        match child.as_mut()?.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(
                    CLEANUP_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                );
            }
            Ok(None) => return None,
            Err(_) => return None,
        }
    }
}

#[cfg(unix)]
fn terminate_process_tree(child_id: u32, child: &mut Option<Child>) {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    if let Ok(process_group) = i32::try_from(child_id) {
        // The worker is started as the leader of a fresh process group.
        let result = unsafe { kill(-process_group, 9) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(3) {
                if let Some(child) = child.as_mut() {
                    let _ = child.kill();
                }
            }
        }
    }
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
    }
}

#[cfg(windows)]
fn terminate_process_tree(child_id: u32, child: &mut Option<Child>, job: &windows_job::ProcessJob) {
    let _ = job.terminate();
    let _ = child_id;
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
    }
}

#[cfg(not(any(unix, windows)))]
fn terminate_process_tree(child_id: u32, child: &mut Option<Child>) {
    let _ = child_id;
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
    }
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
            break;
        }
    }
    Ok(retained)
}

fn join_reader(reader: JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>, ProcessError> {
    reader
        .join()
        .map_err(|_| worker_failed())?
        .map_err(|_| worker_failed())
}

fn join_input_writer(
    writer: JoinHandle<std::io::Result<()>>,
) -> Result<std::io::Result<()>, ProcessError> {
    writer.join().map_err(|_| worker_failed())
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
        "worker_resolution_failed" => (
            "python_worker_resolution_failed",
            "trusted Python analytics worker could not be resolved; select installed mode with a local wheel or source mode from its checkout",
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

fn worker_resolution_error() -> ProcessError {
    ProcessError {
        code: "python_worker_resolution_failed",
        message: "trusted Python analytics worker could not be resolved; select installed mode with a local wheel or source mode from its checkout",
    }
}

fn input_delivery_failed() -> ProcessError {
    ProcessError {
        code: "process_input_failed",
        message: "Python analytics worker did not accept the complete request",
    }
}

fn output_too_large() -> ProcessError {
    ProcessError {
        code: "process_output_too_large",
        message: "Python analytics worker exceeded the result output size limit",
    }
}

fn diagnostics_too_large() -> ProcessError {
    ProcessError {
        code: "process_diagnostics_too_large",
        message: "Python analytics worker exceeded the diagnostic output size limit",
    }
}

fn worker_failed() -> ProcessError {
    ProcessError {
        code: "python_worker_failed",
        message: "Python analytics worker process failed",
    }
}

#[cfg(windows)]
mod windows_job {
    use std::ffi::c_void;
    use std::io;
    use std::process::Child;

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            information_class: i32,
            information: *const c_void,
            information_length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub(super) struct ProcessJob {
        handle: Handle,
    }

    impl ProcessJob {
        pub(super) fn new() -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut information = ExtendedLimitInformation::default();
            information.basic_limit_information.limit_flags = 0x0000_2000;
            let configured = unsafe {
                SetInformationJobObject(
                    handle,
                    9,
                    (&information as *const ExtendedLimitInformation).cast(),
                    std::mem::size_of::<ExtendedLimitInformation>() as u32,
                )
            };
            if configured == 0 {
                let error = io::Error::last_os_error();
                unsafe {
                    CloseHandle(handle);
                }
                return Err(error);
            }
            Ok(Self { handle })
        }

        pub(super) fn assign(&self, child: &Child) -> io::Result<()> {
            use std::os::windows::io::AsRawHandle;
            let assigned =
                unsafe { AssignProcessToJobObject(self.handle, child.as_raw_handle().cast()) };
            if assigned == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        pub(super) fn terminate(&self) -> io::Result<()> {
            let terminated = unsafe { TerminateJobObject(self.handle, 1) };
            if terminated == 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for ProcessJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

const WORKER_BOOTSTRAP: &str = r#"
import os
import runpy
import site
import sys
import sysconfig
import re
from importlib.metadata import distributions
from pathlib import Path

def fail_resolution():
    sys.stderr.write("worker_resolution_failed: trusted analytics worker could not be resolved\n")
    raise SystemExit(2)

def path_identity(path):
    value = os.path.normcase(os.path.normpath(str(path)))
    if os.name == "nt":
        if value.startswith("\\\\?\\unc\\"):
            return "\\\\" + value[8:]
        if value.startswith("\\\\?\\"):
            return value[4:]
    return value

def path_for_import(path):
    value = str(path)
    if os.name == "nt":
        if value.startswith("\\\\?\\UNC\\"):
            return "\\\\" + value[8:]
        if value.startswith("\\\\?\\"):
            return value[4:]
    return value

def is_below(path, root):
    try:
        path_value = path_identity(path)
        root_value = path_identity(root)
        return os.path.commonpath((path_value, root_value)) == root_value
    except ValueError:
        return False

def trusted_files(root):
    expected = (
        root / "codex_meter" / "__init__.py",
        root / "codex_meter" / "integration" / "__init__.py",
        root / "codex_meter" / "integration" / "worker.py",
    )
    try:
        resolved_root = root.resolve(strict=True)
        resolved = tuple(path.resolve(strict=True) for path in expected)
    except (OSError, RuntimeError):
        return None
    if not resolved_root.is_dir() or any(
        not path.is_file() or not is_below(path, resolved_root) for path in resolved
    ):
        return None
    return resolved_root

def installed_roots():
    roots = set()
    include_system_paths = True
    executable = Path(sys.executable)
    # Do not resolve the executable symlink: venv/bin/python and
    # venv/Scripts/python.exe identify the selected environment even when they
    # point at a base interpreter elsewhere.
    environment = executable.parent.parent
    config = environment / "pyvenv.cfg"
    if config.is_file():
        if os.name == "nt":
            roots.add(str(environment / "Lib" / "site-packages"))
        else:
            version = f"python{sys.version_info.major}.{sys.version_info.minor}"
            roots.add(str(environment / "lib" / version / "site-packages"))
        try:
            includes_system = any(
                line.partition("=")[0].strip().lower() == "include-system-site-packages"
                and line.partition("=")[2].strip().lower() == "true"
                for line in config.read_text(encoding="utf-8").splitlines()
            )
        except (OSError, UnicodeError):
            fail_resolution()
        include_system_paths = includes_system
    if include_system_paths:
        try:
            configured = sysconfig.get_paths()
            roots.update(configured.get(name) for name in ("purelib", "platlib"))
        except Exception:
            pass
        try:
            roots.update(site.getsitepackages())
        except Exception:
            pass
    resolved = set()
    for value in roots:
        if not value:
            continue
        try:
            path = Path(value).resolve(strict=True)
        except (OSError, RuntimeError):
            continue
        if path.is_dir():
            resolved.add(path)
    return resolved

def distribution_roots():
    if sys.version_info < (3, 11):
        fail_resolution()
    candidates = []
    for root in sorted(installed_roots(), key=lambda path: path_identity(path)):
        package_directory = root / "codex_meter"
        matching = []
        try:
            for distribution in distributions(path=[str(root)]):
                name = distribution.metadata.get("Name", "")
                if re.sub(r"[-_.]+", "-", name).lower() == "codex-meter-analytics":
                    matching.append(distribution)
        except Exception:
            fail_resolution()
        if not package_directory.exists() and not matching:
            continue
        if len(matching) != 1:
            fail_resolution()
        distribution = matching[0]
        metadata = distribution.metadata
        if metadata.get("Requires-Python") != ">=3.11":
            fail_resolution()
        files = distribution.files
        if files is None:
            fail_resolution()
        file_names = {item.as_posix() for item in files}
        required = {
            "codex_meter/__init__.py",
            "codex_meter/integration/__init__.py",
            "codex_meter/integration/worker.py",
            "codex_meter/history/__init__.py",
            "codex_meter/history/models.py",
            "codex_meter/history/sqlite.py",
            "codex_meter/estimation/__init__.py",
            "codex_meter/estimation/candidates.py",
            "codex_meter/estimation/result.py",
            "codex_meter/estimation/robust.py",
        }
        if not required.issubset(file_names):
            fail_resolution()
        for name in required:
            try:
                installed_file = Path(distribution.locate_file(name)).resolve(strict=True)
            except (OSError, RuntimeError):
                fail_resolution()
            if not installed_file.is_file() or not is_below(installed_file, root):
                fail_resolution()
        candidate = trusted_files(root)
        if candidate is None:
            fail_resolution()
        candidates.append(candidate)
    return sorted(set(candidates), key=lambda path: path_identity(path))

try:
    arguments = sys.argv[1:]
    strategy = arguments.pop(0)
    if strategy == "source" and arguments:
        root = trusted_files(Path(arguments.pop(0)))
        candidates = [] if root is None else [root]
    elif strategy == "installed" and not (arguments and arguments[0] == "--"):
        candidates = distribution_roots()
    else:
        fail_resolution()
    if len(candidates) != 1:
        fail_resolution()
    worker_root = candidates[0]
    standard = sysconfig.get_paths()
    standard_roots = []
    for name in ("stdlib", "platstdlib"):
        value = standard.get(name)
        if value:
            try:
                standard_roots.append(Path(value).resolve())
            except (OSError, RuntimeError):
                pass
    windows_standard_paths = set()
    if os.name == "nt":
        # Windows keeps extension modules and the standard-library zip beside Lib.
        try:
            interpreter_root = Path(sys.base_prefix).resolve()
            windows_standard_paths = {
                path_identity(interpreter_root / "DLLs"),
                path_identity(
                    interpreter_root
                    / f"python{sys.version_info.major}{sys.version_info.minor}.zip"
                ),
            }
        except (OSError, RuntimeError):
            pass
    safe_standard_path = []
    for entry in sys.path:
        if not entry:
            continue
        try:
            resolved_entry = Path(entry).resolve()
        except (OSError, RuntimeError):
            continue
        if any(is_below(resolved_entry, root) for root in standard_roots) or (
            os.name == "nt" and path_identity(resolved_entry) in windows_standard_paths
        ):
            safe_standard_path.append(path_for_import(resolved_entry))
    sys.path[:] = [path_for_import(worker_root), *safe_standard_path]
    sys.argv[:] = ["codex_meter.integration.worker", *arguments]
    runpy.run_module("codex_meter.integration.worker", run_name="__main__", alter_sys=True)
except SystemExit:
    raise
except Exception:
    fail_resolution()
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;
    use std::time::Instant;

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory(label: &str) -> PathBuf {
        let index = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "codex meter p028 {label} {} {index}",
            std::process::id()
        ))
    }

    fn copy_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_directory(&source_path, &destination_path)?;
            } else if entry.file_type()?.is_file() {
                fs::copy(source_path, destination_path)?;
            }
        }
        Ok(())
    }

    fn run_python_script(script: &str, limits: ProcessLimits) -> Result<Vec<u8>, ProcessError> {
        let mut command = Command::new("python");
        command
            .arg("-I")
            .arg("-S")
            .arg("-c")
            .arg(format!("import sys; sys.stdin.read()\n{script}"));
        Ok(run_command(command, b"{}", limits)?.stdout)
    }

    #[test]
    fn bounded_reader_retains_only_the_configured_limit_and_stops_after_overflow() {
        let exceeded = Arc::new(AtomicBool::new(false));
        let output = read_bounded(Cursor::new(b"abcdefgh"), 5, Arc::clone(&exceeded)).unwrap();
        assert_eq!(output, b"abcde");
        assert!(exceeded.load(Ordering::Acquire));
    }

    #[test]
    fn process_limits_reject_zero_and_overflow_prone_values() {
        for limits in [
            ProcessLimits {
                maximum_output_bytes: 0,
                ..ProcessLimits::default()
            },
            ProcessLimits {
                maximum_diagnostic_bytes: usize::MAX,
                ..ProcessLimits::default()
            },
            ProcessLimits {
                timeout: Duration::ZERO,
                ..ProcessLimits::default()
            },
            ProcessLimits {
                timeout: Duration::from_secs(61),
                ..ProcessLimits::default()
            },
        ] {
            assert_eq!(
                validate_limits(limits).unwrap_err().code,
                "invalid_process_limits"
            );
        }
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
        for marker in [
            "SYNTHETIC_SECRET_MARKER",
            "SYNTHETIC_PROMPT_MARKER",
            "SYNTHETIC_RESPONSE_MARKER",
            "SYNTHETIC_SOURCE_MARKER",
        ] {
            let error = worker_exit_error(marker.as_bytes());
            assert!(!error.to_string().contains(marker));
        }
    }

    #[test]
    fn isolated_worker_rejects_invalid_requests_and_missing_python_is_typed() {
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

        let side_effect = temporary_directory("shell marker");
        let configured = format!("python; touch {}", side_effect.display());
        let shell_free = invoke_worker(
            OsStr::new(&configured),
            Path::new("missing-database.sqlite"),
            "2026-10-16T07:00:00Z",
            None,
            None,
            b"{}",
            ProcessLimits::default(),
        )
        .unwrap_err();
        assert_eq!(shell_free.code, "python_unavailable");
        assert!(!side_effect.exists());
    }

    #[test]
    fn missing_worker_root_fails_closed_without_exposing_the_path() {
        let missing = temporary_directory("missing worker root");
        let mut command = Command::new("python");
        command
            .args(["-I", "-S", "-c", WORKER_BOOTSTRAP])
            .arg("source")
            .arg(&missing)
            .arg("--database")
            .arg("missing.sqlite")
            .arg("--runtime-context-version")
            .arg(ANALYTICS_PROCESS_PROTOCOL_VERSION)
            .arg("--generated-at")
            .arg("2026-10-16T07:00:00Z");
        let error = run_command(command, b"{}", ProcessLimits::default()).unwrap_err();
        assert_eq!(error.code, "python_worker_resolution_failed");
        assert!(!error
            .to_string()
            .contains(&missing.to_string_lossy().to_string()));
    }

    #[test]
    fn worker_success_marker_is_exact_and_required() {
        assert!(validate_success_marker(SUCCESS_MARKER).is_ok());
        for invalid in [
            b"".as_slice(),
            b"codex_meter_worker_v1 input_complete\nextra\n",
            b"banner\ncodex_meter_worker_v1 input_complete\n",
        ] {
            assert_eq!(
                validate_success_marker(invalid).unwrap_err().code,
                "python_worker_protocol_failed"
            );
        }
    }

    #[test]
    fn working_directory_pythonpath_and_pythonhome_cannot_shadow_worker_resolution() {
        let shadow = temporary_directory("module shadow");
        let package = shadow.join("codex_meter");
        fs::create_dir_all(package.join("integration")).unwrap();
        fs::write(package.join("__init__.py"), "raise SystemExit(71)\n").unwrap();
        fs::write(package.join("integration/__init__.py"), "").unwrap();
        fs::write(
            package.join("integration/worker.py"),
            "raise SystemExit(72)\n",
        )
        .unwrap();

        let mut command = Command::new("python");
        command
            .args(["-I", "-S", "-c", WORKER_BOOTSTRAP])
            .arg("source")
            .arg(source_tree_root().unwrap().unwrap())
            .arg("--database")
            .arg("missing.sqlite")
            .arg("--runtime-context-version")
            .arg(ANALYTICS_PROCESS_PROTOCOL_VERSION)
            .arg("--generated-at")
            .arg("2026-10-16T07:00:00Z")
            .current_dir(&shadow)
            .env("PYTHONPATH", &shadow)
            .env("PYTHONHOME", "malformed-python-home");
        let error = run_command(command, b"{}", ProcessLimits::default()).unwrap_err();
        assert_eq!(error.code, "invalid_request");
        assert!(!error.to_string().contains("shadow"));
        fs::remove_dir_all(shadow).unwrap();
    }

    #[test]
    fn installed_worker_loads_from_a_selected_interpreter_path_with_spaces() {
        let environment = temporary_directory("installed interpreter");
        let status = Command::new("python")
            .args(["-m", "venv", "--without-pip"])
            .arg(&environment)
            .status()
            .expect("Python interpreter should start");
        assert!(status.success(), "Python venv should be created");

        #[cfg(windows)]
        let python = environment.join("Scripts/python.exe");
        #[cfg(windows)]
        let package_root = environment.join("Lib/site-packages");
        #[cfg(unix)]
        let python = environment.join("bin/python");
        #[cfg(unix)]
        let package_root = fs::read_dir(environment.join("lib"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("site-packages"))
            .find(|path| path.is_dir())
            .expect("venv site-packages should exist");

        let source_package =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../python/codex_meter");
        copy_directory(&source_package, &package_root.join("codex_meter")).unwrap();
        let metadata = package_root.join("codex_meter_analytics-0.0.0.dist-info");
        fs::create_dir_all(&metadata).unwrap();
        fs::write(
            metadata.join("METADATA"),
            "Metadata-Version: 2.3\nName: codex-meter-analytics\nVersion: 0.0.0\nRequires-Python: >=3.11\n",
        )
        .unwrap();
        let required_files = [
            "codex_meter/__init__.py",
            "codex_meter/integration/__init__.py",
            "codex_meter/integration/worker.py",
            "codex_meter/history/__init__.py",
            "codex_meter/history/models.py",
            "codex_meter/history/sqlite.py",
            "codex_meter/estimation/__init__.py",
            "codex_meter/estimation/candidates.py",
            "codex_meter/estimation/result.py",
            "codex_meter/estimation/robust.py",
        ];
        let mut record = required_files
            .iter()
            .map(|path| format!("{path},,\n"))
            .collect::<String>();
        record.push_str("codex_meter_analytics-0.0.0.dist-info/METADATA,,\n");
        record.push_str("codex_meter_analytics-0.0.0.dist-info/RECORD,,\n");
        fs::write(metadata.join("RECORD"), record).unwrap();
        assert!(python.to_string_lossy().contains(' '));

        let mut command = Command::new(&python);
        command
            .args(["-I", "-S", "-c", WORKER_BOOTSTRAP])
            .arg("installed")
            .arg("--database")
            .arg("missing.sqlite")
            .arg("--runtime-context-version")
            .arg(ANALYTICS_PROCESS_PROTOCOL_VERSION)
            .arg("--generated-at")
            .arg("2026-10-16T07:00:00Z");
        let error = run_command(command, b"{}", ProcessLimits::default()).unwrap_err();
        assert_eq!(error.code, "invalid_request");
        fs::remove_dir_all(environment).unwrap();
    }

    #[test]
    fn subprocess_failure_and_empty_output_are_categorized_without_echoing_tracebacks() {
        let import_failure =
            run_python_script("import p028_missing_test_module", ProcessLimits::default())
                .unwrap_err();
        assert_eq!(import_failure.code, "python_worker_failed");
        assert!(!import_failure
            .to_string()
            .contains("p028_missing_test_module"));

        let nonzero =
            run_python_script("raise SystemExit(7)", ProcessLimits::default()).unwrap_err();
        assert_eq!(nonzero.code, "python_worker_failed");

        let empty = run_python_script("pass", ProcessLimits::default()).unwrap_err();
        assert_eq!(empty.code, "invalid_result_json");
    }

    #[test]
    fn environment_is_cleared_except_for_the_documented_runtime_allowlist() {
        let mut command = Command::new("python");
        command
            .arg("-I")
            .arg("-S")
            .arg("-c")
            .arg("import sys; sys.stdin.read(); import os; print(os.environ.get('SYNTHETIC_SECRET_MARKER', 'absent'))")
            .env("SYNTHETIC_SECRET_MARKER", "SYNTHETIC_SECRET_MARKER");
        let output = run_command(command, b"{}", ProcessLimits::default()).unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim_end(), "absent");
    }

    #[test]
    fn output_and_diagnostic_limits_terminate_the_worker() {
        let output = run_python_script(
            "import sys; sys.stdout.buffer.write(b'x' * 32)",
            ProcessLimits {
                maximum_output_bytes: 16,
                ..ProcessLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(output.code, "process_output_too_large");

        let diagnostics = run_python_script(
            "import sys; sys.stderr.write('x' * 32)",
            ProcessLimits {
                maximum_diagnostic_bytes: 16,
                ..ProcessLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(diagnostics.code, "process_diagnostics_too_large");
    }

    #[test]
    fn concurrent_pipe_draining_handles_output_pressure() {
        let output = run_python_script(
            "import sys; sys.stdout.buffer.write(b'o' * 1048576); sys.stdout.flush(); sys.stderr.write('e' * 7000)",
            ProcessLimits::default(),
        )
        .unwrap();
        assert_eq!(output.len(), 1048576);
    }

    #[test]
    fn timeout_terminates_and_reaps_worker_within_the_execution_bound() {
        let started = Instant::now();
        let error = run_python_script(
            "import time; time.sleep(5)",
            ProcessLimits {
                timeout: Duration::from_secs(1),
                ..ProcessLimits::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "process_timeout");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn direct_exit_with_descendant_holding_pipes_is_failed_and_cleaned_up() {
        let error = run_python_script(
            "import subprocess, sys; subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)']); print('ok')",
            ProcessLimits::default(),
        )
        .unwrap_err();
        assert_eq!(error.code, "process_lifecycle_failed");
    }

    #[test]
    fn successful_exit_without_worker_input_acknowledgement_fails_closed() {
        let mut command = Command::new("python");
        command
            .arg("-I")
            .arg("-S")
            .arg("-c")
            .arg("import sys; sys.stdin.read(1); sys.stdout.write('{}')");
        let output = run_command(command, b"a", ProcessLimits::default()).unwrap();
        assert_ne!(output.stderr, SUCCESS_MARKER);
    }
}
