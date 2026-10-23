#![cfg(windows)]

use portable_pty::{native_pty_system, Child, MasterPty, PtySize};
use std::fs;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PROMPT: &str = "P038_λ_SYNTHETIC_PROMPT";
const TERMINAL_ROWS: usize = 40;
const TERMINAL_COLUMNS: usize = 120;
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

enum ParserState {
    Ground,
    Escape,
    Csi(String),
    Osc(bool),
}

struct TerminalScreen {
    cells: Vec<Vec<char>>,
    row: usize,
    column: usize,
    saved_cursor: Option<(usize, usize)>,
    pending_input: Vec<u8>,
    parser_state: ParserState,
}

impl TerminalScreen {
    fn new() -> Self {
        Self {
            cells: vec![vec![' '; TERMINAL_COLUMNS]; TERMINAL_ROWS],
            row: 0,
            column: 0,
            saved_cursor: None,
            pending_input: Vec::new(),
            parser_state: ParserState::Ground,
        }
    }

    fn process(&mut self, bytes: &[u8]) {
        let mut input = std::mem::take(&mut self.pending_input);
        input.extend_from_slice(bytes);
        let valid_length = match std::str::from_utf8(&input) {
            Ok(_) => input.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => input.len(),
        };
        self.pending_input.extend_from_slice(&input[valid_length..]);
        let input = String::from_utf8_lossy(&input[..valid_length]);
        for character in input.chars() {
            let state = std::mem::replace(&mut self.parser_state, ParserState::Ground);
            match state {
                ParserState::Ground => match character {
                    '\x1b' => self.parser_state = ParserState::Escape,
                    '\r' => self.column = 0,
                    '\n' => self.line_feed(),
                    '\x08' => self.column = self.column.saturating_sub(1),
                    '\t' => self.column = ((self.column / 8) + 1) * 8,
                    character if character.is_control() => {}
                    character => self.put(character),
                },
                ParserState::Escape => match character {
                    '[' => self.parser_state = ParserState::Csi(String::new()),
                    ']' => self.parser_state = ParserState::Osc(false),
                    '7' => self.saved_cursor = Some((self.row, self.column)),
                    '8' => self.restore_cursor(),
                    'D' => self.line_feed(),
                    'M' => self.scroll_down(1),
                    'c' => *self = Self::new(),
                    '\x1b' => self.parser_state = ParserState::Escape,
                    _ => {}
                },
                ParserState::Csi(mut parameters) => {
                    if ('@'..='~').contains(&character) {
                        self.process_csi(&parameters, character);
                    } else {
                        parameters.push(character);
                        self.parser_state = ParserState::Csi(parameters);
                    }
                }
                ParserState::Osc(previous_escape) => {
                    if character == '\x07' || (previous_escape && character == '\\') {
                        self.parser_state = ParserState::Ground;
                    } else {
                        self.parser_state = ParserState::Osc(character == '\x1b');
                    }
                }
            }
        }
    }

