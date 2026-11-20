use super::args::{CliError, QuotaArguments};
use crate::codex_app_server::{
    AccountRateLimitsSnapshot, CodexAppError, CodexAppServer, MeterSnapshot,
};
use crate::storage::{SavedLiveAccountSnapshot, SqliteStore, StorageError};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub(super) fn run(arguments: &QuotaArguments) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(app_server_error)?;
    run_with_client(arguments, client)
}

fn run_with_client(arguments: &QuotaArguments, client: CodexAppServer) -> Result<String, CliError> {
    let snapshot = client.read_rate_limits().map_err(app_server_error)?;
    if arguments.diagnose {
        let diagnostic = crate::telemetry::evaluate_live_quota_source(
            &snapshot,
            (!client.info().user_agent.is_empty()).then_some(client.info().user_agent.as_str()),
        );
        return render_diagnostic(arguments, &diagnostic);
    }
    if !arguments.save {
        return render_unpersisted(arguments, &snapshot);
    }

    // The live request completes before touching the database. A transport or
    // normalization failure therefore cannot create an empty database or row.
    let database = database_path(arguments.database.as_deref())?;
    if arguments.database.is_none() {
        let directory = database.parent().ok_or_else(database_location_error)?;
        super::sync::prepare_private_data_directory(directory).map_err(not_persisted_error)?;
    }
    super::sync::prepare_private_database_file(&database).map_err(not_persisted_error)?;
    let mut store = SqliteStore::open(&database).map_err(storage_save_error)?;
    let saved = store
        .save_live_account_snapshot(&snapshot)
        .map_err(storage_save_error)?;

    if arguments.json {
        return serialize_saved_result(&snapshot, &saved);
    }
    let mut output = render(&snapshot);
    output.push_str(&format!(
        "Persistence: saved account snapshot {} ({})\n",
        saved.snapshot.snapshot_id,
        match saved.persistence {
            crate::storage::LiveSnapshotSaveDisposition::Persisted => "committed",
            crate::storage::LiveSnapshotSaveDisposition::IdempotentRetry => "already committed",
        }
    ));
    Ok(output)
}

