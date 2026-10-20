use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;
use serde_json::{json, Value};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex meter p031 résumé {} {id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temporary test directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const SESSION_META: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/session-meta.json"
));
const TURN_CONTEXT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/turn-context.json"
));
const THREAD_SETTINGS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/thread-settings-applied.json"
));
const TOKEN_USAGE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/token-usage-record.json"
));
const TOKEN_USAGE_MISSING_REASONING: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/token-usage-missing-reasoning.json"
));
const TOKEN_USAGE_ZERO_REASONING: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/token-usage-zero-reasoning.json"
));
const TOKEN_COUNT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/token-count.json"
));
const NORMAL_QUOTA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/quota/normal.json"
));
const TASK_COMPLETED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/task-complete.json"
));
const TASK_STARTED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../fixtures/codex-rollout/v0.157.1/task-started.json"
));

fn active_rollout(home: &Path, file_name: &str) -> PathBuf {
    let directory = home.join("sessions/2026/10/03");
    fs::create_dir_all(&directory).expect("active rollout directories should be created");
    directory.join(file_name)
}

fn json_line_with_ordinal(input: &str, ordinal: u64) -> String {
    let mut value: Value = serde_json::from_str(input).expect("synthetic fixture should parse");
    value["ordinal"] = json!(ordinal);
    serde_json::to_string(&value).expect("synthetic fixture should serialize")
}

fn stream_with_two_responses() -> String {
    [
        SESSION_META.trim(),
        THREAD_SETTINGS.trim(),
        TURN_CONTEXT.trim(),
        &json_line_with_ordinal(TOKEN_USAGE, 12),
        &json_line_with_ordinal(TOKEN_USAGE, 13),
        TOKEN_COUNT.trim(),
        NORMAL_QUOTA.trim(),
    ]
    .join("\n")
        + "\n"
}

fn run_sync(home: &Path, database: &Path) -> (Output, Value) {
    run_sync_with(home, Some(database), true, None, None)
}

fn run_sync_with(
    home: &Path,
    database: Option<&Path>,
    explicit_home: bool,
    code_home_env: Option<&Path>,
    home_env: Option<&Path>,
) -> (Output, Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex-meter"));
    command.arg("sync");
    if explicit_home {
        command.arg("--codex-home").arg(home);
    }
    if let Some(database) = database {
        command.arg("--database").arg(database);
    }
    command.arg("--json");
    if let Some(code_home_env) = code_home_env {
        command.env("CODEX_HOME", code_home_env);
    } else {
        command.env_remove("CODEX_HOME");
    }
    if let Some(home_env) = home_env {
        command.env("HOME", home_env);
    }
    let output = command.output().expect("native sync process should start");
    let summary = serde_json::from_slice(&output.stdout).expect("JSON mode should emit a summary");
    (output, summary)
}

fn sync_command(home: &Path, database: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex-meter"));
    command
        .arg("sync")
        .arg("--codex-home")
        .arg(home)
        .arg("--database")
        .arg(database)
        .arg("--json")
        .env_remove("CODEX_HOME")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    command
}

fn write_basic_rollout(home: &Path, file_name: &str) -> PathBuf {
    let path = active_rollout(home, file_name);
    fs::write(&path, stream_with_two_responses()).expect("rollout should be created");
    path
}

fn diagnostic_count(database: &Path, code: &str) -> i64 {
    Connection::open(database)
        .expect("history database should open")
        .query_row(
            "SELECT COUNT(*) FROM ingestion_diagnostics WHERE diagnostic_code = ?1",
            [code],
            |row| row.get(0),
        )
        .expect("diagnostic count should be queryable")
}

