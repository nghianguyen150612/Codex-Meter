#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);
static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());
const PROMPT: &str = "P037_λ_SYNTHETIC_PROMPT";

fn serialize_pty_test() -> MutexGuard<'static, ()> {
    PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct TestPaths {
    root: std::path::PathBuf,
    bin: std::path::PathBuf,
    workspace: std::path::PathBuf,
    data: std::path::PathBuf,
    state: std::path::PathBuf,
    request_log: std::path::PathBuf,
    continue_file: std::path::PathBuf,
    approval: std::path::PathBuf,
}

impl TestPaths {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos());
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "codex-meter-pty-{}-{timestamp}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let bin = root.join("bin");
        let workspace = root.join("workspace");
        let data = root.join("data");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&data).unwrap();
        let wrapper = bin.join("codex");
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/fake_pty_app_server.py");
        let wrapper_text = format!(
            "#!/bin/sh\n[ \"$1\" = \"app-server\" ] || exit 39\nexec python3 '{}' \"$CODEX_METER_TEST_MODE\" \"$CODEX_METER_TEST_WORKSPACE\"\n",
            fixture.display()
        );
        fs::write(&wrapper, wrapper_text).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            state: root.join("fake-state.json"),
            request_log: root.join("requests.log"),
            continue_file: root.join("continue"),
            approval: root.join("fake-state.json.approval"),
            root,
            bin,
            workspace,
            data,
        }
    }

    fn journal(&self) -> std::path::PathBuf {
        #[cfg(target_os = "macos")]
        {
            self.root
                .join("Library/Application Support/codex-meter/recovery/journal.json")
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.data.join("codex-meter/recovery/journal.json")
        }
    }

    fn request_count(&self, method: &str) -> usize {
        fs::read_to_string(&self.request_log)
            .unwrap_or_default()
            .lines()
            .filter(|line| *line == method)
            .count()
    }

    fn wait_for_request_count(&self, method: &str, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.request_count(method) >= count {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

impl Drop for TestPaths {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct PtyHarness {
    master: fs::File,
    slave: fs::File,
    terminal_probe: fs::File,
    baseline: libc::termios,
    output: Arc<Mutex<Vec<u8>>>,
    reader_stop: Arc<std::sync::atomic::AtomicBool>,
    reader: Option<JoinHandle<()>>,
    child: Option<Child>,
    paths: TestPaths,
}

impl PtyHarness {
    fn new(paths: TestPaths) -> Self {
        let mut master_fd = -1;
        let mut slave_fd = -1;
        let mut window = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let opened = unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::addr_of_mut!(window),
            )
        };
        assert_eq!(opened, 0);
        let master = unsafe { fs::File::from_raw_fd(master_fd) };
        let slave = unsafe { fs::File::from_raw_fd(slave_fd) };
        let terminal_probe = slave.try_clone().unwrap();
        let mut baseline = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(terminal_probe.as_raw_fd(), &mut baseline) },
            0
        );

        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut reader_file = master.try_clone().unwrap();
        let flags = unsafe { libc::fcntl(reader_file.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(
            unsafe {
                libc::fcntl(
                    reader_file.as_raw_fd(),
                    libc::F_SETFL,
                    flags | libc::O_NONBLOCK,
                )
            },
            0
        );
        let reader_output = Arc::clone(&output);
        let stop = Arc::clone(&reader_stop);
        let reader = thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            while !stop.load(Ordering::Acquire) {
                match reader_file.read(&mut buffer) {
                    Ok(0) => thread::sleep(Duration::from_millis(10)),
                    Ok(count) => {
                        let mut output = reader_output
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if output.len() < 8 * 1024 * 1024 {
                            output.extend_from_slice(&buffer[..count]);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            master,
            slave,
            terminal_probe,
            baseline,
            output,
            reader_stop,
            reader: Some(reader),
            child: None,
            paths,
        }
    }

    fn spawn(&mut self, command: &str, mode: &str) {
        assert!(self.child.is_none());
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let mut search_path = self.paths.bin.as_os_str().to_os_string();
        search_path.push(":");
        search_path.push(original_path);
        let mut child_command = Command::new(env!("CARGO_BIN_EXE_codex-meter"));
        child_command
            .arg(command)
            .current_dir(&self.paths.workspace)
            .env("PATH", search_path)
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .env("LC_ALL", "C")
            // macOS stores recovery under ~/Library/Application Support;
            // point HOME at this test's isolated root so no user's journal
            // can affect the test and restarts share only this fixture.
            .env("HOME", &self.paths.root)
            .env("XDG_DATA_HOME", &self.paths.data)
            .env("CODEX_METER_TEST_MODE", mode)
            .env("CODEX_METER_TEST_WORKSPACE", &self.paths.workspace)
            .env("CODEX_METER_TEST_STATE", &self.paths.state)
            .env("CODEX_METER_TEST_REQUEST_LOG", &self.paths.request_log)
            .env("CODEX_METER_TEST_CONTINUE", &self.paths.continue_file)
            .env("CODEX_METER_TEST_PROMPT", PROMPT)
            .stdin(Stdio::from(self.slave.try_clone().unwrap()))
            .stdout(Stdio::from(self.slave.try_clone().unwrap()))
            .stderr(Stdio::from(self.slave.try_clone().unwrap()));
        child_command.process_group(0);
        let child = child_command.spawn().unwrap();
        self.child = Some(child);
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
    }

    fn key(&mut self, byte: u8) {
        self.send(&[byte]);
        thread::sleep(Duration::from_millis(120));
    }

    fn back_tab(&mut self) {
        self.send(b"\x1b[Z");
        thread::sleep(Duration::from_millis(120));
    }

    fn wait_for(&self, text: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let output = self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let found = visible_screen(&output).contains(text);
            if found {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn assert_text(&self, text: &str) {
        if !self.wait_for(text, Duration::from_secs(15)) {
            let visible = self.output_text();
            let raw = self.raw_output_text();
            let raw_tail = raw
                .chars()
                .rev()
                .take(4_000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>();
            let request_log = fs::read_to_string(&self.paths.request_log).unwrap_or_default();
            let state = fs::read_to_string(&self.paths.state).unwrap_or_default();
            panic!(
                "expected PTY output to contain {text:?}; visible screen: {visible}; recent raw PTY output: {raw_tail}; fake requests: {request_log:?}; fake state: {state:?}"
            );
        }
    }

    fn output_text(&self) -> String {
        visible_screen(
            &self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn raw_output_text(&self) -> String {
        String::from_utf8_lossy(
            &self
                .output
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .into_owned()
    }

    fn begin_task(&mut self) {
        self.assert_text("Composer");
        thread::sleep(Duration::from_millis(300));
        self.send(PROMPT.as_bytes());
        thread::sleep(Duration::from_millis(500));
        for _ in 0..3 {
            self.back_tab();
        }
        self.key(b'w');
        for _ in 0..4 {
            self.key(b'\t');
        }
        self.key(b'r');
        self.assert_text("Task preflight");
        self.key(b'e');
        self.assert_text("Confirm Codex task");
        self.key(b'\r');
    }

    fn resize(&self, columns: u16, rows: u16) {
        let window = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::ioctl(
                    self.master.as_raw_fd(),
                    libc::TIOCSWINSZ,
                    &window as *const libc::winsize,
                )
            },
            0
        );
        if let Some(child) = self.child.as_ref() {
            assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGWINCH) }, 0);
        }
    }

    fn restore_terminal_after_forceful_kill(&self) {
        assert_eq!(
            unsafe {
                libc::tcsetattr(
                    self.terminal_probe.as_raw_fd(),
                    libc::TCSANOW,
                    &self.baseline,
                )
            },
            0
        );
    }

    fn kill_process_group(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let process_group = -(child.id() as i32);
            assert_eq!(unsafe { libc::kill(process_group, libc::SIGKILL) }, 0);
            let _ = child.wait();
        }
        self.child = None;
    }

    fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let child = self.child.as_mut().expect("child is running");
            if let Some(status) = child.try_wait().unwrap() {
                self.child = None;
                return status;
            }
            assert!(Instant::now() < deadline, "TUI process did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_terminal_restored(&self) {
        let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.terminal_probe.as_raw_fd(), &mut after) },
            0
        );
        assert_eq!(after.c_lflag, self.baseline.c_lflag);
        assert_eq!(after.c_iflag, self.baseline.c_iflag);
        let output = self.raw_output_text();
        assert!(
            output.contains("?1049h"),
            "alternate screen was not entered"
        );
        assert!(
            output.contains("?1049l"),
            "alternate screen was not restored"
        );
        assert!(output.contains("?25l"), "cursor was not hidden");
        assert!(output.contains("?25h"), "cursor was not restored");
        assert!(
            !output.contains("\x1b[38;5;"),
            "NO_COLOR output contained indexed colors"
        );
    }
}

fn visible_screen(bytes: &[u8]) -> String {
    const ROWS: usize = 64;
    const COLUMNS: usize = 180;
    let mut screen = vec![vec![' '; COLUMNS]; ROWS];
    let (mut row, mut column) = (0_usize, 0_usize);
    let visible = String::from_utf8_lossy(bytes);
    let mut chars = visible.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut sequence = String::new();
                    for value in chars.by_ref() {
                        if ('@'..='~').contains(&value) {
                            let parameters = sequence
                                .trim_start_matches('?')
                                .split(';')
                                .filter_map(|part| part.parse::<usize>().ok())
                                .collect::<Vec<_>>();
                            match value {
                                'H' | 'f' => {
                                    row = parameters
                                        .first()
                                        .copied()
                                        .unwrap_or(1)
                                        .saturating_sub(1)
                                        .min(ROWS - 1);
                                    column = parameters
                                        .get(1)
                                        .copied()
                                        .unwrap_or(1)
                                        .saturating_sub(1)
                                        .min(COLUMNS - 1);
                                }
                                'A' => {
                                    row =
                                        row.saturating_sub(parameters.first().copied().unwrap_or(1))
                                }
                                'B' => {
                                    row = row
                                        .saturating_add(parameters.first().copied().unwrap_or(1))
                                        .min(ROWS - 1)
                                }
                                'C' => {
                                    column = column
                                        .saturating_add(parameters.first().copied().unwrap_or(1))
                                        .min(COLUMNS - 1)
                                }
                                'D' => {
                                    column = column
                                        .saturating_sub(parameters.first().copied().unwrap_or(1))
                                }
                                'G' => {
                                    column = parameters
                                        .first()
                                        .copied()
                                        .unwrap_or(1)
                                        .saturating_sub(1)
                                        .min(COLUMNS - 1)
                                }
                                'd' => {
                                    row = parameters
                                        .first()
                                        .copied()
                                        .unwrap_or(1)
                                        .saturating_sub(1)
                                        .min(ROWS - 1)
                                }
                                'J' if parameters.first() == Some(&2)
                                    || parameters.first() == Some(&3) =>
                                {
                                    for line in &mut screen {
                                        line.fill(' ');
                                    }
                                }
                                'K' => screen[row][column..].fill(' '),
                                _ => {}
                            }
                            break;
                        }
                        sequence.push(value);
                    }
                }
                Some(']') => {
                    while let Some(value) = chars.next() {
                        if value == '\x07' {
                            break;
                        }
                        if value == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) | None => {}
            },
            '\r' => column = 0,
            '\n' => row = row.saturating_add(1).min(ROWS - 1),
            '\x08' => column = column.saturating_sub(1),
            '\t' => column = ((column / 8) + 1) * 8,
            value if !value.is_control() => {
                if column < COLUMNS {
                    screen[row][column] = value;
                }
                column = column.saturating_add(1).min(COLUMNS - 1);
            }
            _ => {}
        }
    }
    screen
        .into_iter()
        .map(|line| line.into_iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

impl Drop for PtyHarness {
    fn drop(&mut self) {
        self.kill_process_group();
        self.reader_stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn temp_file(path: &std::path::Path) {
    fs::write(path, b"continue").unwrap();
}

#[test]
fn compiled_tui_executes_unicode_prompt_and_restores_the_terminal() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "success");
    pty.begin_task();
    pty.assert_text("Running");
    pty.resize(16, 8);
    pty.assert_text("Resize terminal");
    pty.resize(120, 40);
    temp_file(&pty.paths.continue_file);
    pty.assert_text("Completed");
    pty.key(b'\r');
    pty.assert_text("Composer");
    pty.key(b'q');
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
    assert_eq!(pty.paths.request_count("thread/start"), 1);
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    let journal = fs::read(pty.paths.journal()).unwrap();
    let journal_text = String::from_utf8(journal).unwrap();
    assert!(journal_text.contains("thread-pty"));
    assert!(journal_text.contains("turn-pty"));
    assert!(!journal_text.contains(PROMPT));
    assert!(!journal_text.contains(pty.paths.workspace.to_str().unwrap()));
}

#[test]
fn pty_approval_allows_only_the_explicit_one_time_decision() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "approval_allow");
    pty.begin_task();
    assert!(pty.wait_for("APPROVAL REQUIRED", Duration::from_secs(10)));
    pty.assert_text("printf synthetic");
    pty.key(b'a');
    pty.assert_text("Completed");
    pty.key(b'\r');
    pty.assert_text("Overview");
    pty.key(b'q');
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
    let decision: serde_json::Value =
        serde_json::from_slice(&fs::read(&pty.paths.approval).unwrap()).unwrap();
    assert_eq!(decision, serde_json::json!({"decision":"accept"}));
}

