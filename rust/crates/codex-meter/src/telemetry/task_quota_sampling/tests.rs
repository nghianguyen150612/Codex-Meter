use super::*;
use crate::telemetry::identity;
use crate::telemetry::normalized::{
    AcquisitionStatus, ConfigurationIdentity, ConfigurationValue, PercentageMetric,
    QuotaIdentityConfidence, QuotaIdentityProvenance, QuotaSourceKind, SchemaVersion,
};
use crate::telemetry::quota_reconciliation::{
    AcquisitionFailureReason, AttributionRisk, BaselineUnavailableReason, PolicyValidationError,
    ReconciliationOutput,
};
use crate::telemetry::quota_tracking::{
    BoundaryEvidence, MeterInstabilityReason, QuotaTrackingError,
};
use time::Duration;

type RejectionCase<T> = (fn(&mut T), SamplingRejection);

const START: &str = "2026-10-04T10:00:00Z";
const END: &str = "2026-10-04T10:05:00Z";
const BEFORE: &str = "2026-10-04T09:59:00Z";
const FIRST: &str = "2026-10-04T10:05:15Z";
const SECOND: &str = "2026-10-04T10:05:30Z";
const THIRD: &str = "2026-10-04T10:05:45Z";
const DEADLINE: &str = "2026-10-04T10:06:00Z";
const RESET: &str = "2026-10-04T10:10:00Z";
const METERS: [QuotaMeterType; 2] = [QuotaMeterType::FiveHour, QuotaMeterType::Weekly];

/// Synthetic fixtures only: no live provider observations and no production issuer.
pub(crate) mod synthetic {
    use super::*;

    pub(crate) fn credential() -> AssociationCredential {
        let mut task = TaskReconciliationTarget::new(
            identity::task_id("task"),
            Some(START.to_owned()),
            Some(END.to_owned()),
        );
        task.session_id = Some(identity::session_id("session"));
        AssociationCredential {
            authority_contract: "synthetic-task-quota/v1",
            binding: AssociationClaim {
                contract_version: 1,
                task,
                source_generation_id: identity::source_generation_identity(
                    "session", "thread", "test", START,
                ),
                policy: ReconciliationPolicy {
                    max_before_sample_age: Duration::hours(1),
                    post_task_sample_offsets: vec![
                        Duration::ZERO,
                        Duration::seconds(15),
                        Duration::seconds(30),
                    ],
                    stabilization_not_before: Duration::seconds(15),
                    required_stable_confirmations: 2,
                    deadline: Duration::seconds(60),
                },
                phase: SamplingPhase::TaskAssociation,
            },
        }
    }

    pub(crate) fn claim(credential: &AssociationCredential) -> AssociationClaim {
        credential.binding.clone()
    }

    pub(crate) fn source_generation(credential: &AssociationCredential) -> String {
        credential.binding.source_generation_id.clone()
    }

    pub(crate) fn sample(
        credential: &AssociationCredential,
        phase: SamplingPhase,
        meter: QuotaMeterType,
        id: &str,
        at: &str,
        used: f64,
    ) -> QualifiedQuotaEvent {
        QualifiedQuotaEvent {
            credential: credential.clone(),
            phase,
            attempt: QuotaAcquisitionAttempt {
                sampled_at: at.to_owned(),
                meter_type: meter,
                result: QuotaAcquisitionResult::Sample(NormalizedQuotaSample {
                    schema_version: SchemaVersion::V1,
                    sample_id: id.to_owned(),
                    meter_type: meter,
                    sampled_at: at.to_owned(),
                    used_percent: PercentageMetric::observed(used),
                    remaining_percent: PercentageMetric::derived(100.0 - used),
                    reset_evidence: QuotaWindowIdentity::Available {
                        meter_type: meter,
                        local_window_id: None,
                        observed_reset_at: Some(RESET.to_owned()),
                        evidence_first_sample_at: None,
                        evidence_last_sample_at: None,
                        identity_provenance: QuotaIdentityProvenance::ObservedReset,
                        identity_confidence: QuotaIdentityConfidence::High,
                    },
                    configuration: ConfigurationIdentity {
                        plan: ConfigurationValue::observed("plus"),
                        model: ConfigurationValue::unavailable(),
                        reasoning_level: ConfigurationValue::unavailable(),
                        speed_mode: ConfigurationValue::unavailable(),
                        codex_version: ConfigurationValue::unavailable(),
                    },
                    acquisition_status: AcquisitionStatus::Succeeded,
                    source_kind: QuotaSourceKind::LocalMeter,
                }),
            },
        }
    }

    pub(super) fn failure(
        credential: &AssociationCredential,
        meter: QuotaMeterType,
    ) -> QualifiedQuotaEvent {
        QualifiedQuotaEvent {
            credential: credential.clone(),
            phase: SamplingPhase::AfterTask,
            attempt: QuotaAcquisitionAttempt {
                sampled_at: FIRST.to_owned(),
                meter_type: meter,
                result: QuotaAcquisitionResult::Failed {
                    reason: AcquisitionFailureReason::SourceUnavailable,
                },
            },
        }
    }
}

fn before(c: &AssociationCredential, meter: QuotaMeterType) -> QualifiedQuotaEvent {
    synthetic::sample(c, SamplingPhase::BeforeTask, meter, "before", BEFORE, 20.0)
}

