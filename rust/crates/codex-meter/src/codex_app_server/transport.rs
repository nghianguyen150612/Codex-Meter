use super::errors::CodexAppError;
use super::protocol::{
    notification, reject_server_request, request, MAX_MESSAGE_BYTES, MAX_PENDING_REQUESTS,
    MAX_REQUEST_ID, NOTIFICATION_QUEUE_CAPACITY, REQUEST_TIMEOUT_SECS,
};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const STDERR_READ_BUFFER: usize = 8 * 1024;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(20);
const EOF_PROCESS_EXIT_GRACE: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
enum TransportFault {
    AppServerUnsupported,
    AppServerStartupFailure,
    AuthenticationUnavailable,
    ServerShutdown,
    UnexpectedProcessExit,
    ProcessIoFailure,
    MalformedResponse,
}

impl TransportFault {
    fn error(&self) -> CodexAppError {
        match self {
            Self::AppServerUnsupported => CodexAppError::AppServerUnsupported,
            Self::AppServerStartupFailure => CodexAppError::AppServerStartupFailure,
            Self::AuthenticationUnavailable => CodexAppError::AuthenticationUnavailable,
            Self::ServerShutdown => CodexAppError::ServerShutdown,
            Self::UnexpectedProcessExit => CodexAppError::UnexpectedProcessExit,
            Self::ProcessIoFailure => CodexAppError::ProcessIoFailure,
            Self::MalformedResponse => CodexAppError::MalformedResponse,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RpcResponseError {
    pub(crate) code: Option<i64>,
    pub(crate) authentication: bool,
    pub(crate) unknown_thread: bool,
}

struct SharedState {
    pending: Mutex<HashMap<u64, mpsc::Sender<PendingResponse>>>,
    next_id: AtomicU64,
    issued_ids: AtomicU64,
    terminal: Mutex<Option<TransportFault>>,
    notification_sender: SyncSender<String>,
    dropped_notifications: AtomicU64,
    stderr_bytes_read: AtomicU64,
    unsupported_hint: AtomicBool,
    authentication_hint: AtomicBool,
    initialized: AtomicBool,
    process_exited: AtomicBool,
    closing: AtomicBool,
}

enum PendingResponse {
    Result(Value),
    Remote(RpcResponseError),
    Transport(TransportFault),
}

impl SharedState {
    fn fail(&self, fault: TransportFault) {
        if self.closing.load(Ordering::Acquire) {
            return;
        }
        let mut terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        if terminal.is_none()
            || matches!(fault, TransportFault::UnexpectedProcessExit)
            || matches!(fault, TransportFault::AppServerUnsupported)
        {
            *terminal = Some(fault.clone());
        }
        let fault = terminal.as_ref().expect("terminal was set").clone();
        drop(terminal);
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        for (_, sender) in pending.drain() {
            let _ = sender.send(PendingResponse::Transport(fault.clone()));
        }
    }

    fn current_fault(&self) -> Option<TransportFault> {
        self.terminal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn allocate_id(&self) -> Result<u64, CodexAppError> {
        loop {
            let id = self.next_id.load(Ordering::Acquire);
            if id == 0 || id > MAX_REQUEST_ID {
                return Err(CodexAppError::RequestIdExhausted);
            }
            if self
                .next_id
                .compare_exchange(id, id + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.issued_ids.fetch_max(id, Ordering::Release);
                return Ok(id);
            }
        }
    }
}

/// One bounded stdio process connection. The application layer owns and drops it.
pub(crate) struct ProcessTransport {
    writer: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
    state: Arc<SharedState>,
    notification_receiver: Mutex<Receiver<String>>,
    workers: Vec<JoinHandle<()>>,
}

impl ProcessTransport {
    pub(crate) fn spawn(program: &OsStr, args: &[&OsStr]) -> Result<Self, CodexAppError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CodexAppError::CodexCliUnavailable
            } else {
                CodexAppError::AppServerStartupFailure
            }
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or(CodexAppError::AppServerStartupFailure)?;
        let stdout = child
            .stdout
            .take()
            .ok_or(CodexAppError::AppServerStartupFailure)?;
        let stderr = child
            .stderr
            .take()
            .ok_or(CodexAppError::AppServerStartupFailure)?;
        let child = Arc::new(Mutex::new(child));
        let writer = Arc::new(Mutex::new(stdin));
        let (notification_sender, notification_receiver) =
            mpsc::sync_channel(NOTIFICATION_QUEUE_CAPACITY);
        let state = Arc::new(SharedState {
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            issued_ids: AtomicU64::new(0),
            terminal: Mutex::new(None),
            notification_sender,
            dropped_notifications: AtomicU64::new(0),
            stderr_bytes_read: AtomicU64::new(0),
            unsupported_hint: AtomicBool::new(false),
            authentication_hint: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            process_exited: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        });

        let mut workers = Vec::with_capacity(3);
        {
            let state = Arc::clone(&state);
            let writer = Arc::clone(&writer);
            let child = Arc::clone(&child);
            workers.push(
                thread::Builder::new()
                    .name("codex-meter-app-server-reader".to_owned())
                    .spawn(move || read_stdout(stdout, state, writer, child))
                    .map_err(|_| CodexAppError::AppServerStartupFailure)?,
            );
        }
        {
            let state = Arc::clone(&state);
            workers.push(
                thread::Builder::new()
                    .name("codex-meter-app-server-stderr".to_owned())
                    .spawn(move || drain_stderr(stderr, state))
                    .map_err(|_| CodexAppError::AppServerStartupFailure)?,
            );
        }
        {
            let state = Arc::clone(&state);
            let child = Arc::clone(&child);
            workers.push(
                thread::Builder::new()
                    .name("codex-meter-app-server-monitor".to_owned())
                    .spawn(move || monitor_child(child, state))
                    .map_err(|_| CodexAppError::AppServerStartupFailure)?,
            );
        }

        Ok(Self {
            writer,
            child,
            state,
            notification_receiver: Mutex::new(notification_receiver),
            workers,
        })
    }

    pub(crate) fn request(
        &self,
        method: &'static str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CodexAppError> {
        let deadline = Instant::now() + timeout;
        if let Some(fault) = self.state.current_fault() {
            return Err(fault.error());
        }
        let initialized = self.state.initialized.load(Ordering::Acquire);
        if (!initialized && method != "initialize") || (initialized && method == "initialize") {
            return Err(CodexAppError::InitializationFailure);
        }
        let id = self.state.allocate_id()?;
        let (sender, receiver) = mpsc::channel();
        {
            let mut pending = self.state.pending.lock().unwrap_or_else(|e| e.into_inner());
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(CodexAppError::RequestLimitExceeded);
            }
            pending.insert(id, sender);
        }
        if let Err(error) = self.write_value(&request(id, method, params), deadline) {
            self.state
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            self.state.fail(TransportFault::ProcessIoFailure);
            return Err(error);
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(PendingResponse::Result(result)) => Ok(result),
            Ok(PendingResponse::Remote(remote)) => Err(remote_error(method, remote)),
            Ok(PendingResponse::Transport(fault)) => Err(fault.error()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.state
                    .pending
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                Err(CodexAppError::RequestTimeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(self
                .state
                .current_fault()
                .map_or(CodexAppError::UnexpectedProcessExit, |fault| fault.error())),
        }
    }

    pub(crate) fn notify(&self, method: &str, params: Value) -> Result<(), CodexAppError> {
        if let Some(fault) = self.state.current_fault() {
            return Err(fault.error());
        }
        self.write_value(
            &notification(method, params),
            Instant::now() + Duration::from_secs(REQUEST_TIMEOUT_SECS),
        )
    }

    pub(crate) fn mark_initialized(&self) {
        self.state.initialized.store(true, Ordering::Release);
    }

    pub(crate) fn try_notification(&self) -> Option<String> {
        self.notification_receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv()
            .ok()
    }

    pub(crate) fn dropped_notifications(&self) -> u64 {
        self.state.dropped_notifications.load(Ordering::Relaxed)
    }

    fn write_value(&self, value: &Value, deadline: Instant) -> Result<(), CodexAppError> {
        let mut line = serde_json::to_vec(value).map_err(|_| CodexAppError::MalformedResponse)?;
        if line.len() + 1 > MAX_MESSAGE_BYTES {
            return Err(CodexAppError::MalformedResponse);
        }
        line.push(b'\n');
        let writer = Arc::clone(&self.writer);
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("codex-meter-app-server-writer".to_owned())
            .spawn(move || {
                let result = writer
                    .lock()
                    .map_err(|_| CodexAppError::ProcessIoFailure)
                    .and_then(|mut writer| {
                        writer
                            .write_all(&line)
                            .and_then(|()| writer.flush())
                            .map_err(|_| CodexAppError::ProcessIoFailure)
                    });
                let _ = sender.send(result);
            })
            .map_err(|_| CodexAppError::AppServerStartupFailure)?;
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => {
                let _ = worker.join();
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.kill_child();
                self.state.fail(TransportFault::ProcessIoFailure);
                let _ = worker.join();
                Err(CodexAppError::RequestTimeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                Err(CodexAppError::ProcessIoFailure)
            }
        }
    }

    fn kill_child(&self) {
        if let Ok(mut child) = self.child.lock() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            self.state.process_exited.store(true, Ordering::Release);
        }
    }
}

impl Drop for ProcessTransport {
    fn drop(&mut self) {
        self.state.closing.store(true, Ordering::Release);
        let mut pending = self.state.pending.lock().unwrap_or_else(|e| e.into_inner());
        for (_, sender) in pending.drain() {
            let _ = sender.send(PendingResponse::Transport(TransportFault::ServerShutdown));
        }
        drop(pending);
        if let Ok(mut child) = self.child.lock() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            self.state.process_exited.store(true, Ordering::Release);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn remote_error(method: &'static str, error: RpcResponseError) -> CodexAppError {
    if error.authentication {
        CodexAppError::AuthenticationUnavailable
    } else if error.unknown_thread {
        CodexAppError::UnknownThread
    } else if error.code == Some(-32601) {
        CodexAppError::UnsupportedMethod(method)
    } else {
        CodexAppError::RemoteRequestRejected {
            method,
            code: error.code,
        }
    }
}

fn read_stdout(
    stdout: ChildStdout,
    state: Arc<SharedState>,
    writer: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_message(&mut reader, MAX_MESSAGE_BYTES) {
            Ok(Some(line)) => {
                let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                    state.fail(TransportFault::MalformedResponse);
                    return;
                };
                if let Err(error) = route_message(value, &state, &writer) {
                    state.fail(match error {
                        RouteMessageError::Malformed => TransportFault::MalformedResponse,
                        RouteMessageError::Io => TransportFault::ProcessIoFailure,
                    });
                    return;
                }
            }
            Ok(None) => {
                if state.closing.load(Ordering::Acquire) {
                    return;
                }
                let deadline = Instant::now() + EOF_PROCESS_EXIT_GRACE;
                while !state.process_exited.load(Ordering::Acquire) && Instant::now() < deadline {
                    let status = match child.lock() {
                        Ok(mut child) => child.try_wait(),
                        Err(_) => {
                            state.fail(TransportFault::ProcessIoFailure);
                            return;
                        }
                    };
                    match status {
                        Ok(Some(_)) => {
                            state.process_exited.store(true, Ordering::Release);
                            state.fail(process_exit_fault(&state));
                            break;
                        }
                        Ok(None) => {}
                        Err(_) => {
                            state.fail(TransportFault::ProcessIoFailure);
                            return;
                        }
                    }
                    thread::sleep(Duration::from_millis(5));
                }
                let fault = if let Some(fault) = state.current_fault() {
                    fault
                } else if state.process_exited.load(Ordering::Acquire) {
                    process_exit_fault(&state)
                } else {
                    TransportFault::ServerShutdown
                };
                state.fail(fault);
                return;
            }
            Err(ReadMessageError::TooLarge | ReadMessageError::Malformed) => {
                state.fail(TransportFault::MalformedResponse);
                return;
            }
            Err(ReadMessageError::Io) => {
                if !state.closing.load(Ordering::Acquire) {
                    state.fail(TransportFault::ProcessIoFailure);
                }
                return;
            }
        }
    }
}

fn read_message<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<Vec<u8>>, ReadMessageError> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().map_err(|_| ReadMessageError::Io)?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Err(ReadMessageError::Malformed)
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(count) > limit {
            return Err(ReadMessageError::TooLarge);
        }
        let payload_count = newline.unwrap_or(count);
        line.extend_from_slice(&available[..payload_count]);
        reader.consume(count);
        if newline.is_some() {
            return Ok(Some(line));
        }
    }
}