#[test]
fn sync_stores_per_response_evidence_quota_snapshots_and_only_sanitized_values() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home 空間");
    let database = tree.path().join("Meter History 数据.sqlite3");
    write_basic_rollout(&home, "rollout-one.jsonl");

    let (first_output, first) = run_sync(&home, &database);
    assert!(first_output.status.success(), "sync should complete");
    assert_eq!(first["responses_added"], 2);
    assert_eq!(first["saved_response_events"], 2);
    assert_eq!(first["saved_raw_total_known_sum"], 200);
    assert_eq!(first["saved_raw_total_known_events"], 2);
    assert_eq!(first["saved_raw_total_missing_events"], 0);
    assert_eq!(first["quota_snapshots_added"], 2);
    assert_eq!(first["five_hour_meter"]["used_percent"], 42.5);
    assert_eq!(first["weekly_meter"]["used_percent"], 18.0);
    assert!(!String::from_utf8_lossy(&first_output.stdout).contains(home.to_str().unwrap()));
    assert!(!String::from_utf8_lossy(&first_output.stdout).contains(database.to_str().unwrap()));

    let (second_output, second) = run_sync(&home, &database);
    assert!(second_output.status.success());
    assert_eq!(second["responses_added"], 0);
    assert_eq!(second["saved_response_events"], 2);
    assert_eq!(second["saved_raw_total_known_sum"], 200);

    let connection = Connection::open(&database).expect("history database should open");
    let response_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic = 'per_response'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let snapshot_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic LIKE '%snapshot'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let counters: (i64, i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT input_tokens, cached_input_tokens, cache_write_input_tokens, output_tokens, reasoning_output_tokens, raw_total FROM token_usage_evidence WHERE record_semantic = 'per_response' ORDER BY rowid LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(response_rows, 2);
    assert_eq!(snapshot_rows, 6);
    assert_eq!(counters, (50, 20, 5, 30, 10, 100));
    let configuration: (String, String, String, String) = connection
        .query_row(
            "SELECT model, reasoning_level, service_tier, codex_version FROM token_usage_evidence WHERE record_semantic = 'per_response' ORDER BY rowid LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        configuration,
        (
            "gpt-synthetic".to_owned(),
            "high".to_owned(),
            "synthetic-tier".to_owned(),
            "0.157.1".to_owned()
        )
    );
    let quota_rows: i64 = connection
        .query_row("SELECT COUNT(*) FROM quota_snapshot_evidence", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(quota_rows, 2);
    drop(connection);

    let database_files = [
        database.clone(),
        PathBuf::from(format!("{}-wal", database.display())),
        PathBuf::from(format!("{}-shm", database.display())),
    ];
    for file in database_files.into_iter().filter(|file| file.exists()) {
        let bytes = fs::read(file).expect("database files should be readable in the test");
        let contents = String::from_utf8_lossy(&bytes);
        assert!(!contents.contains("fake prompt must disappear"));
        assert!(!contents.contains("provider-response-must-not-be-retained"));
        assert!(!contents.contains("fake-rate-limit-payload"));
        assert!(!contents.contains("/synthetic/private/repository"));
    }
}

#[test]
fn incomplete_trailing_record_is_recovered_after_append_and_replay_adds_nothing() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let path = active_rollout(&home, "rollout-tail.jsonl");
    let response = json_line_with_ordinal(TOKEN_USAGE, 12);
    let split = response.len() / 2;
    fs::write(
        &path,
        format!("{}\n{}", SESSION_META.trim(), &response[..split]),
    )
    .expect("partial rollout should be created");

    let (first_output, first) = run_sync(&home, &database);
    assert!(first_output.status.success());
    assert_eq!(first["responses_added"], 0);
    let before_append = Connection::open(&database).unwrap();
    let count: i64 = before_append
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic = 'per_response'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    drop(before_append);

    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("{}\n", &response[split..]).as_bytes())
        .unwrap();
    let (second_output, second) = run_sync(&home, &database);
    assert!(second_output.status.success());
    assert_eq!(second["responses_added"], 1);
    let (third_output, third) = run_sync(&home, &database);
    assert!(third_output.status.success());
    assert_eq!(third["responses_added"], 0);
    assert_eq!(third["saved_response_events"], 1);
}

#[test]
fn explicit_home_overrides_environment_and_codex_home_environment_is_used_without_override() {
    let tree = TempTree::new();
    let explicit = tree.path().join("explicit home");
    let environment = tree.path().join("environment home");
    let explicit_database = tree.path().join("explicit.sqlite3");
    let environment_database = tree.path().join("environment.sqlite3");
    write_basic_rollout(&explicit, "explicit.jsonl");
    write_basic_rollout(&environment, "environment.jsonl");
    let second_session = SESSION_META
        .replace("session-synthetic-001", "session-synthetic-002")
        .replace("thread-synthetic-001", "thread-synthetic-002");
    fs::write(
        active_rollout(&environment, "environment-second.jsonl"),
        second_session,
    )
    .unwrap();

    let (explicit_output, explicit_summary) = run_sync_with(
        &explicit,
        Some(&explicit_database),
        true,
        Some(&environment),
        None,
    );
    assert!(explicit_output.status.success());
    assert_eq!(explicit_summary["sessions_seen"], 1);
    assert!(!environment_database.exists());

    let (environment_output, environment_summary) = run_sync_with(
        &environment,
        Some(&environment_database),
        false,
        Some(&environment),
        None,
    );
    assert!(environment_output.status.success());
    assert_eq!(environment_summary["responses_added"], 2);
    assert_eq!(environment_summary["sessions_seen"], 2);
}