fn after(
    c: &AssociationCredential,
    meter: QuotaMeterType,
    id: &str,
    at: &str,
    used: f64,
) -> QualifiedQuotaEvent {
    synthetic::sample(c, SamplingPhase::AfterTask, meter, id, at, used)
}

fn sample_mut(event: &mut QualifiedQuotaEvent) -> &mut NormalizedQuotaSample {
    let QuotaAcquisitionResult::Sample(sample) = &mut event.attempt.result else {
        panic!("fixture sample")
    };
    sample
}

fn registered(c: &AssociationCredential) -> TaskQuotaSampling {
    let mut coordinator = TaskQuotaSampling::default();
    assert_eq!(
        coordinator.register(&c.binding, Some(c)),
        Ok(Admission::Accepted)
    );
    coordinator
}

fn initialized(c: &AssociationCredential, meters: &[QuotaMeterType]) -> TaskQuotaSampling {
    let mut coordinator = registered(c);
    let events: Vec<_> = meters.iter().map(|meter| before(c, *meter)).collect();
    assert_eq!(
        coordinator.admit_before(&events),
        Ok(vec![Admission::Accepted; meters.len()])
    );
    coordinator.initialize(END).unwrap();
    coordinator
}

fn meter(
    coordinator: &TaskQuotaSampling,
    meter: QuotaMeterType,
) -> &super::super::quota_reconciliation::MeterReconciliation {
    let state = &coordinator.batch().unwrap().next_state;
    match meter {
        QuotaMeterType::FiveHour => &state.five_hour,
        QuotaMeterType::Weekly => &state.weekly,
    }
}

fn confirm(
    c: &AssociationCredential,
    meter: QuotaMeterType,
    used: f64,
) -> Vec<QualifiedQuotaEvent> {
    vec![
        after(c, meter, "a", FIRST, used),
        after(c, meter, "b", SECOND, used),
    ]
}

fn assert_stable(coordinator: &TaskQuotaSampling, which: QuotaMeterType, delta: f64) {
    let MeterReconciliationState::Stable { evidence } = &meter(coordinator, which).state else {
        panic!("expected stable: {:?}", meter(coordinator, which).state)
    };
    assert_eq!(evidence.meter_type, which);
    assert_eq!(evidence.delta_percentage_points.value(), delta);
    assert_eq!(evidence.before_sample.used_percent.value, Some(20.0));
    assert_eq!(evidence.after_sample.used_percent.value, Some(20.0 + delta));
    assert_eq!(evidence.stabilized_at, SECOND);
    assert_eq!(evidence.acquisition_attempt_count, 2);
}

#[test]
fn valid_association_is_immutable_and_replayable() {
    let c = synthetic::credential();
    let mut coordinator = TaskQuotaSampling::default();
    assert_eq!(coordinator.stage(), SamplingStage::Uninitialized);
    assert!(coordinator.batch().is_none());
    assert!(!coordinator.is_complete());
    assert_eq!(
        coordinator.register(&c.binding, Some(&c)),
        Ok(Admission::Accepted)
    );
    assert_eq!(coordinator.stage(), SamplingStage::QualifiedAssociation);
    assert_eq!(
        coordinator.register(&c.binding, Some(&c)),
        Ok(Admission::Replay)
    );
    let mut other = c.clone();
    other.binding.task.ended_at = Some(SECOND.to_owned());
    assert_eq!(
        coordinator.register(&other.binding, Some(&other)),
        Err(SamplingRejection::ConflictingAssociation)
    );
    coordinator.initialize(END).unwrap();
    assert_eq!(
        coordinator.register(&c.binding, Some(&c)),
        Ok(Admission::Replay)
    );
}

#[test]
fn missing_authority_cannot_promote_a_claim() {
    let c = synthetic::credential();
    let mut coordinator = TaskQuotaSampling::default();
    assert_eq!(
        coordinator.register(&c.binding, None),
        Err(SamplingRejection::MissingAuthority)
    );
    assert_eq!(coordinator.stage(), SamplingStage::Uninitialized);
}

