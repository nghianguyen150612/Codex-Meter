use std::path::{Path, PathBuf};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::storage::{
    ActivityDeclaration, ControlledMeasurementSession, MeasurementSessionState, SqliteReadStore,
    SqliteStore, StorageError,
};
use crate::telemetry::{evaluate_live_quota_comparison, LiveQuotaComparison};

use super::args::{CliError, QuotaMeasureCommand};

#[derive(serde::Serialize)]
struct MeasurementView<'a> {
    schema_version: &'static str,
    session: &'a ControlledMeasurementSession,
    deadline_passed: Option<bool>,
    evaluator_contract_version: Option<&'static str>,
    local_consistency: &'static str,
    production_semantics: &'static str,
    acquisition_interval_seconds: Option<i64>,
    comparison: Option<LiveQuotaComparison>,
    limitations: [&'static str; 9],
}

const LIMITATIONS: [&str; 9] = [
    "task causality is unverified",
    "actual workload execution interval is not independently verified",
    "external account activity is unverified",
    "authenticated account identity continuity is unverified",
    "provider quota accuracy and token event coverage are unverified",
    "controlled benchmark isolation is absent",
    "official token allowance is unavailable",
    "self-reported activity is not independently verified isolation",
    "estimator qualification is not established",
];

pub(super) fn run(command: &QuotaMeasureCommand) -> Result<String, CliError> {
    match command {
        QuotaMeasureCommand::Start {
            before_snapshot,
            database,
            duration_minutes,
            json,
        } => {
            let path = database_path(database.as_deref())?;
            let mut store = SqliteStore::open(&path).map_err(storage_error)?;
            let session = store
                .start_controlled_measurement(before_snapshot, *duration_minutes)
                .map_err(storage_error)?;
            session_output(&session, None, None, *json)
        }
        QuotaMeasureCommand::Finish {
            session_id,
            after_snapshot,
            activity,
            database,
            json,
        } => {
            let path = database_path(database.as_deref())?;
            let mut store = SqliteStore::open(&path).map_err(storage_error)?;
            let activity = parse_activity(activity)?;
            let session = store
                .finish_controlled_measurement(session_id, after_snapshot, activity)
                .map_err(storage_error)?;
            drop(store);
            completed_status(&path, &session, *json)
        }
        QuotaMeasureCommand::Status {
            session_id,
            database,
            json,
        } => {
            let path = database_path(database.as_deref())?;
            ensure_existing_database(&path)?;
            let mut store = SqliteReadStore::open(&path).map_err(storage_error)?;
            let session = store
                .controlled_measurement_session(session_id)
                .map_err(storage_error)?
                .ok_or_else(session_missing)?;
            let (comparison, acquisition_interval_seconds) = if let Some((before, after)) = store
                .controlled_measurement_snapshots(&session)
                .map_err(storage_error)?
            {
                let comparison = evaluate_live_quota_comparison(&before, &after)
                    .map_err(|_| corrupt_session())?;
                let elapsed = (parse_time(&comparison.after_acquired_at_utc)?
                    - parse_time(&comparison.before_acquired_at_utc)?)
                .whole_seconds();
                (Some(comparison), Some(elapsed))
            } else {
                (None, None)
            };
            session_output(&session, comparison, acquisition_interval_seconds, *json)
        }
        QuotaMeasureCommand::Abort {
            session_id,
            database,
            json,
        } => {
            let path = database_path(database.as_deref())?;
            ensure_existing_database(&path)?;
            let mut store = SqliteStore::open(&path).map_err(storage_error)?;
            let session = store
                .abort_controlled_measurement(session_id)
                .map_err(storage_error)?;
            session_output(&session, None, None, *json)
        }
    }
}

fn completed_status(
    path: &Path,
    session: &ControlledMeasurementSession,
    json: bool,
) -> Result<String, CliError> {
    let mut read = SqliteReadStore::open(path).map_err(storage_error)?;
    let (before, after) = read
        .controlled_measurement_snapshots(session)
        .map_err(storage_error)?
        .ok_or_else(corrupt_session)?;
    let comparison =
        evaluate_live_quota_comparison(&before, &after).map_err(|_| corrupt_session())?;
    let span = parse_time(&after.acquired_at)? - parse_time(&before.acquired_at)?;
    session_output(session, Some(comparison), Some(span.whole_seconds()), json)
}