#[test]
fn default_codex_home_and_default_sqlite_location_are_per_user() {
    let tree = TempTree::new();
    let home_directory = tree.path().join("user home μ");
    let codex_home = home_directory.join(".codex");
    let expected_database = home_directory.join(".local/share/codex-meter/codex-meter.sqlite3");
    write_basic_rollout(&codex_home, "default.jsonl");
    let mut command = Command::new(env!("CARGO_BIN_EXE_codex-meter"));
    command
        .arg("sync")
        .arg("--json")
        .env("HOME", &home_directory)
        .env_remove("CODEX_HOME")
        .env_remove("XDG_DATA_HOME");
    let output = command.output().expect("native sync process should start");
    assert!(output.status.success());
    assert!(expected_database.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(expected_database.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&expected_database)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["responses_added"], 2);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(expected_database.to_str().unwrap()));
}

#[test]
fn absolute_xdg_data_home_selects_the_linux_application_database_directory() {
    let tree = TempTree::new();
    let home_directory = tree.path().join("user home");
    let data_directory = tree.path().join("xdg data");
    let codex_home = home_directory.join(".codex");
    let expected_database = data_directory.join("codex-meter/codex-meter.sqlite3");
    write_basic_rollout(&codex_home, "xdg.jsonl");

    let mut command = Command::new(env!("CARGO_BIN_EXE_codex-meter"));
    command
        .arg("sync")
        .arg("--json")
        .env("HOME", &home_directory)
        .env("XDG_DATA_HOME", &data_directory)
        .env_remove("CODEX_HOME");
    let output = command.output().unwrap();
    assert!(output.status.success());
    assert!(expected_database.exists());
    let summary: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["responses_added"], 2);
}

