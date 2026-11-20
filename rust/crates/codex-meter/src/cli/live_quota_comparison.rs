use std::path::{Path, PathBuf};

use crate::storage::{SqliteReadStore, StorageError};
use crate::telemetry::{evaluate_live_quota_comparison, ComparisonError, MeterComparison};

use super::args::{CliError, QuotaArguments};

pub(super) fn run(arguments: &QuotaArguments) -> Result<String, CliError> {
    let before_id = arguments
        .before_snapshot
        .as_deref()
        .ok_or_else(missing_ids)?;
    let after_id = arguments
        .after_snapshot
        .as_deref()
        .ok_or_else(missing_ids)?;
    let path = database_path(arguments.database.as_deref())?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            missing_database()
        } else {
            database_open_error()
        }
    })?;
    if !metadata.is_file() {
        return Err(missing_database());
    }
    let mut store = SqliteReadStore::open(&path).map_err(storage_error)?;
    let before = store
        .live_account_snapshot(before_id)
        .map_err(storage_error)?
        .ok_or_else(snapshot_missing)?;
    let after = store
        .live_account_snapshot(after_id)
        .map_err(storage_error)?
        .ok_or_else(snapshot_missing)?;
    let comparison = evaluate_live_quota_comparison(&before, &after).map_err(comparison_error)?;
    if arguments.json {
        encode_json(&comparison)
    } else {
        Ok(render(&comparison))
    }
}

fn database_path(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    match explicit {
        Some(path) => Ok(path.to_path_buf()),
        None => super::default_database_path().map_err(|_| CliError {
            code: "database_location_unavailable",
            message: "could not determine the Codex Meter database location".into(),
        }),
    }
}

fn encode_json(comparison: &crate::telemetry::LiveQuotaComparison) -> Result<String, CliError> {
    serde_json::to_string(comparison)
        .map(|mut value| {
            value.push('\n');
            value
        })
        .map_err(|_| CliError {
            code: "serialization_failed",
            message: "live quota comparison could not be encoded".into(),
        })
}

fn render(comparison: &crate::telemetry::LiveQuotaComparison) -> String {
    let mut output = String::from("Codex Meter — Controlled Account Quota Comparison\n\n");
    output.push_str("Source: saved live App Server snapshots\n");
    output.push_str("Evidence scope: account-level\n\n");
    output.push_str(&format!(
        "Before: {} ({})\nAfter:  {} ({})\n\n",
        comparison.before_snapshot_id,
        comparison.before_acquired_at_utc,
        comparison.after_snapshot_id,
        comparison.after_acquired_at_utc
    ));
    output.push_str(&format!(
        "Bucket identity: before {:?}, after {:?} (pair: {:?})\n",
        comparison.before_bucket_identity,
        comparison.after_bucket_identity,
        comparison.bucket_identity
    ));
    output.push_str("Acquisition chronology: increasing\n\n");
    render_meter(&mut output, "Five-hour", &comparison.five_hour);
    render_meter(&mut output, "Weekly", &comparison.weekly);
    output.push_str("\nTask causality: unverified\n");
    output.push_str("External account activity: unknown\n");
    output.push_str("Account identity continuity: unverified\n");
    output.push_str("Controlled benchmark isolation: absent\n");
    output.push_str("Token coverage closure: unverified\n");
    output.push_str("Production quota accuracy: unverified\n");
    output.push_str("Official token allowance: unavailable\n");
    output.push_str("Estimator eligibility: not established\n");
    output
}

fn render_meter(output: &mut String, label: &str, meter: &MeterComparison) {
    output.push_str(&format!("{label}:\n"));
    output.push_str(&format!("  Evidence: {}\n", status_label(meter)));
    output.push_str(&format!(
        "  Before used: {}\n  After used: {}\n",
        percent(meter.before_used_percent),
        percent(meter.after_used_percent)
    ));
    output.push_str(&format!(
        "  Window continuity: {}\n",
        continuity_label(meter.window_continuity)
    ));
    output.push_str(&format!(
        "  Reset evidence: {}\n",
        reset_label(meter.reset_evidence)
    ));
    if let Some(movement) = meter.observed_movement_percentage_points {
        output.push_str(&format!(
            "  Account movement: +{movement} percentage points\n"
        ));
    }
}