fn render_diagnostic(
    arguments: &QuotaArguments,
    diagnostic: &crate::telemetry::LiveQuotaSourceDiagnostic,
) -> Result<String, CliError> {
    if arguments.json {
        return serde_json::to_string(diagnostic)
            .map(|mut output| {
                output.push('\n');
                output
            })
            .map_err(|_| CliError {
                code: "serialization_failed",
                message: "live quota diagnostic could not be encoded".into(),
            });
    }
    let mut output = String::from(
        "Codex Meter — Live Quota Source Diagnostic\n\nConnection: available\nRate-limit RPC: supported\nSource: Codex App Server\nEvidence scope: account-level\n",
    );
    output.push_str(&format!(
        "Runtime version: {}\nRuntime compatibility: unverified\n",
        diagnostic
            .runtime_compatibility
            .runtime_version
            .as_deref()
            .unwrap_or("unavailable")
    ));
    output.push_str(&format!(
        "Main Codex bucket: {}\n\n",
        status_word(diagnostic.main_bucket)
    ));
    render_diagnostic_meter(&mut output, "Five-hour", &diagnostic.five_hour);
    render_diagnostic_meter(&mut output, "Weekly", &diagnostic.weekly);
    output.push_str(&format!(
        "\nReset stability: unverified from one read\nAccount-level provenance: {}\nTask association: not established\nExternal usage isolation: unverified\nToken coverage closure: unverified\nProduction semantic qualification: unverified\nProduction quota accuracy: unverified\nCapacity estimator qualification: not established\nOverall: structurally {}\n",
        status_word(diagnostic.account_level_provenance),
        match diagnostic.structural_eligibility {
            crate::telemetry::LiveQuotaEvidenceStatus::Supported => "available",
            crate::telemetry::LiveQuotaEvidenceStatus::Partial => "partial",
            crate::telemetry::LiveQuotaEvidenceStatus::Unavailable => "unavailable",
            crate::telemetry::LiveQuotaEvidenceStatus::Unsupported => "unsupported",
            crate::telemetry::LiveQuotaEvidenceStatus::Ambiguous => "ambiguous",
            crate::telemetry::LiveQuotaEvidenceStatus::Unverified => "unverified",
        }
    ));
    if !diagnostic.warnings.is_empty() {
        output.push_str("Warnings: ");
        output.push_str(
            &diagnostic
                .warnings
                .iter()
                .map(|warning| format!("{warning:?}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push('\n');
    }
    Ok(output)
}

fn render_diagnostic_meter(
    output: &mut String,
    label: &str,
    meter: &crate::telemetry::MeterSourceDiagnostic,
) {
    output.push_str(&format!(
        "{label}:\n  Structural evidence: {}\n",
        status_word(meter.structural_evidence)
    ));
    let observed = meter
        .observed_used_percent
        .map(display_percent)
        .map(|percent| format!("{percent}% (observed)"))
        .unwrap_or_else(|| status_word(meter.used_percent).to_owned());
    output.push_str(&format!(
        "  Used percentage: {observed}\n  Reset metadata: {}\n",
        status_word(meter.reset_metadata)
    ));
}

fn status_word(status: crate::telemetry::LiveQuotaEvidenceStatus) -> &'static str {
    use crate::telemetry::LiveQuotaEvidenceStatus as Status;
    match status {
        Status::Supported => "available",
        Status::Partial => "partial",
        Status::Unavailable => "unavailable",
        Status::Unsupported => "unsupported",
        Status::Ambiguous => "ambiguous",
        Status::Unverified => "unverified",
    }
}

fn render_unpersisted(
    arguments: &QuotaArguments,
    snapshot: &AccountRateLimitsSnapshot,
) -> Result<String, CliError> {
    if arguments.json {
        return serde_json::to_string(&snapshot)
            .map(|mut output| {
                output.push('\n');
                output
            })
            .map_err(|_| CliError {
                code: "serialization_failed",
                message: "live quota result could not be encoded".into(),
            });
    }
    Ok(render(snapshot))
}

#[derive(Serialize)]
struct SavedLiveQuotaResult<'a> {
    schema_version: &'static str,
    acquisition: &'a AccountRateLimitsSnapshot,
    persistence: SavedLiveQuotaPersistence<'a>,
}

#[derive(Serialize)]
struct SavedLiveQuotaPersistence<'a> {
    status: &'static str,
    disposition: &'static str,
    snapshot_id: &'a str,
}

fn serialize_saved_result(
    snapshot: &AccountRateLimitsSnapshot,
    saved: &SavedLiveAccountSnapshot,
) -> Result<String, CliError> {
    let result = SavedLiveQuotaResult {
        schema_version: "1.0.0",
        acquisition: snapshot,
        persistence: SavedLiveQuotaPersistence {
            status: "persisted",
            disposition: match saved.persistence {
                crate::storage::LiveSnapshotSaveDisposition::Persisted => "committed",
                crate::storage::LiveSnapshotSaveDisposition::IdempotentRetry => "idempotent_retry",
            },
            snapshot_id: &saved.snapshot.snapshot_id,
        },
    };
    serde_json::to_string(&result)
        .map(|mut output| {
            output.push('\n');
            output
        })
        .map_err(|_| CliError {
            code: "serialization_failed",
            message: "saved live quota result could not be encoded".into(),
        })
}

fn database_path(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    match explicit {
        Some(path) => Ok(path.to_path_buf()),
        None => super::default_database_path().map_err(|_| database_location_error()),
    }
}

fn database_location_error() -> CliError {
    CliError {
        code: "database_location_unavailable",
        message: "the live quota was acquired but not persisted because the database location is unavailable".into(),
    }
}

