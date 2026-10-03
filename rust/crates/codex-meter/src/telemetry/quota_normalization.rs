//! Normalization of Codex token-count rate-limit snapshots into v1 quota samples.

use std::fmt;

use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use super::codex_rollout::{EventMessage, RateLimitSnapshot, RateLimitWindow, RolloutRecordKind};
use super::identity;
use super::incremental_jsonl::{ReadItem, ReadItemOutcome, RejectedLineReason, SourceIdentity};
use super::normalized::{
    AcquisitionStatus, ConfigurationIdentity, ConfigurationValue, NormalizedQuotaSample,
    PercentageMetric, QuotaIdentityConfidence, QuotaIdentityProvenance, QuotaMeterType,
    QuotaSourceKind, QuotaWindowIdentity, SchemaVersion,
};
use super::token_normalization::{validate_timestamp, TimestampErrorReason};

const FIVE_HOUR_MINUTES_MIN: i64 = 285;
const FIVE_HOUR_MINUTES_MAX: i64 = 315;
const WEEKLY_MINUTES_MIN: i64 = 9_576;
const WEEKLY_MINUTES_MAX: i64 = 10_584;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaNormalizationOutcome {
    Samples(Vec<NormalizedQuotaSample>),
    NoQuotaEvidence,
    NonMainLimit,
    UnsupportedWindows,
    AmbiguousMeter { meter_type: QuotaMeterType },
    RejectedSourceLine { reason: RejectedLineReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaNormalizationError {
    InvalidTimestamp(TimestampErrorReason),
    InvalidResetTimestamp,
}

impl fmt::Display for QuotaNormalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimestamp(reason) => {
                write!(
                    formatter,
                    "source timestamp is not valid UTC RFC 3339: {reason}"
                )
            }
            Self::InvalidResetTimestamp => {
                formatter.write_str("provider reset timestamp is outside the supported range")
            }
        }
    }
}

impl std::error::Error for QuotaNormalizationError {}