fn percent(value: Option<f64>) -> String {
    value.map_or_else(
        || "unavailable".to_owned(),
        |value| format!("{value}% (observed)"),
    )
}

fn status_label(meter: &MeterComparison) -> &'static str {
    match meter.status {
        crate::telemetry::MeterComparisonStatus::SameObservedWindow => "paired observation",
        crate::telemetry::MeterComparisonStatus::ResetBoundaryDetected => "reset boundary detected",
        crate::telemetry::MeterComparisonStatus::WindowContinuityUnverified => {
            "window continuity unverified"
        }
        crate::telemetry::MeterComparisonStatus::MeterUnavailable => "unavailable",
        crate::telemetry::MeterComparisonStatus::AmbiguousEvidence => "ambiguous evidence",
        crate::telemetry::MeterComparisonStatus::MeterInstability => "meter instability",
    }
}

fn continuity_label(continuity: crate::telemetry::WindowContinuity) -> &'static str {
    match continuity {
        crate::telemetry::WindowContinuity::ObservedCompatible => "observed-compatible",
        crate::telemetry::WindowContinuity::ResetBoundaryDetected => "reset boundary detected",
        crate::telemetry::WindowContinuity::Unverified => "unverified",
        crate::telemetry::WindowContinuity::NotAssessable => "not assessable",
    }
}

fn reset_label(reset: crate::telemetry::ResetEvidence) -> &'static str {
    match reset {
        crate::telemetry::ResetEvidence::MatchingObservedReset => "matching observed reset",
        crate::telemetry::ResetEvidence::ResetBoundaryCrossed => "reset boundary crossed",
        crate::telemetry::ResetEvidence::ResetNotAfterBaseline => "reset time not after baseline",
        crate::telemetry::ResetEvidence::MissingResetMetadata => "missing reset metadata",
        crate::telemetry::ResetEvidence::ChangedResetWithoutBoundaryEvidence => {
            "changed reset without boundary evidence"
        }
        crate::telemetry::ResetEvidence::Unavailable => "unavailable",
        crate::telemetry::ResetEvidence::Ambiguous => "ambiguous",
    }
}

fn missing_ids() -> CliError {
    CliError {
        code: "missing_snapshot_id",
        message: "--compare-live requires both selected snapshot IDs".into(),
    }
}

fn missing_database() -> CliError {
    CliError {
        code: "live_snapshot_database_missing",
        message: "no existing live snapshot database was found".into(),
    }
}

fn snapshot_missing() -> CliError {
    CliError {
        code: "live_snapshot_not_found",
        message: "a selected saved live snapshot was not found".into(),
    }
}

fn database_open_error() -> CliError {
    CliError {
        code: "database_open_failed",
        message: "the Codex Meter database could not be opened read-only".into(),
    }
}

fn storage_error(error: StorageError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}

