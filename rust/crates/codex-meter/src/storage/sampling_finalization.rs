//! P013 assembly and P016 terminal persistence for authorized qualified sampling.

use rusqlite::TransactionBehavior;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use crate::telemetry::identity::source_instance_id;
use crate::telemetry::task_quota_sampling::{RecoveredTaskQuotaSampling, TaskQuotaSampling};
use crate::telemetry::{
    build_observation, AttributedTokenEvent, IsolationEvidence, NormalizedEventType,
    NormalizedObservation, ObservationInput, ReconciliationAction, SourceIdentity,
    TaskQuotaReconciliation, TaskReconciliationTarget, TokenTelemetryCompleteness,
};

use super::checkpoints::{self, QualifiedSamplingCheckpoint, SamplingFinalizationReceipt};
use super::observations::{self, StoredObservation};
use super::{RuntimeCheckpoint, SqliteStore, StorageError};

/// Binds finalization to the expected qualified task and storage revisions.
#[derive(Debug)]
pub struct SamplingFinalizationRequest<'a> {
    pub source: &'a SourceIdentity,
    pub task: &'a TaskReconciliationTarget,
    pub workflow_id: &'a str,
    pub checkpoint_expected_revision: u64,
    pub observation_expected_revision: Option<u64>,
    /// Verified, normalized per-response evidence. Sampling supplies no token closure proof.
    pub token_events: &'a [AttributedTokenEvent],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SamplingFinalization {
    Committed {
        observation: StoredObservation,
        checkpoint: RuntimeCheckpoint,
    },
    Replayed {
        observation: StoredObservation,
        checkpoint: RuntimeCheckpoint,
    },
}

impl SqliteStore {
    /// Recover terminal replay material after a restart. The returned data still
    /// requires fresh authorization before it can be used for finalization.
    pub fn recover_finalized_sampling(
        &self,
        source: &SourceIdentity,
        workflow_id: &str,
    ) -> Result<Option<RecoveredTaskQuotaSampling>, StorageError> {
        let Some(checkpoint) = self.load_runtime_checkpoint(source)? else {
            return Ok(None);
        };
        checkpoint
            .state
            .sampling_finalizations
            .get(workflow_id)
            .map(|receipt| {
                TaskQuotaSampling::recover(receipt.snapshot())
                    .map_err(|_| StorageError::CheckpointStateInvalid)
            })
            .transpose()
    }

    /// Commit a terminal Observation and replace the exact pending workflow with
    /// a receipt. An authorized coordinator and matching evidence are required
    /// even when replaying an already committed result.
    ///
    /// ```compile_fail
    /// use codex_meter::storage::{SamplingFinalizationRequest, SqliteStore};
    /// use codex_meter::telemetry::task_quota_sampling::RecoveredTaskQuotaSampling;
    /// fn finalize_without_authority(
    ///     store: &mut SqliteStore,
    ///     recovered: &RecoveredTaskQuotaSampling,
    ///     request: &SamplingFinalizationRequest<'_>,
    /// ) {
    ///     store.finalize_sampling(recovered, request).unwrap();
    /// }
    /// ```
    pub fn finalize_sampling(
        &mut self,
        sampling: &TaskQuotaSampling,
        request: &SamplingFinalizationRequest<'_>,
    ) -> Result<SamplingFinalization, StorageError> {
        let snapshot = sampling
            .snapshot()
            .map_err(|_| StorageError::CheckpointStateInvalid)?;
        let workflow = &snapshot.checkpoint;
        if workflow.source_generation_id != request.source.source_generation() {
            return Err(StorageError::CheckpointSourceMismatch);
        }
        let reconciliation = snapshot
            .reconciliation
            .as_ref()
            .ok_or(StorageError::CheckpointStateInvalid)?;
        if workflow.workflow_id() != request.workflow_id
            || reconciliation.reconciliation.task != *request.task
            || workflow.session_id.is_none()
        {
            return Err(StorageError::CheckpointIdentityConflict);
        }
        if !sampling
            .batch()
            .is_some_and(|batch| batch.actions == [ReconciliationAction::ReconciliationComplete])
        {
            return Err(StorageError::CheckpointStateInvalid);
        }
        let observation = assemble(request, workflow, &reconciliation.reconciliation)?;
        let (_, observation_sha256) = observations::canonical_payload(&observation)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(StorageError::sqlite)?;
        let mut checkpoint = checkpoints::load_runtime_checkpoint(&transaction, request.source)?
            .ok_or(StorageError::CheckpointRevisionConflict {
                expected: Some(request.checkpoint_expected_revision),
                actual: None,
            })?;
        let existing = observations::load_observation_from_connection(
            &transaction,
            &observation.observation_id,
        )?;

        if let Some(committed) = checkpoint
            .state
            .sampling_finalizations
            .get(request.workflow_id)
        {
            if &committed.workflow != workflow
                || committed.reconciliation != reconciliation.reconciliation
            {
                return Err(StorageError::CheckpointIdentityConflict);
            }
            let existing = existing.ok_or(StorageError::ObservationCorrupt)?;
            observations::enforce_identity(&existing.observation, &observation)?;
            if committed.observation_sha256 != observation_sha256
                || existing.payload_sha256 != committed.observation_sha256
                || existing.observation != observation
            {
                return Err(StorageError::ObservationTerminalConflict);
            }
            // A retry can repeat the pre-commit CAS or use revisions from a fresh read.
            if committed.checkpoint_expected_revision != request.checkpoint_expected_revision {
                check_checkpoint_revision(&checkpoint, request)?;
            }
            if committed.observation_expected_revision != request.observation_expected_revision {
                check_observation_revision(Some(&existing), request)?;
            }
            return Ok(SamplingFinalization::Replayed {
                observation: existing,
                checkpoint,
            });
        }

        check_checkpoint_revision(&checkpoint, request)?;
        if checkpoint.state.qualified_sampling.get(request.workflow_id) != Some(workflow)
            || checkpoint
                .state
                .active_reconciliations
                .get(&workflow.task_id)
                != Some(reconciliation)
        {
            return Err(StorageError::CheckpointIdentityConflict);
        }
        if let Some(existing) = &existing {
            observations::enforce_identity(&existing.observation, &observation)?;
            if observations::is_terminal(existing.observation.lifecycle_state)
                && (existing.payload_sha256 != observation_sha256
                    || existing.observation != observation)
            {
                return Err(StorageError::ObservationTerminalConflict);
            }
        }
        // P016 permits a payload-level no-op regardless of revision. For the
        // first finalization we require CAS even when adopting an identical row.
        check_observation_revision(existing.as_ref(), request)?;
        let stored = observations::save_observation_in_transaction(
            &transaction,
            &observation,
            request.observation_expected_revision,
        )?;
        checkpoint
            .state
            .qualified_sampling
            .remove(request.workflow_id);
        if !checkpoint
            .state
            .qualified_sampling
            .values()
            .any(|other| other.task_id == workflow.task_id && other.reconciliation_initialized)
        {
            checkpoint
                .state
                .active_reconciliations
                .remove(&workflow.task_id);
        }
        checkpoint.state.sampling_finalizations.insert(
            request.workflow_id.to_owned(),
            SamplingFinalizationReceipt {
                workflow: snapshot.checkpoint,
                reconciliation: reconciliation.reconciliation.clone(),
                observation_sha256,
                checkpoint_expected_revision: request.checkpoint_expected_revision,
                observation_expected_revision: request.observation_expected_revision,
            },
        );
        let (state_json, state_sha256) = checkpoints::encode_state(&checkpoint.state)?;
        let checkpoint = checkpoints::save_in_transaction(
            &transaction,
            request.source,
            &checkpoint.cursor,
            &checkpoint.state,
            state_json,
            state_sha256,
            Some(request.checkpoint_expected_revision),
        )?;
        transaction.commit().map_err(StorageError::sqlite)?;
        Ok(SamplingFinalization::Committed {
            observation: stored,
            checkpoint,
        })
    }
}