/// Normalizes only supported main Codex quota windows from one rollout item.
pub fn normalize_quota_item(
    source: &SourceIdentity,
    item: &ReadItem,
) -> Result<QuotaNormalizationOutcome, QuotaNormalizationError> {
    let record = match &item.outcome {
        ReadItemOutcome::Decoded(record) => record.as_ref(),
        ReadItemOutcome::Rejected(rejected) => {
            return Ok(QuotaNormalizationOutcome::RejectedSourceLine {
                reason: rejected.reason.clone(),
            });
        }
    };

    let rate_limits = match &record.kind {
        RolloutRecordKind::EventMsg(EventMessage::TokenCount(event)) => event.rate_limits.as_ref(),
        _ => return Ok(QuotaNormalizationOutcome::NoQuotaEvidence),
    };
    let Some(rate_limits) = rate_limits else {
        return Ok(QuotaNormalizationOutcome::NoQuotaEvidence);
    };
    if !is_main_limit(rate_limits) {
        return Ok(QuotaNormalizationOutcome::NonMainLimit);
    }

    validate_timestamp(&record.timestamp).map_err(QuotaNormalizationError::InvalidTimestamp)?;
    let windows = [
        ("primary", rate_limits.primary),
        ("secondary", rate_limits.secondary),
    ];
    let classified: Vec<_> = windows
        .into_iter()
        .filter_map(|(slot, window)| {
            window.and_then(|window| {
                classify_window(window.window_minutes).map(|meter| (slot, meter, window))
            })
        })
        .collect();

    if classified.is_empty() {
        return if rate_limits.primary.is_none() && rate_limits.secondary.is_none() {
            Ok(QuotaNormalizationOutcome::NoQuotaEvidence)
        } else {
            Ok(QuotaNormalizationOutcome::UnsupportedWindows)
        };
    }
    for meter_type in [QuotaMeterType::FiveHour, QuotaMeterType::Weekly] {
        if classified
            .iter()
            .filter(|(_, observed_meter, _)| *observed_meter == meter_type)
            .count()
            > 1
        {
            return Ok(QuotaNormalizationOutcome::AmbiguousMeter { meter_type });
        }
    }

    let samples = classified
        .into_iter()
        .map(|(slot, meter_type, window)| {
            normalize_window(
                source,
                item,
                &record.timestamp,
                slot,
                meter_type,
                window,
                rate_limits,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(QuotaNormalizationOutcome::Samples(samples))
}

fn is_main_limit(rate_limits: &RateLimitSnapshot) -> bool {
    rate_limits
        .limit_id
        .as_deref()
        .is_none_or(|limit_id| limit_id.eq_ignore_ascii_case("codex"))
}

fn classify_window(window_minutes: Option<i64>) -> Option<QuotaMeterType> {
    match window_minutes {
        Some(minutes) if (FIVE_HOUR_MINUTES_MIN..=FIVE_HOUR_MINUTES_MAX).contains(&minutes) => {
            Some(QuotaMeterType::FiveHour)
        }
        Some(minutes) if (WEEKLY_MINUTES_MIN..=WEEKLY_MINUTES_MAX).contains(&minutes) => {
            Some(QuotaMeterType::Weekly)
        }
        _ => None,
    }
}

fn normalize_window(
    source: &SourceIdentity,
    item: &ReadItem,
    sampled_at: &str,
    slot: &str,
    meter_type: QuotaMeterType,
    window: RateLimitWindow,
    rate_limits: &RateLimitSnapshot,
) -> Result<NormalizedQuotaSample, QuotaNormalizationError> {
    let observed_reset_at = window.resets_at.map(format_reset_timestamp).transpose()?;
    let reset_evidence = match observed_reset_at {
        Some(observed_reset_at) => QuotaWindowIdentity::Available {
            meter_type,
            observed_reset_at: Some(observed_reset_at),
            identity_provenance: QuotaIdentityProvenance::ObservedReset,
            identity_confidence: QuotaIdentityConfidence::High,
        },
        None => QuotaWindowIdentity::Unavailable,
    };
    let used_percent = window.used_percent.get();
    Ok(NormalizedQuotaSample {
        schema_version: SchemaVersion::V1,
        sample_id: identity::quota_sample_id(
            source,
            item.start_offset,
            item.end_offset,
            item.ordinal,
            slot,
            meter_type.as_str(),
        ),
        meter_type,
        sampled_at: sampled_at.to_owned(),
        used_percent: PercentageMetric::observed(used_percent),
        remaining_percent: PercentageMetric::derived(100.0 - used_percent),
        reset_evidence,
        configuration: quota_configuration(rate_limits),
        acquisition_status: AcquisitionStatus::Succeeded,
        source_kind: QuotaSourceKind::LocalMeter,
    })
}

fn quota_configuration(rate_limits: &RateLimitSnapshot) -> ConfigurationIdentity {
    ConfigurationIdentity {
        plan: rate_limits
            .plan_type
            .map_or_else(ConfigurationValue::unavailable, |plan| {
                ConfigurationValue::observed(plan.as_str())
            }),
        model: ConfigurationValue::unavailable(),
        reasoning_level: ConfigurationValue::unavailable(),
        speed_mode: ConfigurationValue::unavailable(),
        codex_version: ConfigurationValue::unavailable(),
    }
}

fn format_reset_timestamp(seconds: i64) -> Result<String, QuotaNormalizationError> {
    OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|_| QuotaNormalizationError::InvalidResetTimestamp)?
        .to_offset(UtcOffset::UTC)
        .format(&Rfc3339)
        .map_err(|_| QuotaNormalizationError::InvalidResetTimestamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::codex_rollout::parse_rollout_record;
    use crate::telemetry::incremental_jsonl::{ReadItem, ReadItemOutcome};

    const NORMAL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/quota/normal.json"
    ));
    const INVALID_BELOW_ZERO: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/quota/invalid-below-zero.json"
    ));
    const INVALID_ABOVE_HUNDRED: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/codex-rollout/v0.157.1/quota/invalid-above-hundred.json"
    ));

    fn item(input: &str) -> (SourceIdentity, ReadItem) {
        let source = SourceIdentity::new("quota-test-rollout", "generation-a").unwrap();
        let record = parse_rollout_record(input).expect("synthetic record should decode");
        (
            source,
            ReadItem {
                start_offset: 7,
                end_offset: 211,
                ordinal: Some(4),
                outcome: ReadItemOutcome::Decoded(Box::new(record)),
            },
        )
    }

    fn token_count(rate_limits: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-10-03T10:01:01Z","type":"event_msg","payload":{{"type":"token_count","info":null,"rate_limits":{rate_limits}}}}}"#
        )
    }

    #[test]
    fn classifies_reversed_slots_by_duration() {
        let (source, read_item) = item(&token_count(
            r#"{"limit_id":"CoDeX","plan_type":"plus","primary":{"used_percent":20,"window_minutes":10080,"resets_at":1791000000},"secondary":{"used_percent":40,"window_minutes":300,"resets_at":1790982000}}"#,
        ));
        let QuotaNormalizationOutcome::Samples(samples) =
            normalize_quota_item(&source, &read_item).unwrap()
        else {
            panic!("expected normalized samples");
        };
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].meter_type, QuotaMeterType::Weekly);
        assert_eq!(samples[1].meter_type, QuotaMeterType::FiveHour);
        assert_eq!(samples[0].configuration.plan.value.as_deref(), Some("plus"));
        assert_eq!(samples[0].source_kind, QuotaSourceKind::LocalMeter);
    }

    #[test]
    fn classifies_inclusive_tolerance_boundaries_only() {
        for minutes in [285, 300, 315] {
            assert_eq!(
                classify_window(Some(minutes)),
                Some(QuotaMeterType::FiveHour)
            );
        }
        for minutes in [284, 316] {
            assert_eq!(classify_window(Some(minutes)), None);
        }
        for minutes in [9_576, 10_080, 10_584] {
            assert_eq!(classify_window(Some(minutes)), Some(QuotaMeterType::Weekly));
        }
        for minutes in [9_575, 10_585, 1_440, 0, -1] {
            assert_eq!(classify_window(Some(minutes)), None);
        }
        assert_eq!(classify_window(None), None);
    }

    #[test]
    fn reset_conversion_is_utc_and_replay_is_stable() {
        let (source, read_item) = item(NORMAL);
        let first = normalize_quota_item(&source, &read_item).unwrap();
        let second = normalize_quota_item(&source, &read_item).unwrap();
        assert_eq!(first, second);
        let QuotaNormalizationOutcome::Samples(samples) = first else {
            panic!("expected normalized samples");
        };
        assert_ne!(samples[0].sample_id, samples[1].sample_id);
        assert_eq!(samples[0].sampled_at, "2026-10-03T10:01:01Z");
        assert_eq!(
            samples[0].reset_evidence,
            QuotaWindowIdentity::Available {
                meter_type: QuotaMeterType::FiveHour,
                observed_reset_at: Some("2026-10-03T15:00:00Z".to_owned()),
                identity_provenance: QuotaIdentityProvenance::ObservedReset,
                identity_confidence: QuotaIdentityConfidence::High,
            }
        );
    }

    #[test]
    fn normalized_sample_serializes_to_the_v1_shape() {
        let (source, read_item) = item(NORMAL);
        let QuotaNormalizationOutcome::Samples(samples) =
            normalize_quota_item(&source, &read_item).unwrap()
        else {
            panic!("expected normalized samples");
        };
        let value = serde_json::to_value(&samples[0]).expect("sample should serialize");
        assert_eq!(value["schema_version"], "1.0.0");
        assert_eq!(value["meter_type"], "five_hour");
        assert_eq!(value["source_kind"], "local_meter");
        assert_eq!(value["acquisition_status"], "succeeded");
        assert_eq!(value["used_percent"]["provenance"], "observed");
        assert_eq!(value["remaining_percent"]["provenance"], "derived");
        assert_eq!(value["reset_evidence"]["availability"], "available");
        assert_eq!(value["configuration"]["plan"]["value"], "plus");
    }

    #[test]
    fn malformed_percentages_are_rejected_before_normalization() {
        for fixture in [INVALID_BELOW_ZERO, INVALID_ABOVE_HUNDRED] {
            let error = parse_rollout_record(fixture).expect_err("invalid percentage must reject");
            assert!(matches!(
                error,
                crate::telemetry::codex_rollout::RolloutDecodeError::InvalidFieldValue {
                    field: "used_percent",
                    reason:
                        crate::telemetry::codex_rollout::InvalidFieldReason::PercentageOutOfRange,
                    ..
                }
            ));
            assert!(!error.to_string().contains("rate_limits"));
        }
    }

    #[test]
    fn missing_limit_id_is_main_and_unknown_plan_is_unavailable() {
        let (source, read_item) = item(&token_count(
            r#"{"primary":{"used_percent":20,"window_minutes":300},"plan_type":"future_plan"}"#,
        ));
        let QuotaNormalizationOutcome::Samples(samples) =
            normalize_quota_item(&source, &read_item).unwrap()
        else {
            panic!("expected normalized sample");
        };
        assert_eq!(
            samples[0].configuration.plan.availability,
            super::super::normalized::MetricAvailability::Unavailable
        );
    }

    #[test]
    fn derives_remaining_and_preserves_missing_reset() {
        let (source, read_item) = item(&token_count(
            r#"{"plan_type":"plus","primary":{"used_percent":42.5,"window_minutes":300}}"#,
        ));
        let QuotaNormalizationOutcome::Samples(samples) =
            normalize_quota_item(&source, &read_item).unwrap()
        else {
            panic!("expected normalized samples");
        };
        let sample = &samples[0];
        assert_eq!(sample.used_percent.value, Some(42.5));
        assert_eq!(sample.remaining_percent.value, Some(57.5));
        assert_eq!(sample.reset_evidence, QuotaWindowIdentity::Unavailable);
        assert_eq!(sample.acquisition_status, AcquisitionStatus::Succeeded);
    }

    #[test]
    fn rejects_non_main_and_ambiguous_windows() {
        let (source, read_item) = item(&token_count(
            r#"{"limit_id":"codex_some_model","primary":{"used_percent":20,"window_minutes":300}}"#,
        ));
        assert_eq!(
            normalize_quota_item(&source, &read_item).unwrap(),
            QuotaNormalizationOutcome::NonMainLimit
        );

        let (source, read_item) = item(&token_count(
            r#"{"primary":{"used_percent":20,"window_minutes":300},"secondary":{"used_percent":30,"window_minutes":315}}"#,
        ));
        assert_eq!(
            normalize_quota_item(&source, &read_item).unwrap(),
            QuotaNormalizationOutcome::AmbiguousMeter {
                meter_type: QuotaMeterType::FiveHour
            }
        );
    }
}
