use super::args::{CliError, QuotaArguments};
use crate::codex_app_server::{
    AccountRateLimitsSnapshot, CodexAppError, CodexAppServer, MeterSnapshot,
};

pub(super) fn run(arguments: &QuotaArguments) -> Result<String, CliError> {
    let client = CodexAppServer::connect().map_err(app_server_error)?;
    let snapshot = client.read_rate_limits().map_err(app_server_error)?;
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
    Ok(render(&snapshot))
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
}