fn comparison_error(error: ComparisonError) -> CliError {
    let code = match error {
        ComparisonError::SnapshotIdInvalid => "invalid_snapshot_id",
        ComparisonError::SameSnapshotSelected => "same_snapshot_selected",
        ComparisonError::UnsupportedSchemaVersion => "snapshot_schema_unsupported",
        ComparisonError::UnsupportedSource => "snapshot_source_unsupported",
        ComparisonError::EvidenceScopeInvalid => "snapshot_evidence_scope_invalid",
        ComparisonError::ProvenanceInvalid => "snapshot_provenance_invalid",
        ComparisonError::AcquisitionTimestampInvalid => "snapshot_timestamp_invalid",
        ComparisonError::AcquisitionTimesNotIncreasing => "snapshot_chronology_invalid",
        ComparisonError::InvalidMeterEvidence => "snapshot_meter_invalid",
    };
    CliError {
        code,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{
        AccountRateLimitsSnapshot, BucketIdentity, MeterAvailability, MeterSnapshot, QuotaWarning,
        SnapshotAvailability,
    };
    use crate::storage::{SqliteStore, LIVE_ACCOUNT_QUOTA_PROVENANCE};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn database_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-p046b1-comparison-{}-{}.sqlite3",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn remove_database(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    fn snapshot(acquired_at: &str, five: f64, weekly: f64) -> AccountRateLimitsSnapshot {
        let meter = |used: f64, reset: &str| MeterSnapshot {
            availability: MeterAvailability::Available,
            used_percent: Some(used),
            used_percent_provenance: "observed",
            remaining_percent: Some(100.0 - used),
            remaining_percent_provenance: "derived",
            reset_at_utc: Some(reset.to_owned()),
        };
        AccountRateLimitsSnapshot {
            schema_version: "1.0.0",
            source: crate::storage::LIVE_ACCOUNT_QUOTA_SOURCE,
            acquired_at: acquired_at.to_owned(),
            availability: SnapshotAvailability::Available,
            bucket_identity: BucketIdentity::MainCodex,
            five_hour: meter(five, "2026-01-01T05:00:00Z"),
            weekly: meter(weekly, "2026-01-08T00:00:00Z"),
            warnings: Vec::<QuotaWarning>::new(),
            provenance: LIVE_ACCOUNT_QUOTA_PROVENANCE,
        }
    }

    #[test]
    fn file_backed_comparison_is_read_only_reproducible_and_has_bounded_output() {
        let database = database_path();
        let database_text = database.to_string_lossy().into_owned();
        let mut writer = SqliteStore::open(&database).unwrap();
        let before = writer
            .save_live_account_snapshot(&snapshot("2026-01-01T00:00:00Z", 25.0, 10.0))
            .unwrap();
        let after = writer
            .save_live_account_snapshot(&snapshot("2026-01-01T01:00:00Z", 29.0, 12.5))
            .unwrap();
        writer
            .connection()
            .execute(
                "INSERT INTO quota_snapshot_evidence (sample_id, source_id, session_id, observed_at, meter_type, used_percent, reset_at, plan, codex_version, evidence_status, schema_version, evidence_sha256) VALUES ('existing-rollout', 'local', NULL, '2026-01-01T00:00:00Z', 'five_hour', 5.0, NULL, NULL, NULL, 'observed', 1, ?1)",
                ["0".repeat(64)],
            )
            .unwrap();
        drop(writer);
        let bytes_before = fs::read(&database).unwrap();

        let human = crate::cli::run([
            "quota",
            "--compare-live",
            "--before-snapshot",
            before.snapshot.snapshot_id.as_str(),
            "--after-snapshot",
            after.snapshot.snapshot_id.as_str(),
            "--database",
            &database_text,
        ])
        .unwrap();
        assert!(human.contains("observed-compatible"));
        assert!(human.contains("+4 percentage points"));
        assert!(human.contains("+2.5 percentage points"));
        assert!(human.contains("Task causality: unverified"));
        assert!(human.contains("Account identity continuity: unverified"));

        let json = crate::cli::run([
            "quota",
            "--compare-live",
            "--before-snapshot",
            before.snapshot.snapshot_id.as_str(),
            "--after-snapshot",
            after.snapshot.snapshot_id.as_str(),
            "--database",
            &database_text,
            "--json",
        ])
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["schema_version"], "1.0.0");
        assert_eq!(value["evidence_scope"], "account_level");
        assert_eq!(
            value["five_hour"]["observed_movement_percentage_points"],
            4.0
        );
        assert_eq!(value["weekly"]["observed_movement_percentage_points"], 2.5);
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "credential",
            "providerSecret",
            "task_id",
            "task_delta",
            database_text.as_str(),
        ] {
            assert!(!json.contains(forbidden), "output contained {forbidden}");
        }

        let bytes_after = fs::read(&database).unwrap();
        assert_eq!(bytes_before, bytes_after);
        let mut reader = SqliteReadStore::open(&database).unwrap();
        assert_eq!(
            reader
                .live_account_snapshot(&before.snapshot.snapshot_id)
                .unwrap()
                .unwrap()
                .payload_sha256,
            before.snapshot.payload_sha256
        );
        drop(reader);
        let check = rusqlite::Connection::open_with_flags(
            &database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        let rollout_rows: i64 = check
            .query_row("SELECT count(*) FROM quota_snapshot_evidence", [], |row| {
                row.get(0)
            })
            .unwrap();
        let observations: i64 = check
            .query_row("SELECT count(*) FROM observations", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rollout_rows, 1);
        assert_eq!(observations, 0);
        remove_database(&database);
    }
}
