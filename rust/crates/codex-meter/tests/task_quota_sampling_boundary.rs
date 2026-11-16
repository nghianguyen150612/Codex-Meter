//! This integration test links the production library (without cfg(test)).
use codex_meter::telemetry::quota_reconciliation::{
    ReconciliationPolicy, TaskReconciliationTarget,
};
use codex_meter::telemetry::task_quota_sampling::{
    AssociationClaim, SamplingPhase, SamplingRejection, SamplingStage, TaskQuotaSampling,
};
use time::Duration;

#[test]
fn production_claims_cannot_issue_authority_or_initialize_sampling() {
    let claim = AssociationClaim {
        contract_version: 1,
        task: TaskReconciliationTarget::new(
            "task:unverified",
            Some("2026-10-04T10:00:00Z".into()),
            Some("2026-10-04T10:05:00Z".into()),
        ),
        source_generation_id: "generation:unverified".into(),
        policy: ReconciliationPolicy {
            max_before_sample_age: Duration::minutes(5),
            post_task_sample_offsets: vec![Duration::seconds(15), Duration::seconds(30)],
            stabilization_not_before: Duration::seconds(15),
            required_stable_confirmations: 2,
            deadline: Duration::seconds(60),
        },
        phase: SamplingPhase::TaskAssociation,
    };
    let mut coordinator = TaskQuotaSampling::default();
    assert_eq!(
        coordinator.register(&claim, None),
        Err(SamplingRejection::MissingAuthority)
    );
    assert_eq!(coordinator.stage(), SamplingStage::Uninitialized);
    assert_eq!(
        coordinator.initialize("2026-10-04T10:05:00Z").unwrap_err(),
        SamplingRejection::NotRegistered
    );
    assert_eq!(coordinator.batch(), None);
}