#[test]
fn snapshot_reauthorization_restores_baselines_and_pending_event_payload() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &METERS);
    let pending = after(&c, QuotaMeterType::FiveHour, "pending", SECOND, 25.0);
    coordinator.advance(&[pending], FIRST).unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.checkpoint.baseline_samples.len(), 2);
    assert_eq!(snapshot.checkpoint.accepted_events.len(), 1);
    assert_eq!(
        snapshot.checkpoint.last_evaluation_at.as_deref(),
        Some(FIRST)
    );

    let recovered = TaskQuotaSampling::recover(snapshot.clone()).unwrap();
    let restored = recovered.reauthorize(&c).unwrap();
    assert_eq!(
        restored.batch().unwrap().next_state,
        coordinator.batch().unwrap().next_state
    );
    assert_eq!(restored.snapshot().unwrap(), snapshot);

    let mut resumed = restored;
    resumed.advance(&[], SECOND).unwrap();
    let MeterReconciliationState::Reconciling {
        candidate: Some(candidate),
    } = &meter(&resumed, QuotaMeterType::FiveHour).state
    else {
        panic!("pending event should become eligible after restart")
    };
    assert_eq!(candidate.sample.sample_id, "pending");
    assert_eq!(candidate.consecutive_confirmations, 1);

    assert_eq!(
        resumed.advance(
            &[after(&c, QuotaMeterType::FiveHour, "pending", SECOND, 25.0)],
            SECOND
        ),
        Ok(vec![Admission::Replay])
    );
    let before_conflict = resumed.snapshot().unwrap();
    assert_eq!(
        resumed.advance(
            &[after(&c, QuotaMeterType::FiveHour, "pending", SECOND, 26.0)],
            SECOND
        ),
        Err(SamplingRejection::ConflictingDuplicate)
    );
    assert_eq!(resumed.snapshot().unwrap(), before_conflict);
    assert_eq!(
        resumed.advance(
            &[after(&c, QuotaMeterType::FiveHour, "next", THIRD, 25.0)],
            THIRD
        ),
        Ok(vec![Admission::Accepted])
    );
    let MeterReconciliationState::Stable { evidence } =
        &meter(&resumed, QuotaMeterType::FiveHour).state
    else {
        panic!("restored pending event and new qualified sample should stabilize")
    };
    assert_eq!(evidence.delta_percentage_points.value(), 5.0);
    assert_eq!(evidence.stabilized_at, THIRD);
    assert_eq!(evidence.acquisition_attempt_count, 2);
}

#[test]
fn recovered_data_needs_exact_fresh_authority() {
    let c = synthetic::credential();
    let snapshot = registered(&c).snapshot().unwrap();
    let recovered = TaskQuotaSampling::recover(snapshot).unwrap();
    let mut wrong = c.clone();
    wrong.binding.source_generation_id.push_str("-other");
    assert!(matches!(
        recovered.reauthorize(&wrong),
        Err(SamplingRejection::WrongSourceGeneration)
    ));
}

#[test]
fn deadline_reached_during_downtime_is_applied_on_resumed_evaluation() {
    let c = synthetic::credential();
    let coordinator = initialized(&c, &METERS);
    let snapshot = coordinator.snapshot().unwrap();
    let deadline = snapshot.checkpoint.original_deadline_at.clone();
    let mut resumed = TaskQuotaSampling::recover(snapshot)
        .unwrap()
        .reauthorize(&c)
        .unwrap();
    resumed.advance(&[], DEADLINE).unwrap();
    assert_eq!(
        resumed.snapshot().unwrap().checkpoint.original_deadline_at,
        deadline
    );
    for which in METERS {
        assert!(matches!(
            meter(&resumed, which).state,
            MeterReconciliationState::TimedOut { .. }
                | MeterReconciliationState::AcquisitionFailed { .. }
        ));
    }
    assert!(resumed.is_complete());
}

