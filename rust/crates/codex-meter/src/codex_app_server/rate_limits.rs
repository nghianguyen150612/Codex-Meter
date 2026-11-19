//! Privacy-filtered, account-level snapshots from `account/rateLimits/read`.

use super::{client::CodexAppServer, CodexAppError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

const METHOD: &str = "account/rateLimits/read";
const FIVE_HOUR_MINUTES: std::ops::RangeInclusive<i64> = 285..=315;
const WEEKLY_MINUTES: std::ops::RangeInclusive<i64> = 9_576..=10_584;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotAvailability {
    Available,
    Partial,
    Unavailable,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterAvailability {
    Available,
    Unavailable,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BucketIdentity {
    MainCodex,
    OtherCodexRelated,
    OtherBucket,
    Unknown,
    Conflicting,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWarning {
    NoQuotaData,
    MainBucketUnavailable,
    UnknownBucketIdentity,
    NonMainBucketsIgnored,
    ConflictingBucketViews,
    MissingFiveHour,
    MissingWeekly,
    MissingPercentage,
    MissingDuration,
    UnsupportedDuration,
    DuplicateFiveHour,
    DuplicateWeekly,
    MissingResetTimestamp,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeterSnapshot {
    pub availability: MeterAvailability,
    pub used_percent: Option<f64>,
    pub used_percent_provenance: &'static str,
    pub remaining_percent: Option<f64>,
    pub remaining_percent_provenance: &'static str,
    pub reset_at_utc: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountRateLimitsSnapshot {
    pub schema_version: &'static str,
    pub source: &'static str,
    pub acquired_at: String,
    pub availability: SnapshotAvailability,
    pub bucket_identity: BucketIdentity,
    pub five_hour: MeterSnapshot,
    pub weekly: MeterSnapshot,
    pub warnings: Vec<QuotaWarning>,
    pub provenance: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MeterKind {
    FiveHour,
    Weekly,
}

#[derive(Clone, Debug, PartialEq)]
struct Window {
    used_percent: Option<f64>,
    duration_minutes: Option<i64>,
    reset_at: Option<String>,
}

impl CodexAppServer {
    /// Reads one account-level quota snapshot. The RPC is read-only and never
    /// creates task-specific evidence.
    pub fn read_rate_limits(&self) -> Result<AccountRateLimitsSnapshot, CodexAppError> {
        let response = self.call(
            METHOD,
            json!({"supportsLunaReserve": false, "excludeResetCreditDetails": true}),
        )?;
        let acquired_at = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| CodexAppError::MalformedResponse)?;
        normalize_response(&response, acquired_at)
    }
}

fn normalize_response(
    value: &Value,
    acquired_at: String,
) -> Result<AccountRateLimitsSnapshot, CodexAppError> {
    let object = value.as_object().ok_or(CodexAppError::MalformedResponse)?;
    let mut warnings = Vec::new();
    let mut bucket_label = BucketIdentity::Unknown;
    let main = if let Some(map_value) = object
        .get("rateLimitsByLimitId")
        .filter(|value| !value.is_null())
    {
        let buckets = map_value
            .as_object()
            .ok_or(CodexAppError::MalformedResponse)?;
        let mut codex = buckets
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("codex"));
        let chosen = codex.next();
        if codex.next().is_some() {
            return Ok(ambiguous_snapshot(
                acquired_at,
                vec![QuotaWarning::ConflictingBucketViews],
            ));
        }
        if buckets.keys().any(|key| !key.eq_ignore_ascii_case("codex")) {
            warnings.push(QuotaWarning::NonMainBucketsIgnored);
        }
        match chosen {
            Some((key, bucket)) => {
                if bucket_identity(bucket)?.is_none_or(|id| id.eq_ignore_ascii_case(key)) {
                    bucket_label = BucketIdentity::MainCodex;
                    let legacy = object.get("rateLimits").filter(|entry| !entry.is_null());
                    if let Some(legacy) = legacy {
                        let legacy_identity = bucket_identity(legacy)?;
                        if legacy_identity.is_some_and(|id| !id.eq_ignore_ascii_case("codex"))
                            || parse_bucket(legacy)? != parse_bucket(bucket)?
                        {
                            return Ok(ambiguous_snapshot(
                                acquired_at,
                                vec![QuotaWarning::ConflictingBucketViews],
                            ));
                        }
                    }
                    Some(bucket)
                } else {
                    return Ok(ambiguous_snapshot(
                        acquired_at,
                        vec![QuotaWarning::ConflictingBucketViews],
                    ));
                }
            }
            None => {
                bucket_label = if buckets
                    .keys()
                    .any(|key| key.to_ascii_lowercase().starts_with("codex"))
                {
                    BucketIdentity::OtherCodexRelated
                } else if buckets.is_empty() {
                    BucketIdentity::Unknown
                } else {
                    BucketIdentity::OtherBucket
                };
                warnings.push(QuotaWarning::MainBucketUnavailable);
                if object
                    .get("rateLimits")
                    .is_some_and(|entry| !entry.is_null())
                {
                    return Ok(ambiguous_snapshot(
                        acquired_at,
                        vec![QuotaWarning::ConflictingBucketViews],
                    ));
                }
                None
            }
        }
    } else {
        match object.get("rateLimits").filter(|entry| !entry.is_null()) {
            Some(bucket) => match bucket_identity(bucket)? {
                Some(id) if id.eq_ignore_ascii_case("codex") => {
                    bucket_label = BucketIdentity::MainCodex;
                    Some(bucket)
                }
                Some(id) => {
                    warnings.push(QuotaWarning::NonMainBucketsIgnored);
                    bucket_label = if id.to_ascii_lowercase().starts_with("codex") {
                        BucketIdentity::OtherCodexRelated
                    } else {
                        BucketIdentity::OtherBucket
                    };
                    None
                }
                None => {
                    warnings.push(QuotaWarning::UnknownBucketIdentity);
                    bucket_label = BucketIdentity::Unknown;
                    None
                }
            },
            None => None,
        }
    };

    let Some(bucket) = main else {
        if warnings.is_empty() {
            warnings.push(QuotaWarning::NoQuotaData);
        }
        warnings.push(QuotaWarning::MissingFiveHour);
        warnings.push(QuotaWarning::MissingWeekly);
        return Ok(build_snapshot(
            acquired_at,
            SnapshotAvailability::Unavailable,
            bucket_label,
            unavailable_meter(),
            unavailable_meter(),
            warnings,
        ));
    };
    let (primary, secondary) = parse_bucket(bucket)?;
    let (five_hour, five_warnings) =
        meter_from_windows([primary.as_ref(), secondary.as_ref()], MeterKind::FiveHour)?;
    let (weekly, weekly_warnings) =
        meter_from_windows([primary.as_ref(), secondary.as_ref()], MeterKind::Weekly)?;
    warnings.extend(five_warnings);
    warnings.extend(weekly_warnings);
    let availability = match (five_hour.availability, weekly.availability) {
        (MeterAvailability::Ambiguous, _) | (_, MeterAvailability::Ambiguous) => {
            SnapshotAvailability::Ambiguous
        }
        (MeterAvailability::Available, MeterAvailability::Available) => {
            SnapshotAvailability::Available
        }
        (MeterAvailability::Unavailable, MeterAvailability::Unavailable) => {
            SnapshotAvailability::Unavailable
        }
        _ => SnapshotAvailability::Partial,
    };
    Ok(build_snapshot(
        acquired_at,
        availability,
        bucket_label,
        five_hour,
        weekly,
        warnings,
    ))
}

fn parse_bucket(value: &Value) -> Result<(Option<Window>, Option<Window>), CodexAppError> {
    let object = value.as_object().ok_or(CodexAppError::MalformedResponse)?;
    Ok((
        parse_window(object.get("primary"))?,
        parse_window(object.get("secondary"))?,
    ))
}

fn bucket_identity(value: &Value) -> Result<Option<&str>, CodexAppError> {
    let object = value.as_object().ok_or(CodexAppError::MalformedResponse)?;
    match object.get("limitId") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(identity)) => Ok(Some(identity)),
        _ => Err(CodexAppError::MalformedResponse),
    }
}

fn parse_window(value: Option<&Value>) -> Result<Option<Window>, CodexAppError> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let object = value.as_object().ok_or(CodexAppError::MalformedResponse)?;
    let used_percent = match object.get("usedPercent") {
        None | Some(Value::Null) => None,
        Some(number) => {
            let number = number.as_f64().ok_or(CodexAppError::MalformedResponse)?;
            if !number.is_finite() || number.fract() != 0.0 || !(0.0..=100.0).contains(&number) {
                return Err(CodexAppError::MalformedResponse);
            }
            Some(number)
        }
    };
    let duration_minutes = optional_i64(object.get("windowDurationMins"))?;
    let reset_at = match optional_i64(object.get("resetsAt"))? {
        Some(seconds) => Some(
            OffsetDateTime::from_unix_timestamp(seconds)
                .map_err(|_| CodexAppError::MalformedResponse)?
                .to_offset(UtcOffset::UTC)
                .format(&Rfc3339)
                .map_err(|_| CodexAppError::MalformedResponse)?,
        ),
        None => None,
    };
    Ok(Some(Window {
        used_percent,
        duration_minutes,
        reset_at,
    }))
}

fn optional_i64(value: Option<&Value>) -> Result<Option<i64>, CodexAppError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or(CodexAppError::MalformedResponse),
    }
}

