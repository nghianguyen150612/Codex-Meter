//! Evidence inventory and a presentation of the validated P041 outcome.
use super::{CliError, ReadinessArguments};
use crate::analytics::{invoke_worker_with_mode, ProcessLimits, WorkerInvocation, WorkerOperation};
use crate::storage::{ReadinessLedgerCoverage, SqliteReadStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const HELP: &str = "Codex Meter — forecast evidence readiness\n\nUSAGE:\n    codex-meter readiness [--database PATH] [--request FILE] [--generated-at UTC_TIMESTAMP] [--five-hour-used-percent N] [--weekly-used-percent N] [--json]\n\nWithout a request this is an informational inventory, not a forecast.\nScoped mode accepts AnalyticsRequest v1. Meter positions are manual and unverified.\nWorker options: --python PATH, --worker-mode auto|source|installed.\n";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ObservationCoverage {
    candidate_terminal_count: u64,
    scan_limit_reached: bool,
    raw_total_available: u64,
    raw_total_missing: u64,
    known_configuration_dimensions: BTreeMap<String, u64>,
    token_validity: BTreeMap<String, u64>,
    token_quality: BTreeMap<String, u64>,
}

#[derive(Serialize)]
struct ReadinessResult {
    schema_version: &'static str,
    analysis_type: &'static str,
    mode: &'static str,
    generated_at: String,
    request_id: Option<String>,
    requested_selection: Option<Value>,
    status: &'static str,
    database_present: bool,
    source_compatibility: &'static str,
    observation_coverage: Option<ObservationCoverage>,
    ledger_coverage: Option<ReadinessLedgerCoverage>,
    scoped_coverage: Option<Value>,
    per_meter_readiness: Vec<Value>,
    diagnostics: Vec<&'static str>,
    next_action_codes: Vec<&'static str>,
    partial: bool,
}

pub fn run(arguments: ReadinessArguments) -> Result<String, CliError> {
    let mut analytics = arguments.analytics;
    if !crate::analytics::utc_timestamp(&analytics.generated_at) {
        return Err(CliError {
            code: "invalid_generated_at",
            message: "generated-at must be a valid UTC timestamp".into(),
        });
    }
    // Validate a scoped request before inspecting any evidence, including a missing DB.
    let request_bytes = if arguments.scoped {
        Some(super::workload::read_bounded_request(&analytics.request)?)
    } else {
        None
    };
    let request = if let Some(bytes) = &request_bytes {
        Some(
            crate::analytics::AnalyticsRequest::parse(bytes).map_err(|reason| CliError {
                code: reason.code,
                message: reason.message.into(),
            })?,
        )
    } else {
        None
    };
    for percent in [
        analytics.five_hour_used_percent,
        analytics.weekly_used_percent,
    ]
    .into_iter()
    .flatten()
    {
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return Err(CliError {
                code: "invalid_manual_meter_percentage",
                message: "meter percentage must be between 0 and 100".into(),
            });
        }
    }
    if let Some(selected) = &request {
        if (analytics.five_hour_used_percent.is_some()
            && !selected
                .target_quota_windows
                .contains(&crate::analytics::MeterType::FiveHour))
            || (analytics.weekly_used_percent.is_some()
                && !selected
                    .target_quota_windows
                    .contains(&crate::analytics::MeterType::Weekly))
        {
            return Err(CliError {
                code: "invalid_manual_meter_percentage",
                message: "manual position must match a requested meter".into(),
            });
        }
    }
    if let Some(selected) = &request {
        if selected
            .dataset_selector
            .configuration
            .values()
            .into_iter()
            .flatten()
            .any(|value| {
                !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
            })
        {
            return Err(CliError {
                code: "invalid_request",
                message: "readiness configuration metadata must use bounded safe labels".into(),
            });
        }
    }
    let json_output = analytics.json;
    let mut result = ReadinessResult {
        schema_version: "1.0.0",
        analysis_type: "forecast_readiness",
        mode: if arguments.scoped {
            "scoped"
        } else {
            "inventory"
        },
        generated_at: analytics.generated_at.clone(),
        request_id: request.as_ref().map(|item| item.request_id.clone()),
        requested_selection: request_bytes.as_ref().map(|bytes| {
            serde_json::from_slice::<Value>(bytes).expect("validated request")["dataset_selector"]
                .clone()
        }),
        status: "selection_required",
        database_present: analytics.database.is_file(),
        source_compatibility: "unavailable",
        observation_coverage: None,
        ledger_coverage: None,
        scoped_coverage: None,
        per_meter_readiness: vec![],
        diagnostics: vec![],
        next_action_codes: vec![],
        partial: false,
    };
    if !result.database_present {
        result.status = "unavailable";
        result.diagnostics.push("database_missing");
        result.next_action_codes.push("select_existing_database");
    } else {
        let mut store = SqliteReadStore::open(&analytics.database).map_err(|reason| CliError {
            code: reason.code(),
            message: "readiness database validation failed".into(),
        })?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        store
            .cancel_reporting_when(move || std::time::Instant::now() >= deadline)
            .map_err(|reason| CliError {
                code: reason.code(),
                message: "readiness scan setup failed".into(),
            })?;
        let ledger = store.readiness_coverage().map_err(|reason| CliError {
            code: reason.code(),
            message: "readiness ledger integrity or bounded scan failed".into(),
        })?;
        if ledger.canonical_response_count == 0 {
            result.diagnostics.push("response_evidence_absent");
        }
        if ledger.quota_snapshot_count == 0 {
            result.diagnostics.push("quota_snapshots_absent");
        }
        if ledger.unattributed_response_count > 0 {
            result.diagnostics.push("task_attribution_incomplete");
        }
        if ledger.raw_total_missing > 0 {
            result.diagnostics.push("response_raw_totals_missing");
        }
        result.partial |= ledger.scan_limit_reached || !ledger.diagnostics.is_empty();
        result.source_compatibility = if ledger
            .diagnostics
            .keys()
            .any(|code| code.contains("unsupported"))
        {
            "unsupported_source"
        } else {
            "supported_recorded_schema"
        };
        result.ledger_coverage = Some(ledger);
        drop(store);
        if arguments.scoped {
            analytics.json = true;
            let selected = request.as_ref().ok_or_else(invalid_result)?;
            for meter in &selected.target_quota_windows {
                let mut single: Value =
                    serde_json::from_slice(request_bytes.as_ref().ok_or_else(invalid_result)?)
                        .map_err(|_| invalid_result())?;
                single["target_quota_windows"] = json!([meter.as_str()]);
                let bytes = serde_json::to_vec(&single).map_err(|_| invalid_result())?;
                let mut context = analytics.clone();
                if meter.as_str() == "five_hour" {
                    context.weekly_used_percent = None;
                } else {
                    context.five_hour_used_percent = None;
                }
                match super::workload::run_request(context, &bytes) {
                    Ok(output) => {
                        // run() has already performed closed typed and semantic validation
                        // against the original request, timestamps and manual positions.
                        let value: Value =
                            serde_json::from_str(&output).map_err(|_| invalid_result())?;
                        let distribution = &value["task_size_distribution"];
                        if distribution["accounting"]["candidate_observations"] == 0 {
                            result.diagnostics.push("selected_observations_absent");
                        }
                        let mut reasons = BTreeMap::<String, u64>::new();
                        for decision in distribution["decisions"]
                            .as_array()
                            .ok_or_else(invalid_result)?
                        {
                            if let Some(items) = decision["reasons"].as_array() {
                                for reason in items {
                                    *reasons
                                        .entry(reason.as_str().ok_or_else(invalid_result)?.into())
                                        .or_default() += 1;
                                }
                            }
                        }
                        let scoped_coverage = Some(
                            json!({"configuration": value["dataset_configuration"], "accounting": distribution["accounting"], "history_status": distribution["status"], "exclusion_reasons": reasons}),
                        );
                        if result.scoped_coverage.is_some()
                            && result.scoped_coverage != scoped_coverage
                        {
                            return Err(invalid_result());
                        }
                        result.scoped_coverage = scoped_coverage;
                        for prediction in value["meter_predictions"]
                            .as_array()
                            .ok_or_else(invalid_result)?
                        {
                            result.per_meter_readiness.push(json!({
                            "meter_type": prediction["meter_type"],
                            "status": if prediction["status"] == "available" { json!("ready") } else { prediction["status"].clone() },
                            "insufficiency_reasons": prediction["insufficiency_reasons"],
                            "capacity_reason_codes": prediction["capacity_reason_codes"],
                            "capacity_confidence": prediction["capacity_confidence"],
                            "capacity_sample_accounting": value["capacity_result"]["quota_capacity_estimates"][0]["sample_accounting"],
                            "manual_position_present": if prediction["meter_type"] == "five_hour" { analytics.five_hour_used_percent.is_some() } else { analytics.weekly_used_percent.is_some() }
                        }));
                        }
                        result.status = if result
                            .per_meter_readiness
                            .iter()
                            .all(|item| item["status"] == "ready")
                            && !result.per_meter_readiness.is_empty()
                        {
                            "ready"
                        } else {
                            "insufficient_evidence"
                        };
                        if result.status != "ready" {
                            result
                                .next_action_codes
                                .push("qualified_observation_integration_required");
                        }
                    }
                    Err(reason) if reason.code == "current_quota_position_required" => {
                        result.status = "unavailable";
                        result.diagnostics.push("current_quota_position_required");
                        result.per_meter_readiness.push(json!({"meter_type": meter.as_str(), "status": "unavailable", "insufficiency_reasons": ["current_quota_position_required"], "capacity_reason_codes": [], "capacity_confidence": null, "capacity_sample_accounting": null, "manual_position_present": false}));
                        result
                            .next_action_codes
                            .push("supply_manual_meter_position");
                    }
                    Err(reason) => return Err(reason),
                }
            }
            result.status = if result.per_meter_readiness.is_empty() {
                "selection_required"
            } else if result
                .per_meter_readiness
                .iter()
                .all(|item| item["status"] == "ready")
            {
                "ready"
            } else {
                "insufficient_evidence"
            };
        } else {
            super::estimate::validate_python_executable(&analytics.python)?;
            let database = super::estimate::absolute_database_path(&analytics.database)?;
            let bytes = invoke_worker_with_mode(
                WorkerInvocation {
                    python: &analytics.python,
                    worker_mode: analytics.worker_mode,
                    database_path: &database,
                    generated_at: &analytics.generated_at,
                    five_hour_used_percent: None,
                    weekly_used_percent: None,
                    operation: WorkerOperation::Inventory,
                },
                b"{}",
                ProcessLimits::default(),
            )
            .map_err(|reason| CliError {
                code: reason.code,
                message: reason.message.into(),
            })?;
            let coverage: ObservationCoverage =
                serde_json::from_slice(&bytes).map_err(|_| invalid_result())?;
            if coverage.candidate_terminal_count > 1000
                || coverage
                    .raw_total_available
                    .checked_add(coverage.raw_total_missing)
                    != Some(coverage.candidate_terminal_count)
                || coverage.token_validity.keys().any(|key| {
                    !["valid", "incomplete", "invalid", "unavailable"].contains(&key.as_str())
                })
                || coverage
                    .token_quality
                    .keys()
                    .any(|key| !["A", "B", "C", "D", "X"].contains(&key.as_str()))
                || coverage
                    .token_validity
                    .values()
                    .try_fold(0_u64, |sum, n| sum.checked_add(*n))
                    != Some(coverage.candidate_terminal_count)
                || coverage
                    .token_quality
                    .values()
                    .try_fold(0_u64, |sum, n| sum.checked_add(*n))
                    != Some(coverage.candidate_terminal_count)
            {
                return Err(invalid_result());
            }
            if coverage.known_configuration_dimensions.len() != 5
                || coverage
                    .known_configuration_dimensions
                    .iter()
                    .any(|(key, n)| {
                        !crate::analytics::CONFIGURATION_FIELDS.contains(&key.as_str())
                            || *n > coverage.candidate_terminal_count
                    })
            {
                return Err(invalid_result());
            }
            if coverage.candidate_terminal_count == 0 {
                result.diagnostics.push("qualified_observations_absent");
            }
            if coverage.raw_total_missing > 0 {
                result.diagnostics.push("observation_raw_totals_missing");
            }
            result.partial |= coverage.scan_limit_reached;
            result.observation_coverage = Some(coverage);
            result
                .next_action_codes
                .push("select_configuration_and_meters");
        }
    }
    if result.status == "selection_required" && arguments.scoped {
        result
            .next_action_codes
            .push("select_configuration_and_meters");
    }
    if result.per_meter_readiness.is_empty() && result.status == "unavailable" {
        if let Some(request) = &request {
            for meter in &request.target_quota_windows {
                result.per_meter_readiness.push(json!({"meter_type": meter.as_str(), "status": "unavailable", "insufficiency_reasons": result.diagnostics, "capacity_reason_codes": [], "capacity_confidence": null, "capacity_sample_accounting": null, "manual_position_present": if meter.as_str() == "five_hour" { analytics.five_hour_used_percent.is_some() } else { analytics.weekly_used_percent.is_some() }}));
            }
        }
    }
    if result.partial {
        result.diagnostics.push("partial_source_coverage");
    }
    result.diagnostics.sort_unstable();
    result.diagnostics.dedup();
    result.next_action_codes.sort_unstable();
    result.next_action_codes.dedup();
    let output = serde_json::to_string_pretty(&result).map_err(|_| invalid_result())?;
    if json_output {
        return Ok(output + "\n");
    }
    let mut human = format!(
        "Codex Meter — Forecast Readiness\nMode: {}\nStatus: {}\nSource compatibility: {}\n",
        result.mode, result.status, result.source_compatibility
    );
    if let Some(coverage) = &result.ledger_coverage {
        human.push_str(&format!("P031 response evidence: {}\nTask attribution: {} attributed, {} unattributed\nDirect raw totals: {} available, {} missing\nQuota snapshots: {}\n", coverage.canonical_response_count, coverage.attributed_response_count, coverage.unattributed_response_count, coverage.raw_total_available, coverage.raw_total_missing, coverage.quota_snapshot_count));
        for (code, count) in &coverage.diagnostics {
            human.push_str(&format!("Source warning: {code} ({count})\n"));
        }
    }
    if let Some(coverage) = &result.observation_coverage {
        human.push_str(&format!("P017 terminal candidates: {}\nP017 raw totals: {} available, {} missing\nP017 task-history eligibility: selection required\nP020 capacity: selection required\n", coverage.candidate_terminal_count, coverage.raw_total_available, coverage.raw_total_missing));
    }
    if let Some(coverage) = &result.scoped_coverage {
        human.push_str(&format!("Selected configuration: {}\nP017 selected history: {}\nHistory coverage: {}\nExclusion reasons: {}\n", coverage["configuration"], coverage["history_status"], coverage["accounting"], coverage["exclusion_reasons"]));
    }
    for meter in &result.per_meter_readiness {
        human.push_str(&format!("{} forecast: {}\n  P020 confidence: {}; reasons: {}\n  P022 reasons: {}\n  Manual unverified position present: {}\n", meter["meter_type"], meter["status"], meter["capacity_confidence"], meter["capacity_reason_codes"], meter["insufficiency_reasons"], meter["manual_position_present"]));
    }
    for code in &result.diagnostics {
        human.push_str(&format!("Diagnostic: {code}\n"));
    }
    for code in &result.next_action_codes {
        let explanation = match *code {
            "select_existing_database" => "Select an existing evidence database; this command does not create one.",
            "select_configuration_and_meters" => "Supply AnalyticsRequest v1 with an exact configuration and target meters.",
            "supply_manual_meter_position" => "Supply an explicit current used percentage for the blocked meter; it remains manual and unverified.",
            _ => "Qualified historical Observation and/or capacity evidence is insufficient. Complete P017 collection is not integrated by sync; further evidence integration may be required.",
        };
        human.push_str(&format!("Next action ({code}): {explanation}\n"));
    }
    if result.mode == "inventory" {
        human.push_str(
            "Five-hour forecast: selection required\nWeekly forecast: selection required\n",
        );
    }

    human.push_str("\nP031 records directly observed response tokens; P039 attribution does not establish P017 lifecycle or quality. P020 capacity is an empirical raw-token-equivalent estimate, never an official allowance. Manual positions are unverified.\nInventory does not determine configuration-specific eligibility. sync collects supported P031 evidence; complete P017 Observation generation is not integrated by sync. Qualified Observation integration may require a future milestone.\n");
    Ok(human)
}
fn invalid_result() -> CliError {
    CliError {
        code: "invalid_readiness_result",
        message: "readiness result failed validation".into(),
    }
}