#[test]
fn durable_transition_cas_failure_keeps_live_coordinator_unchanged() {
    use crate::storage::{SqliteStore, StorageError};
    use crate::telemetry::SourceIdentity;

    let c = synthetic::credential();
    let source = SourceIdentity::new("sampling-cas", synthetic::source_generation(&c)).unwrap();
    let path = std::env::temp_dir().join(format!(
        "codex-meter-sampling-cas-{}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let mut coordinator = registered(&c);
    let initial_snapshot = coordinator.snapshot().unwrap();
    let mut store = SqliteStore::open(&path).unwrap();
    store
        .save_sampling_snapshot(&source, &initial_snapshot, None)
        .unwrap();
    let result = coordinator.transition_and_persist(&mut store, &source, Some(0), |candidate| {
        candidate
            .admit_before(&[before(&c, QuotaMeterType::FiveHour)])
            .map(|_| ())
    });
    assert!(matches!(
        result,
        Err(DurableSamplingError::Storage(
            StorageError::CheckpointRevisionConflict { .. }
        ))
    ));
    assert_eq!(coordinator.snapshot().unwrap(), initial_snapshot);
    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
    let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
}

#[test]
fn wrong_authority_contract_and_production_rollout_are_unsupported() {
    for authority in [
        "synthetic-task-quota/v2",
        "codex-rollout",
        "trusted=true",
        "",
    ] {
        let mut c = synthetic::credential();
        c.authority_contract = authority;
        let mut coordinator = TaskQuotaSampling::default();
        assert_eq!(
            coordinator.register(&c.binding, Some(&c)),
            Err(SamplingRejection::UnsupportedAuthority)
        );
        assert_eq!(coordinator.stage(), SamplingStage::Uninitialized);
    }
}

#[test]
fn unsupported_version_and_association_phase_fail_closed() {
    let mut c = synthetic::credential();
    c.binding.contract_version = 2;
    assert_eq!(
        TaskQuotaSampling::default().register(&c.binding, Some(&c)),
        Err(SamplingRejection::UnsupportedContractVersion)
    );
    c.binding.contract_version = 1;
    c.binding.phase = SamplingPhase::BeforeTask;
    assert_eq!(
        TaskQuotaSampling::default().register(&c.binding, Some(&c)),
        Err(SamplingRejection::WrongPhase)
    );
}

#[test]
fn claim_must_match_sealed_task_session_generation_policy_and_interval() {
    let c = synthetic::credential();
    let cases: [RejectionCase<AssociationClaim>; 7] = [
        (
            |v| v.task.task_id = identity::task_id("other"),
            SamplingRejection::WrongTask,
        ),
        (
            |v| v.task.session_id = Some(identity::session_id("other")),
            SamplingRejection::WrongSession,
        ),
        (
            |v| v.source_generation_id = "other".into(),
            SamplingRejection::WrongSourceGeneration,
        ),
        (
            |v| v.contract_version = 2,
            SamplingRejection::UnsupportedContractVersion,
        ),
        (
            |v| v.phase = SamplingPhase::AfterTask,
            SamplingRejection::WrongPhase,
        ),
        (
            |v| v.policy.required_stable_confirmations = 3,
            SamplingRejection::WrongPolicy,
        ),
        (
            |v| v.task.started_at = None,
            SamplingRejection::ConflictingAssociation,
        ),
    ];
    for (change, reason) in cases {
        let mut claim = c.binding.clone();
        change(&mut claim);
        assert_eq!(
            TaskQuotaSampling::default().register(&claim, Some(&c)),
            Err(reason)
        );
    }
}

#[test]
fn malformed_identity_and_invalid_policy_are_rejected() {
    let mut c = synthetic::credential();
    c.binding.source_generation_id.clear();
    assert_eq!(
        TaskQuotaSampling::default().register(&c.binding, Some(&c)),
        Err(SamplingRejection::InvalidIdentity)
    );
    c = synthetic::credential();
    c.binding.policy.required_stable_confirmations = 0;
    assert_eq!(
        TaskQuotaSampling::default().register(&c.binding, Some(&c)),
        Err(SamplingRejection::Reconciliation(
            ReconciliationError::InvalidPolicy(PolicyValidationError::ZeroStableConfirmations)
        ))
    );
}

#[test]
fn events_must_match_task_session_generation_version_policy_and_authority() {
    let c = synthetic::credential();
    let cases: [RejectionCase<QualifiedQuotaEvent>; 7] = [
        (
            |v| v.credential.binding.task.task_id = identity::task_id("other"),
            SamplingRejection::WrongTask,
        ),
        (
            |v| v.credential.binding.task.session_id = None,
            SamplingRejection::WrongSession,
        ),
        (
            |v| v.credential.binding.source_generation_id = "other".into(),
            SamplingRejection::WrongSourceGeneration,
        ),
        (
            |v| v.credential.binding.contract_version = 2,
            SamplingRejection::UnsupportedContractVersion,
        ),
        (
            |v| v.credential.authority_contract = "codex-rollout",
            SamplingRejection::UnsupportedAuthority,
        ),
        (
            |v| v.credential.binding.policy.deadline = Duration::seconds(90),
            SamplingRejection::WrongPolicy,
        ),
        (
            |v| v.phase = SamplingPhase::AfterTask,
            SamplingRejection::WrongPhase,
        ),
    ];
    for (change, reason) in cases {
        let mut coordinator = registered(&c);
        let mut event = before(&c, METERS[0]);
        change(&mut event);
        assert_eq!(coordinator.admit_before(&[event]), Err(reason));
        coordinator.initialize(END).unwrap();
        assert_eq!(
            meter(&coordinator, METERS[0]).state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::NoSampleBeforeTaskStart
            }
        );
    }
}

#[test]
fn incompatible_meter_and_window_identity_fail_closed() {
    let c = synthetic::credential();
    let mut coordinator = registered(&c);
    let mut event = before(&c, METERS[0]);
    event.attempt.meter_type = METERS[1];
    assert_eq!(
        coordinator.admit_before(&[event]),
        Err(SamplingRejection::MeterMismatch)
    );
    let mut event = before(&c, METERS[0]);
    let QuotaWindowIdentity::Available { meter_type, .. } =
        &mut sample_mut(&mut event).reset_evidence
    else {
        unreachable!()
    };
    *meter_type = METERS[1];
    assert_eq!(
        coordinator.admit_before(&[event]),
        Err(SamplingRejection::MeterMismatch)
    );
}

#[test]
fn qualified_baseline_and_initial_actions_are_exactly_p012() {
    let c = synthetic::credential();
    let coordinator = initialized(&c, &METERS);
    for which in METERS {
        assert_eq!(
            meter(&coordinator, which).before_sample,
            Some(sample_mut(&mut before(&c, which)).clone())
        );
        assert_eq!(
            meter(&coordinator, which).state,
            MeterReconciliationState::AwaitingAfterSample
        );
    }
    assert_eq!(
        coordinator.batch().unwrap().actions,
        METERS.map(|meter_type| ReconciliationAction::RequestAnotherSample {
            meter_type,
            not_before: END.to_owned()
        })
    );
}

#[test]
fn missing_baseline_is_terminal_unavailable_not_zero() {
    let c = synthetic::credential();
    let coordinator = initialized(&c, &[]);
    for which in METERS {
        assert_eq!(
            meter(&coordinator, which).state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::NoSampleBeforeTaskStart
            }
        );
        assert_eq!(meter(&coordinator, which).before_sample, None);
    }
    assert!(coordinator.is_complete());
    assert_eq!(
        coordinator.batch().unwrap().actions,
        [ReconciliationAction::ReconciliationComplete]
    );
}

#[test]
fn missing_start_preserves_p012_reason_even_with_samples() {
    let mut c = synthetic::credential();
    c.binding.task.started_at = None;
    let coordinator = initialized(&c, &METERS);
    for which in METERS {
        assert_eq!(
            meter(&coordinator, which).state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::MissingTaskStart
            }
        );
    }
    assert!(coordinator.is_complete());
}