#[test]
fn archived_move_resumes_checkpoint_and_compressed_only_sources_are_reported() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let active = write_basic_rollout(&home, "rollout-move.jsonl");
    let (first_output, first) = run_sync(&home, &database);
    assert!(first_output.status.success());
    assert_eq!(first["responses_added"], 2);

    let archive = home.join("archived_sessions");
    fs::create_dir_all(&archive).unwrap();
    fs::rename(active, archive.join("rollout-move.jsonl")).unwrap();
    let (second_output, second) = run_sync(&home, &database);
    assert!(second_output.status.success());
    assert_eq!(second["responses_added"], 0);
    assert_eq!(second["saved_response_events"], 2);

    fs::write(
        archive.join("only-compressed.jsonl.zst"),
        b"unsupported compressed",
    )
    .unwrap();
    let (third_output, third) = run_sync(&home, &database);
    assert!(third_output.status.success());
    assert_eq!(third["unsupported_compressed_sources"], 1);
    assert!(third["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "compressed_sources_unsupported"));
    assert_eq!(
        diagnostic_count(&database, "compressed_source_unsupported"),
        1
    );
}

#[test]
fn explicit_task_completion_saves_incomplete_observation_without_quota_delta() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let records = [
        SESSION_META.trim().to_owned(),
        TASK_STARTED.trim().to_owned(),
        TURN_CONTEXT.trim().to_owned(),
        json_line_with_ordinal(TOKEN_USAGE, 12),
        TASK_COMPLETED.trim().to_owned(),
    ]
    .join("\n")
        + "\n";
    fs::write(active_rollout(&home, "completed-task.jsonl"), records).unwrap();

    let (first_output, first) = run_sync(&home, &database);
    assert!(first_output.status.success());
    assert_eq!(first["responses_added"], 1);
    assert_eq!(first["observations_added"], 1);
    assert!(first["five_hour_meter"].is_null());
    assert!(first["weekly_meter"].is_null());
    let connection = Connection::open(&database).unwrap();
    let (lifecycle, raw_total, five_hour_validity, weekly_validity, payload): (
        String,
        i64,
        String,
        String,
        String,
    ) = connection
        .query_row(
            "SELECT lifecycle_state, raw_total, five_hour_validity, weekly_validity, observation_json FROM observations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(lifecycle, "incomplete");
    assert_eq!(raw_total, 100);
    assert_eq!(five_hour_validity, "unavailable");
    assert_eq!(weekly_validity, "unavailable");
    assert!(!payload.contains("fake prompt must disappear"));
    assert!(!payload.contains("provider-response-must-not-be-retained"));
    assert!(!payload.contains("/synthetic/private/repository"));
    drop(connection);

    let (second_output, second) = run_sync(&home, &database);
    assert!(second_output.status.success());
    assert_eq!(second["responses_added"], 0);
    assert_eq!(second["observations_added"], 0);
}

#[test]
fn copied_rollout_with_same_header_identity_is_skipped_as_ambiguous() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let active = write_basic_rollout(&home, "rollout-copy.jsonl");
    let archive = home.join("archived_sessions");
    fs::create_dir_all(&archive).unwrap();
    fs::copy(&active, archive.join("rollout-copy.jsonl")).unwrap();

    let (output, summary) = run_sync(&home, &database);
    assert!(output.status.success());
    assert_eq!(summary["ambiguous_source_identities"], 1);
    assert_eq!(summary["sessions_seen"], 1);
    assert_eq!(summary["responses_added"], 0);
    assert!(summary["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "ambiguous_source_identity"));
    assert_eq!(diagnostic_count(&database, "ambiguous_source_identity"), 1);
}

#[test]
fn source_replacement_is_not_rewound_or_double_counted() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let path = active_rollout(&home, "rollout-replaced.jsonl");
    let response = json_line_with_ordinal(TOKEN_USAGE, 12);
    fs::write(&path, format!("{}\n{}\n", SESSION_META.trim(), response))
        .expect("rollout should be created");
    let (first_output, first) = run_sync(&home, &database);
    assert!(first_output.status.success());
    assert_eq!(first["responses_added"], 1);

    let replaced = fs::read_to_string(&path)
        .unwrap()
        .replace("\"total_tokens\":100", "\"total_tokens\":101");
    fs::write(&path, replaced).unwrap();
    let (second_output, second) = run_sync(&home, &database);
    assert!(second_output.status.success());
    assert_eq!(second["responses_added"], 0);
    assert_eq!(second["saved_response_events"], 1);
    assert!(second["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "source_changed"));
    assert_eq!(diagnostic_count(&database, "source_changed"), 1);
}

#[test]
fn changed_generation_for_a_known_session_is_skipped_as_ambiguous() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let path = write_basic_rollout(&home, "rollout-generation.jsonl");
    let (first_output, first) = run_sync(&home, &database);
    assert!(first_output.status.success());
    assert_eq!(first["responses_added"], 2);

    let changed_header = fs::read_to_string(&path).unwrap().replacen(
        "2026-10-03T10:00:00Z",
        "2026-10-03T10:00:00.001Z",
        1,
    );
    fs::write(&path, changed_header).unwrap();
    let (second_output, second) = run_sync(&home, &database);
    assert!(second_output.status.success());
    assert_eq!(second["responses_added"], 0);
    assert_eq!(second["saved_response_events"], 2);
    assert!(second["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "source_generation_ambiguous"));
    assert_eq!(
        diagnostic_count(&database, "source_generation_ambiguous"),
        1
    );
}

#[test]
fn null_and_zero_reasoning_counters_remain_distinct() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let mut missing: Value = serde_json::from_str(TOKEN_USAGE_MISSING_REASONING).unwrap();
    missing["ordinal"] = json!(12);
    let mut zero: Value = serde_json::from_str(TOKEN_USAGE_ZERO_REASONING).unwrap();
    zero["ordinal"] = json!(13);
    let content = [
        SESSION_META.trim().to_owned(),
        serde_json::to_string(&missing).unwrap(),
        serde_json::to_string(&zero).unwrap(),
    ]
    .join("\n")
        + "\n";
    fs::write(active_rollout(&home, "reasoning.jsonl"), content).unwrap();

    let (output, summary) = run_sync(&home, &database);
    assert!(output.status.success());
    assert_eq!(summary["responses_added"], 2);
    let connection = Connection::open(database).unwrap();
    let values: Vec<Option<i64>> = connection
        .prepare(
            "SELECT reasoning_output_tokens FROM token_usage_evidence WHERE record_semantic = 'per_response' ORDER BY event_id",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(values.len(), 2);
    assert!(values.contains(&None));
    assert!(values.contains(&Some(0)));
}

#[test]
fn aggregate_raw_total_overflow_remains_unavailable_instead_of_wrapping() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let maximum_safe = 9_007_199_254_740_991_u64;
    let first = json_line_with_ordinal(TOKEN_USAGE, 12).replace(
        "\"total_tokens\":100",
        &format!("\"total_tokens\":{maximum_safe}"),
    );
    let second = json_line_with_ordinal(TOKEN_USAGE, 13).replace(
        "\"total_tokens\":100",
        &format!("\"total_tokens\":{maximum_safe}"),
    );
    fs::write(
        active_rollout(&home, "large-aggregate.jsonl"),
        [SESSION_META.trim(), &first, &second].join("\n") + "\n",
    )
    .unwrap();

    let (output, summary) = run_sync(&home, &database);
    assert!(output.status.success());
    assert_eq!(summary["saved_raw_total_known_events"], 2);
    assert!(summary["saved_raw_total_known_sum"].is_null());
    let connection = Connection::open(database).unwrap();
    let totals: Vec<i64> = connection
        .prepare("SELECT raw_total FROM token_usage_evidence WHERE record_semantic = 'per_response' ORDER BY event_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(totals, vec![maximum_safe as i64; 2]);
}

#[test]
fn modified_ledger_evidence_is_rejected_before_reporting_history_totals() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    write_basic_rollout(&home, "rollout-corrupt.jsonl");
    let (initial_output, initial) = run_sync(&home, &database);
    assert!(initial_output.status.success());
    assert_eq!(initial["saved_response_events"], 2);

    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE token_usage_evidence SET raw_total = raw_total + 1 WHERE event_id = (SELECT event_id FROM token_usage_evidence WHERE record_semantic = 'per_response' LIMIT 1)",
            [],
        )
        .unwrap();
    drop(connection);

    let output = sync_command(&home, &database).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("database_corrupt"));
    assert!(!stderr.contains("Codex Home"));
    assert!(!stderr.contains("provider-response-must-not-be-retained"));
}

#[test]
fn free_text_in_configuration_fields_is_rejected_without_persisting_it() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let unsafe_settings = THREAD_SETTINGS.replace("gpt-synthetic", "/private/model/path");
    fs::write(
        active_rollout(&home, "unsafe-metadata.jsonl"),
        [
            SESSION_META.trim(),
            unsafe_settings.as_str(),
            &json_line_with_ordinal(TOKEN_USAGE, 12),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();

    let (output, summary) = run_sync(&home, &database);
    assert!(output.status.success());
    assert_eq!(summary["responses_added"], 1);
    assert!(summary["skipped_records"].as_u64().unwrap() >= 1);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("/private/model/path"));
    let database_bytes = fs::read(database).unwrap();
    assert!(!String::from_utf8_lossy(&database_bytes).contains("/private/model/path"));
}

#[test]
fn database_trigger_failure_rolls_back_evidence_and_cursor_for_safe_replay() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let path = active_rollout(&home, "rollout-failure.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n{}\n",
            SESSION_META.trim(),
            json_line_with_ordinal(TOKEN_USAGE, 12)
        ),
    )
    .unwrap();
    let (initial_output, initial) = run_sync(&home, &database);
    assert!(initial_output.status.success());
    assert_eq!(initial["responses_added"], 1);

    let second_response = json_line_with_ordinal(TOKEN_USAGE, 13);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(format!("{second_response}\n").as_bytes())
        .unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_usage_insert BEFORE INSERT ON token_usage_evidence BEGIN SELECT RAISE(ABORT, 'test failure'); END;",
        )
        .unwrap();
    drop(connection);

    let (failed_output, failed) = run_sync(&home, &database);
    assert!(failed_output.status.success());
    assert_eq!(failed["responses_added"], 0);
    assert_eq!(failed["saved_response_events"], 1);
    assert!(failed["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "source_transaction_conflict"));

    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TRIGGER reject_usage_insert;")
        .unwrap();
    drop(connection);
    let (retry_output, retry) = run_sync(&home, &database);
    assert!(retry_output.status.success());
    assert_eq!(retry["responses_added"], 1);
    assert_eq!(retry["saved_response_events"], 2);
}

#[test]
fn concurrent_sync_processes_share_single_writer_history_without_duplicate_events() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    write_basic_rollout(&home, "concurrent.jsonl");

    let first = sync_command(&home, &database).spawn().unwrap();
    let second = sync_command(&home, &database).spawn().unwrap();
    let first = first.wait_with_output().unwrap();
    let second = second.wait_with_output().unwrap();
    assert!(first.status.success());
    assert!(second.status.success());
    let connection = Connection::open(&database).unwrap();
    let responses: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM token_usage_evidence WHERE record_semantic = 'per_response'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(responses, 2);
    drop(connection);

    let (replay_output, replay) = run_sync(&home, &database);
    assert!(replay_output.status.success());
    assert_eq!(replay["responses_added"], 0);
    assert_eq!(replay["saved_response_events"], 2);
}

