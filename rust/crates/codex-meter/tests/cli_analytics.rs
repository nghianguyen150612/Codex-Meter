#[cfg(not(feature = "release-artifact"))]
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(feature = "release-artifact"))]
use codex_meter::storage::SqliteStore;
#[cfg(not(feature = "release-artifact"))]
use codex_meter::telemetry::NormalizedObservation;
use serde_json::json;
#[cfg(not(feature = "release-artifact"))]
use serde_json::Value;
#[cfg(not(feature = "release-artifact"))]
use sha2::{Digest, Sha256};

const GENERATED_AT: &str = "2026-10-16T07:00:00Z";
static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

fn temporary_path(suffix: &str) -> PathBuf {
    let sequence = NEXT_DATABASE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "codex meter p027 {} {sequence}.{suffix}",
        std::process::id()
    ))
}

#[cfg(not(feature = "release-artifact"))]
fn synthetic_database(path: &std::path::Path) {
    let mut store = SqliteStore::open(path).expect("synthetic database should migrate");
    for (index, raw_total) in [14_500, 16_000].into_iter().enumerate() {
        let mut document: Value = serde_json::from_str(include_str!(
            "../../../../fixtures/contracts/v1/observation-codex-finalized.json"
        ))
        .expect("published synthetic Observation fixture should parse");
        document["observation_id"] = json!(format!("p027-observation-{index}"));
        document["task_id"] = json!(format!("p027-task-{index}"));
        document["session_id"] = json!(format!("p027-session-{index}"));
        let hour = 2 + index;
        document["timing"]["started_at"] = json!(format!("2026-10-01T{hour:02}:00:00Z"));
        document["timing"]["ended_at"] = json!(format!("2026-10-01T{hour:02}:15:00Z"));
        document["timing"]["finalized_at"] = json!(format!("2026-10-01T{hour:02}:20:00Z"));
        document["token_evidence"]["raw_token_counters"]["raw_total"]["value"] = json!(raw_total);
        let observation: NormalizedObservation = serde_json::from_value(document)
            .expect("synthetic Observation should match the Rust-owned model");
        store
            .save_observation(&observation, None)
            .expect("synthetic Observation should be stored");
    }
    drop(store);
}

fn request_json(observed_from: Option<&str>, model: &str) -> String {
    let mut selector = json!({
        "configuration": {
            "plan": {"availability": "available", "value": "plus", "provenance": "observed"},
            "model": {"availability": "available", "value": model, "provenance": "observed"},
            "reasoning_level": {"availability": "available", "value": "high", "provenance": "observed"},
            "speed_mode": {"availability": "unavailable", "provenance": "unavailable"},
            "codex_version": {"availability": "unavailable", "provenance": "unavailable"}
        },
        "observed_through": "2026-10-15T23:59:59Z"
    });
    if let Some(observed_from) = observed_from {
        selector["observed_from"] = json!(observed_from);
    }
    json!({
        "schema_version": "1.0.0",
        "request_id": "p027-e2e-001",
        "analysis_type": "current_capacity_estimate",
        "dataset_selector": selector,
        "target_quota_windows": ["five_hour"],
        "options": {"maximum_observations": 10}
    })
    .to_string()
}

fn invoke(
    database: &std::path::Path,
    request: &std::path::Path,
    extra: &[&str],
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codex-meter"))
        .arg("estimate")
        .arg("--database")
        .arg(database)
        .arg("--request")
        .arg(request)
        .arg("--generated-at")
        .arg(GENERATED_AT)
        .args(extra)
        .output()
        .expect("CLI process should start")
}