    fn process_csi(&mut self, parameters: &str, final_character: char) {
        let parameter = |index: usize, default: usize| {
            parameters
                .trim_start_matches(|character| matches!(character, '?' | '>' | '!'))
                .split(';')
                .nth(index)
                .and_then(|value| value.split(':').next())
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value != 0)
                .unwrap_or(default)
        };
        let count = parameter(0, 1);
        match final_character {
            'A' => self.row = self.row.saturating_sub(count),
            'B' => self.row = (self.row + count).min(TERMINAL_ROWS - 1),
            'C' => self.column = (self.column + count).min(TERMINAL_COLUMNS - 1),
            'D' => self.column = self.column.saturating_sub(count),
            'E' => {
                self.row = (self.row + count).min(TERMINAL_ROWS - 1);
                self.column = 0;
            }
            'F' => {
                self.row = self.row.saturating_sub(count);
                self.column = 0;
            }
            'G' => self.column = parameter(0, 1).saturating_sub(1).min(TERMINAL_COLUMNS - 1),
            'H' | 'f' => {
                self.row = parameter(0, 1).saturating_sub(1).min(TERMINAL_ROWS - 1);
                self.column = parameter(1, 1).saturating_sub(1).min(TERMINAL_COLUMNS - 1);
            }
            'd' => self.row = parameter(0, 1).saturating_sub(1).min(TERMINAL_ROWS - 1),
            'J' => self.erase_display(parameter(0, 0)),
            'K' => self.erase_line(parameter(0, 0)),
            'X' => self.erase_characters(count),
            'P' => self.delete_characters(count),
            '@' => self.insert_characters(count),
            'L' => self.insert_lines(count),
            'M' => self.delete_lines(count),
            'S' => self.scroll_up(count),
            'T' => self.scroll_down(count),
            's' => self.saved_cursor = Some((self.row, self.column)),
            'u' => self.restore_cursor(),
            _ => {}
        }
    }

    fn put(&mut self, character: char) {
        if self.column >= TERMINAL_COLUMNS {
            self.column = 0;
            self.line_feed();
        }
        self.cells[self.row][self.column] = character;
        self.column += 1;
    }

    fn line_feed(&mut self) {
        if self.row + 1 == TERMINAL_ROWS {
            self.scroll_up(1);
        } else {
            self.row += 1;
        }
    }

    fn restore_cursor(&mut self) {
        if let Some((row, column)) = self.saved_cursor {
            self.row = row;
            self.column = column;
        }
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.cells[self.row][self.column..].fill(' ');
                for row in self.row + 1..TERMINAL_ROWS {
                    self.cells[row].fill(' ');
                }
            }
            1 => {
                for row in 0..self.row {
                    self.cells[row].fill(' ');
                }
                let column = self.column.min(TERMINAL_COLUMNS - 1);
                self.cells[self.row][..=column].fill(' ');
            }
            2 | 3 => {
                for row in &mut self.cells {
                    row.fill(' ');
                }
            }
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        match mode {
            0 => self.cells[self.row][self.column..].fill(' '),
            1 => {
                let column = self.column.min(TERMINAL_COLUMNS - 1);
                self.cells[self.row][..=column].fill(' ')
            }
            2 => self.cells[self.row].fill(' '),
            _ => {}
        }
    }

    fn erase_characters(&mut self, count: usize) {
        let end = self.column.saturating_add(count).min(TERMINAL_COLUMNS);
        self.cells[self.row][self.column..end].fill(' ');
    }

    fn delete_characters(&mut self, count: usize) {
        let row = &mut self.cells[self.row];
        self.column = self.column.min(TERMINAL_COLUMNS - 1);
        let count = count.min(TERMINAL_COLUMNS - self.column);
        row.copy_within(self.column + count.., self.column);
        row[TERMINAL_COLUMNS - count..].fill(' ');
    }

    fn insert_characters(&mut self, count: usize) {
        let row = &mut self.cells[self.row];
        self.column = self.column.min(TERMINAL_COLUMNS - 1);
        let count = count.min(TERMINAL_COLUMNS - self.column);
        row.copy_within(self.column..TERMINAL_COLUMNS - count, self.column + count);
        row[self.column..self.column + count].fill(' ');
    }

    fn insert_lines(&mut self, count: usize) {
        for _ in 0..count.min(TERMINAL_ROWS - self.row) {
            self.cells.insert(self.row, vec![' '; TERMINAL_COLUMNS]);
            self.cells.pop();
        }
    }

    fn delete_lines(&mut self, count: usize) {
        for _ in 0..count.min(TERMINAL_ROWS - self.row) {
            self.cells.remove(self.row);
            self.cells.push(vec![' '; TERMINAL_COLUMNS]);
        }
    }

    fn scroll_up(&mut self, count: usize) {
        for _ in 0..count.min(TERMINAL_ROWS) {
            self.cells.remove(0);
            self.cells.push(vec![' '; TERMINAL_COLUMNS]);
        }
    }

    fn scroll_down(&mut self, count: usize) {
        for _ in 0..count.min(TERMINAL_ROWS) {
            self.cells.pop();
            self.cells.insert(0, vec![' '; TERMINAL_COLUMNS]);
        }
    }

    fn contents(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[test]
fn conpty_text_matching_tracks_cursor_position_erasure_and_chunk_boundaries() {
    let mut screen = TerminalScreen::new();
    screen.process(b"old prompt\x1b[2J\x1b[38;");
    screen.process(b"7HEnter\x1b[1Cconfirm and submit once");
    assert!(screen.contents().contains("Enter confirm and submit once"));
    assert!(!screen.contents().contains("old prompt"));

    screen.process(b"\x1b[38;7HPlease wait | no duplicate submission");
    assert!(screen
        .contents()
        .contains("Please wait | no duplicate submission"));
    assert!(!screen.contents().contains("Enter confirm and submit once"));
}

struct TestPaths {
    root: std::path::PathBuf,
    bin: std::path::PathBuf,
    workspace: std::path::PathBuf,
    local_app_data: std::path::PathBuf,
    state: std::path::PathBuf,
    request_log: std::path::PathBuf,
    continue_file: std::path::PathBuf,
    approval: std::path::PathBuf,
    wrapper_pid: std::path::PathBuf,
    server_pid: std::path::PathBuf,
}

impl TestPaths {
    fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos());
        let local_data = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .expect("the Windows test runner provides a per-user local data directory");
        let root = local_data.join(format!(
            "codex-meter-conpty-{}-{timestamp}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        let bin = root.join("bin");
        let workspace = root.join("workspace");
        let local_app_data = root.join("local-app-data");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&workspace).unwrap();

        let wrapper_source = root.join("fake_codex_windows_wrapper.rs");
        fs::write(
            &wrapper_source,
            include_str!("fixtures/fake_codex_windows_wrapper.rs"),
        )
        .unwrap();
        let wrapper = bin.join("codex.exe");
        let compiled = Command::new("rustc")
            .arg("--edition=2021")
            .arg(&wrapper_source)
            .arg("-o")
            .arg(&wrapper)
            .status()
            .expect("rustc is available in the native Windows test job");
        assert!(
            compiled.success(),
            "the synthetic Windows Codex wrapper must compile"
        );

        Self {
            state: root.join("fake-state.json"),
            request_log: root.join("requests.log"),
            continue_file: root.join("continue"),
            approval: root.join("fake-state.json.approval"),
            wrapper_pid: root.join("wrapper.pid"),
            server_pid: root.join("server.pid"),
            root,
            bin,
            workspace,
            local_app_data,
        }
    }

    fn journal(&self) -> std::path::PathBuf {
        self.local_app_data
            .join("codex-meter/recovery/journal.json")
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
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn process_has_exited(&self, pid_path: &std::path::Path) -> bool {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };

        let Ok(pid) = fs::read_to_string(pid_path) else {
            return false;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            return false;
        };
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            // ERROR_INVALID_PARAMETER means Windows no longer has this process ID.
            return std::io::Error::last_os_error().raw_os_error() == Some(87);
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
        match unsafe { WaitForSingleObject(handle.as_raw_handle().cast(), 0) } {
            WAIT_OBJECT_0 => true,
            WAIT_TIMEOUT => false,
            _ => false,
        }
    }

    fn wait_for_process_exit(&self, pid_path: &std::path::Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.process_has_exited(pid_path) {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        self.process_has_exited(pid_path)
    }
}

impl Drop for TestPaths {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ConPtyHarness {
    paths: TestPaths,
    master: Option<Box<dyn MasterPty + Send>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    child: Option<Box<dyn Child + Send + Sync>>,
    screen: Arc<Mutex<TerminalScreen>>,
    reader: Option<JoinHandle<()>>,
}

impl ConPtyHarness {
    fn new(paths: TestPaths) -> Self {
        Self {
            paths,
            master: None,
            writer: None,
            child: None,
            screen: Arc::new(Mutex::new(TerminalScreen::new())),
            reader: None,
        }
    }

    fn spawn(&mut self, command: &str, mode: &str) {
        self.stop_instance();
        *self
            .screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = TerminalScreen::new();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: TERMINAL_ROWS as u16,
                cols: TERMINAL_COLUMNS as u16,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("Windows ConPTY must initialize");
        let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let response_writer = Arc::clone(&writer);
        let mut builder = portable_pty::CommandBuilder::new(env!("CARGO_BIN_EXE_codex-meter"));
        builder.arg(command);
        builder.cwd(&self.paths.workspace);
        let original_path = std::env::var_os("PATH").unwrap_or_default();
        let search_path = format!(
            "{};{}",
            self.paths.bin.display(),
            original_path.to_string_lossy()
        );
        builder.env("PATH", search_path);
        builder.env("TERM", "xterm-256color");
        builder.env("NO_COLOR", "1");
        builder.env("HOME", &self.paths.root);
        builder.env("LOCALAPPDATA", &self.paths.local_app_data);
        builder.env("CODEX_METER_TEST_MODE", mode);
        builder.env(
            "CODEX_METER_TEST_FIXTURE",
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/fake_pty_app_server.py"),
        );
        builder.env("CODEX_METER_TEST_WORKSPACE", &self.paths.workspace);
        builder.env("CODEX_METER_TEST_STATE", &self.paths.state);
        builder.env("CODEX_METER_TEST_REQUEST_LOG", &self.paths.request_log);
        builder.env("CODEX_METER_TEST_CONTINUE", &self.paths.continue_file);
        builder.env("CODEX_METER_TEST_WRAPPER_PID_FILE", &self.paths.wrapper_pid);
        builder.env("CODEX_METER_TEST_SERVER_PID_FILE", &self.paths.server_pid);
        builder.env("CODEX_METER_TEST_PROMPT", PROMPT);
        builder.env("CODEX_METER_TEST_PLATFORM_FAMILY", "windows");
        builder.env("CODEX_METER_TEST_PLATFORM_OS", "windows");

        let child = pair.slave.spawn_command(builder).unwrap();
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().unwrap();
        let screen = Arc::clone(&self.screen);
        self.reader = Some(thread::spawn(move || {
            let mut reader = reader;
            let mut buffer = [0_u8; 4096];
            let mut terminal_queries = Vec::new();
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(count) => {
                        let query_count = {
                            terminal_queries.extend_from_slice(&buffer[..count]);
                            let mut query_count = 0;
                            while let Some(position) = terminal_queries
                                .windows(4)
                                .position(|query| query == b"\x1b[6n")
                            {
                                terminal_queries.drain(..position + 4);
                                query_count += 1;
                            }
                            let trailing_bytes = terminal_queries.len().saturating_sub(3);
                            terminal_queries.drain(..trailing_bytes);
                            query_count
                        };
                        screen
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .process(&buffer[..count]);
                        for _ in 0..query_count {
                            let mut writer = response_writer
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let _ = writer.write_all(b"\x1b[1;1R");
                            let _ = writer.flush();
                        }
                    }
                }
            }
        }));
        self.writer = Some(writer);
        self.master = Some(pair.master);
        self.child = Some(child);
    }

    fn send(&mut self, bytes: &[u8]) {
        let mut writer = self
            .writer
            .as_ref()
            .expect("ConPTY writer is open")
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }

    fn key(&mut self, byte: u8) {
        self.send(&[byte]);
        thread::sleep(Duration::from_millis(100));
    }

    fn back_tab(&mut self) {
        self.send(b"\x1b[Z");
        thread::sleep(Duration::from_millis(100));
    }

    fn wait_for(&self, text: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let screen = self
                .screen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if screen.contents().contains(text) {
                return true;
            }
            drop(screen);
            thread::sleep(Duration::from_millis(30));
        }
        false
    }

    fn assert_text(&self, text: &str) {
        if !self.wait_for(text, Duration::from_secs(20)) {
            let screen = self
                .screen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            eprintln!(
                "ConPTY fixture diagnostics: requests={:?}, wrapper_pid={:?}, server_pid={:?}",
                fs::read_to_string(&self.paths.request_log).unwrap_or_default(),
                fs::read_to_string(&self.paths.wrapper_pid).unwrap_or_default(),
                fs::read_to_string(&self.paths.server_pid).unwrap_or_default(),
            );
            panic!(
                "expected ConPTY screen to contain {text:?}; current screen:\n{}",
                screen.contents()
            );
        }
    }

    fn begin_task(&mut self) {
        self.assert_text("Composer");
        thread::sleep(Duration::from_millis(300));
        self.send(PROMPT.as_bytes());
        thread::sleep(Duration::from_millis(300));
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

    fn wait_for_exit(&mut self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(child) = self.child.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => return status.success(),
                    Ok(None) => {}
                    Err(_) => return false,
                }
            } else {
                return true;
            }
            thread::sleep(Duration::from_millis(30));
        }
        false
    }

    fn kill_parent(&mut self) {
        if let Some(child) = self.child.as_mut() {
            child.kill().unwrap();
            let _ = child.wait();
        }
        self.child.take();
        self.close_terminal();
    }

    fn stop_instance(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        self.child.take();
        self.close_terminal();
    }

    fn close_terminal(&mut self) {
        self.writer.take();
        self.master.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for ConPtyHarness {
    fn drop(&mut self) {
        self.stop_instance();
    }
}

#[test]
fn windows_conpty_approval_cancellation_and_interrupt_lifecycle() {
    let paths = TestPaths::new();
    let mut pty = ConPtyHarness::new(paths);
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
    assert!(pty.wait_for_exit(), "the approved TUI shutdown must finish");
    let decision: serde_json::Value =
        serde_json::from_slice(&fs::read(&pty.paths.approval).unwrap()).unwrap();
    assert_eq!(decision, serde_json::json!({"decision":"decline"}));

    pty.spawn("compose", "interrupt");
    pty.begin_task();
    pty.assert_text("Running");
    pty.key(b'i');
    pty.assert_text("Interrupt request acknowledged");
    fs::write(&pty.paths.continue_file, b"continue").unwrap();
    pty.assert_text("Interrupted");
    pty.key(b'\r');
    pty.assert_text("Overview");
    pty.key(b'q');
    pty.assert_text("Unsaved in-memory task");
    pty.key(b'y');
    assert!(
        pty.wait_for_exit(),
        "the interrupted TUI shutdown must finish"
    );
    assert_eq!(pty.paths.request_count("turn/start"), 2);
    assert_eq!(pty.paths.request_count("turn/interrupt"), 1);
}

#[test]
fn windows_parent_crash_recovery_never_replays_a_turn() {
    let paths = TestPaths::new();
    let mut pty = ConPtyHarness::new(paths);
    pty.spawn("compose", "crash");
    pty.begin_task();
    pty.assert_text("Running");
    assert!(pty
        .paths
        .wait_for_request_count("turn/start", 1, Duration::from_secs(10)));
    pty.kill_parent();

    assert!(
        pty.paths
            .wait_for_process_exit(&pty.paths.wrapper_pid, Duration::from_secs(10)),
        "the App Server wrapper must exit when its parent loses the Job Object"
    );
    assert!(
        pty.paths
            .wait_for_process_exit(&pty.paths.server_pid, Duration::from_secs(10)),
        "the App Server descendant must exit when its parent loses the Job Object"
    );

    let journal = fs::read_to_string(pty.paths.journal()).unwrap();
    assert!(journal.contains("thread-pty"));
    assert!(journal.contains("turn-pty"));
    assert!(journal.contains("\"recovery_required\":true"));
    assert!(!journal.contains(PROMPT));

    pty.spawn("tui", "recovery");
    pty.assert_text("EXECUTION RECOVERY");
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    pty.key(b'r');
    pty.assert_text("Still running");
    assert_eq!(pty.paths.request_count("turn/start"), 1);
    assert_eq!(pty.paths.request_count("turn/interrupt"), 0);
    pty.key(b'\x1b');
    pty.key(b'q');
    assert!(pty.wait_for_exit(), "recovery review should exit cleanly");
}

#[test]
fn interactive_command_reports_a_missing_console_without_panicking() {
    let result = Command::new(env!("CARGO_BIN_EXE_codex-meter"))
        .arg("compose")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr)
        .contains("requires stdin and stdout to be interactive terminals"));
}