fn meter_from_windows(
    windows: [Option<&Window>; 2],
    meter: MeterKind,
) -> Result<(MeterSnapshot, Vec<QuotaWarning>), CodexAppError> {
    let mut warnings = Vec::new();
    let mut matches = Vec::new();
    let mut unsupported = false;
    let mut missing_duration = false;
    for window in windows.into_iter().flatten() {
        match window.duration_minutes {
            None => missing_duration = true,
            Some(duration) => match match_meter(duration) {
                Some(found) if found == meter => matches.push(window),
                Some(_) => {}
                None => unsupported = true,
            },
        }
    }
    let duplicate_warning = match meter {
        MeterKind::FiveHour => QuotaWarning::DuplicateFiveHour,
        MeterKind::Weekly => QuotaWarning::DuplicateWeekly,
    };
    if matches.len() > 1 {
        warnings.push(duplicate_warning);
        return Ok((
            MeterSnapshot {
                availability: MeterAvailability::Ambiguous,
                used_percent: None,
                used_percent_provenance: "unavailable",
                remaining_percent: None,
                remaining_percent_provenance: "unavailable",
                reset_at_utc: None,
            },
            warnings,
        ));
    }
    let Some(window) = matches.first() else {
        warnings.push(match meter {
            MeterKind::FiveHour => QuotaWarning::MissingFiveHour,
            MeterKind::Weekly => QuotaWarning::MissingWeekly,
        });
        if missing_duration {
            warnings.push(QuotaWarning::MissingDuration);
        }
        if unsupported {
            warnings.push(QuotaWarning::UnsupportedDuration);
        }
        return Ok((unavailable_meter(), warnings));
    };
    let Some(used) = window.used_percent else {
        warnings.push(QuotaWarning::MissingPercentage);
        return Ok((unavailable_meter(), warnings));
    };
    if window.reset_at.is_none() {
        warnings.push(QuotaWarning::MissingResetTimestamp);
    }
    Ok((
        MeterSnapshot {
            availability: MeterAvailability::Available,
            used_percent: Some(used),
            used_percent_provenance: "observed",
            remaining_percent: Some(100.0 - used),
            remaining_percent_provenance: "derived",
            reset_at_utc: window.reset_at.clone(),
        },
        warnings,
    ))
}

