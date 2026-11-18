use super::*;
use crate::storage::{
    RuntimeCheckpoint, RuntimeRecoveryState, SamplingFinalization, SamplingFinalizationRequest,
    SourceBoundaryFingerprint, SqliteStore, StorageError, StoredObservation, UsageEvidenceBatch,
};
use crate::telemetry::{
    build_observation, AttributedTokenEvent, EvidenceValidity, IsolationEvidence,
    NormalizedEventType, NormalizedObservation, NormalizedTokenEvent, NormalizedTokenPayload,
    ObservationInput, ObservationLifecycle, QualityGrade, ResetStatus, RolloutCursor,
    SourceIdentity, TokenCounters, TokenMetric, TokenPayloadKind, TokenTelemetryCompleteness,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fixture {
    path: PathBuf,
    source: SourceIdentity,
    credential: AssociationCredential,
}

impl Fixture {
    fn new() -> Self {
        static NEXT_DB: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "codex-meter-finalization-{}-{}",
            std::process::id(),
            NEXT_DB.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let credential = synthetic::credential();
        let source =
            SourceIdentity::new("fixture-rollout", synthetic::source_generation(&credential))
                .unwrap();
        Self {
            path,
            source,
            credential,
        }
    }

    fn open(&self) -> SqliteStore {
        SqliteStore::open(self.path.join("meter.sqlite")).unwrap()
    }

    fn request<'a>(
        &'a self,
        snapshot: &SamplingSnapshot,
        checkpoint_revision: u64,
        workflow_id: &'a str,
    ) -> SamplingFinalizationRequest<'a> {
        assert_eq!(workflow_id, snapshot.checkpoint.workflow_id());
        SamplingFinalizationRequest {
            source: &self.source,
            task: &self.credential.binding.task,
            workflow_id,
            checkpoint_expected_revision: checkpoint_revision,
            observation_expected_revision: None,
            token_events: &[],
        }
    }

    fn checkpoint(&self, store: &SqliteStore) -> RuntimeCheckpoint {
        store
            .load_runtime_checkpoint(&self.source)
            .unwrap()
            .unwrap()
    }

    fn recover(&self, checkpoint: &RuntimeCheckpoint, workflow_id: &str) -> TaskQuotaSampling {
        let workflow = checkpoint.state.qualified_sampling[workflow_id].clone();
        let snapshot = SamplingSnapshot {
            reconciliation: checkpoint
                .state
                .active_reconciliations
                .get(&workflow.task_id)
                .cloned(),
            checkpoint: workflow,
        };
        TaskQuotaSampling::recover(snapshot)
            .unwrap()
            .reauthorize(&self.credential)
            .unwrap()
    }

    fn token(&self) -> AttributedTokenEvent {
        let config = ConfigurationIdentity {
            plan: ConfigurationValue::observed("plus"),
            model: ConfigurationValue::observed("fixture-model"),
            reasoning_level: ConfigurationValue::unavailable(),
            speed_mode: ConfigurationValue::unavailable(),
            codex_version: ConfigurationValue::unavailable(),
        };
        AttributedTokenEvent {
            normalized_event: NormalizedTokenEvent {
                schema_version: SchemaVersion::V1,
                event_id: "fixture-response".into(),
                event_type: NormalizedEventType::TokenCountersUpdated,
                source_instance_id: identity::source_instance_id(&self.source),
                safe_cursor_id: None,
                event_at: END.into(),
                task_id: Some(self.credential.binding.task.task_id.clone()),
                session_id: self.credential.binding.task.session_id.clone(),
                payload: NormalizedTokenPayload {
                    kind: TokenPayloadKind::TokenCounters,
                    token_counters: TokenCounters {
                        uncached_input: TokenMetric::unavailable(),
                        cached_input: TokenMetric::unavailable(),
                        output: TokenMetric::unavailable(),
                        reasoning_output: TokenMetric::unavailable(),
                        raw_total: TokenMetric::observed(37),
                    },
                },
            },
            effective_configuration: config,
            configuration_fingerprint: "fixture-config".into(),
            task_lifecycle: None,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn terminal(c: &AssociationCredential, meters: &[QuotaMeterType], used: f64) -> TaskQuotaSampling {
    let mut coordinator = initialized(c, meters);
    let samples: Vec<_> = meters
        .iter()
        .flat_map(|meter| confirm(c, *meter, used))
        .collect();
    coordinator.advance(&samples, SECOND).unwrap();
    assert_eq!(
        coordinator.advance(&samples, SECOND),
        Ok(vec![Admission::Replay; samples.len()])
    );
    assert!(coordinator.is_complete());
    coordinator
}

fn expected(fixture: &Fixture, sampling: &TaskQuotaSampling) -> NormalizedObservation {
    let snapshot = sampling.snapshot().unwrap();
    let mut observation = build_observation(&ObservationInput::new(
        sampling.batch().unwrap().next_state.clone(),
        vec![],
        TokenTelemetryCompleteness::Incomplete,
        IsolationEvidence::UnknownExternalUsage,
        snapshot.checkpoint.last_evaluation_at,
    ))
    .unwrap();
    observation.source_instance_id = Some(identity::source_instance_id(&fixture.source));
    observation
}

fn checkpoint_payload(store: &SqliteStore, source: &SourceIdentity) -> (String, String) {
    let (json, checksum): (String, String) = store.connection().query_row(
        "SELECT state_json, state_sha256 FROM runtime_checkpoints WHERE rollout_id = ?1 AND source_generation = ?2",
        [source.rollout_id(), source.source_generation()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(checksum, format!("{:x}", Sha256::digest(json.as_bytes())));
    (json, checksum)
}

fn assert_observation(store: &SqliteStore, stored: &StoredObservation, revision: u64) {
    let id = &stored.observation.observation_id;
    let payload = serde_json::to_vec(&stored.observation).unwrap();
    assert_eq!(stored.storage_revision, revision);
    assert_eq!(
        stored.payload_sha256,
        format!("{:x}", Sha256::digest(&payload))
    );
    assert_eq!(store.load_observation(id).unwrap().as_ref(), Some(stored));
    let actual: String = store
        .connection()
        .query_row(
            "SELECT observation_json FROM observations WHERE observation_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(actual.as_bytes(), payload);
}

fn commit(
    fixture: &Fixture,
    store: &mut SqliteStore,
    sampling: &TaskQuotaSampling,
    events: &[AttributedTokenEvent],
) -> StoredObservation {
    let snapshot = sampling.snapshot().unwrap();
    let id = snapshot.checkpoint.workflow_id();
    let before = store
        .save_sampling_snapshot(&fixture.source, &snapshot, None)
        .unwrap();
    let mut req = fixture.request(&snapshot, before.revision, &id);
    req.token_events = events;
    let SamplingFinalization::Committed {
        observation,
        checkpoint,
    } = store
        .finalize_sampling(&fixture.recover(&before, &id), &req)
        .unwrap()
    else {
        panic!("first finalization must commit")
    };
    assert_eq!(checkpoint.revision, before.revision + 1);
    assert_eq!(checkpoint.cursor, before.cursor);
    assert!(checkpoint.state.active_reconciliations.is_empty());
    assert!(checkpoint.state.qualified_sampling.is_empty());
    assert!(checkpoint.state.sampling_finalizations.contains_key(&id));
    assert_eq!(fixture.checkpoint(store), checkpoint);
    assert_observation(store, &observation, 1);
    let (json, _) = checkpoint_payload(store, &fixture.source);
    assert!(json.contains("\"state_format_version\":4"));
    observation
}

#[test]
fn stable_independent_meters_preserve_zero_and_missing_baselines() {
    use QuotaMeterType::{FiveHour, Weekly};
    for (meters, used, five, weekly) in [
        (&METERS[..], 25.0, Some(5.0), Some(5.0)),
        (&METERS[..], 20.0, Some(0.0), Some(0.0)),
        (&[FiveHour][..], 25.0, Some(5.0), None),
        (&[Weekly][..], 25.0, None, Some(5.0)),
        (&[][..], 20.0, None, None),
    ] {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let sampling = terminal(&fixture.credential, meters, used);
        let row = commit(&fixture, &mut store, &sampling, &[]).observation;
        assert_eq!(row, expected(&fixture, &sampling));
        assert_eq!(row.lifecycle_state, ObservationLifecycle::Incomplete);
        assert_eq!(
            row.token_evidence.status.validity,
            EvidenceValidity::Unavailable
        );
        assert_eq!(row.token_evidence.raw_token_counters.raw_total.value, None);
        for (evidence, delta) in [
            (&row.quota_evidence.five_hour, five),
            (&row.quota_evidence.weekly, weekly),
        ] {
            assert_eq!(evidence.delta_percentage_points, delta);
            if delta.is_some() {
                assert_eq!(evidence.status.validity, EvidenceValidity::Valid);
                assert_eq!(evidence.status.quality, QualityGrade::C);
            } else {
                assert_eq!(evidence.status.validity, EvidenceValidity::Unavailable);
                assert!(evidence.before_sample.is_none());
            }
        }
    }
}

#[test]
fn reset_timeout_and_acquisition_failure_keep_terminal_meter_outcomes() {
    for timed_out in [false, true] {
        let fixture = Fixture::new();
        let c = &fixture.credential;
        let mut store = fixture.open();
        let mut sampling = registered(c);
        let mut baseline = before(c, QuotaMeterType::FiveHour);
        let mut reset = after(c, QuotaMeterType::FiveHour, "reset", FIRST, 3.0);
        if !timed_out {
            for event in [&mut baseline, &mut reset] {
                let QuotaWindowIdentity::Available {
                    observed_reset_at, ..
                } = &mut sample_mut(event).reset_evidence
                else {
                    unreachable!()
                };
                *observed_reset_at = Some(START.to_owned());
            }
        }
        sampling
            .admit_before(&[baseline, before(c, QuotaMeterType::Weekly)])
            .unwrap();
        sampling.initialize(END).unwrap();
        let mut events = confirm(c, QuotaMeterType::Weekly, 25.0);
        if !timed_out {
            events.push(reset);
        }
        sampling
            .advance(&events, if timed_out { DEADLINE } else { SECOND })
            .unwrap();
        let row = commit(&fixture, &mut store, &sampling, &[]).observation;
        let quota = row.quota_evidence;
        assert_eq!(quota.five_hour.delta_percentage_points, None);
        assert_eq!(quota.weekly.delta_percentage_points, Some(5.0));
        assert_eq!(quota.weekly.status.validity, EvidenceValidity::Valid);
        if timed_out {
            assert!(matches!(
                meter(&sampling, QuotaMeterType::FiveHour).state,
                MeterReconciliationState::TimedOut { .. }
            ));
            assert_eq!(row.timing.finalized_at.as_deref(), Some(DEADLINE));
        } else {
            assert_eq!(quota.five_hour.reset_status, ResetStatus::Detected);
        }
    }
    let fixture = Fixture::new();
    let mut sampling = initialized(&fixture.credential, &METERS);
    let failures = METERS.map(|meter| synthetic::failure(&fixture.credential, meter));
    sampling.advance(&failures, DEADLINE).unwrap();
    let row = commit(&fixture, &mut fixture.open(), &sampling, &[]).observation;
    assert_eq!(row, expected(&fixture, &sampling));
    assert!(matches!(
        meter(&sampling, QuotaMeterType::FiveHour).state,
        MeterReconciliationState::AcquisitionFailed { .. }
    ));
}

#[test]
fn observed_tokens_and_duplicates_do_not_prove_complete_coverage() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let sampling = terminal(&fixture.credential, &METERS, 25.0);
    let token = fixture.token();
    let row = commit(&fixture, &mut store, &sampling, &[token.clone(), token]).observation;
    assert_eq!(
        row.token_evidence.raw_token_counters.raw_total.value,
        Some(37)
    );
    assert_eq!(
        row.token_evidence.status.validity,
        EvidenceValidity::Incomplete
    );
    assert_eq!(
        row.quota_evidence.five_hour.status.validity,
        EvidenceValidity::Valid
    );
    assert_eq!(
        row.quota_evidence.weekly.status.validity,
        EvidenceValidity::Valid
    );
    assert_eq!(row.summary_quality, QualityGrade::D);
}

#[test]
fn awaiting_and_reconciling_meters_block_finalization() {
    for with_first_sample in [false, true] {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let c = &fixture.credential;
        let mut sampling = initialized(c, &METERS);
        let mut events = confirm(c, QuotaMeterType::FiveHour, 25.0);
        if with_first_sample {
            events.push(after(c, QuotaMeterType::Weekly, "a", FIRST, 26.0));
        }
        sampling.advance(&events, SECOND).unwrap();
        if with_first_sample {
            assert!(matches!(
                meter(&sampling, QuotaMeterType::Weekly).state,
                MeterReconciliationState::Reconciling { .. }
            ));
        } else {
            assert!(matches!(
                meter(&sampling, QuotaMeterType::Weekly).state,
                MeterReconciliationState::AwaitingAfterSample
            ));
        }
        let snapshot = sampling.snapshot().unwrap();
        let id = snapshot.checkpoint.workflow_id();
        let before = store
            .save_sampling_snapshot(&fixture.source, &snapshot, None)
            .unwrap();
        let payload = checkpoint_payload(&store, &fixture.source);
        assert!(matches!(
            store.finalize_sampling(&sampling, &fixture.request(&snapshot, before.revision, &id)),
            Err(StorageError::CheckpointStateInvalid)
        ));
        assert_eq!(fixture.checkpoint(&store), before);
        assert_eq!(checkpoint_payload(&store, &fixture.source), payload);
        assert!(store
            .load_observation(&expected(&fixture, &sampling).observation_id)
            .unwrap()
            .is_none());
    }
}

#[test]
fn identity_authority_policy_deadline_and_persisted_snapshot_must_agree() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let c = &fixture.credential;
    let sampling = terminal(c, &METERS, 25.0);
    let snapshot = sampling.snapshot().unwrap();
    let id = snapshot.checkpoint.workflow_id();
    let before = store
        .save_sampling_snapshot(&fixture.source, &snapshot, None)
        .unwrap();
    let payload = checkpoint_payload(&store, &fixture.source);
    let mut request = fixture.request(&snapshot, before.revision, &id);
    let wrong_source = SourceIdentity::new("fixture-rollout", "different-generation").unwrap();
    request.source = &wrong_source;
    assert!(matches!(
        store.finalize_sampling(&sampling, &request),
        Err(StorageError::CheckpointSourceMismatch)
    ));
    request.source = &fixture.source;
    request.workflow_id = "wrong-workflow";
    assert!(matches!(
        store.finalize_sampling(&sampling, &request),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    request.workflow_id = &id;
    for task in [
        TaskReconciliationTarget {
            task_id: "wrong-task".into(),
            ..request.task.clone()
        },
        TaskReconciliationTarget {
            session_id: None,
            ..request.task.clone()
        },
        TaskReconciliationTarget {
            started_at: Some(BEFORE.into()),
            ..request.task.clone()
        },
    ] {
        let mut wrong = fixture.request(&snapshot, before.revision, &id);
        wrong.task = &task;
        assert!(matches!(
            store.finalize_sampling(&sampling, &wrong),
            Err(StorageError::CheckpointIdentityConflict)
        ));
    }
    let mut unbound = c.clone();
    unbound.binding.task.session_id = None;
    let unbound_sampling = terminal(&unbound, &METERS, 25.0);
    let unbound_id = unbound_sampling
        .snapshot()
        .unwrap()
        .checkpoint
        .workflow_id();
    let unbound_request = SamplingFinalizationRequest {
        task: &unbound.binding.task,
        workflow_id: &unbound_id,
        ..fixture.request(&snapshot, before.revision, &id)
    };
    assert!(matches!(
        store.finalize_sampling(&unbound_sampling, &unbound_request),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    let different = terminal(c, &METERS, 26.0);
    assert!(matches!(
        store.finalize_sampling(&different, &request),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    let mut unauthorized = TaskQuotaSampling::default();
    assert_eq!(
        unauthorized.register(&c.binding, None),
        Err(SamplingRejection::MissingAuthority)
    );
    assert!(matches!(
        store.finalize_sampling(&unauthorized, &request),
        Err(StorageError::CheckpointStateInvalid)
    ));
    let mut wrong_authority = c.clone();
    wrong_authority.authority_contract = "unrecognized";
    assert!(TaskQuotaSampling::recover(snapshot.clone())
        .unwrap()
        .reauthorize(&wrong_authority)
        .is_err());
    let mut invalid = snapshot.clone();
    invalid.checkpoint.policy.deadline = Duration::ZERO;
    assert!(matches!(
        store.save_sampling_snapshot(&fixture.source, &invalid, Some(before.revision)),
        Err(StorageError::CheckpointStateInvalid)
    ));
    let mut invalid = snapshot.clone();
    invalid.checkpoint.original_deadline_at = SECOND.into();
    assert!(matches!(
        store.save_sampling_snapshot(&fixture.source, &invalid, Some(before.revision)),
        Err(StorageError::CheckpointStateInvalid)
    ));
    let mut token = fixture.token();
    token.normalized_event.source_instance_id = identity::source_instance_id(&wrong_source);
    let tokens = [token];
    request.token_events = &tokens;
    assert!(matches!(
        store.finalize_sampling(&sampling, &request),
        Err(StorageError::ObservationIdentityConflict)
    ));
    assert_eq!(fixture.checkpoint(&store), before);
    assert_eq!(checkpoint_payload(&store, &fixture.source), payload);
    assert!(store
        .load_observation(&expected(&fixture, &sampling).observation_id)
        .unwrap()
        .is_none());
}

#[test]
fn reopened_database_recovers_unacknowledged_commit_and_prevents_restoration() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let sampling = terminal(&fixture.credential, &METERS, 25.0);
    let snapshot = sampling.snapshot().unwrap();
    let id = snapshot.checkpoint.workflow_id();
    let mut other_credential = fixture.credential.clone();
    other_credential.binding.task.task_id = identity::task_id("unrelated");
    let other_sampling = initialized(&other_credential, &METERS);
    let other = other_sampling.snapshot().unwrap();
    let other_id = other.checkpoint.workflow_id();

    let cursor = RolloutCursor::from_checkpoint(&fixture.source, 99, Some(7));
    let boundary = SourceBoundaryFingerprint {
        committed_offset: 99,
        last_line_sha256: Some("a".repeat(64)),
        file_identity_sha256: Some("b".repeat(64)),
    };
    let (initial, _) = store
        .commit_usage_batch(
            &fixture.source,
            &cursor,
            &RuntimeRecoveryState::default(),
            None,
            UsageEvidenceBatch {
                tokens: &[],
                quotas: &[],
                diagnostics: &[],
                observations: &[],
                boundary: &boundary,
            },
        )
        .unwrap();
    let before = store
        .save_sampling_snapshot(&fixture.source, &snapshot, Some(initial.revision))
        .unwrap();
    let before = store
        .save_sampling_snapshot(&fixture.source, &other, Some(before.revision))
        .unwrap();
    let original_payload = checkpoint_payload(&store, &fixture.source);
    let req = fixture.request(&snapshot, before.revision, &id);
    let expected_row = expected(&fixture, &sampling);
    drop(store);

    // Reopen after stopping before the terminal transaction.
    let mut store = fixture.open();
    let recovered = fixture.checkpoint(&store);
    assert_eq!(recovered, before);
    assert_eq!(
        checkpoint_payload(&store, &fixture.source),
        original_payload
    );
    assert!(store
        .load_observation(&expected_row.observation_id)
        .unwrap()
        .is_none());
    assert!(store
        .recover_finalized_sampling(&fixture.source, &id)
        .unwrap()
        .is_none());
    let recovered = fixture.recover(&recovered, &id);
    assert!(matches!(
        store.finalize_sampling(&recovered, &req).unwrap(),
        SamplingFinalization::Committed { .. }
    ));
    // Lose the result before acknowledgment and recover solely from the file.
    drop(store);
    drop(recovered);
    let mut store = fixture.open();
    let checkpoint = fixture.checkpoint(&store);
    let committed_payload = checkpoint_payload(&store, &fixture.source);
    assert_ne!(committed_payload, original_payload);
    assert_eq!(checkpoint.revision, before.revision + 1);
    assert_eq!(checkpoint.cursor, cursor);
    assert_eq!(
        store.load_source_boundary(&fixture.source).unwrap(),
        Some(boundary)
    );
    assert_eq!(checkpoint.state.qualified_sampling.len(), 1);
    assert_eq!(
        checkpoint.state.qualified_sampling[&other_id],
        other.checkpoint
    );
    assert_eq!(checkpoint.state.active_reconciliations.len(), 1);
    assert_eq!(
        checkpoint
            .state
            .active_reconciliations
            .get(&other.checkpoint.task_id),
        other.reconciliation.as_ref()
    );
    assert!(checkpoint.state.sampling_finalizations.contains_key(&id));

    let stored = store
        .load_observation(&expected_row.observation_id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.observation, expected_row);
    assert_observation(&store, &stored, 1);
    let recovered = store
        .recover_finalized_sampling(&fixture.source, &id)
        .unwrap()
        .unwrap();
    let coordinator = recovered.reauthorize(&fixture.credential).unwrap();
    assert_eq!(coordinator.snapshot().unwrap(), snapshot);
    assert_eq!(
        store.finalize_sampling(&coordinator, &req).unwrap(),
        SamplingFinalization::Replayed {
            observation: stored.clone(),
            checkpoint: checkpoint.clone()
        }
    );
    let mut stale = fixture.request(&snapshot, before.revision - 1, &id);
    assert!(matches!(
        store.finalize_sampling(&coordinator, &stale),
        Err(StorageError::CheckpointRevisionConflict { .. })
    ));
    stale.checkpoint_expected_revision = checkpoint.revision;
    stale.observation_expected_revision = Some(stored.storage_revision + 1);
    assert!(matches!(
        store.finalize_sampling(&coordinator, &stale),
        Err(StorageError::ObservationRevisionConflict { .. })
    ));
    stale.observation_expected_revision = Some(stored.storage_revision);
    assert!(matches!(
        store.finalize_sampling(&coordinator, &stale).unwrap(),
        SamplingFinalization::Replayed { .. }
    ));

    let contradictory = terminal(&fixture.credential, &METERS, 26.0);
    assert!(matches!(
        store.finalize_sampling(&contradictory, &req),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    let events = [fixture.token()];
    let mut conflicting = req;
    conflicting.token_events = &events;
    assert!(matches!(
        store.finalize_sampling(&coordinator, &conflicting),
        Err(StorageError::ObservationTerminalConflict)
    ));
    assert!(matches!(
        store.save_sampling_snapshot(&fixture.source, &snapshot, Some(checkpoint.revision)),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    assert!(matches!(
        store.save_runtime_checkpoint(
            &fixture.source,
            &cursor,
            &before.state,
            Some(checkpoint.revision)
        ),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    assert_eq!(store.save_observation(&expected_row, None).unwrap(), stored);
    assert_eq!(
        store
            .list_terminal_history(Default::default())
            .unwrap()
            .observations
            .len(),
        1
    );
    assert_eq!(fixture.checkpoint(&store), checkpoint);
    assert_eq!(
        checkpoint_payload(&store, &fixture.source),
        committed_payload
    );
    store
        .connection()
        .execute(
            "DELETE FROM observations WHERE observation_id = ?1",
            [&stored.observation.observation_id],
        )
        .unwrap();
    let retry = fixture.request(&snapshot, before.revision, &id);
    assert!(matches!(
        store.finalize_sampling(&coordinator, &retry),
        Err(StorageError::ObservationCorrupt)
    ));
    assert_eq!(fixture.checkpoint(&store), checkpoint);
}

#[test]
fn missing_pending_workflow_with_identical_row_is_ambiguous() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let sampling = terminal(&fixture.credential, &METERS, 25.0);
    let snapshot = sampling.snapshot().unwrap();
    let id = snapshot.checkpoint.workflow_id();
    let before = store
        .save_runtime_checkpoint(
            &fixture.source,
            &RolloutCursor::at_start(&fixture.source),
            &RuntimeRecoveryState::default(),
            None,
        )
        .unwrap();
    let row = store
        .save_observation(&expected(&fixture, &sampling), None)
        .unwrap();
    let mut req = fixture.request(&snapshot, before.revision, &id);
    req.observation_expected_revision = Some(row.storage_revision);
    assert!(matches!(
        store.finalize_sampling(&sampling, &req),
        Err(StorageError::CheckpointIdentityConflict)
    ));
    assert!(store
        .recover_finalized_sampling(&fixture.source, &id)
        .unwrap()
        .is_none());
    assert_eq!(fixture.checkpoint(&store), before);
    assert_observation(&store, &row, 1);
}

#[test]
fn terminal_collisions_and_identical_adoption_enforce_both_revisions() {
    for kind in ["identical", "different_payload", "different_generation"] {
        let fixture = Fixture::new();
        let mut store = fixture.open();
        let sampling = terminal(&fixture.credential, &METERS, 25.0);
        let snapshot = sampling.snapshot().unwrap();
        let id = snapshot.checkpoint.workflow_id();
        let before = store
            .save_sampling_snapshot(&fixture.source, &snapshot, None)
            .unwrap();
        let before_payload = checkpoint_payload(&store, &fixture.source);
        let mut existing = expected(&fixture, &sampling);
        if kind == "different_payload" {
            existing.timing.finalized_at = Some(DEADLINE.into());
        }
        if kind == "different_generation" {
            let other = SourceIdentity::new("fixture-rollout", "different-generation").unwrap();
            existing.source_instance_id = Some(identity::source_instance_id(&other));
        }
        let existing = store.save_observation(&existing, None).unwrap();
        let mut req = fixture.request(&snapshot, before.revision - 1, &id);
        req.observation_expected_revision = Some(existing.storage_revision);
        assert!(matches!(
            store.finalize_sampling(&sampling, &req),
            Err(StorageError::CheckpointRevisionConflict { .. })
        ));
        req.checkpoint_expected_revision = before.revision;
        match kind {
            "identical" => {
                req.observation_expected_revision = None;
                assert!(matches!(
                    store.finalize_sampling(&sampling, &req),
                    Err(StorageError::ObservationRevisionConflict { .. })
                ));
                assert_eq!(fixture.checkpoint(&store), before);
                assert_eq!(checkpoint_payload(&store, &fixture.source), before_payload);
                req.observation_expected_revision = Some(existing.storage_revision);
                let SamplingFinalization::Committed {
                    observation,
                    checkpoint,
                } = store.finalize_sampling(&sampling, &req).unwrap()
                else {
                    panic!("an identical P016 row can be adopted")
                };
                assert_eq!(observation, existing);
                assert_eq!(checkpoint.revision, before.revision + 1);
                assert!(checkpoint.state.qualified_sampling.is_empty());
                assert_ne!(checkpoint_payload(&store, &fixture.source), before_payload);
            }
            _ => {
                let error = store.finalize_sampling(&sampling, &req).unwrap_err();
                assert!(matches!(
                    (kind, error),
                    (
                        "different_payload",
                        StorageError::ObservationTerminalConflict
                    ) | (
                        "different_generation",
                        StorageError::ObservationIdentityConflict
                    )
                ));
                assert_eq!(fixture.checkpoint(&store), before);
                assert_eq!(checkpoint_payload(&store, &fixture.source), before_payload);
            }
        }
        assert_observation(&store, &existing, 1);
    }
}

#[test]
fn sqlite_failure_after_provisional_update_rolls_back_both_rows() {
    let fixture = Fixture::new();
    let mut store = fixture.open();
    let sampling = terminal(&fixture.credential, &METERS, 25.0);
    let snapshot = sampling.snapshot().unwrap();
    let id = snapshot.checkpoint.workflow_id();
    let before = store
        .save_sampling_snapshot(&fixture.source, &snapshot, None)
        .unwrap();
    let before_payload = checkpoint_payload(&store, &fixture.source);
    let mut provisional = expected(&fixture, &initialized(&fixture.credential, &METERS));
    provisional.lifecycle_state = ObservationLifecycle::AwaitingMeter;
    let provisional = store.save_observation(&provisional, None).unwrap();
    let mut req = fixture.request(&snapshot, before.revision, &id);
    req.observation_expected_revision = Some(provisional.storage_revision + 1);
    assert!(matches!(
        store.finalize_sampling(&sampling, &req),
        Err(StorageError::ObservationRevisionConflict { .. })
    ));
    req.observation_expected_revision = Some(provisional.storage_revision);
    store
        .connection()
        .execute_batch(
            "CREATE TRIGGER reject_finalization BEFORE UPDATE ON runtime_checkpoints
         WHEN NEW.state_format_version = 4 BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
        )
        .unwrap();
    assert!(matches!(
        store.finalize_sampling(&sampling, &req),
        Err(StorageError::Sqlite { .. })
    ));
    drop(store);
    let mut store = fixture.open();
    assert_observation(&store, &provisional, 1);
    assert_eq!(fixture.checkpoint(&store), before);
    assert_eq!(checkpoint_payload(&store, &fixture.source), before_payload);
    assert!(store
        .recover_finalized_sampling(&fixture.source, &id)
        .unwrap()
        .is_none());
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_finalization")
        .unwrap();
    let SamplingFinalization::Committed {
        observation,
        checkpoint,
    } = store
        .finalize_sampling(&fixture.recover(&before, &id), &req)
        .unwrap()
    else {
        panic!("retry must commit")
    };
    assert_eq!(observation.observation, expected(&fixture, &sampling));
    assert_observation(&store, &observation, 2);
    assert_eq!(checkpoint.revision, before.revision + 1);
    assert_eq!(checkpoint.cursor, before.cursor);
    assert!(checkpoint.state.active_reconciliations.is_empty());
    assert!(checkpoint.state.qualified_sampling.is_empty());
    assert_ne!(checkpoint_payload(&store, &fixture.source), before_payload);
}