fn storage_save_error(error: StorageError) -> CliError {
    CliError {
        code: error.code(),
        message: "the live quota was acquired but persistence could not be confirmed".into(),
    }
}

fn not_persisted_error(mut error: CliError) -> CliError {
    error.message = format!(
        "the live quota was acquired but its snapshot was not persisted: {}",
        error.message
    );
    error
}

fn app_server_error(error: CodexAppError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}

fn render(snapshot: &AccountRateLimitsSnapshot) -> String {
    let mut output = String::from("Codex Meter — Live account quota\n\nSource: Codex App Server\nAcquisition: explicit live read\n");
    render_meter(&mut output, "Five-hour", &snapshot.five_hour);
    render_meter(&mut output, "Weekly", &snapshot.weekly);
    output
        .push_str("\nTask attribution: not established\nForecast qualification: not established\n");
    if !snapshot.warnings.is_empty() {
        output.push_str("Warnings: ");
        output.push_str(
            &snapshot
                .warnings
                .iter()
                .map(|warning| format!("{warning:?}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push('\n');
    }
    output
}

fn render_meter(output: &mut String, label: &str, meter: &MeterSnapshot) {
    output.push_str(&format!("{label}:\n"));
    if let Some(used) = meter.used_percent {
        output.push_str(&format!("  Used: {}% (observed)\n", display_percent(used)));
        output.push_str(&format!(
            "  Remaining: {}% (derived)\n",
            display_percent(meter.remaining_percent.unwrap_or_default())
        ));
        output.push_str(&format!(
            "  Reset: {}\n",
            meter.reset_at_utc.as_deref().unwrap_or("Unavailable")
        ));
    } else {
        output.push_str("  Evidence unavailable\n");
    }
}

fn display_percent(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{
        BucketIdentity, MeterAvailability, QuotaWarning, SnapshotAvailability,
    };
    use crate::storage::{SqliteReadStore, SqliteStore};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn database_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-p045b-cli-{}-{}.sqlite3",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove_database(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    fn parsed_quota(arguments: &[&str]) -> QuotaArguments {
        let crate::cli::CliAction::Quota(arguments) = crate::cli::parse_args(arguments).unwrap()
        else {
            panic!("expected quota arguments")
        };
        arguments
    }

    #[test]
    fn live_output_states_account_level_provenance() {
        let snapshot = AccountRateLimitsSnapshot {
            schema_version: "1.0.0", source: "codex_app_server_account_rate_limits", acquired_at: "2026-11-18T00:00:00Z".into(),
            availability: SnapshotAvailability::Partial, bucket_identity: BucketIdentity::MainCodex,
            five_hour: MeterSnapshot { availability: MeterAvailability::Available, used_percent: Some(42.0), used_percent_provenance: "observed", remaining_percent: Some(58.0), remaining_percent_provenance: "derived", reset_at_utc: None },
            weekly: MeterSnapshot { availability: MeterAvailability::Unavailable, used_percent: None, used_percent_provenance: "unavailable", remaining_percent: None, remaining_percent_provenance: "unavailable", reset_at_utc: None },
            warnings: vec![QuotaWarning::MissingWeekly], provenance: "account_level_snapshot; task_attribution_not_established; forecast_qualification_not_established",
        };
        let output = render(&snapshot);
        assert!(output.contains("Used: 42% (observed)"));
        assert!(output.contains("Remaining: 58% (derived)"));
        assert!(output.contains("Task attribution: not established"));
    }

    #[test]
    fn explicit_cli_save_reads_once_persists_one_privacy_filtered_snapshot_and_queries_history() {
        let database = database_path();
        let database_text = database.to_string_lossy().into_owned();
        let arguments = parsed_quota(&[
            "quota",
            "--live",
            "--save",
            "--database",
            &database_text,
            "--json",
        ]);
        let client = CodexAppServer::connect_test_server("rate_limits_once").unwrap();
        let output = run_with_client(&arguments, client).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["schema_version"], "1.0.0");
        assert_eq!(value["persistence"]["status"], "persisted");
        assert_eq!(
            value["acquisition"]["source"],
            "codex_app_server_account_rate_limits"
        );
        assert_eq!(value["acquisition"]["five_hour"]["used_percent"], 42.0);
        assert_eq!(value["acquisition"]["weekly"]["used_percent"], 0.0);
        let snapshot_id = value["persistence"]["snapshot_id"].as_str().unwrap();
        assert!(snapshot_id.starts_with("lqs_"));
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "never-log-this",
            "unknownProviderField",
            "providerSecret",
        ] {
            assert!(!output.contains(forbidden));
        }

        let mut reader = SqliteReadStore::open(&database).unwrap();
        let page = reader.live_account_snapshot_history(20, None).unwrap();
        assert_eq!(page.snapshots.len(), 1);
        assert_eq!(page.snapshots[0].snapshot_id, snapshot_id);
        assert_eq!(page.snapshots[0].five_hour.used_percent, Some(42.0));
        assert_eq!(page.snapshots[0].weekly.used_percent, Some(0.0));
        assert_eq!(page.snapshots[0].task_attribution, "not_established");
        drop(reader);

        let history_arguments = parsed_quota(&[
            "quota",
            "--history",
            "--source",
            "live",
            "--database",
            &database_text,
            "--json",
        ]);
        let history_json = super::super::live_quota_history::run(&history_arguments).unwrap();
        let history: serde_json::Value = serde_json::from_str(&history_json).unwrap();
        assert_eq!(history["source"], "codex_app_server_account_rate_limits");
        assert_eq!(history["snapshots"].as_array().unwrap().len(), 1);
        assert_eq!(history["snapshots"][0]["snapshot_id"], snapshot_id);
        assert_eq!(history["task_attribution"], "not_established");
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "never-log-this",
            "unknownProviderField",
            "providerSecret",
        ] {
            assert!(!history_json.contains(forbidden));
        }

        let human_arguments = parsed_quota(&[
            "quota",
            "--history",
            "--source",
            "live",
            "--database",
            database_text.as_str(),
        ]);
        let human_history = super::super::live_quota_history::run(&human_arguments).unwrap();
        assert!(human_history.contains("Acquired:"));
        assert!(human_history.contains("Five-hour: Available; used 42% (observed)"));
        assert!(human_history.contains("Weekly: Available; used 0% (observed)"));
        assert!(human_history.contains(snapshot_id));
        assert!(human_history.contains("Task attribution: not established"));

        let rollout_history = crate::cli::run([
            "quota",
            "--history",
            "--database",
            database_text.as_str(),
            "--json",
        ])
        .unwrap();
        assert!(!rollout_history.contains(snapshot_id));
        assert!(rollout_history.contains("no_supported_samples"));

        let store = SqliteStore::open(&database).unwrap();
        let columns = store
            .connection()
            .prepare("PRAGMA table_info('live_account_quota_snapshots')")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for private_field in [
            "account_id",
            "email",
            "access_token",
            "raw_response",
            "prompt",
            "task_id",
            "source_path",
        ] {
            assert!(!columns.iter().any(|column| column == private_field));
        }
        let normalized_storage: String = store.connection().query_row(
            "SELECT acquired_at_utc || warning_codes || source || evidence_scope || task_attribution || forecast_qualification FROM live_account_quota_snapshots",
            [],
            |row| row.get(0),
        ).unwrap();
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "never-log-this",
            "unknownProviderField",
            "providerSecret",
        ] {
            assert!(!normalized_storage.contains(forbidden));
        }
        drop(store);
        remove_database(&database);
    }

    #[test]
    fn live_without_save_keeps_p045a_json_and_does_not_open_database() {
        let arguments = parsed_quota(&["quota", "--live", "--json"]);
        assert!(!arguments.save);
        let client = CodexAppServer::connect_test_server("rate_limits_once").unwrap();
        let output = run_with_client(&arguments, client).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["source"], "codex_app_server_account_rate_limits");
        assert!(value.get("persistence").is_none());
        assert!(value.get("snapshot_id").is_none());
    }

    #[test]
    fn diagnostic_is_single_read_account_level_json_and_has_no_persistence_or_sensitive_fields() {
        let database = database_path();
        let seed_client = CodexAppServer::connect_test_server("rate_limits_once").unwrap();
        let seeded_snapshot = seed_client.read_rate_limits().unwrap();
        let mut store = SqliteStore::open(&database).unwrap();
        let seeded = store.save_live_account_snapshot(&seeded_snapshot).unwrap();
        drop(store);
        let arguments = parsed_quota(&["quota", "--live", "--diagnose", "--json"]);
        let client = CodexAppServer::connect_test_server("rate_limits_once").unwrap();
        let output = run_with_client(&arguments, client).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["schema_version"], "1.0.0");
        assert_eq!(value["source"], "codex_app_server_account_rate_limits");
        assert_eq!(value["evidence_scope"], "account_level");
        assert_eq!(value["connection"], "supported");
        assert_eq!(value["rate_limit_rpc"], "supported");
        assert_eq!(value["five_hour"]["observed_used_percent"], 42.0);
        assert_eq!(value["weekly"]["observed_used_percent"], 0.0);
        assert_eq!(value["task_association"], "unverified");
        assert_eq!(value["production_quota_accuracy"], "unverified");
        assert_eq!(value["runtime_compatibility"]["runtime_version"], "0.163.0");
        assert_eq!(
            value["runtime_compatibility"]["compatibility"],
            "unverified"
        );
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "never-log-this",
            "unknownProviderField",
            "raw_response",
            "database",
            "snapshot_id",
            "credential",
            "task_id",
        ] {
            assert!(
                !output.contains(forbidden),
                "forbidden diagnostic field: {forbidden}"
            );
        }
        let mut reader = SqliteReadStore::open(&database).unwrap();
        let history = reader.live_account_snapshot_history(20, None).unwrap();
        assert_eq!(history.snapshots.len(), 1);
        assert_eq!(
            history.snapshots[0].snapshot_id,
            seeded.snapshot.snapshot_id
        );
        drop(reader);
        remove_database(&database);
    }

    #[test]
    fn diagnostic_human_output_keeps_structural_and_semantic_claims_separate() {
        let arguments = parsed_quota(&["quota", "--live", "--diagnose"]);
        let client = CodexAppServer::connect_test_server("rate_limits_once").unwrap();
        let output = run_with_client(&arguments, client).unwrap();
        assert!(output.contains("Used percentage: 42% (observed)"));
        assert!(output.contains("Task association: not established"));
        assert!(output.contains("Reset stability: unverified from one read"));
        assert!(output.contains("Production quota accuracy: unverified"));
        assert!(output.contains("Overall: structurally available"));
    }

    #[test]
    fn diagnostic_uses_normalized_independent_meter_evidence_for_fake_server_shapes() {
        for (mode, expected_five, expected_weekly, expected_overall) in [
            (
                "rate_limits_five_hour_only",
                "supported",
                "unavailable",
                "partial",
            ),
            (
                "rate_limits_weekly_only",
                "unavailable",
                "supported",
                "partial",
            ),
            (
                "rate_limits_weekly_reversed",
                "supported",
                "supported",
                "supported",
            ),
            (
                "rate_limits_missing_reset",
                "supported",
                "supported",
                "supported",
            ),
            (
                "rate_limits_no_meters",
                "unavailable",
                "unavailable",
                "unavailable",
            ),
            (
                "rate_limits_unknown_bucket",
                "unavailable",
                "unavailable",
                "unavailable",
            ),
            (
                "rate_limits_nonmain",
                "unavailable",
                "unavailable",
                "unavailable",
            ),
            (
                "rate_limits_short_once",
                "unavailable",
                "unavailable",
                "unavailable",
            ),
            (
                "rate_limits_missing_percentage",
                "unavailable",
                "supported",
                "partial",
            ),
            (
                "rate_limits_missing_duration",
                "unavailable",
                "supported",
                "partial",
            ),
            (
                "rate_limits_duplicate",
                "ambiguous",
                "unavailable",
                "ambiguous",
            ),
            (
                "rate_limits_conflicting_views",
                "ambiguous",
                "ambiguous",
                "ambiguous",
            ),
        ] {
            let arguments = parsed_quota(&["quota", "--live", "--diagnose", "--json"]);
            let client = CodexAppServer::connect_test_server(mode).unwrap();
            let output = run_with_client(&arguments, client).unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert_eq!(
                value["five_hour"]["structural_evidence"], expected_five,
                "{mode}"
            );
            assert_eq!(
                value["weekly"]["structural_evidence"], expected_weekly,
                "{mode}"
            );
            assert_eq!(value["structural_eligibility"], expected_overall, "{mode}");
            assert_eq!(value["task_association"], "unverified", "{mode}");
            let warnings = value["warnings"].as_array().unwrap();
            match mode {
                "rate_limits_short_once" => {
                    assert!(warnings.contains(&serde_json::json!("unsupported_duration")))
                }
                "rate_limits_missing_percentage" => {
                    assert!(warnings.contains(&serde_json::json!("missing_percentage")))
                }
                "rate_limits_missing_duration" => {
                    assert!(warnings.contains(&serde_json::json!("missing_duration")))
                }
                "rate_limits_duplicate" => {
                    assert!(warnings.contains(&serde_json::json!("duplicate_five_hour")))
                }
                "rate_limits_conflicting_views" => {
                    assert!(warnings.contains(&serde_json::json!("conflicting_bucket_views")))
                }
                _ => {}
            }
        }
    }

    #[test]
    fn diagnostic_rpc_failures_remain_sanitized_errors_and_do_not_create_a_database() {
        let database = database_path();
        let arguments = parsed_quota(&["quota", "--live", "--diagnose"]);
        let client = CodexAppServer::connect_test_server("rate_limits_unsupported").unwrap();
        let error = run_with_client(&arguments, client).unwrap_err();
        assert_eq!(error.code, "unsupported_method");
        assert!(!error.message.contains("unsupported"));
        assert!(!database.exists());
        remove_database(&database);
    }

    #[test]
    fn diagnostic_process_exit_and_timeout_are_sanitized_failures() {
        let arguments = parsed_quota(&["quota", "--live", "--diagnose", "--json"]);
        for (mode, expected) in [
            ("rate_limits_process_exit", "unexpected_process_exit"),
            ("rate_limits_timeout", "request_timeout"),
        ] {
            let client = CodexAppServer::connect_test_server(mode).unwrap();
            let error = run_with_client(&arguments, client).unwrap_err();
            assert_eq!(error.code, expected, "{mode}");
            assert!(!error.message.contains("SENSITIVE"), "{mode}");
        }
    }

    #[test]
    fn missing_or_ambiguous_runtime_metadata_stays_unavailable_and_is_not_echoed() {
        for mode in [
            "rate_limits_missing_user_agent",
            "rate_limits_ambiguous_user_agent",
        ] {
            let arguments = parsed_quota(&["quota", "--live", "--diagnose", "--json"]);
            let client = CodexAppServer::connect_test_server(mode).unwrap();
            let output = run_with_client(&arguments, client).unwrap();
            let value: serde_json::Value = serde_json::from_str(&output).unwrap();
            assert!(value["runtime_compatibility"]["runtime_version"].is_null());
            assert_eq!(
                value["runtime_compatibility"]["compatibility"],
                "unverified"
            );
            assert!(!output.contains("private-metadata"));
            assert_eq!(value["connection"], "supported");
        }
    }

    #[test]
    fn app_server_failure_during_explicit_save_creates_no_database_or_snapshot() {
        let database = database_path();
        let database_text = database.to_string_lossy().into_owned();
        let arguments = parsed_quota(&["quota", "--live", "--save", "--database", &database_text]);
        let client = CodexAppServer::connect_test_server("rate_limits_unsupported").unwrap();
        let error = run_with_client(&arguments, client).unwrap_err();
        assert_eq!(error.code, "unsupported_method");
        assert!(!database.exists());
        remove_database(&database);
    }

    #[test]
    fn successful_rpc_without_main_bucket_is_saved_as_unavailable_not_as_zero() {
        let database = database_path();
        let database_text = database.to_string_lossy().into_owned();
        let arguments = parsed_quota(&[
            "quota",
            "--live",
            "--save",
            "--database",
            database_text.as_str(),
            "--json",
        ]);
        let client = CodexAppServer::connect_test_server("rate_limits_missing").unwrap();
        let output = run_with_client(&arguments, client).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["acquisition"]["availability"], "unavailable");
        assert_eq!(
            value["acquisition"]["five_hour"]["used_percent"],
            serde_json::Value::Null
        );
        assert_eq!(
            value["acquisition"]["weekly"]["used_percent"],
            serde_json::Value::Null
        );
        assert_eq!(value["persistence"]["status"], "persisted");
        assert!(value["acquisition"]["warnings"].is_array());
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "unknownProviderField",
        ] {
            assert!(!output.contains(forbidden));
        }
        let mut reader = SqliteReadStore::open(&database).unwrap();
        let page = reader.live_account_snapshot_history(20, None).unwrap();
        assert_eq!(page.snapshots.len(), 1);
        assert_eq!(
            page.snapshots[0].availability,
            SnapshotAvailability::Unavailable
        );
        assert_eq!(page.snapshots[0].five_hour.used_percent, None);
        drop(reader);
        remove_database(&database);
    }

    #[test]
    fn successful_rpc_with_only_short_windows_is_saved_as_unavailable() {
        let database = database_path();
        let database_text = database.to_string_lossy().into_owned();
        let arguments = parsed_quota(&[
            "quota",
            "--live",
            "--save",
            "--database",
            database_text.as_str(),
            "--json",
        ]);
        let client = CodexAppServer::connect_test_server("rate_limits_short_once").unwrap();
        let output = run_with_client(&arguments, client).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["acquisition"]["availability"], "unavailable");
        assert_eq!(value["persistence"]["status"], "persisted");
        assert_eq!(
            value["acquisition"]["five_hour"]["availability"],
            "unavailable"
        );
        assert_eq!(
            value["acquisition"]["weekly"]["availability"],
            "unavailable"
        );
        let warnings = value["acquisition"]["warnings"].as_array().unwrap();
        assert_eq!(
            warnings
                .iter()
                .filter(|warning| warning.as_str() == Some("unsupported_duration"))
                .count(),
            2
        );

        let mut reader = SqliteReadStore::open(&database).unwrap();
        let page = reader.live_account_snapshot_history(10, None).unwrap();
        assert_eq!(page.snapshots.len(), 1);
        assert_eq!(
            page.snapshots[0].availability,
            SnapshotAvailability::Unavailable
        );
        assert_eq!(
            page.snapshots[0]
                .warnings
                .iter()
                .filter(|warning| **warning == QuotaWarning::UnsupportedDuration)
                .count(),
            1
        );
        assert_eq!(page.snapshots[0].five_hour.used_percent, None);
        assert_eq!(page.snapshots[0].weekly.used_percent, None);
        drop(reader);
        remove_database(&database);
    }
}