fn session_output(
    session: &ControlledMeasurementSession,
    comparison: Option<LiveQuotaComparison>,
    acquisition_interval_seconds: Option<i64>,
    json: bool,
) -> Result<String, CliError> {
    let deadline_passed = (session.state == MeasurementSessionState::Open).then(|| {
        parse_time(&session.deadline_utc).is_ok_and(|deadline| OffsetDateTime::now_utc() > deadline)
    });
    let view = MeasurementView {
        schema_version: "1.0.0",
        session,
        deadline_passed,
        evaluator_contract_version: (session.state == MeasurementSessionState::Completed)
            .then_some(crate::storage::EVALUATOR_CONTRACT_VERSION),
        local_consistency: if session.state == MeasurementSessionState::Completed {
            "snapshot_checksums_and_terminal_receipt_verified"
        } else {
            "before_snapshot_checksum_verified"
        },
        production_semantics: "unverified",
        acquisition_interval_seconds,
        comparison,
        limitations: LIMITATIONS,
    };
    if json {
        return serde_json::to_string(&view)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|_| CliError {
                code: "serialization_failed",
                message: "measurement result could not be encoded".into(),
            });
    }
    let mut out = String::from("Codex Meter — Controlled Quota Measurement v1\n\n");
    out.push_str(&format!(
        "Session: {}\nStatus: {}\nBefore snapshot: {}\n",
        session.session_id,
        state_label(session.state),
        session.before_snapshot_id
    ));
    if let Some(after) = &session.after_snapshot_id {
        out.push_str(&format!("After snapshot:  {after}\n"));
    }
    out.push_str(&format!(
        "Started: {}\nDeadline: {}\nDuration policy: {} minutes\n",
        session.created_at_utc, session.deadline_utc, session.max_duration_minutes
    ));
    if let Some(completed_at) = &session.completed_at_utc {
        out.push_str(&format!("Closed: {completed_at}\n"));
    }
    if session.state == MeasurementSessionState::Open {
        out.push_str(&format!(
            "Deadline passed: {}\n",
            if deadline_passed.unwrap_or(false) {
                "yes; run abort to close as expired"
            } else {
                "no"
            }
        ));
    }
    if let Some(seconds) = acquisition_interval_seconds {
        out.push_str(&format!(
            "Elapsed acquisition interval: {} seconds\n",
            seconds
        ));
    }
    if let Some(activity) = session.activity_declaration {
        out.push_str(&format!(
            "Activity declaration: {}\n",
            activity_label(activity)
        ));
    }
    out.push_str(&format!(
        "Evidence scope: {}\nTask attribution: {}\n",
        session.evidence_scope, session.task_attribution
    ));
    out.push_str("Source: saved Codex App Server account-level snapshots\n");
    out.push_str(&format!(
        "Local consistency: {}\nProduction quota semantics: unverified\n",
        view.local_consistency
    ));
    if let Some(comparison) = &view.comparison {
        out.push_str(&format!(
            "Before acquired: {}\nAfter acquired:  {}\n",
            comparison.before_acquired_at_utc, comparison.after_acquired_at_utc
        ));
        render_meter(&mut out, "Five-hour", &comparison.five_hour);
        render_meter(&mut out, "Weekly", &comparison.weekly);
    }
    for limitation in LIMITATIONS {
        out.push_str(&format!("{}\n", title_first(limitation)));
    }
    out.push_str("Account movement is not task consumption.\n");
    Ok(out)
}