#[test]
fn pty_approval_denial_is_the_default_enter_action() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "approval_deny");
    pty.begin_task();
    pty.assert_text("APPROVAL REQUIRED");
    pty.key(b'\r');
    pty.assert_text("Completed");
    pty.key(b'\r');
    pty.assert_text("Overview");
    pty.key(b'q');
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
    let decision: serde_json::Value =
        serde_json::from_slice(&fs::read(&pty.paths.approval).unwrap()).unwrap();
    assert_eq!(decision, serde_json::json!({"decision":"decline"}));
}

#[test]
fn pty_interrupt_acknowledgement_waits_for_terminal_turn_state() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "interrupt");
    pty.begin_task();
    pty.assert_text("Running");
    pty.key(b'i');
    pty.assert_text("Interrupt request acknowledged");
    assert!(!pty.output_text().contains("Interrupted"));
    temp_file(&pty.paths.continue_file);
    pty.assert_text("Interrupted");
    pty.key(b'\r');
    pty.assert_text("Overview");
    pty.key(b'q');
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    assert_eq!(pty.paths.request_count("turn/interrupt"), 1);
}

#[test]
fn pty_connection_loss_reconciles_without_resubmission() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "connection_loss");
    pty.begin_task();
    pty.assert_text("Outcome unknown");
    pty.key(b'r');
    pty.assert_text("Codex reports the known turn is still running");
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    pty.key(b'i');
    pty.assert_text("Interrupt request acknowledged");
    temp_file(&pty.paths.continue_file);
    pty.assert_text("Interrupted");
    pty.key(b'\r');
    pty.assert_text("Overview");
    pty.key(b'q');
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
}