enum ReadMessageError {
    TooLarge,
    Malformed,
    Io,
}

enum RouteMessageError {
    Malformed,
    Io,
}

fn route_message(
    value: Value,
    state: &SharedState,
    writer: &Arc<Mutex<ChildStdin>>,
) -> Result<(), RouteMessageError> {
    let malformed = || RouteMessageError::Malformed;
    let object = value.as_object().ok_or_else(malformed)?;
    if let Some(method) = object.get("method") {
        let method = method.as_str().ok_or_else(malformed)?;
        if method.is_empty() || method.len() > 128 || method.chars().any(char::is_control) {
            return Err(RouteMessageError::Malformed);
        }
        if let Some(id) = object.get("id") {
            let response = reject_server_request(id.clone());
            let mut bytes = serde_json::to_vec(&response).map_err(|_| malformed())?;
            bytes.push(b'\n');
            if bytes.len() > MAX_MESSAGE_BYTES {
                return Err(RouteMessageError::Malformed);
            }
            let mut writer = writer.lock().map_err(|_| RouteMessageError::Io)?;
            writer
                .write_all(&bytes)
                .and_then(|()| writer.flush())
                .map_err(|_| RouteMessageError::Io)?;
        } else if state
            .notification_sender
            .try_send(method.to_owned())
            .is_err()
        {
            state.dropped_notifications.fetch_add(1, Ordering::Relaxed);
        }
        return Ok(());
    }

    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(malformed)?;
    let result = object.get("result");
    let error = object.get("error");
    if result.is_some() == error.is_some() {
        return Err(RouteMessageError::Malformed);
    }
    let sender = state
        .pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id);
    let response = if let Some(result) = result {
        PendingResponse::Result(result.clone())
    } else {
        PendingResponse::Remote(parse_response_error(error.ok_or_else(malformed)?))
    };
    if let Some(sender) = sender {
        let _ = sender.send(response);
    } else if id > state.issued_ids.load(Ordering::Acquire) {
        return Err(RouteMessageError::Malformed);
    }
    Ok(())
}

