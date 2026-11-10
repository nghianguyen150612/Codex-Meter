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
    slave_path: std::path::PathBuf,
    terminal_probe: fs::File,
    baseline: libc::termios,
    output: Arc<Mutex<Vec<u8>>>,
    resizes: Mutex<Vec<(usize, u16, u16)>>,
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
        for fd in [master_fd, slave_fd] {
            assert_eq!(
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
        let master = unsafe { fs::File::from_raw_fd(master_fd) };
        let slave = unsafe { fs::File::from_raw_fd(slave_fd) };
        let mut name = [0_i8; 1024];
        assert_eq!(
            unsafe { libc::ttyname_r(slave_fd, name.as_mut_ptr(), name.len()) },
            0
        );
        let slave_path = std::path::PathBuf::from(
            unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
                .to_str()
                .unwrap(),
        );
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
            slave_path,
            terminal_probe,
            baseline,
            output,
            resizes: Mutex::new(Vec::new()),
            reader_stop,
            reader: Some(reader),
            child: None,
            paths,
        }
    }

    fn spawn(&mut self, command: &str, mode: &str) {
        assert!(self.child.is_none());
        self.slave = self.open_slave();
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let mut search_path = self.paths.bin.as_os_str().to_os_string();
        search_path.push(":");
        search_path.push(original_path);
        let exit_file = self.paths.root.join("application-exit");
        let release_file = self.paths.root.join("release-terminal");
        let _ = fs::remove_file(&exit_file);
        let _ = fs::remove_file(&release_file);
        // Keep a shell as session leader until restoration has been inspected.
        // Darwin revokes/resets the terminal when the session leader exits.
        let mut child_command = Command::new("/bin/sh");
        child_command
            .arg("-c")
            .arg("\"$1\" \"$2\"; result=$?; printf '%s' \"$result\" > \"$3\"; while [ ! -f \"$4\" ]; do sleep 0.02; done; exit \"$result\"")
            .arg("pty-supervisor")
            .arg(env!("CARGO_BIN_EXE_codex-meter"))
            .arg(command)
            .arg(exit_file)
            .arg(release_file)
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
        // A process group alone inherits the caller's controlling terminal.
        // Crossterm queries /dev/tty before stdout for window dimensions.
        // Only async-signal-safe syscalls may run between fork and exec.
        unsafe {
            child_command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::tcgetpgrp(0) != libc::getpid() {
                    return Err(std::io::Error::other("child PTY foreground group mismatch"));
                }
                Ok(())
            });
        }
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
            let found = self.output_text().contains(text);
            if found {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn assert_text(&self, text: &str) {
        if !self.wait_for(text, Duration::from_secs(15)) {
            let output = self.output.lock().unwrap();
            let raw_contains = String::from_utf8_lossy(&output).contains(text);
            let screen = self.screen(&output);
            let status = self.child.as_ref().map(|child| {
                // kill(pid, 0) checks existence without changing the process.
                unsafe { libc::kill(child.id() as i32, 0) == 0 }
            });
            eprintln!("PTY_DIAG bytes={} raw_contains={raw_contains} running={} narrow_fallback={} approval={} tty_columns={} tty_rows={}",
                output.len(), screen.contains("Running"), screen.contains("Resize terminal"),
                screen.contains("APPROVAL REQUIRED"), self.dimensions().0, self.dimensions().1);
            panic!(
                "PTY assertion failed: expected={text:?} bytes={} raw_contains={raw_contains} screen_contains=false tty_size={:?} child_exists={status:?} thread_starts={} turn_starts={}",
                output.len(), self.dimensions(), self.paths.request_count("thread/start"),
                self.paths.request_count("turn/start")
            );
        }
    }

    fn output_text(&self) -> String {
        self.screen(
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
        self.assert_text("Enter confirm and submit once");
        self.key(b'\r');
    }

    fn open_slave(&self) -> fs::File {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.slave_path)
            .unwrap()
    }

    fn dimensions(&self) -> (u16, u16) {
        let mut size = unsafe { std::mem::zeroed::<libc::winsize>() };
        assert_eq!(
            unsafe { libc::ioctl(self.terminal_probe.as_raw_fd(), libc::TIOCGWINSZ, &mut size) },
            0
        );
        (size.ws_col, size.ws_row)
    }

    fn screen(&self, bytes: &[u8]) -> String {
        let mut parser = vt100::Parser::new(40, 120, 0);
        let mut offset = 0;
        for &(end, rows, columns) in self.resizes.lock().unwrap().iter() {
            parser.process(&bytes[offset..end]);
            parser.set_size(rows, columns);
            offset = end;
        }
        parser.process(&bytes[offset..]);
        parser.screen().contents()
    }

    fn resize(&self, columns: u16, rows: u16) {
        let output = self.output.lock().unwrap();
        self.resizes
            .lock()
            .unwrap()
            .push((output.len(), rows, columns));
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
        assert_eq!(self.dimensions(), (columns, rows));
        // TIOCSWINSZ signals the slave foreground process group naturally.
        drop(output);
    }

    fn restore_terminal_after_forceful_kill(&mut self) {
        // Darwin revokes slave descriptors when the controlling session leader dies.
        // Reopen the same PTY; this does not reset its termios.
        self.terminal_probe = self.open_slave();
        self.resize(120, 40);
        assert_eq!(
            unsafe {
                libc::tcsetattr(
                    self.terminal_probe.as_raw_fd(),
                    libc::TCSAFLUSH,
                    &self.baseline,
                )
            },
            0
        );
        let mut restored = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.terminal_probe.as_raw_fd(), &mut restored) },
            0
        );
        assert_eq!(
            restored.c_lflag, self.baseline.c_lflag,
            "test driver could not restore terminal mode after forceful termination"
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
        let exit_file = self.paths.root.join("application-exit");
        let mut released = false;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if !released
                && exit_file.exists()
                && self.raw_output_text().contains("?1049l")
                && self.raw_output_text().contains("?25h")
            {
                // Inspect modes while the original terminal still exists, before
                // Darwin revokes it at the supervising session leader's exit.
                self.assert_terminal_restored();
                fs::write(self.paths.root.join("release-terminal"), b"release").unwrap();
                released = true;
            }
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
        // Do not query a descriptor revoked by Darwin at session-leader exit.
        let probe = self.open_slave();
        let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(unsafe { libc::tcgetattr(probe.as_raw_fd(), &mut after) }, 0);
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

impl Drop for PtyHarness {
    fn drop(&mut self) {
        // Cleanup must not panic if the failed assertion raced with child exit.
        // Explicit crash tests still require kill_process_group to succeed.
        if let Some(child) = self.child.as_mut() {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
        }
        self.child = None;
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
    assert_eq!(pty.dimensions(), (120, 40));
    pty.spawn("compose", "success");
    pty.begin_task();
    pty.assert_text("Running");
    pty.resize(16, 8);
    pty.assert_text("Resize terminal");
    pty.assert_text("to view Codex");
    pty.resize(120, 40);
    pty.assert_text("Running");
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
        "thread/start was not observed before bounded timeout"
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
        "turn/start was not observed before bounded timeout"
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

#[test]
fn vt_screen_tracks_unicode_diff_clear_and_resize() {
    let mut parser = vt100::Parser::new(40, 120, 0);
    parser.process(b"\x1b[?1049h\x1b[2J\x1b[HRunning");
    parser.set_size(8, 16);
    parser.process(b"\x1b[2J\x1b[HResize terminal\x1b[2;1Hto view Codex");
    assert!(parser.screen().contents().contains("Resize terminal"));
    assert!(!parser.screen().contents().contains("Running"));
    parser.set_size(40, 120);
    parser.process(b"\x1b[2J\x1b[HRunning ");
    for byte in "λ".as_bytes() {
        parser.process(&[*byte]);
    }
    assert_eq!(parser.screen().contents(), "Running λ");
    parser.process(b"\x1b[1;1HCompleted\x1b[K");
    assert_eq!(parser.screen().contents(), "Completed");
    parser.process(b"\x1b[?1049l\x1b[?25h");
    assert!(!parser.screen().alternate_screen());
    assert!(!parser.screen().hide_cursor());
}

#[test]
fn reopening_slave_preserves_terminal_modes() {
    let _serial = serialize_pty_test();
    let pty = PtyHarness::new(TestPaths::new());
    let mut raw = pty.baseline;
    unsafe { libc::cfmakeraw(&mut raw) };
    assert_eq!(
        unsafe { libc::tcsetattr(pty.slave.as_raw_fd(), libc::TCSANOW, &raw) },
        0
    );
    let probe = pty.open_slave();
    let mut actual = unsafe { std::mem::zeroed::<libc::termios>() };
    assert_eq!(
        unsafe { libc::tcgetattr(probe.as_raw_fd(), &mut actual) },
        0
    );
    assert_eq!(actual.c_lflag, raw.c_lflag);
    assert_ne!(actual.c_lflag, pty.baseline.c_lflag);
}