#[test]
fn stale_and_during_task_samples_cannot_manufacture_a_baseline() {
    let c = synthetic::credential();
    for (at, reason) in [
        (
            "2026-10-04T08:00:00Z",
            BaselineUnavailableReason::SampleOlderThanMaximumAge,
        ),
        (END, BaselineUnavailableReason::NoSampleBeforeTaskStart),
    ] {
        let mut coordinator = registered(&c);
        let event = synthetic::sample(&c, SamplingPhase::BeforeTask, METERS[0], "stale", at, 20.0);
        assert_eq!(
            coordinator.admit_before(&[event]),
            Ok(vec![Admission::Accepted])
        );
        coordinator.initialize(END).unwrap();
        assert_eq!(
            meter(&coordinator, METERS[0]).state,
            MeterReconciliationState::NoBaseline { reason }
        );
    }
}

#[test]
fn both_meters_stable_with_positive_and_verified_zero_deltas() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &METERS);
    let mut events = confirm(&c, METERS[0], 25.0);
    events.extend(confirm(&c, METERS[1], 20.0));
    assert_eq!(
        coordinator.advance(&events, SECOND),
        Ok(vec![Admission::Accepted; 4])
    );
    assert_stable(&coordinator, METERS[0], 5.0);
    assert_stable(&coordinator, METERS[1], 0.0);
    assert!(coordinator.is_complete());
    assert_eq!(
        coordinator.batch().unwrap().actions,
        [ReconciliationAction::ReconciliationComplete]
    );
}

#[test]
fn each_meter_can_stabilize_while_the_other_has_no_baseline() {
    let c = synthetic::credential();
    for which in METERS {
        let mut coordinator = initialized(&c, &[which]);
        assert_eq!(
            coordinator.advance(&confirm(&c, which, 25.0), SECOND),
            Ok(vec![Admission::Accepted; 2])
        );
        assert_stable(&coordinator, which, 5.0);
        let other = if which == METERS[0] {
            METERS[1]
        } else {
            METERS[0]
        };
        assert_eq!(
            meter(&coordinator, other).state,
            MeterReconciliationState::NoBaseline {
                reason: BaselineUnavailableReason::NoSampleBeforeTaskStart
            }
        );
        assert!(coordinator.is_complete());
    }
}

#[test]
fn either_meter_can_cross_reset_without_corrupting_the_other() {
    let c = synthetic::credential();
    for reset_meter in METERS {
        let other = if reset_meter == METERS[0] {
            METERS[1]
        } else {
            METERS[0]
        };
        let mut reset = after(&c, reset_meter, "reset", FIRST, 3.0);
        let QuotaWindowIdentity::Available {
            observed_reset_at, ..
        } = &mut sample_mut(&mut reset).reset_evidence
        else {
            unreachable!()
        };
        *observed_reset_at = Some("2026-10-04T10:00:00Z".into());
        // Cross a confirmed boundary rather than inferring movement from a decrease.
        let mut baseline = before(&c, reset_meter);
        let QuotaWindowIdentity::Available {
            observed_reset_at, ..
        } = &mut sample_mut(&mut baseline).reset_evidence
        else {
            unreachable!()
        };
        *observed_reset_at = Some("2026-10-04T10:00:00Z".into());
        // Register the boundary baseline in a fresh coordinator.
        let mut coordinator = registered(&c);
        coordinator
            .admit_before(&[baseline, before(&c, other)])
            .unwrap();
        coordinator.initialize(END).unwrap();
        let mut events = vec![reset];
        events.extend(confirm(&c, other, 25.0));
        assert_eq!(
            coordinator.advance(&events, SECOND),
            Ok(vec![Admission::Accepted; 3])
        );
        let MeterReconciliationState::ResetCrossed { evidence } =
            &meter(&coordinator, reset_meter).state
        else {
            panic!("reset")
        };
        assert_eq!(evidence.meter_type, reset_meter);
        assert_eq!(evidence.evidence, BoundaryEvidence::ObservedReset);
        assert_eq!(evidence.boundary_sample.used_percent.value, Some(3.0));
        assert_stable(&coordinator, other, 5.0);
        assert!(coordinator.is_complete());
    }
}

#[test]
fn plan_discontinuity_uses_p012_terminal_evidence() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    let mut event = after(&c, METERS[0], "changed", FIRST, 25.0);
    sample_mut(&mut event).configuration.plan = ConfigurationValue::observed("pro");
    assert_eq!(
        coordinator.advance(&[event], FIRST),
        Ok(vec![Admission::Accepted])
    );
    let MeterReconciliationState::PlanDiscontinuity { evidence } =
        &meter(&coordinator, METERS[0]).state
    else {
        panic!("plan discontinuity")
    };
    assert_eq!(
        evidence.before_sample.configuration.plan.value.as_deref(),
        Some("plus")
    );
    assert_eq!(
        evidence.relevant_sample.configuration.plan.value.as_deref(),
        Some("pro")
    );
    assert!(coordinator.is_complete());
}