fn match_meter(duration: i64) -> Option<MeterKind> {
    if FIVE_HOUR_MINUTES.contains(&duration) {
        Some(MeterKind::FiveHour)
    } else if WEEKLY_MINUTES.contains(&duration) {
        Some(MeterKind::Weekly)
    } else {
        None
    }
}

fn unavailable_meter() -> MeterSnapshot {
    MeterSnapshot {
        availability: MeterAvailability::Unavailable,
        used_percent: None,
        used_percent_provenance: "unavailable",
        remaining_percent: None,
        remaining_percent_provenance: "unavailable",
        reset_at_utc: None,
    }
}

fn ambiguous_snapshot(
    acquired_at: String,
    warnings: Vec<QuotaWarning>,
) -> AccountRateLimitsSnapshot {
    build_snapshot(
        acquired_at,
        SnapshotAvailability::Ambiguous,
        BucketIdentity::Conflicting,
        ambiguous_meter(),
        ambiguous_meter(),
        warnings,
    )
}

fn ambiguous_meter() -> MeterSnapshot {
    MeterSnapshot {
        availability: MeterAvailability::Ambiguous,
        used_percent: None,
        used_percent_provenance: "unavailable",
        remaining_percent: None,
        remaining_percent_provenance: "unavailable",
        reset_at_utc: None,
    }
}