fn assemble(
    request: &SamplingFinalizationRequest<'_>,
    workflow: &QualifiedSamplingCheckpoint,
    reconciliation: &TaskQuotaReconciliation,
) -> Result<NormalizedObservation, StorageError> {
    let source_id = source_instance_id(request.source);
    let start = observation_timestamp(&workflow.started_at)?;
    let end = observation_timestamp(&workflow.ended_at)?;
    for event in request.token_events {
        let event = &event.normalized_event;
        if event.source_instance_id != source_id {
            return Err(StorageError::ObservationIdentityConflict);
        }
        if event.task_id.as_deref() != Some(workflow.task_id.as_str())
            || event.session_id != workflow.session_id
        {
            return Err(StorageError::ObservationIdentityConflict);
        }
        if event.event_id.is_empty()
            || event.event_type != NormalizedEventType::TokenCountersUpdated
            || !(start..=end).contains(&observation_timestamp(&event.event_at)?)
        {
            return Err(StorageError::EvidenceInvalid);
        }
    }

    let mut observation = build_observation(&ObservationInput::new(
        reconciliation.clone(),
        request.token_events.to_vec(),
        TokenTelemetryCompleteness::Incomplete,
        IsolationEvidence::UnknownExternalUsage,
        workflow.last_evaluation_at.clone(),
    ))
    .map_err(|_| StorageError::EvidenceInvalid)?;
    if !observations::is_terminal(observation.lifecycle_state) {
        return Err(StorageError::CheckpointStateInvalid);
    }
    match &observation.source_instance_id {
        Some(observed) if *observed != source_id => {
            return Err(StorageError::ObservationIdentityConflict);
        }
        None if request.token_events.is_empty() => observation.source_instance_id = Some(source_id),
        None => return Err(StorageError::ObservationIdentityConflict),
        Some(_) => {}
    }
    Ok(observation)
}

fn observation_timestamp(timestamp: &str) -> Result<OffsetDateTime, StorageError> {
    OffsetDateTime::parse(timestamp, &Rfc3339).map_err(|_| StorageError::EvidenceInvalid)
}

fn check_checkpoint_revision(
    checkpoint: &RuntimeCheckpoint,
    request: &SamplingFinalizationRequest<'_>,
) -> Result<(), StorageError> {
    if checkpoint.revision != request.checkpoint_expected_revision {
        return Err(StorageError::CheckpointRevisionConflict {
            expected: Some(request.checkpoint_expected_revision),
            actual: Some(checkpoint.revision),
        });
    }
    Ok(())
}

fn check_observation_revision(
    existing: Option<&StoredObservation>,
    request: &SamplingFinalizationRequest<'_>,
) -> Result<(), StorageError> {
    let actual = existing.map(|stored| stored.storage_revision);
    if actual != request.observation_expected_revision {
        return Err(StorageError::ObservationRevisionConflict {
            expected: request.observation_expected_revision,
            actual,
        });
    }
    Ok(())
}