#[test]
fn decreasing_same_window_is_unstable_with_exact_reason() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    assert_eq!(
        coordinator.advance(&[after(&c, METERS[0], "decrease", FIRST, 19.0)], FIRST),
        Ok(vec![Admission::Accepted])
    );
    let MeterReconciliationState::MeterUnstable { evidence } =
        &meter(&coordinator, METERS[0]).state
    else {
        panic!("unstable")
    };
    assert_eq!(
        evidence.reason,
        MeterInstabilityReason::SameObservedResetUsageDecreased
    );
    assert_eq!(evidence.relevant_sample.used_percent.value, Some(19.0));
    assert!(coordinator.is_complete());
}

#[test]
fn threshold_and_confirmation_count_are_owned_by_p012() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    coordinator
        .advance(&[after(&c, METERS[0], "early", END, 25.0)], END)
        .unwrap();
    assert_eq!(
        meter(&coordinator, METERS[0]).state,
        MeterReconciliationState::Reconciling { candidate: None }
    );
    coordinator
        .advance(&[after(&c, METERS[0], "a", FIRST, 25.0)], FIRST)
        .unwrap();
    let MeterReconciliationState::Reconciling {
        candidate: Some(candidate),
    } = &meter(&coordinator, METERS[0]).state
    else {
        panic!("one confirmation")
    };
    assert_eq!(candidate.consecutive_confirmations, 1);
    assert_eq!(candidate.sample.used_percent.value, Some(25.0));
    assert!(!coordinator.is_complete());
    assert_eq!(
        coordinator.batch().unwrap().actions,
        [ReconciliationAction::AwaitSample {
            meter_type: METERS[0],
            due_at: SECOND.into()
        }]
    );
}

#[test]
fn insufficient_confirmations_expire_without_promoting_the_candidate() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    coordinator
        .advance(&[after(&c, METERS[0], "a", FIRST, 25.0)], FIRST)
        .unwrap();
    assert_eq!(coordinator.advance(&[], DEADLINE), Ok(vec![]));
    let MeterReconciliationState::TimedOut { evidence } = &meter(&coordinator, METERS[0]).state
    else {
        panic!("timeout")
    };
    assert_eq!(
        evidence
            .latest_candidate
            .as_ref()
            .unwrap()
            .used_percent
            .value,
        Some(25.0)
    );
    assert_eq!(evidence.deadline, DEADLINE);
    assert_eq!(evidence.acquisition_attempt_count, 1);
    assert!(coordinator.is_complete());
}

#[test]
fn either_meter_can_fail_or_timeout_while_the_other_stabilizes() {
    let c = synthetic::credential();
    for unavailable in METERS {
        for fail in [false, true] {
            let other = if unavailable == METERS[0] {
                METERS[1]
            } else {
                METERS[0]
            };
            let mut coordinator = initialized(&c, &METERS);
            let mut events = confirm(&c, other, 25.0);
            if fail {
                events.push(synthetic::failure(&c, unavailable));
            }
            assert_eq!(
                coordinator.advance(&events, DEADLINE),
                Ok(vec![Admission::Accepted; events.len()])
            );
            let evidence = match &meter(&coordinator, unavailable).state {
                MeterReconciliationState::AcquisitionFailed { evidence } if fail => evidence,
                MeterReconciliationState::TimedOut { evidence } if !fail => evidence,
                state => panic!("unexpected {state:?}"),
            };
            assert_eq!(evidence.latest_candidate, None);
            assert_eq!(evidence.last_successful_sample, None);
            assert_eq!(evidence.acquisition_attempt_count, usize::from(fail));
            assert_eq!(evidence.deadline, DEADLINE);
            assert_stable(&coordinator, other, 5.0);
            assert!(coordinator.is_complete());
            if fail {
                assert!(coordinator.batch().unwrap().outputs.contains(
                    &ReconciliationOutput::AcquisitionFailed {
                        meter_type: unavailable,
                        reason: AcquisitionFailureReason::SourceUnavailable
                    }
                ));
            }
        }
    }
}

#[test]
fn duplicate_replay_cannot_supply_confirmations_or_movement() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    let event = after(&c, METERS[0], "a", FIRST, 25.0);
    assert_eq!(
        coordinator.advance(&[event.clone(), event.clone()], FIRST),
        Ok(vec![Admission::Accepted, Admission::Replay])
    );
    assert_eq!(meter(&coordinator, METERS[0]).acquisition_attempt_count, 1);
    assert!(!coordinator.is_complete());
    let state = coordinator.batch().unwrap().next_state.clone();
    assert_eq!(
        coordinator.advance(std::slice::from_ref(&event), FIRST),
        Ok(vec![Admission::Replay])
    );
    assert_eq!(coordinator.batch().unwrap().next_state, state);
    coordinator
        .advance(&[after(&c, METERS[0], "b", SECOND, 25.0)], SECOND)
        .unwrap();
    assert_stable(&coordinator, METERS[0], 5.0);
    let state = coordinator.batch().unwrap().next_state.clone();
    assert_eq!(
        coordinator.advance(&[event], SECOND),
        Ok(vec![Admission::Replay])
    );
    assert_eq!(coordinator.batch().unwrap().next_state, state);
}

