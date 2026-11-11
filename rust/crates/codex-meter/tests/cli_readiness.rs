#[cfg(not(feature = "release-artifact"))]
use codex_meter::storage::SqliteStore;
use serde_json::Value;
use std::{fs, process::Command};

fn path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("p042-{}-{name}.sqlite", std::process::id()))
}
fn inventory(database: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codex-meter"))
        .args(["readiness", "--database"])
        .arg(database)
        .args(["--generated-at", "2026-10-16T07:00:00Z", "--json"])
        .output()
        .unwrap()
}
#[test]
fn missing_database_never_created_or_path_disclosed() {
    let database = path("private-missing");
    let _ = fs::remove_file(&database);
    let output = inventory(&database);
    assert!(output.status.success());
    assert!(!database.exists());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "unavailable");
    assert_eq!(result["database_present"], false);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private-missing"));
}
#[test]
#[cfg(not(feature = "release-artifact"))]
fn empty_inventory_is_selection_required_and_read_only() {
    let database = path("empty");
    let _ = fs::remove_file(&database);
    drop(SqliteStore::open(&database).unwrap());
    let before = fs::read(&database).unwrap();
    let output = inventory(&database);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["status"], "selection_required");
    assert_eq!(
        result["observation_coverage"]["candidate_terminal_count"],
        0
    );
    assert_eq!(result["ledger_coverage"]["canonical_response_count"], 0);
    assert_eq!(before, fs::read(&database).unwrap());
    fs::remove_file(database).unwrap();
}
#[test]
fn parser_rejects_duplicate_and_unscoped_position() {
    for flags in [
        vec!["--json", "--json"],
        vec!["--weekly-used-percent", "20"],
    ] {
        let mut args = vec!["readiness"];
        args.extend(flags);
        assert!(codex_meter::cli::parse_args(args).is_err());
    }
}
