use std::path::{Path, PathBuf};

use crate::storage::{
    LiveAccountSnapshotHistoryPage, SqliteReadStore, StorageError, StoredLiveAccountSnapshot,
};

use super::args::{CliError, QuotaArguments};

pub(super) fn run(arguments: &QuotaArguments) -> Result<String, CliError> {
    let path = database_path(arguments.database.as_deref())?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            no_history_error()
        } else {
            database_open_error()
        }
    })?;
    if !metadata.is_file() {
        return Err(no_history_error());
    }
    let mut store = SqliteReadStore::open(&path).map_err(storage_error)?;
    let page = store
        .live_account_snapshot_history(arguments.limit, arguments.after.as_deref())
        .map_err(storage_error)?;
    if arguments.json {
        return encode_json(&page);
    }
    Ok(render(&page))
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

fn no_history_error() -> CliError {
    CliError {
        code: "live_quota_history_missing",
        message: "No saved live account snapshots found. Use `codex-meter quota --live --save` to save one.".into(),
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

fn encode_json(page: &LiveAccountSnapshotHistoryPage) -> Result<String, CliError> {
    serde_json::to_string(page)
        .map(|mut value| {
            value.push('\n');
            value
        })
        .map_err(|_| CliError {
            code: "serialization_failed",
            message: "live quota history could not be encoded".into(),
        })
}

fn render(page: &LiveAccountSnapshotHistoryPage) -> String {
    let mut output = String::from("Codex Meter — Saved live account quota history\n\n");
    output.push_str("Source: Codex App Server account rate limits\n");
    output.push_str("Evidence scope: account-level\n");
    output.push_str("Task attribution: not established\n");
    output.push_str("Forecast qualification: not established\n\n");
    if page.snapshots.is_empty() {
        output.push_str("No saved live account snapshots.\n");
        return output;
    }
    for (index, snapshot) in page.snapshots.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        render_snapshot(&mut output, snapshot);
    }
    if let Some(cursor) = &page.next_cursor {
        output.push_str(&format!(
            "\nMore snapshots are available. Continue with --after {cursor}.\n"
        ));
    }
    output
}

fn render_snapshot(output: &mut String, snapshot: &StoredLiveAccountSnapshot) {
    output.push_str(&format!(
        "Acquired: {}\nSnapshot ID: {}\nAvailability: {:?}\nBucket: {:?}\n",
        snapshot.acquired_at, snapshot.snapshot_id, snapshot.availability, snapshot.bucket_identity
    ));
    render_meter(output, "Five-hour", &snapshot.five_hour);
    render_meter(output, "Weekly", &snapshot.weekly);
    if snapshot.warnings.is_empty() {
        output.push_str("Warnings: none\n");
    } else {
        output.push_str(&format!("Warnings: {:?}\n", snapshot.warnings));
    }
    output.push_str(&format!(
        "Provenance: {} / {} / {}\n",
        snapshot.source, snapshot.evidence_scope, snapshot.task_attribution
    ));
}

fn render_meter(output: &mut String, label: &str, meter: &crate::storage::StoredLiveQuotaMeter) {
    output.push_str(&format!("{label}: {:?}", meter.availability));
    if let Some(used) = meter.used_percent {
        output.push_str(&format!("; used {used}% (observed)"));
    }
    if let Some(reset) = &meter.reset_at_utc {
        output.push_str(&format!("; reset {reset}"));
    }
    output.push('\n');
}