#[test]
fn baseline_duplicate_and_conflict_are_checked_before_selection() {
    let c = synthetic::credential();
    let mut coordinator = registered(&c);
    let event = before(&c, METERS[0]);
    assert_eq!(
        coordinator.admit_before(&[event.clone(), event.clone()]),
        Ok(vec![Admission::Accepted, Admission::Replay])
    );
    let mut conflict = event.clone();
    sample_mut(&mut conflict).used_percent = PercentageMetric::observed(50.0);
    assert_eq!(
        coordinator.admit_before(&[conflict]),
        Err(SamplingRejection::ConflictingDuplicate)
    );
    coordinator.initialize(END).unwrap();
    assert_eq!(
        meter(&coordinator, METERS[0])
            .before_sample
            .as_ref()
            .unwrap()
            .used_percent
            .value,
        Some(20.0)
    );
}

#[test]
fn same_identity_changed_value_or_timestamp_fails_atomically() {
    let c = synthetic::credential();
    for change_time in [false, true] {
        let mut coordinator = initialized(&c, &[METERS[0]]);
        let event = after(&c, METERS[0], "a", FIRST, 25.0);
        coordinator
            .advance(std::slice::from_ref(&event), FIRST)
            .unwrap();
        let state = coordinator.batch().unwrap().clone();
        let mut conflict = event;
        if change_time {
            conflict.attempt.sampled_at = SECOND.into();
            sample_mut(&mut conflict).sampled_at = SECOND.into();
        } else {
            sample_mut(&mut conflict).used_percent = PercentageMetric::observed(26.0);
        }
        let valid = after(&c, METERS[0], "b", SECOND, 25.0);
        assert_eq!(
            coordinator.advance(&[valid.clone(), conflict], SECOND),
            Err(SamplingRejection::ConflictingDuplicate)
        );
        assert_eq!(coordinator.batch(), Some(&state));
        assert_eq!(
            coordinator.advance(&[valid], SECOND),
            Ok(vec![Admission::Accepted])
        );
        assert_stable(&coordinator, METERS[0], 5.0);
    }
}

#[test]
fn post_terminal_new_evidence_is_explicitly_rejected() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &METERS);
    coordinator
        .advance(&confirm(&c, METERS[0], 25.0), SECOND)
        .unwrap();
    let state = coordinator.batch().unwrap().clone();
    assert_eq!(
        coordinator.advance(&[after(&c, METERS[0], "late", DEADLINE, 99.0)], DEADLINE),
        Err(SamplingRejection::TerminalMeter)
    );
    assert_eq!(coordinator.batch(), Some(&state));
    assert_eq!(
        coordinator.advance(&confirm(&c, METERS[1], 20.0), SECOND),
        Ok(vec![Admission::Accepted; 2])
    );
    assert_stable(&coordinator, METERS[1], 0.0);
    assert!(coordinator.is_complete());
}

#[test]
fn initialized_source_mismatch_and_wrong_phase_leave_state_unchanged() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    let state = coordinator.batch().unwrap().clone();
    let mut event = after(&c, METERS[0], "a", FIRST, 25.0);
    event.credential.binding.source_generation_id = "replaced".into();
    assert_eq!(
        coordinator.advance(&[event], FIRST),
        Err(SamplingRejection::WrongSourceGeneration)
    );
    assert_eq!(
        coordinator.advance(&[before(&c, METERS[0])], FIRST),
        Err(SamplingRejection::WrongPhase)
    );
    assert_eq!(coordinator.batch(), Some(&state));
}

#[test]
fn equivalent_batch_orders_produce_the_exact_p012_batch() {
    let c = synthetic::credential();
    let mut left = initialized(&c, &METERS);
    let mut right = initialized(&c, &METERS);
    let initial = left.batch().unwrap().next_state.clone();
    let mut events = confirm(&c, METERS[0], 25.0);
    events.extend(confirm(&c, METERS[1], 20.0));
    let inputs: Vec<_> = events.iter().map(|v| v.attempt.clone()).collect();
    let expected = reconcile_task_quota_at(&initial, &inputs, &c.binding.policy, SECOND).unwrap();
    left.advance(&events, SECOND).unwrap();
    events.reverse();
    right.advance(&events, SECOND).unwrap();
    assert_eq!(left.batch(), Some(&expected));
    assert_eq!(right.batch(), Some(&expected));
}

#[test]
fn stale_post_event_does_not_replace_latest_candidate() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    coordinator
        .advance(&[after(&c, METERS[0], "a", FIRST, 25.0)], FIRST)
        .unwrap();
    let candidate = meter(&coordinator, METERS[0]).state.clone();
    coordinator
        .advance(&[after(&c, METERS[0], "stale", END, 21.0)], FIRST)
        .unwrap();
    assert_eq!(meter(&coordinator, METERS[0]).state, candidate);
    assert_eq!(
        meter(&coordinator, METERS[0])
            .last_successful_sample
            .as_ref()
            .unwrap()
            .sample_id,
        "a"
    );
    coordinator
        .advance(&[after(&c, METERS[0], "b", SECOND, 25.0)], SECOND)
        .unwrap();
    let MeterReconciliationState::Stable { evidence } = &meter(&coordinator, METERS[0]).state
    else {
        panic!("stable")
    };
    assert_eq!(evidence.delta_percentage_points.value(), 5.0);
    // P012 counts acquisition attempts even when tracking ignores stale evidence.
    assert_eq!(evidence.acquisition_attempt_count, 3);
}

