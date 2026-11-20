//! Conservative qualification of the structure supplied by one live account quota read.
//!
//! This diagnostic describes account-level source evidence. It never creates task
//! attribution, a quota event, an Observation, or an estimator input.

use crate::codex_app_server::{
    AccountRateLimitsSnapshot, BucketIdentity, MeterAvailability, QuotaWarning,
    SnapshotAvailability,
};
use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStatus {
    Supported,
    Partial,
    Unavailable,
    Unsupported,
    Ambiguous,
    Unverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLimitation {
    OfficialTokenAllowanceUnknown,
    ExhaustiveTokenCoverageUnverified,
    TaskCausalityUnverified,
    ExclusiveTaskUseUnverified,
    ResetStabilityUnverified,
    ExternalUsageIsolationUnverified,
    RuntimeCompatibilityUnverified,
    ProductionAccuracyUnverified,
    CapacityEstimatorReadinessUnverified,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MeterSourceDiagnostic {
    pub structural_evidence: EvidenceStatus,
    pub used_percent: EvidenceStatus,
    pub observed_used_percent: Option<f64>,
    pub reset_metadata: EvidenceStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RuntimeCompatibilityEvidence {
    /// Present only when the initialized server advertises a bounded
    /// `codex_cli_rs/<major>.<minor>.<patch>` user-agent.
    pub runtime_version: Option<String>,
    /// A version match is metadata and does not establish compatibility.
    pub pinned_audit_version_match: EvidenceStatus,
    pub compatibility: EvidenceStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LiveQuotaSourceDiagnostic {
    pub schema_version: &'static str,
    pub acquired_at: String,
    pub source: &'static str,
    pub evidence_scope: &'static str,
    pub connection: EvidenceStatus,
    pub rate_limit_rpc: EvidenceStatus,
    pub response_structure: EvidenceStatus,
    pub snapshot_availability: SnapshotAvailability,
    pub main_bucket: EvidenceStatus,
    pub five_hour: MeterSourceDiagnostic,
    pub weekly: MeterSourceDiagnostic,
    pub reset_metadata: EvidenceStatus,
    pub account_level_provenance: EvidenceStatus,
    pub task_association: EvidenceStatus,
    pub external_usage_isolation: EvidenceStatus,
    pub token_coverage_closure: EvidenceStatus,
    pub production_semantic_qualification: EvidenceStatus,
    pub production_quota_accuracy: EvidenceStatus,
    pub capacity_estimator_qualification: EvidenceStatus,
    pub runtime_compatibility: RuntimeCompatibilityEvidence,
    pub warnings: Vec<QuotaWarning>,
    pub limitations: Vec<DiagnosticLimitation>,
    pub structural_eligibility: EvidenceStatus,
}

/// Evaluate only the already normalized P045A result. No raw RPC JSON is accepted.
pub fn evaluate_live_quota_source(
    snapshot: &AccountRateLimitsSnapshot,
    user_agent: Option<&str>,
) -> LiveQuotaSourceDiagnostic {
    let bucket = match snapshot.bucket_identity {
        BucketIdentity::MainCodex => EvidenceStatus::Supported,
        BucketIdentity::Conflicting => EvidenceStatus::Ambiguous,
        BucketIdentity::Unknown
        | BucketIdentity::OtherBucket
        | BucketIdentity::OtherCodexRelated => EvidenceStatus::Unavailable,
    };
    let response_structure = match snapshot.availability {
        SnapshotAvailability::Ambiguous => EvidenceStatus::Ambiguous,
        SnapshotAvailability::Unavailable if bucket == EvidenceStatus::Unavailable => {
            EvidenceStatus::Partial
        }
        _ => EvidenceStatus::Supported,
    };
    let five_hour = meter_diagnostic(
        &snapshot.five_hour.availability,
        snapshot.five_hour.used_percent,
        snapshot.five_hour.reset_at_utc.is_some(),
    );
    let weekly = meter_diagnostic(
        &snapshot.weekly.availability,
        snapshot.weekly.used_percent,
        snapshot.weekly.reset_at_utc.is_some(),
    );
    let reset_metadata = combine(five_hour.reset_metadata, weekly.reset_metadata);
    let structural_eligibility = match snapshot.availability {
        SnapshotAvailability::Ambiguous => EvidenceStatus::Ambiguous,
        SnapshotAvailability::Available => EvidenceStatus::Supported,
        SnapshotAvailability::Partial => EvidenceStatus::Partial,
        SnapshotAvailability::Unavailable => EvidenceStatus::Unavailable,
    };
    LiveQuotaSourceDiagnostic {
        schema_version: "1.0.0",
        acquired_at: snapshot.acquired_at.clone(),
        source: "codex_app_server_account_rate_limits",
        evidence_scope: "account_level",
        connection: EvidenceStatus::Supported,
        rate_limit_rpc: EvidenceStatus::Supported,
        response_structure,
        snapshot_availability: snapshot.availability,
        main_bucket: bucket,
        five_hour,
        weekly,
        reset_metadata,
        account_level_provenance: if bucket == EvidenceStatus::Supported {
            EvidenceStatus::Supported
        } else {
            bucket
        },
        task_association: EvidenceStatus::Unverified,
        external_usage_isolation: EvidenceStatus::Unverified,
        token_coverage_closure: EvidenceStatus::Unverified,
        production_semantic_qualification: EvidenceStatus::Unverified,
        production_quota_accuracy: EvidenceStatus::Unverified,
        capacity_estimator_qualification: EvidenceStatus::Unverified,
        runtime_compatibility: runtime_compatibility(user_agent),
        warnings: snapshot.warnings.clone(),
        limitations: vec![
            DiagnosticLimitation::OfficialTokenAllowanceUnknown,
            DiagnosticLimitation::ExhaustiveTokenCoverageUnverified,
            DiagnosticLimitation::TaskCausalityUnverified,
            DiagnosticLimitation::ExclusiveTaskUseUnverified,
            DiagnosticLimitation::ResetStabilityUnverified,
            DiagnosticLimitation::ExternalUsageIsolationUnverified,
            DiagnosticLimitation::RuntimeCompatibilityUnverified,
            DiagnosticLimitation::ProductionAccuracyUnverified,
            DiagnosticLimitation::CapacityEstimatorReadinessUnverified,
        ],
        structural_eligibility,
    }
}

fn meter_diagnostic(
    availability: &MeterAvailability,
    observed_used_percent: Option<f64>,
    has_reset: bool,
) -> MeterSourceDiagnostic {
    let status = match availability {
        MeterAvailability::Available => EvidenceStatus::Supported,
        MeterAvailability::Unavailable => EvidenceStatus::Unavailable,
        MeterAvailability::Ambiguous => EvidenceStatus::Ambiguous,
    };
    MeterSourceDiagnostic {
        structural_evidence: status,
        used_percent: if observed_used_percent.is_some() {
            EvidenceStatus::Supported
        } else {
            status
        },
        observed_used_percent,
        reset_metadata: if has_reset {
            EvidenceStatus::Supported
        } else {
            EvidenceStatus::Unavailable
        },
    }
}

fn combine(left: EvidenceStatus, right: EvidenceStatus) -> EvidenceStatus {
    use EvidenceStatus::{Ambiguous, Partial, Supported, Unavailable};
    match (left, right) {
        (Ambiguous, _) | (_, Ambiguous) => Ambiguous,
        (Supported, Supported) => Supported,
        (Unavailable, Unavailable) => Unavailable,
        _ => Partial,
    }
}

fn runtime_compatibility(user_agent: Option<&str>) -> RuntimeCompatibilityEvidence {
    const AUDITED: &str = "0.163.0";
    let runtime_version = user_agent.and_then(parse_runtime_version);
    RuntimeCompatibilityEvidence {
        pinned_audit_version_match: match runtime_version.as_deref() {
            Some(AUDITED) => EvidenceStatus::Supported,
            Some(_) => EvidenceStatus::Unavailable,
            None => EvidenceStatus::Unavailable,
        },
        runtime_version,
        compatibility: EvidenceStatus::Unverified,
    }
}

fn parse_runtime_version(user_agent: &str) -> Option<String> {
    let version = user_agent.strip_prefix("codex_cli_rs/")?;
    let mut components = version.split('.');
    let valid = (0..3).all(|_| {
        components
            .next()
            .is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    });
    (valid && components.next().is_none() && version.len() <= 24).then(|| version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_app_server::MeterSnapshot;

    fn snapshot(five: MeterAvailability, weekly: MeterAvailability) -> AccountRateLimitsSnapshot {
        let meter =
            |availability: MeterAvailability, percent: Option<f64>, reset: bool| MeterSnapshot {
                availability,
                used_percent: percent,
                used_percent_provenance: if percent.is_some() {
                    "observed"
                } else {
                    "unavailable"
                },
                remaining_percent: percent.map(|value| 100.0 - value),
                remaining_percent_provenance: "derived",
                reset_at_utc: reset.then(|| "2026-11-20T00:00:00Z".into()),
            };
        let availability = match (five, weekly) {
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
        AccountRateLimitsSnapshot {
            schema_version: "1.0.0",
            source: "codex_app_server_account_rate_limits",
            acquired_at: "2026-11-20T00:00:00Z".into(),
            availability,
            bucket_identity: BucketIdentity::MainCodex,
            five_hour: meter(five, (five == MeterAvailability::Available).then_some(0.0), true),
            weekly: meter(weekly, (weekly == MeterAvailability::Available).then_some(100.0), false),
            warnings: vec![QuotaWarning::MissingResetTimestamp],
            provenance: "account_level_snapshot; task_attribution_not_established; forecast_qualification_not_established",
        }
    }

    #[test]
    fn meters_are_qualified_independently_and_zero_and_one_hundred_are_observations() {
        let result = evaluate_live_quota_source(
            &snapshot(MeterAvailability::Available, MeterAvailability::Unavailable),
            Some("codex_cli_rs/0.163.0"),
        );
        assert_eq!(result.structural_eligibility, EvidenceStatus::Partial);
        assert_eq!(result.five_hour.used_percent, EvidenceStatus::Supported);
        assert_eq!(result.weekly.used_percent, EvidenceStatus::Unavailable);
        assert_eq!(result.five_hour.reset_metadata, EvidenceStatus::Supported);
        assert_eq!(result.weekly.reset_metadata, EvidenceStatus::Unavailable);
        assert_eq!(
            result.runtime_compatibility.runtime_version.as_deref(),
            Some("0.163.0")
        );
        assert_eq!(
            result.runtime_compatibility.compatibility,
            EvidenceStatus::Unverified
        );
        assert_eq!(result.task_association, EvidenceStatus::Unverified);
        assert_eq!(result.production_quota_accuracy, EvidenceStatus::Unverified);
    }

    #[test]
    fn both_supported_meters_do_not_establish_stability_or_account_isolation() {
        let result = evaluate_live_quota_source(
            &snapshot(MeterAvailability::Available, MeterAvailability::Available),
            None,
        );
        assert_eq!(result.structural_eligibility, EvidenceStatus::Supported);
        assert_eq!(result.reset_metadata, EvidenceStatus::Partial);
        assert_eq!(result.task_association, EvidenceStatus::Unverified);
        assert_eq!(result.external_usage_isolation, EvidenceStatus::Unverified);
        assert_eq!(result.token_coverage_closure, EvidenceStatus::Unverified);
        assert_eq!(
            result.production_semantic_qualification,
            EvidenceStatus::Unverified
        );
        assert_eq!(
            result.capacity_estimator_qualification,
            EvidenceStatus::Unverified
        );
        assert_eq!(
            result.runtime_compatibility.compatibility,
            EvidenceStatus::Unverified
        );
    }

    #[test]
    fn ambiguous_and_unavailable_snapshots_stay_explicit() {
        let mut ambiguous = snapshot(MeterAvailability::Ambiguous, MeterAvailability::Available);
        ambiguous.availability = SnapshotAvailability::Ambiguous;
        let result = evaluate_live_quota_source(&ambiguous, None);
        assert_eq!(result.structural_eligibility, EvidenceStatus::Ambiguous);
        let mut unavailable = snapshot(
            MeterAvailability::Unavailable,
            MeterAvailability::Unavailable,
        );
        unavailable.bucket_identity = BucketIdentity::Unknown;
        unavailable.availability = SnapshotAvailability::Unavailable;
        let result = evaluate_live_quota_source(&unavailable, None);
        assert_eq!(result.main_bucket, EvidenceStatus::Unavailable);
        assert_eq!(result.structural_eligibility, EvidenceStatus::Unavailable);
    }

    #[test]
    fn runtime_metadata_is_bounded_and_version_match_is_not_qualification() {
        assert_eq!(
            runtime_compatibility(Some("codex_cli_rs/0.163.0; secret")).runtime_version,
            None
        );
        assert_eq!(
            runtime_compatibility(Some("codex_cli_rs/test")).runtime_version,
            None
        );
        assert_eq!(
            runtime_compatibility(Some("codex_cli_rs/0.163.0")).pinned_audit_version_match,
            EvidenceStatus::Supported
        );
        assert_eq!(
            runtime_compatibility(Some("codex_cli_rs/0.163.0")).compatibility,
            EvidenceStatus::Unverified
        );
    }
}
