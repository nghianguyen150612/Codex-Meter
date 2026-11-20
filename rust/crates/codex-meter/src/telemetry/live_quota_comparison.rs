//! Deterministic pairwise comparison of two saved, account-level quota samples.
//!
//! This deliberately does not adapt App Server data into rollout telemetry or
//! task reconciliation evidence. A movement is only a percentage-point change
//! between two observed account meter values with a compatible observed reset.

use serde::Serialize;
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use crate::codex_app_server::{BucketIdentity, MeterAvailability};
use crate::storage::{
    StoredLiveAccountSnapshot, StoredLiveQuotaMeter, LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
    LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION, LIVE_ACCOUNT_QUOTA_PROVENANCE,
    LIVE_ACCOUNT_QUOTA_SOURCE, LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION,
    LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION,
};

pub const LIVE_QUOTA_COMPARISON_SCHEMA_VERSION: &str = "1.0.0";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCompatibility {
    CompatibleFixedSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowContinuity {
    ObservedCompatible,
    ResetBoundaryDetected,
    Unverified,
    NotAssessable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetEvidence {
    MatchingObservedReset,
    ResetBoundaryCrossed,
    ResetNotAfterBaseline,
    MissingResetMetadata,
    ChangedResetWithoutBoundaryEvidence,
    Unavailable,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterComparisonStatus {
    SameObservedWindow,
    ResetBoundaryDetected,
    WindowContinuityUnverified,
    MeterUnavailable,
    AmbiguousEvidence,
    MeterInstability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeterKind {
    FiveHour,
    Weekly,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeterComparison {
    pub meter: MeterKind,
    pub status: MeterComparisonStatus,
    pub window_continuity: WindowContinuity,
    pub reset_evidence: ResetEvidence,
    pub before_used_percent: Option<f64>,
    pub after_used_percent: Option<f64>,
    /// Observed account percentage-point movement. Never task consumption.
    pub observed_movement_percentage_points: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonWarning {
    AccountIdentityContinuityUnverified,
    ExternalAccountActivityUnknown,
    TaskCausalityUnverified,
    ControlledBenchmarkIsolationAbsent,
    TokenCoverageClosureUnverified,
    ProductionQuotaAccuracyUnverified,
    OfficialTokenAllowanceUnavailable,
    EstimatorEligibilityNotEstablished,
    AccountMovementIsNotTaskConsumption,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveQuotaComparison {
    pub schema_version: &'static str,
    pub source: &'static str,
    pub evidence_scope: &'static str,
    pub source_compatibility: SourceCompatibility,
    pub account_identity_continuity_unverified: bool,
    pub before_snapshot_id: String,
    pub after_snapshot_id: String,
    pub before_acquired_at_utc: String,
    pub after_acquired_at_utc: String,
    pub chronology_valid: bool,
    pub before_bucket_identity: BucketIdentity,
    pub after_bucket_identity: BucketIdentity,
    pub bucket_identity: BucketIdentity,
    pub five_hour: MeterComparison,
    pub weekly: MeterComparison,
    pub warnings: Vec<ComparisonWarning>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonError {
    SnapshotIdInvalid,
    SameSnapshotSelected,
    UnsupportedSchemaVersion,
    UnsupportedSource,
    EvidenceScopeInvalid,
    ProvenanceInvalid,
    AcquisitionTimestampInvalid,
    AcquisitionTimesNotIncreasing,
    InvalidMeterEvidence,
}

impl std::fmt::Display for ComparisonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SnapshotIdInvalid => "snapshot ID is invalid",
            Self::SameSnapshotSelected => "the same snapshot cannot be selected twice",
            Self::UnsupportedSchemaVersion => "saved snapshot schema version is unsupported",
            Self::UnsupportedSource => "saved snapshot source is unsupported",
            Self::EvidenceScopeInvalid => "saved snapshot is not account-level evidence",
            Self::ProvenanceInvalid => "saved snapshot provenance is unsupported",
            Self::AcquisitionTimestampInvalid => "snapshot acquisition timestamp is invalid",
            Self::AcquisitionTimesNotIncreasing => {
                "after snapshot must have a later acquisition time"
            }
            Self::InvalidMeterEvidence => "saved meter evidence is invalid",
        })
    }
}

impl std::error::Error for ComparisonError {}

pub fn evaluate_live_quota_comparison(
    before: &StoredLiveAccountSnapshot,
    after: &StoredLiveAccountSnapshot,
) -> Result<LiveQuotaComparison, ComparisonError> {
    validate_snapshot(before)?;
    validate_snapshot(after)?;
    if before.snapshot_id == after.snapshot_id {
        return Err(ComparisonError::SameSnapshotSelected);
    }
    let before_at = parse_utc(&before.acquired_at)?;
    let after_at = parse_utc(&after.acquired_at)?;
    if after_at <= before_at {
        return Err(ComparisonError::AcquisitionTimesNotIncreasing);
    }

    let bucket_identity = if before.bucket_identity == BucketIdentity::MainCodex
        && after.bucket_identity == BucketIdentity::MainCodex
    {
        BucketIdentity::MainCodex
    } else if before.bucket_identity == after.bucket_identity {
        before.bucket_identity
    } else {
        BucketIdentity::Conflicting
    };
    let bucket_is_comparable = bucket_identity == BucketIdentity::MainCodex;
    let five_hour = compare_meter(
        MeterKind::FiveHour,
        &before.five_hour,
        &after.five_hour,
        before_at,
        after_at,
        bucket_is_comparable,
    )?;
    let weekly = compare_meter(
        MeterKind::Weekly,
        &before.weekly,
        &after.weekly,
        before_at,
        after_at,
        bucket_is_comparable,
    )?;

    Ok(LiveQuotaComparison {
        schema_version: LIVE_QUOTA_COMPARISON_SCHEMA_VERSION,
        source: LIVE_ACCOUNT_QUOTA_SOURCE,
        evidence_scope: LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
        source_compatibility: SourceCompatibility::CompatibleFixedSource,
        account_identity_continuity_unverified: true,
        before_snapshot_id: before.snapshot_id.clone(),
        after_snapshot_id: after.snapshot_id.clone(),
        before_acquired_at_utc: before.acquired_at.clone(),
        after_acquired_at_utc: after.acquired_at.clone(),
        chronology_valid: true,
        before_bucket_identity: before.bucket_identity,
        after_bucket_identity: after.bucket_identity,
        bucket_identity,
        five_hour,
        weekly,
        warnings: vec![
            ComparisonWarning::AccountIdentityContinuityUnverified,
            ComparisonWarning::ExternalAccountActivityUnknown,
            ComparisonWarning::TaskCausalityUnverified,
            ComparisonWarning::ControlledBenchmarkIsolationAbsent,
            ComparisonWarning::TokenCoverageClosureUnverified,
            ComparisonWarning::ProductionQuotaAccuracyUnverified,
            ComparisonWarning::OfficialTokenAllowanceUnavailable,
            ComparisonWarning::EstimatorEligibilityNotEstablished,
            ComparisonWarning::AccountMovementIsNotTaskConsumption,
        ],
    })
}

fn validate_snapshot(snapshot: &StoredLiveAccountSnapshot) -> Result<(), ComparisonError> {
    if !valid_snapshot_id(&snapshot.snapshot_id) {
        return Err(ComparisonError::SnapshotIdInvalid);
    }
    if snapshot.storage_schema_version != LIVE_ACCOUNT_QUOTA_STORAGE_SCHEMA_VERSION
        || snapshot.schema_version != "1.0.0"
    {
        return Err(ComparisonError::UnsupportedSchemaVersion);
    }
    if snapshot.source != LIVE_ACCOUNT_QUOTA_SOURCE {
        return Err(ComparisonError::UnsupportedSource);
    }
    if snapshot.evidence_scope != LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE {
        return Err(ComparisonError::EvidenceScopeInvalid);
    }
    if snapshot.task_attribution != LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION
        || snapshot.forecast_qualification != LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION
        || snapshot.provenance != LIVE_ACCOUNT_QUOTA_PROVENANCE
    {
        return Err(ComparisonError::ProvenanceInvalid);
    }
    parse_utc(&snapshot.acquired_at)?;
    validate_meter(&snapshot.five_hour)?;
    validate_meter(&snapshot.weekly)?;
    Ok(())
}

fn valid_snapshot_id(snapshot_id: &str) -> bool {
    snapshot_id.strip_prefix("lqs_").is_some_and(|random| {
        random.len() == 32
            && random
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn parse_utc(value: &str) -> Result<OffsetDateTime, ComparisonError> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| ComparisonError::AcquisitionTimestampInvalid)?;
    if parsed.offset() != UtcOffset::UTC {
        return Err(ComparisonError::AcquisitionTimestampInvalid);
    }
    Ok(parsed)
}

fn validate_meter(meter: &StoredLiveQuotaMeter) -> Result<(), ComparisonError> {
    match meter.availability {
        MeterAvailability::Available => {
            if !meter
                .used_percent
                .is_some_and(|value| value.is_finite() && (0.0..=100.0).contains(&value))
                || meter.used_percent_provenance != "observed"
                || meter.remaining_percent_provenance != "derived"
                || !meter.remaining_percent_derived.is_some_and(|remaining| {
                    meter
                        .used_percent
                        .is_some_and(|used| remaining.is_finite() && remaining == 100.0 - used)
                })
            {
                return Err(ComparisonError::InvalidMeterEvidence);
            }
            if let Some(reset) = &meter.reset_at_utc {
                parse_utc(reset)?;
            }
        }
        MeterAvailability::Unavailable | MeterAvailability::Ambiguous => {
            if meter.used_percent.is_some()
                || meter.reset_at_utc.is_some()
                || meter.used_percent_provenance != "unavailable"
                || meter.remaining_percent_derived.is_some()
                || meter.remaining_percent_provenance != "unavailable"
            {
                return Err(ComparisonError::InvalidMeterEvidence);
            }
        }
    }
    Ok(())
}

fn compare_meter(
    meter: MeterKind,
    before: &StoredLiveQuotaMeter,
    after: &StoredLiveQuotaMeter,
    before_at: OffsetDateTime,
    after_at: OffsetDateTime,
    bucket_is_comparable: bool,
) -> Result<MeterComparison, ComparisonError> {
    validate_meter(before)?;
    validate_meter(after)?;
    let values = (before.used_percent, after.used_percent);
    let result = |status, continuity, reset, movement| MeterComparison {
        meter,
        status,
        window_continuity: continuity,
        reset_evidence: reset,
        before_used_percent: values.0,
        after_used_percent: values.1,
        observed_movement_percentage_points: movement,
    };

    if !bucket_is_comparable
        || before.availability == MeterAvailability::Ambiguous
        || after.availability == MeterAvailability::Ambiguous
    {
        return Ok(result(
            MeterComparisonStatus::AmbiguousEvidence,
            WindowContinuity::NotAssessable,
            ResetEvidence::Ambiguous,
            None,
        ));
    }
    if before.availability == MeterAvailability::Unavailable
        || after.availability == MeterAvailability::Unavailable
    {
        return Ok(result(
            MeterComparisonStatus::MeterUnavailable,
            WindowContinuity::NotAssessable,
            ResetEvidence::Unavailable,
            None,
        ));
    }
    let (Some(before_used), Some(after_used)) = values else {
        return Err(ComparisonError::InvalidMeterEvidence);
    };
    let (Some(before_reset_text), Some(after_reset_text)) = (
        before.reset_at_utc.as_deref(),
        after.reset_at_utc.as_deref(),
    ) else {
        if after_used < before_used {
            return Ok(result(
                MeterComparisonStatus::MeterInstability,
                WindowContinuity::Unverified,
                ResetEvidence::MissingResetMetadata,
                None,
            ));
        }
        return Ok(result(
            MeterComparisonStatus::WindowContinuityUnverified,
            WindowContinuity::Unverified,
            ResetEvidence::MissingResetMetadata,
            None,
        ));
    };
    let before_reset = parse_utc(before_reset_text)?;
    let after_reset = parse_utc(after_reset_text)?;
    if before_reset != after_reset {
        if before_reset > before_at && before_reset <= after_at && after_reset > after_at {
            return Ok(result(
                MeterComparisonStatus::ResetBoundaryDetected,
                WindowContinuity::ResetBoundaryDetected,
                ResetEvidence::ResetBoundaryCrossed,
                None,
            ));
        }
        return Ok(result(
            MeterComparisonStatus::MeterInstability,
            WindowContinuity::Unverified,
            ResetEvidence::ChangedResetWithoutBoundaryEvidence,
            None,
        ));
    }
    if before_reset <= before_at {
        return Ok(result(
            MeterComparisonStatus::MeterInstability,
            WindowContinuity::Unverified,
            ResetEvidence::ResetNotAfterBaseline,
            None,
        ));
    }
    if before_reset <= after_at {
        return Ok(result(
            MeterComparisonStatus::ResetBoundaryDetected,
            WindowContinuity::ResetBoundaryDetected,
            ResetEvidence::ResetBoundaryCrossed,
            None,
        ));
    }
    if after_used < before_used {
        return Ok(result(
            MeterComparisonStatus::MeterInstability,
            WindowContinuity::ObservedCompatible,
            ResetEvidence::MatchingObservedReset,
            None,
        ));
    }
    let movement = (after_used - before_used).max(0.0);
    Ok(result(
        MeterComparisonStatus::SameObservedWindow,
        WindowContinuity::ObservedCompatible,
        ResetEvidence::MatchingObservedReset,
        Some(movement),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::{BucketIdentity, SnapshotAvailability};

    fn snapshot(id: &str, acquired_at: &str, five: f64, weekly: f64) -> StoredLiveAccountSnapshot {
        StoredLiveAccountSnapshot {
            snapshot_id: format!("lqs_{id:0<32}"),
            storage_schema_version: 1,
            schema_version: "1.0.0",
            source: LIVE_ACCOUNT_QUOTA_SOURCE,
            acquired_at: acquired_at.to_owned(),
            availability: SnapshotAvailability::Available,
            bucket_identity: BucketIdentity::MainCodex,
            five_hour: meter(five, "2026-01-01T05:00:00Z"),
            weekly: meter(weekly, "2026-01-08T00:00:00Z"),
            warnings: Vec::new(),
            evidence_scope: LIVE_ACCOUNT_QUOTA_EVIDENCE_SCOPE,
            task_attribution: LIVE_ACCOUNT_QUOTA_TASK_ATTRIBUTION,
            forecast_qualification: LIVE_ACCOUNT_QUOTA_FORECAST_QUALIFICATION,
            provenance: LIVE_ACCOUNT_QUOTA_PROVENANCE,
            payload_sha256: "0".repeat(64),
        }
    }

    fn meter(value: f64, reset: &str) -> StoredLiveQuotaMeter {
        StoredLiveQuotaMeter {
            availability: MeterAvailability::Available,
            used_percent: Some(value),
            used_percent_provenance: "observed",
            remaining_percent_derived: Some(100.0 - value),
            remaining_percent_provenance: "derived",
            reset_at_utc: Some(reset.to_owned()),
        }
    }

    fn pair() -> (StoredLiveAccountSnapshot, StoredLiveAccountSnapshot) {
        (
            snapshot("a", "2026-01-01T00:00:00Z", 25.0, 10.0),
            snapshot("b", "2026-01-01T01:00:00Z", 29.0, 12.5),
        )
    }

    #[test]
    fn compares_meters_independently_and_keeps_fractional_movement() {
        let (before, after) = pair();
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            Some(4.0)
        );
        assert_eq!(
            comparison.weekly.observed_movement_percentage_points,
            Some(2.5)
        );
        assert!(comparison.account_identity_continuity_unverified);
        assert!(comparison
            .warnings
            .contains(&ComparisonWarning::AccountMovementIsNotTaskConsumption));
    }

    #[test]
    fn unchanged_is_observed_zero_not_stability_confirmation() {
        let (mut before, mut after) = pair();
        before.five_hour.used_percent = Some(29.0);
        after.five_hour.used_percent = Some(29.0);
        before.five_hour.remaining_percent_derived = Some(71.0);
        after.five_hour.remaining_percent_derived = Some(71.0);
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            Some(0.0)
        );
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::SameObservedWindow
        );
    }

    #[test]
    fn missing_resets_and_missing_meter_values_never_become_zero() {
        let (mut before, mut after) = pair();
        before.five_hour.reset_at_utc = None;
        after.five_hour.reset_at_utc = None;
        before.weekly.availability = MeterAvailability::Unavailable;
        before.weekly.used_percent = None;
        before.weekly.remaining_percent_derived = None;
        before.weekly.used_percent_provenance = "unavailable";
        before.weekly.remaining_percent_provenance = "unavailable";
        before.weekly.reset_at_utc = None;
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::WindowContinuityUnverified
        );
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            None
        );
        assert_eq!(
            comparison.weekly.status,
            MeterComparisonStatus::MeterUnavailable
        );
    }

    #[test]
    fn reset_boundary_and_percentage_decrease_emit_no_movement() {
        let (before, mut after) = pair();
        after.acquired_at = "2026-01-01T05:00:00Z".into();
        after.five_hour.used_percent = Some(4.0);
        after.five_hour.remaining_percent_derived = Some(96.0);
        after.five_hour.reset_at_utc = Some("2026-01-01T10:00:00Z".into());
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::ResetBoundaryDetected
        );
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            None
        );
    }

    #[test]
    fn conflicting_bucket_evidence_is_ambiguous_and_never_emits_movement() {
        let (before, mut after) = pair();
        after.bucket_identity = BucketIdentity::Conflicting;
        after.availability = SnapshotAvailability::Ambiguous;
        for meter in [&mut after.five_hour, &mut after.weekly] {
            meter.availability = MeterAvailability::Ambiguous;
            meter.used_percent = None;
            meter.remaining_percent_derived = None;
            meter.reset_at_utc = None;
            meter.used_percent_provenance = "unavailable";
            meter.remaining_percent_provenance = "unavailable";
        }
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(comparison.bucket_identity, BucketIdentity::Conflicting);
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::AmbiguousEvidence
        );
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            None
        );
    }

    #[test]
    fn source_and_provenance_are_independently_validated() {
        let (before, mut after) = pair();
        after.source = "unsupported_source";
        assert_eq!(
            evaluate_live_quota_comparison(&before, &after).unwrap_err(),
            ComparisonError::UnsupportedSource
        );
        after.source = LIVE_ACCOUNT_QUOTA_SOURCE;
        after.evidence_scope = "task_level";
        assert_eq!(
            evaluate_live_quota_comparison(&before, &after).unwrap_err(),
            ComparisonError::EvidenceScopeInvalid
        );
    }

    #[test]
    fn changed_reset_without_a_proven_crossing_is_meter_instability() {
        let (before, mut after) = pair();
        after.five_hour.reset_at_utc = Some("2026-01-01T06:00:00Z".into());
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::MeterInstability
        );
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            None
        );
    }

    #[test]
    fn same_reset_with_decrease_is_instability_not_negative_movement() {
        let (before, mut after) = pair();
        after.five_hour.used_percent = Some(20.0);
        after.five_hour.remaining_percent_derived = Some(80.0);
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::MeterInstability
        );
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            None
        );
    }

    #[test]
    fn decrease_with_missing_reset_is_unverified_instability() {
        let (mut before, mut after) = pair();
        before.five_hour.reset_at_utc = None;
        after.five_hour.reset_at_utc = None;
        after.five_hour.used_percent = Some(20.0);
        after.five_hour.remaining_percent_derived = Some(80.0);
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::MeterInstability
        );
        assert_eq!(
            comparison.five_hour.window_continuity,
            WindowContinuity::Unverified
        );
        assert_eq!(
            comparison.five_hour.observed_movement_percentage_points,
            None
        );
    }

    #[test]
    fn two_unavailable_meters_remain_unavailable() {
        let (mut before, mut after) = pair();
        for snapshot in [&mut before, &mut after] {
            snapshot.availability = SnapshotAvailability::Unavailable;
            for meter in [&mut snapshot.five_hour, &mut snapshot.weekly] {
                meter.availability = MeterAvailability::Unavailable;
                meter.used_percent = None;
                meter.remaining_percent_derived = None;
                meter.reset_at_utc = None;
                meter.used_percent_provenance = "unavailable";
                meter.remaining_percent_provenance = "unavailable";
            }
        }
        let comparison = evaluate_live_quota_comparison(&before, &after).unwrap();
        assert_eq!(
            comparison.five_hour.status,
            MeterComparisonStatus::MeterUnavailable
        );
        assert_eq!(
            comparison.weekly.status,
            MeterComparisonStatus::MeterUnavailable
        );
        assert_eq!(comparison.five_hour.before_used_percent, None);
    }

    #[test]
    fn rejects_same_snapshot_and_non_increasing_timestamps() {
        let (before, after) = pair();
        assert_eq!(
            evaluate_live_quota_comparison(&before, &before).unwrap_err(),
            ComparisonError::SameSnapshotSelected
        );
        let mut equal = after.clone();
        equal.acquired_at = before.acquired_at.clone();
        assert_eq!(
            evaluate_live_quota_comparison(&before, &equal).unwrap_err(),
            ComparisonError::AcquisitionTimesNotIncreasing
        );
        let mut reversed = after;
        reversed.acquired_at = "2025-12-31T23:59:59Z".into();
        assert_eq!(
            evaluate_live_quota_comparison(&before, &reversed).unwrap_err(),
            ComparisonError::AcquisitionTimesNotIncreasing
        );
    }
}