#[cfg(unix)]
#[test]
fn symlinked_rollout_files_and_directories_are_not_followed() {
    use std::os::unix::fs::symlink;

    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let outside = tree.path().join("outside");
    let database = tree.path().join("history.sqlite3");
    let outside_file = outside.join("rollout.jsonl");
    fs::create_dir_all(&outside).unwrap();
    fs::write(&outside_file, stream_with_two_responses()).unwrap();
    let day = active_rollout(&home, "anchor.jsonl")
        .parent()
        .unwrap()
        .to_path_buf();
    let anchor = day.join("anchor.jsonl");
    fs::write(&anchor, SESSION_META).unwrap();
    symlink(&outside_file, day.join("linked-file.jsonl")).unwrap();
    symlink(&outside, day.join("linked-directory")).unwrap();

    let (output, summary) = run_sync(&home, &database);
    assert!(output.status.success());
    assert_eq!(summary["responses_added"], 0);
    assert_eq!(summary["rollout_sources_seen"], 1);
    assert!(summary["symlink_entries_skipped"].as_u64().unwrap() >= 2);
    assert_eq!(summary["saved_response_events"], 0);
}

#[test]
fn unsupported_cli_version_and_missing_home_are_distinct_json_states() {
    let tree = TempTree::new();
    let home = tree.path().join("Codex Home");
    let database = tree.path().join("history.sqlite3");
    let old = SESSION_META.replace("0.157.1", "0.999.0");
    fs::write(active_rollout(&home, "future-version.jsonl"), old).unwrap();
    let (unsupported_output, unsupported) = run_sync(&home, &database);
    assert!(unsupported_output.status.success());
    assert_eq!(unsupported["status"], "unsupported_codex_version");
    assert_eq!(unsupported["unsupported_version_sources"], 1);
    assert_eq!(unsupported["sessions_seen"], 1);
    assert_eq!(diagnostic_count(&database, "unsupported_codex_version"), 1);

    let missing = tree.path().join("not-a-codex-home");
    let (missing_output, missing_summary) = run_sync(&missing, &database);
    assert!(missing_output.status.success());
    assert_eq!(missing_summary["status"], "explicit_codex_home_unavailable");
}