#[test]
fn pty_process_crash_persists_recovery_and_never_replays_the_turn() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "crash");
    pty.begin_task();
    pty.assert_text("Running");
    pty.kill_process_group();
    let bytes = fs::read(pty.paths.journal()).unwrap();
    let journal_text = String::from_utf8(bytes).unwrap();
    assert!(journal_text.contains("thread-pty"));
    assert!(journal_text.contains("turn-pty"));
    assert!(journal_text.contains("\"recovery_required\":true"));

    // SIGKILL bypasses terminal cleanup; the test shell restores its own PTY
    // before reopening the app, matching the recovery guidance in the docs.
    pty.restore_terminal_after_forceful_kill();
    pty.spawn("tui", "recovery");
    pty.assert_text("EXECUTION RECOVERY");
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    pty.key(b'r');
    pty.assert_text("Still running");
    pty.key(b'i');
    pty.assert_text("Interrupt acknowledged; awaiting final state");
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    assert_eq!(pty.paths.request_count("turn/interrupt"), 1);
    pty.key(b'\x1b');
    pty.assert_text("Overview");
    pty.key(b'q');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
}

#[test]
fn pty_crash_after_possible_thread_start_preserves_unknown_identity_without_replay() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "thread_start_crash");
    pty.begin_task();
    assert!(
        pty.paths
            .wait_for_request_count("thread/start", 1, Duration::from_secs(10)),
        "thread/start was not observed; PTY output: {}; fake requests: {:?}; fake state: {:?}",
        pty.output_text(),
        fs::read_to_string(&pty.paths.request_log).unwrap_or_default(),
        fs::read_to_string(&pty.paths.state).unwrap_or_default()
    );
    pty.kill_process_group();

    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(pty.paths.journal()).unwrap()).unwrap();
    let record = &journal["payload"]["records"][0];
    assert_eq!(record["execution_phase"], "thread_start_may_have_been_sent");
    assert!(record["known_thread_id"].is_null());
    assert_eq!(record["recovery_required"], true);

    pty.restore_terminal_after_forceful_kill();
    pty.spawn("tui", "recovery");
    pty.assert_text("EXECUTION RECOVERY");
    assert_eq!(pty.paths.request_count("thread/start"), 1);
    assert_eq!(pty.paths.request_count("turn/start"), 0);
    pty.key(b'r');
    pty.assert_text("Outcome unknown");
    assert_eq!(pty.paths.request_count("thread/start"), 1);
    assert_eq!(pty.paths.request_count("turn/start"), 0);
    pty.key(b'\x1b');
    pty.key(b'q');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
}