#[test]
#[cfg(not(feature = "release-artifact"))]
fn native_cli_runs_python_estimator_read_only_and_validates_json_output() {
    let database = temporary_path("estimate 数据.sqlite");
    let request = temporary_path("json");
    synthetic_database(&database);
    fs::write(&request, request_json(None, "gpt-5.6-sol")).expect("request file should be created");
    let before = Sha256::digest(fs::read(&database).expect("database should exist"));

    let output = invoke(
        &database,
        &request,
        &["--five-hour-used-percent", "24", "--json"],
    );
    assert!(
        output.status.success(),
        "CLI failed with categorized stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len(), 1_159);
    assert_eq!(
        format!("{:x}", Sha256::digest(&output.stdout)),
        "13ba5044594d01cff0313d14a6fd514883d4512b6278b5cd8db74f34be5a296a",
        "P029 must preserve the representative AnalyticsResult bytes at the starting SHA"
    );
    let result: Value = serde_json::from_slice(&output.stdout).expect("JSON mode emits one result");
    assert_eq!(result["result_status"], "succeeded");
    assert_eq!(result["request_id"], "p027-e2e-001");
    assert_eq!(
        result
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        [
            "analysis_type",
            "dataset_configuration",
            "estimator_method_version",
            "generated_at",
            "quota_capacity_estimates",
            "request_id",
            "result_status",
            "schema_version",
            "warnings",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    );
    assert_eq!(
        result["quota_capacity_estimates"][0]["meter_type"],
        "five_hour"
    );
    assert_eq!(
        result["dataset_configuration"]["speed_mode"]["availability"],
        "unavailable"
    );
    let allowed_estimate_fields = [
        "meter_type",
        "metric_kind",
        "provider_authority",
        "confidence",
        "sample_accounting",
        "estimated_full_capacity_raw_tokens",
        "estimated_used_raw_tokens",
        "estimated_remaining_raw_tokens",
        "capacity_percentiles_raw_tokens",
        "reason_codes",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert!(result["quota_capacity_estimates"][0]
        .as_object()
        .unwrap()
        .keys()
        .all(|field| allowed_estimate_fields.contains(field.as_str())));
    let full_capacity = result["quota_capacity_estimates"][0]["estimated_full_capacity_raw_tokens"]
        .as_f64()
        .expect("sufficient result should contain full capacity");
    let used_capacity = result["quota_capacity_estimates"][0]["estimated_used_raw_tokens"]
        .as_f64()
        .expect("manual position should produce used capacity");
    assert!((used_capacity - full_capacity * 0.24).abs() < 1e-8);
    let replay = invoke(
        &database,
        &request,
        &["--five-hour-used-percent", "24", "--json"],
    );
    assert_eq!(replay.stdout, output.stdout);
    assert_eq!(
        before.as_slice(),
        Sha256::digest(fs::read(&database).unwrap()).as_slice()
    );

    let unrequested_position = invoke(
        &database,
        &request,
        &["--weekly-used-percent", "20", "--json"],
    );
    assert!(!unrequested_position.status.success());
    assert!(String::from_utf8_lossy(&unrequested_position.stderr)
        .contains("invalid_manual_meter_percentage"));

    let _ = fs::remove_file(database);
    let _ = fs::remove_file(request);
}

#[test]
fn rejected_request_privacy_canaries_never_reach_cli_error_output() {
    let request = temporary_path("privacy-request.json");
    let markers = [
        "SYNTHETIC_SECRET_MARKER",
        "SYNTHETIC_PROMPT_MARKER",
        "SYNTHETIC_RESPONSE_MARKER",
        "SYNTHETIC_SOURCE_MARKER",
    ];
    let invalid = format!(
        "{{\"schema_version\":\"1.0.0\",\"request_id\":\"request-1\",\"unknown\":\"{} {} {} {}\"}}",
        markers[0], markers[1], markers[2], markers[3]
    );
    fs::write(&request, invalid).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_codex-meter"))
        .args(["estimate", "--database", "missing.sqlite", "--request"])
        .arg(&request)
        .args(["--generated-at", GENERATED_AT, "--json"])
        .output()
        .expect("CLI process should start");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(diagnostic.contains("invalid_request"));
    assert!(markers.iter().all(|marker| !diagnostic.contains(marker)));
    let _ = fs::remove_file(request);
}

#[test]
#[cfg(not(feature = "release-artifact"))]
fn empty_or_incompatible_history_returns_insufficient_evidence_without_pooling() {
    let database = temporary_path("sqlite");
    let request = temporary_path("json");
    synthetic_database(&database);

    fs::write(
        &request,
        request_json(Some("2026-10-03T00:00:00Z"), "gpt-5.6-sol"),
    )
    .unwrap();
    let empty = invoke(&database, &request, &["--json"]);
    assert!(
        empty.status.success(),
        "{}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let empty_result: Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(empty_result["result_status"], "insufficient_evidence");
    assert_eq!(
        empty_result["quota_capacity_estimates"][0]["sample_accounting"]["candidate_observations"],
        0
    );

    fs::write(&request, request_json(None, "different-model")).unwrap();
    let incompatible = invoke(&database, &request, &["--json"]);
    assert!(
        incompatible.status.success(),
        "{}",
        String::from_utf8_lossy(&incompatible.stderr)
    );
    let incompatible_result: Value = serde_json::from_slice(&incompatible.stdout).unwrap();
    assert_eq!(
        incompatible_result["result_status"],
        "insufficient_evidence"
    );
    assert_eq!(
        incompatible_result["quota_capacity_estimates"][0]["sample_accounting"]
            ["candidate_observations"],
        0
    );

    let human = invoke(&database, &request, &["--five-hour-used-percent", "24"]);
    assert!(human.status.success());
    let human_output = String::from_utf8_lossy(&human.stdout);
    assert!(human_output
        .contains("Codex Meter empirical estimates are not official provider quota allowances."));

    let _ = fs::remove_file(database);
    let _ = fs::remove_file(request);
}

#[test]
fn native_help_version_and_missing_database_behave_explicitly() {
    let binary = env!("CARGO_BIN_EXE_codex-meter");
    assert!(Command::new(binary)
        .arg("--help")
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new(binary)
        .arg("--version")
        .output()
        .unwrap()
        .status
        .success());
    assert!(Command::new(binary)
        .args(["estimate", "--help"])
        .output()
        .unwrap()
        .status
        .success());

    let database = temporary_path("missing.sqlite");
    let request = temporary_path("request.json");
    fs::write(&request, request_json(None, "gpt-5.6-sol")).unwrap();
    let output = invoke(&database, &request, &["--json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("database_missing"));
    assert!(!database.exists());
    let _ = fs::remove_file(request);
}