#[test]
fn help_empty_source_and_malformed_source_states_are_actionable_and_private() {
    let top_help = Command::new(env!("CARGO_BIN_EXE_codex-meter"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(top_help.status.success());
    assert!(String::from_utf8_lossy(&top_help.stdout).contains("sync"));
    let sync_help = Command::new(env!("CARGO_BIN_EXE_codex-meter"))
        .args(["sync", "--help"])
        .output()
        .unwrap();
    assert!(sync_help.status.success());
    assert!(String::from_utf8_lossy(&sync_help.stdout).contains("--codex-home"));

    let tree = TempTree::new();
    let empty_home = tree.path().join("empty Codex Home");
    fs::create_dir_all(&empty_home).unwrap();
    let database = tree.path().join("history.sqlite3");
    let (empty_output, empty) = run_sync(&empty_home, &database);
    assert!(empty_output.status.success());
    assert_eq!(empty["status"], "codex_home_without_sessions");

    let source_home = tree.path().join("malformed source");
    let source = active_rollout(&source_home, "malformed.jsonl");
    fs::write(
        &source,
        format!(
            "{}\n{{malformed private text fake prompt must disappear\n{{\"timestamp\":\"2026-10-03T10:01:03Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"future_event\",\"private\":\"provider-response-must-not-be-retained\"}}}}\n",
            SESSION_META.trim()
        ),
    )
    .unwrap();
    let (malformed_output, malformed) = run_sync(&source_home, &database);
    assert!(malformed_output.status.success());
    assert!(malformed["skipped_records"].as_u64().unwrap() >= 2);
    let connection = Connection::open(&database).unwrap();
    let diagnostics: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM ingestion_diagnostics WHERE diagnostic_code IN ('malformed_record', 'unsupported_event')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(diagnostics, 2);
    drop(connection);
    let db = fs::read(&database).unwrap();
    let db_text = String::from_utf8_lossy(&db);
    assert!(!db_text.contains("fake prompt must disappear"));
    assert!(!db_text.contains("provider-response-must-not-be-retained"));
}