#[test]
fn future_event_replay_waits_for_explicit_evaluation_and_deadline_is_final() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    let events = confirm(&c, METERS[0], 25.0);
    coordinator.advance(&events, END).unwrap();
    assert_eq!(meter(&coordinator, METERS[0]).acquisition_attempt_count, 0);
    assert_eq!(
        coordinator.advance(&events, SECOND),
        Ok(vec![Admission::Replay; 2])
    );
    assert_stable(&coordinator, METERS[0], 5.0);
    let mut coordinator = initialized(&c, &[METERS[0]]);
    let late = after(&c, METERS[0], "late", "2026-10-04T10:06:01Z", 25.0);
    coordinator
        .advance(std::slice::from_ref(&late), DEADLINE)
        .unwrap();
    let MeterReconciliationState::TimedOut { evidence } = &meter(&coordinator, METERS[0]).state
    else {
        panic!("timeout")
    };
    assert_eq!(evidence.acquisition_attempt_count, 0);
    let state = coordinator.batch().unwrap().next_state.clone();
    assert_eq!(
        coordinator.advance(&[late], "2026-10-04T10:06:01Z"),
        Ok(vec![Admission::Replay])
    );
    assert_eq!(coordinator.batch().unwrap().next_state, state);
}

#[test]
fn p012_errors_and_evaluation_reversal_do_not_mutate_state() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    let state = coordinator.batch().unwrap().clone();
    let mut invalid = after(&c, METERS[0], "invalid", FIRST, 25.0);
    sample_mut(&mut invalid).used_percent = PercentageMetric::observed(101.0);
    assert_eq!(
        coordinator.advance(&[invalid], FIRST),
        Err(SamplingRejection::Reconciliation(
            ReconciliationError::Tracking(QuotaTrackingError::InvalidUsedPercentage)
        ))
    );
    assert_eq!(coordinator.batch(), Some(&state));
    let mut invalid = after(&c, METERS[0], "invalid", FIRST, 25.0);
    invalid.attempt.sampled_at = SECOND.into();
    assert_eq!(
        coordinator.advance(&[invalid], SECOND),
        Err(SamplingRejection::Reconciliation(
            ReconciliationError::AttemptSampleTimestampMismatch
        ))
    );
    assert_eq!(
        coordinator.advance(&[], START),
        Err(SamplingRejection::EvaluationTimeReversed)
    );
    assert_eq!(coordinator.batch(), Some(&state));
    assert_eq!(
        coordinator.advance(&[after(&c, METERS[0], "invalid", FIRST, 25.0)], FIRST),
        Ok(vec![Admission::Accepted])
    );
}

#[test]
fn workflow_guards_and_missing_task_end_are_typed() {
    let c = synthetic::credential();
    let mut coordinator = TaskQuotaSampling::default();
    assert_eq!(
        coordinator.admit_before(&[]),
        Err(SamplingRejection::NotRegistered)
    );
    assert_eq!(
        coordinator.initialize(END).unwrap_err(),
        SamplingRejection::NotRegistered
    );
    assert_eq!(
        coordinator.advance(&[], END),
        Err(SamplingRejection::NotInitialized)
    );
    let mut c = c;
    c.binding.task.ended_at = None;
    let mut coordinator = registered(&c);
    assert_eq!(
        coordinator.initialize(END).unwrap_err(),
        SamplingRejection::Reconciliation(ReconciliationError::MissingTaskEnd)
    );
    assert_eq!(coordinator.stage(), SamplingStage::QualifiedAssociation);
    let mut coordinator = initialized(&synthetic::credential(), &[]);
    assert_eq!(
        coordinator.initialize(END).unwrap_err(),
        SamplingRejection::AlreadyInitialized
    );
    assert_eq!(
        coordinator.admit_before(&[]),
        Err(SamplingRejection::AlreadyInitialized)
    );
}

#[test]
fn known_local_overlap_survives_reconciliation_without_allocating_movement() {
    let mut c = synthetic::credential();
    c.binding
        .task
        .known_overlapping_task_ids
        .insert(identity::task_id("other"));
    let mut coordinator = initialized(&c, &[METERS[0]]);
    coordinator
        .advance(&confirm(&c, METERS[0], 25.0), SECOND)
        .unwrap();
    assert_eq!(
        coordinator.batch().unwrap().next_state.attribution_risk,
        AttributionRisk::KnownLocalOverlap {
            task_ids: c.binding.task.known_overlapping_task_ids.clone()
        }
    );
    assert_stable(&coordinator, METERS[0], 5.0);
}

#[test]
fn no_known_local_overlap_does_not_prove_external_isolation() {
    let c = synthetic::credential();
    let mut coordinator = initialized(&c, &[METERS[0]]);
    coordinator
        .advance(&confirm(&c, METERS[0], 25.0), SECOND)
        .unwrap();
    assert_eq!(
        coordinator.batch().unwrap().next_state.attribution_risk,
        AttributionRisk::NoKnownLocalOverlap
    );
    // The API returns P012 risk only, with no account-isolation proof or grade.
    assert_eq!(
        coordinator
            .batch()
            .unwrap()
            .next_state
            .task
            .known_overlapping_task_ids
            .len(),
        0
    );
    assert_stable(&coordinator, METERS[0], 5.0);
}

mod finalization;