fn render_meter(out: &mut String, label: &str, meter: &crate::telemetry::MeterComparison) {
    out.push_str(&format!(
        "\n{label}: {:?}; window continuity {:?}; reset evidence {:?}\n",
        meter.status, meter.window_continuity, meter.reset_evidence
    ));
    match meter.observed_movement_percentage_points {
        Some(movement) => out.push_str(&format!(
            "  Observed account movement: {movement:+} percentage points\n"
        )),
        None => out.push_str("  Observed account movement: unavailable\n"),
    }
}
fn state_label(state: MeasurementSessionState) -> &'static str {
    match state {
        MeasurementSessionState::Open => "open",
        MeasurementSessionState::Completed => "completed",
        MeasurementSessionState::Aborted => "aborted",
        MeasurementSessionState::Expired => "expired",
    }
}
fn activity_label(value: ActivityDeclaration) -> &'static str {
    match value {
        ActivityDeclaration::Unknown => "unknown",
        ActivityDeclaration::KnownLocalOverlap => "known local overlap",
        ActivityDeclaration::SelfReportedNoKnownOverlap => {
            "self-reported no known overlap (unverified)"
        }
    }
}
fn title_first(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}
fn parse_activity(value: &str) -> Result<ActivityDeclaration, CliError> {
    match value {
        "unknown" => Ok(ActivityDeclaration::Unknown),
        "known_local_overlap" => Ok(ActivityDeclaration::KnownLocalOverlap),
        "self_reported_no_known_overlap" => Ok(ActivityDeclaration::SelfReportedNoKnownOverlap),
        _ => Err(CliError {
            code: "invalid_activity_declaration",
            message: "activity declaration is invalid".into(),
        }),
    }
}
fn parse_time(value: &str) -> Result<OffsetDateTime, CliError> {
    OffsetDateTime::parse(value, &Rfc3339).map_err(|_| corrupt_session())
}
fn database_path(explicit: Option<&Path>) -> Result<PathBuf, CliError> {
    explicit.map(Path::to_path_buf).map(Ok).unwrap_or_else(|| {
        super::default_database_path().map_err(|_| CliError {
            code: "database_location_unavailable",
            message: "could not determine the Codex Meter database location".into(),
        })
    })
}
fn ensure_existing_database(path: &Path) -> Result<(), CliError> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(()),
        _ => Err(CliError {
            code: "measurement_database_missing",
            message: "an existing measurement database is required".into(),
        }),
    }
}
fn storage_error(error: StorageError) -> CliError {
    CliError {
        code: error.code(),
        message: error.to_string(),
    }
}
fn session_missing() -> CliError {
    CliError {
        code: "measurement_not_found",
        message: "measurement session was not found".into(),
    }
}
fn corrupt_session() -> CliError {
    CliError {
        code: "database_corrupt",
        message: "measurement session or referenced snapshots failed integrity validation".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{
        AccountRateLimitsSnapshot, BucketIdentity, MeterAvailability, MeterSnapshot, QuotaWarning,
        SnapshotAvailability,
    };
    use crate::storage::LIVE_ACCOUNT_QUOTA_PROVENANCE;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DB: AtomicU64 = AtomicU64::new(0);
    fn db() -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-meter-measure-cli-{}-{}.sqlite",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ))
    }
    fn cleanup(path: &Path) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }
    fn snapshot(
        at: String,
        percent: f64,
        five_hour_reset: &str,
        weekly_reset: &str,
    ) -> AccountRateLimitsSnapshot {
        let meter = |used: f64, reset: &str| MeterSnapshot {
            availability: MeterAvailability::Available,
            used_percent: Some(used),
            used_percent_provenance: "observed",
            remaining_percent: Some(100.0 - used),
            remaining_percent_provenance: "derived",
            reset_at_utc: Some(reset.into()),
        };
        AccountRateLimitsSnapshot {
            schema_version: "1.0.0",
            source: crate::storage::LIVE_ACCOUNT_QUOTA_SOURCE,
            acquired_at: at,
            availability: SnapshotAvailability::Available,
            bucket_identity: BucketIdentity::MainCodex,
            five_hour: meter(percent, five_hour_reset),
            weekly: meter(percent, weekly_reset),
            warnings: Vec::<QuotaWarning>::new(),
            provenance: LIVE_ACCOUNT_QUOTA_PROVENANCE,
        }
    }
    fn timestamp(at: OffsetDateTime) -> String {
        at.format(&Rfc3339).unwrap()
    }

    #[test]
    fn cli_start_status_finish_json_and_abort_use_explicit_saved_snapshots() {
        let reference = OffsetDateTime::now_utc();
        let five_hour_reset = timestamp(reference + time::Duration::hours(4));
        let weekly_reset = timestamp(reference + time::Duration::days(4));
        let path = db();
        let path_text = path.to_string_lossy().to_string();
        let before = {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .save_live_account_snapshot(&snapshot(
                    timestamp(reference - time::Duration::seconds(30)),
                    10.0,
                    &five_hour_reset,
                    &weekly_reset,
                ))
                .unwrap()
                .snapshot
                .snapshot_id
        };
        let start = crate::cli::run([
            "quota",
            "measure",
            "start",
            "--before-snapshot",
            &before,
            "--confirm-account-scope",
            "--database",
            &path_text,
            "--json",
        ])
        .unwrap();
        let start_json: serde_json::Value = serde_json::from_str(&start).unwrap();
        let session_id = start_json["session"]["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(start_json["schema_version"], "1.0.0");
        assert_eq!(start_json["session"]["state"], "open");
        let bytes_before_status = std::fs::read(&path).unwrap();
        let status = crate::cli::run([
            "quota",
            "measure",
            "status",
            &session_id,
            "--database",
            &path_text,
            "--json",
        ])
        .unwrap();
        let status_json: serde_json::Value = serde_json::from_str(&status).unwrap();
        assert_eq!(status_json["session"]["before_snapshot_id"], before);
        assert!(status_json["comparison"].is_null());
        assert_eq!(std::fs::read(&path).unwrap(), bytes_before_status);
        let after = {
            let mut store = SqliteStore::open(&path).unwrap();
            store
                .save_live_account_snapshot(&snapshot(
                    start_json["session"]["created_at_utc"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                    13.0,
                    &five_hour_reset,
                    &weekly_reset,
                ))
                .unwrap()
                .snapshot
                .snapshot_id
        };
        let finished = crate::cli::run([
            "quota",
            "measure",
            "finish",
            &session_id,
            "--after-snapshot",
            &after,
            "--activity",
            "self_reported_no_known_overlap",
            "--confirm-account-scope",
            "--database",
            &path_text,
            "--json",
        ])
        .unwrap();
        let result: serde_json::Value = serde_json::from_str(&finished).unwrap();
        assert_eq!(result["session"]["state"], "completed");
        assert_eq!(
            result["session"]["activity_declaration"],
            "self_reported_no_known_overlap"
        );
        assert_eq!(
            result["comparison"]["five_hour"]["observed_movement_percentage_points"],
            3.0
        );
        assert!(finished.contains("task causality is unverified"));
        let terminal_bytes = std::fs::read(&path).unwrap();
        let inspection = crate::cli::run([
            "quota",
            "measure",
            "status",
            &session_id,
            "--database",
            &path_text,
        ])
        .unwrap();
        assert!(inspection.contains("unverified"));
        assert_eq!(std::fs::read(&path).unwrap(), terminal_bytes);
        let abort_id = {
            let mut store = SqliteStore::open(&path).unwrap();
            let new_before = store
                .save_live_account_snapshot(&snapshot(
                    timestamp(reference),
                    14.0,
                    &five_hour_reset,
                    &weekly_reset,
                ))
                .unwrap()
                .snapshot
                .snapshot_id;
            drop(store);
            let text = crate::cli::run([
                "quota",
                "measure",
                "start",
                "--before-snapshot",
                &new_before,
                "--confirm-account-scope",
                "--database",
                &path_text,
            ])
            .unwrap();
            text.lines()
                .find_map(|line| line.strip_prefix("Session: "))
                .unwrap()
                .to_owned()
        };
        let aborted = crate::cli::run([
            "quota",
            "measure",
            "abort",
            &abort_id,
            "--database",
            &path_text,
            "--json",
        ])
        .unwrap();
        let aborted_json: serde_json::Value = serde_json::from_str(&aborted).unwrap();
        assert_eq!(aborted_json["session"]["state"], "aborted");
        cleanup(&path);
    }
}