fn parse_response_error(error: &Value) -> RpcResponseError {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let data = error.get("data");
    RpcResponseError {
        code,
        authentication: contains_auth_marker(data)
            || message.contains("unauthorized")
            || message.contains("not authenticated")
            || message.contains("authentication required"),
        unknown_thread: message.contains("thread not found") || message.contains("unknown thread"),
    }
}

fn contains_auth_marker(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(value)) => {
            matches!(
                value.as_str(),
                "unauthorized" | "authenticationRequired" | "notAuthenticated"
            )
        }
        Some(Value::Object(object)) => object.iter().any(|(key, value)| {
            matches!(key.as_str(), "codexErrorInfo" | "type" | "code")
                && contains_auth_marker(Some(value))
        }),
        Some(Value::Array(values)) => values.iter().any(|value| contains_auth_marker(Some(value))),
        _ => false,
    }
}

fn drain_stderr(mut stderr: ChildStderr, state: Arc<SharedState>) {
    let mut buffer = [0_u8; STDERR_READ_BUFFER];
    loop {
        match stderr.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                state
                    .stderr_bytes_read
                    .fetch_add(count as u64, Ordering::Relaxed);
                let text = String::from_utf8_lossy(&buffer[..count]).to_ascii_lowercase();
                if [
                    "unknown subcommand",
                    "unrecognized subcommand",
                    "no such command",
                ]
                .iter()
                .any(|marker| text.contains(marker))
                {
                    state.unsupported_hint.store(true, Ordering::Release);
                }
                if [
                    "not logged in",
                    "authentication required",
                    "not authenticated",
                ]
                .iter()
                .any(|marker| text.contains(marker))
                {
                    // Retain only a typed marker. Never retain or print the stderr bytes.
                    state.authentication_hint.store(true, Ordering::Release);
                }
            }
        }
    }
}