#[test]
fn pty_crash_after_possible_turn_start_keeps_thread_known_and_never_replays() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "turn_start_crash");
    pty.begin_task();
    assert!(
        pty.paths
            .wait_for_request_count("turn/start", 1, Duration::from_secs(10)),
        "turn/start was not observed; PTY output: {}; fake requests: {:?}; fake state: {:?}",
        pty.output_text(),
        fs::read_to_string(&pty.paths.request_log).unwrap_or_default(),
        fs::read_to_string(&pty.paths.state).unwrap_or_default()
    );
    pty.kill_process_group();

    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(pty.paths.journal()).unwrap()).unwrap();
    let record = &journal["payload"]["records"][0];
    assert_eq!(record["execution_phase"], "turn_start_may_have_been_sent");
    assert_eq!(record["known_thread_id"], "thread-pty");
    assert!(record["known_turn_id"].is_null());
    assert_eq!(record["recovery_required"], true);

    pty.restore_terminal_after_forceful_kill();
    pty.spawn("tui", "recovery");
    pty.assert_text("EXECUTION RECOVERY");
    assert_eq!(pty.paths.request_count("thread/start"), 1);
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    pty.key(b'r');
    pty.assert_text("Outcome unknown");
    assert_eq!(pty.paths.request_count("thread/start"), 1);
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    assert_eq!(pty.paths.request_count("thread/read"), 0);
    pty.key(b'\x1b');
    pty.key(b'q');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
}

#[test]
fn pty_dirty_composer_exit_restores_terminal_settings() {
    let _serial = serialize_pty_test();
    let paths = TestPaths::new();
    let mut pty = PtyHarness::new(paths);
    pty.spawn("compose", "success");
    pty.assert_text("Composer");
    pty.send(PROMPT.as_bytes());
    thread::sleep(Duration::from_millis(500));
    pty.send(b"\x03");
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(pty.wait_for_exit().success());
    pty.assert_terminal_restored();
}