fn build_snapshot(
    acquired_at: String,
    availability: SnapshotAvailability,
    bucket_identity: BucketIdentity,
    five_hour: MeterSnapshot,
    weekly: MeterSnapshot,
    warnings: Vec<QuotaWarning>,
) -> AccountRateLimitsSnapshot {
    AccountRateLimitsSnapshot { schema_version: "1.0.0", source: "codex_app_server_account_rate_limits", acquired_at, availability, bucket_identity, five_hour, weekly, warnings, provenance: "account_level_snapshot; task_attribution_not_established; forecast_qualification_not_established" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_not_slot_selects_five_hour_and_weekly() {
        let result = normalize_response(&json!({"rateLimitsByLimitId":{"codex":{"limitId":"codex","primary":{"usedPercent":8,"windowDurationMins":100,"resetsAt":2000000000},"secondary":{"usedPercent":0,"windowDurationMins":10080}}}}), "2026-11-18T00:00:00Z".into()).unwrap();
        assert_eq!(
            result.five_hour.availability,
            MeterAvailability::Unavailable
        );
        assert_eq!(result.weekly.used_percent, Some(0.0));
        assert_eq!(result.weekly.remaining_percent, Some(100.0));
        assert_eq!(result.weekly.reset_at_utc, None);
    }

    #[test]
    fn reversed_slots_missing_metadata_and_short_windows_are_not_guessed() {
        let reversed = normalize_response(
            &json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":20,"windowDurationMins":10080},"secondary":{"usedPercent":0,"windowDurationMins":300}}}),
            "now".into(),
        )
        .unwrap();
        assert_eq!(reversed.five_hour.used_percent, Some(0.0));
        assert_eq!(reversed.weekly.used_percent, Some(20.0));

        let missing_primary = normalize_response(
            &json!({"rateLimits":{"limitId":"codex","primary":null,"secondary":{"usedPercent":100,"windowDurationMins":10080}}}),
            "now".into(),
        )
        .unwrap();
        assert_eq!(missing_primary.weekly.used_percent, Some(100.0));
        assert_eq!(
            missing_primary.five_hour.availability,
            MeterAvailability::Unavailable
        );

        let missing_percentage = normalize_response(
            &json!({"rateLimits":{"limitId":"codex","primary":{"windowDurationMins":300}}}),
            "now".into(),
        )
        .unwrap();
        assert!(missing_percentage
            .warnings
            .contains(&QuotaWarning::MissingPercentage));
        assert_eq!(
            missing_percentage.five_hour.availability,
            MeterAvailability::Unavailable
        );

        let missing_duration = normalize_response(
            &json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":42}}}),
            "now".into(),
        )
        .unwrap();
        assert!(missing_duration
            .warnings
            .contains(&QuotaWarning::MissingDuration));

        let short_window = normalize_response(
            &json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":42,"windowDurationMins":15}}}),
            "now".into(),
        )
        .unwrap();
        assert!(short_window
            .warnings
            .contains(&QuotaWarning::UnsupportedDuration));
        assert_eq!(
            short_window.five_hour.availability,
            MeterAvailability::Unavailable
        );
    }

    #[test]
    fn main_bucket_legacy_and_multi_views_must_agree() {
        let one = json!({"limitId":"codex","primary":{"usedPercent":42,"windowDurationMins":300}});
        let result = normalize_response(
            &json!({"rateLimits":one,"rateLimitsByLimitId":{"codex":one}}),
            "now".into(),
        )
        .unwrap();
        assert_eq!(result.availability, SnapshotAvailability::Partial);
        assert_eq!(result.five_hour.availability, MeterAvailability::Available);
        let conflicting = normalize_response(&json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":43,"windowDurationMins":300}},"rateLimitsByLimitId":{"codex":one}}), "now".into()).unwrap();
        assert_eq!(conflicting.availability, SnapshotAvailability::Ambiguous);
    }

    #[test]
    fn rejects_bad_values_and_unknown_identity_without_disclosing_labels() {
        for percent in [-1, 101] {
            let response = json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":percent,"windowDurationMins":300}}});
            assert_eq!(
                normalize_response(&response, "now".into()).unwrap_err(),
                CodexAppError::MalformedResponse
            );
        }
        assert_eq!(
            normalize_response(
                &json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":42.5,"windowDurationMins":300}}}),
                "now".into()
            ).unwrap_err(),
            CodexAppError::MalformedResponse
        );
        let unknown = normalize_response(&json!({"rateLimits":{"primary":{"usedPercent":20,"windowDurationMins":300,"email":"private@example.invalid"},"accountId":"secret"}}), "now".into()).unwrap();
        assert_eq!(unknown.availability, SnapshotAvailability::Unavailable);
        let encoded = serde_json::to_string(&unknown).unwrap();
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("secret"));
    }

    #[test]
    fn initialized_client_uses_only_the_documented_read_rpc() {
        let client = CodexAppServer::connect_test_server("rate_limits").unwrap();
        let snapshot = client.read_rate_limits().unwrap();
        assert_eq!(snapshot.availability, SnapshotAvailability::Available);
        assert_eq!(snapshot.five_hour.used_percent, Some(42.0));
        assert_eq!(snapshot.weekly.used_percent, Some(0.0));
        let json = serde_json::to_string(&snapshot).unwrap();
        for forbidden in [
            "sensitive-account-id",
            "private@example.invalid",
            "never-log-this",
            "unknownProviderField",
            "limitName",
        ] {
            assert!(!json.contains(forbidden));
        }
    }

    #[test]
    fn legacy_read_preserves_observed_one_hundred_percent() {
        let client = CodexAppServer::connect_test_server("rate_limits_legacy").unwrap();
        let snapshot = client.read_rate_limits().unwrap();
        assert_eq!(snapshot.bucket_identity, BucketIdentity::MainCodex);
        assert_eq!(snapshot.five_hour.used_percent, Some(100.0));
        assert_eq!(snapshot.five_hour.remaining_percent, Some(0.0));
        assert_eq!(
            snapshot.five_hour.reset_at_utc.as_deref(),
            Some("2033-05-18T03:33:20Z")
        );
        assert_eq!(snapshot.weekly.availability, MeterAvailability::Unavailable);
    }

    #[test]
    fn typed_reader_classifies_unsupported_auth_malformed_and_oversized_responses() {
        for (mode, expected) in [
            (
                "rate_limits_unsupported",
                CodexAppError::UnsupportedMethod("account/rateLimits/read"),
            ),
            (
                "rate_limits_authentication",
                CodexAppError::AuthenticationUnavailable,
            ),
            ("rate_limits_malformed", CodexAppError::MalformedResponse),
            ("rate_limits_wrong_id", CodexAppError::MalformedResponse),
            ("rate_limits_wrong_type", CodexAppError::MalformedResponse),
            ("rate_limits_nonfinite", CodexAppError::MalformedResponse),
            (
                "rate_limits_invalid_reset",
                CodexAppError::MalformedResponse,
            ),
            ("rate_limits_oversized", CodexAppError::MalformedResponse),
        ] {
            let client = CodexAppServer::connect_test_server(mode).unwrap();
            let error = client.read_rate_limits().unwrap_err();
            assert_eq!(error, expected, "mode: {mode}");
            assert!(!error.to_string().contains("unauthorized"));
        }
    }

    #[test]
    fn non_main_only_and_missing_quota_remain_unavailable() {
        for mode in ["rate_limits_missing", "rate_limits_nonmain"] {
            let client = CodexAppServer::connect_test_server(mode).unwrap();
            let snapshot = client.read_rate_limits().unwrap();
            assert_eq!(snapshot.availability, SnapshotAvailability::Unavailable);
            let json = serde_json::to_string(&snapshot).unwrap();
            assert!(!json.contains("sensitive-account-id"));
            assert!(!json.contains("private@example.invalid"));
            if mode == "rate_limits_nonmain" {
                assert_eq!(snapshot.bucket_identity, BucketIdentity::OtherCodexRelated);
            }
        }
    }

    #[test]
    fn duplicate_meters_and_disagreeing_bucket_views_are_ambiguous() {
        for mode in ["rate_limits_duplicate", "rate_limits_conflicting_views"] {
            let client = CodexAppServer::connect_test_server(mode).unwrap();
            let snapshot = client.read_rate_limits().unwrap();
            assert_eq!(snapshot.availability, SnapshotAvailability::Ambiguous);
        }
    }
}