fn monitor_child(child: Arc<Mutex<Child>>, state: Arc<SharedState>) {
    loop {
        if state.closing.load(Ordering::Acquire) {
            return;
        }
        let status = match child.lock() {
            Ok(mut child) => match child.try_wait() {
                Ok(status) => status,
                Err(_) => {
                    state.fail(TransportFault::ProcessIoFailure);
                    return;
                }
            },
            Err(_) => {
                state.fail(TransportFault::ProcessIoFailure);
                return;
            }
        };
        if status.is_some() {
            state.process_exited.store(true, Ordering::Release);
            state.fail(process_exit_fault(&state));
            return;
        }
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

fn process_exit_fault(state: &SharedState) -> TransportFault {
    if state.authentication_hint.load(Ordering::Acquire) {
        TransportFault::AuthenticationUnavailable
    } else if state.unsupported_hint.load(Ordering::Acquire) {
        TransportFault::AppServerUnsupported
    } else if state.initialized.load(Ordering::Acquire) {
        TransportFault::UnexpectedProcessExit
    } else {
        TransportFault::AppServerStartupFailure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn python() -> OsString {
        std::env::var_os("PYTHON").unwrap_or_else(|| {
            if cfg!(windows) {
                OsString::from("python")
            } else {
                OsString::from("python3")
            }
        })
    }

    fn script() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_app_server.py")
    }

    fn client(mode: &str, timeout: Duration) -> ProcessTransport {
        let program = python();
        let script = script();
        let args = [script.as_os_str(), OsStr::new(mode)];
        let transport = ProcessTransport::spawn(&program, &args).expect("fake server starts");
        let initialize = transport
            .request(
                "initialize",
                json!({"clientInfo":{"name":"codex-meter","title":"Codex Meter","version":"test"}}),
                timeout.max(Duration::from_secs(10)),
            )
            .expect("initialize response");
        assert!(initialize.get("userAgent").is_some());
        transport.mark_initialized();
        transport
            .notify("initialized", json!({}))
            .expect("initialized notification");
        transport
    }

    #[test]
    fn matches_responses_and_keeps_notifications_separate() {
        let client = client("notification", Duration::from_secs(2));
        let response = client
            .request("probe", json!({}), Duration::from_secs(2))
            .unwrap();
        assert_eq!(response, json!({"ok":true}));
        assert_eq!(
            client.try_notification().as_deref(),
            Some("thread/status/changed")
        );
    }

    #[test]
    fn notification_queue_is_bounded_and_discards_payloads() {
        let client = client("notification_flood", Duration::from_secs(2));
        assert_eq!(
            client
                .request("probe", json!({}), Duration::from_secs(2))
                .unwrap(),
            json!({"ok":true})
        );
        let methods = (0..NOTIFICATION_QUEUE_CAPACITY)
            .map(|_| client.try_notification().expect("bounded method name"))
            .collect::<Vec<_>>();
        assert!(methods
            .iter()
            .all(|method| method == "thread/status/changed"));
        assert_eq!(
            client.dropped_notifications(),
            256 - NOTIFICATION_QUEUE_CAPACITY as u64
        );
    }

    #[test]
    fn matches_concurrent_out_of_order_responses() {
        let client = Arc::new(client("concurrent", Duration::from_secs(3)));
        let first = {
            let client = Arc::clone(&client);
            thread::spawn(move || client.request("first", json!({}), Duration::from_secs(3)))
        };
        let second = {
            let client = Arc::clone(&client);
            thread::spawn(move || client.request("second", json!({}), Duration::from_secs(3)))
        };
        assert_eq!(first.join().unwrap().unwrap(), json!({"method":"first"}));
        assert_eq!(second.join().unwrap().unwrap(), json!({"method":"second"}));
    }

    #[test]
    fn server_requests_are_rejected_without_approval() {
        let client = client("server_request", Duration::from_secs(2));
        assert_eq!(
            client
                .request("probe", json!({}), Duration::from_secs(2))
                .unwrap(),
            json!({"approved":false,"code":-32601})
        );
    }

    #[test]
    fn remote_error_text_is_not_exposed_in_typed_failures() {
        let client = client("remote_sensitive", Duration::from_secs(2));
        let error = client
            .request("probe", json!({}), Duration::from_secs(2))
            .unwrap_err();
        assert_eq!(
            error,
            CodexAppError::RemoteRequestRejected {
                method: "probe",
                code: Some(-32000),
            }
        );
        assert!(!error.to_string().contains("SENSITIVE_ERROR_SENTINEL"));
        assert!(!error.to_string().contains("/private/path"));
    }

    #[test]
    fn malformed_and_oversized_messages_fail_closed() {
        for mode in ["malformed", "oversized"] {
            let client = client(mode, Duration::from_secs(2));
            assert_eq!(
                client
                    .request("probe", json!({}), Duration::from_secs(2))
                    .unwrap_err(),
                CodexAppError::MalformedResponse
            );
        }
    }

    #[test]
    fn early_exit_is_reported_and_a_timeout_does_not_hang() {
        let exited_client = client("exit", Duration::from_secs(2));
        let error = exited_client
            .request("probe", json!({}), Duration::from_secs(2))
            .unwrap_err();
        assert!(matches!(
            error,
            CodexAppError::UnexpectedProcessExit | CodexAppError::AppServerStartupFailure
        ));

        let timeout_client = client("timeout", Duration::from_millis(50));
        assert_eq!(
            timeout_client
                .request("probe", json!({}), Duration::from_millis(50))
                .unwrap_err(),
            CodexAppError::RequestTimeout
        );
    }

    #[test]
    fn request_deadline_covers_a_server_that_stops_reading_stdin() {
        let client = client("no_read", Duration::from_secs(2));
        let started = Instant::now();
        assert_eq!(
            client
                .request(
                    "probe",
                    json!({"payload":"x".repeat(3 * 1024 * 1024)}),
                    Duration::from_millis(100),
                )
                .unwrap_err(),
            CodexAppError::RequestTimeout
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(client.state.process_exited.load(Ordering::Acquire));
    }

    #[test]
    fn broken_pipe_and_server_shutdown_are_typed_transport_failures() {
        let broken_client = Arc::new(client("broken_pipe", Duration::from_secs(2)));
        let first_request = {
            let client = Arc::clone(&broken_client);
            thread::spawn(move || client.request("probe", json!({}), Duration::from_secs(3)))
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut ready = false;
        while Instant::now() < deadline && !ready {
            ready = broken_client.try_notification().as_deref() == Some("ready-to-break");
            thread::sleep(Duration::from_millis(5));
        }
        assert!(ready, "fake server should close its input pipe");
        assert_eq!(
            broken_client
                .request("probe", json!({}), Duration::from_secs(1))
                .unwrap_err(),
            CodexAppError::ProcessIoFailure
        );
        assert_eq!(
            first_request.join().unwrap().unwrap_err(),
            CodexAppError::ProcessIoFailure
        );

        let client = client("server_shutdown", Duration::from_secs(2));
        assert_eq!(
            client
                .request("probe", json!({}), Duration::from_secs(2))
                .unwrap_err(),
            CodexAppError::ServerShutdown
        );
    }

    #[test]
    fn dropping_the_connection_kills_and_reaps_the_server() {
        let client = client("idle", Duration::from_secs(2));
        let state = Arc::clone(&client.state);
        drop(client);
        assert!(state.process_exited.load(Ordering::Acquire));
    }

    #[test]
    fn stderr_flood_is_drained_without_being_retained() {
        let client = client("stderr_flood", Duration::from_secs(4));
        assert_eq!(
            client
                .request("probe", json!({}), Duration::from_secs(4))
                .unwrap(),
            json!({"ok":true})
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while client.state.stderr_bytes_read.load(Ordering::Relaxed) < 1024 * 1024
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(client.state.stderr_bytes_read.load(Ordering::Relaxed) >= 1024 * 1024);
    }

    #[test]
    fn repeated_lifecycles_and_large_partial_writes_are_bounded() {
        for _ in 0..3 {
            let client = client("large_echo", Duration::from_secs(5));
            let payload = "x".repeat(3 * 1024 * 1024);
            let response = client
                .request("probe", json!({"payload":payload}), Duration::from_secs(5))
                .unwrap();
            assert_eq!(response["length"], json!(3 * 1024 * 1024));
        }
    }
}
