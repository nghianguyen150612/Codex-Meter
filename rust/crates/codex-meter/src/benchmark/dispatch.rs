//! Crash-safe, privacy-limited benchmark dispatch reservations.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::evidence::ObservationId;
use super::model::{parse_utc_timestamp, AuthorizationId, ExperimentId, RunId};

/// On-disk format version for dispatch reservations and Observation claims.
pub const BENCHMARK_DISPATCH_JOURNAL_VERSION: &str = "1.0.0";

const MAX_RECORD_BYTES: u64 = 4096;
const MAX_JOURNAL_RECORDS: usize = 10_000;
const JOURNAL_LOCK: &str = ".codex-meter-benchmark-journal.lock";
pub(crate) const ACTIVE_DISPATCH_LOCK: &str = ".codex-meter-benchmark-active-dispatch.lock";

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Exact scope authorized for one possible adapter invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchScope {
    experiment_id: ExperimentId,
    run_id: RunId,
    repetition_index: u32,
    authorization_id: AuthorizationId,
}

impl DispatchScope {
    pub fn new(
        experiment_id: ExperimentId,
        run_id: RunId,
        repetition_index: u32,
        authorization_id: AuthorizationId,
    ) -> Self {
        Self {
            experiment_id,
            run_id,
            repetition_index,
            authorization_id,
        }
    }

    pub fn experiment_id(&self) -> &ExperimentId {
        &self.experiment_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub fn repetition_index(&self) -> u32 {
        self.repetition_index
    }

    pub fn authorization_id(&self) -> &AuthorizationId {
        &self.authorization_id
    }
}

/// Immutable reservation token returned only to the process that created it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchReservation {
    reservation_id: String,
    scope: DispatchScope,
}

/// Process and cross-process single-active-dispatch lease. A crash leaves its
/// lock file behind, which blocks future dispatch until an operator reviews it.
#[derive(Debug)]
pub struct DispatchExecutionLease {
    path: PathBuf,
}

impl Drop for DispatchExecutionLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl DispatchReservation {
    pub fn reservation_id(&self) -> &str {
        &self.reservation_id
    }

    pub fn scope(&self) -> &DispatchScope {
        &self.scope
    }
}

/// Terminal dispatch outcome recorded after one attempt is resolved locally.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchTerminalOutcome {
    NotStarted,
    ExecutionEnded,
    Failed,
    Cancelled,
    CancelledBeforeDispatch,
    TimedOut,
    OutcomeUnknown,
}

/// State reconstructed from the durable journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchJournalState {
    NotReserved,
    ReservedOutcomeUnknown,
    DispatchAcknowledged,
    Terminal(DispatchTerminalOutcome),
}

/// Narrow atomic dispatch and claim persistence contract.
#[allow(private_bounds)]
pub trait BenchmarkDispatchGuard: sealed::Sealed + Send + Sync {
    /// Atomically reserve the exact authorization scope. Existing reservations
    /// are conflicts and never grant permission to invoke an adapter again.
    fn reserve(
        &self,
        scope: &DispatchScope,
        reserved_at: &str,
    ) -> Result<DispatchReservation, DispatchJournalError>;

    /// Record that the one adapter call returned an acknowledgement.
    fn acknowledge(
        &self,
        reservation: &DispatchReservation,
        acknowledged_at: &str,
    ) -> Result<(), DispatchJournalError>;

    /// Record a terminal outcome. This is idempotent only for identical data.
    fn finish(
        &self,
        reservation: &DispatchReservation,
        outcome: DispatchTerminalOutcome,
        ended_at: &str,
    ) -> Result<(), DispatchJournalError>;

    /// Read the validated state for this exact scope. A reserved entry with no
    /// acknowledgement is intentionally reported as outcome-unknown.
    fn state(&self, scope: &DispatchScope) -> Result<DispatchJournalState, DispatchJournalError>;

    /// Persist a unique Observation owner before committing its in-memory P024
    /// claim, so a process restart cannot silently make the Observation free.
    fn claim_observation(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
        claimed_at: &str,
    ) -> Result<(), DispatchJournalError>;

    /// Exclude overlapping benchmark invocations across coordinator instances
    /// and processes. A stale lock is deliberately not auto-removed.
    fn acquire_execution_slot(
        &self,
        reservation: &DispatchReservation,
    ) -> Result<DispatchExecutionLease, DispatchJournalError>;
}

/// Filesystem-backed journal. Its directory is selected by the caller.
#[derive(Clone, Debug)]
pub struct FileBenchmarkDispatchGuard {
    directory: PathBuf,
}

impl sealed::Sealed for FileBenchmarkDispatchGuard {}

impl FileBenchmarkDispatchGuard {
    pub fn open(directory: impl AsRef<Path>) -> Result<Self, DispatchJournalError> {
        let directory = directory.as_ref().to_path_buf();
        fs::create_dir_all(&directory).map_err(|_| DispatchJournalError::Io)?;
        let metadata = fs::metadata(&directory).map_err(|_| DispatchJournalError::Io)?;
        if !metadata.is_dir() {
            return Err(DispatchJournalError::InvalidDirectory);
        }
        Ok(Self { directory })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Permanently claim an authoritative Observation across processes and
    /// restarts. Exact same-owner replay is idempotent; another owner conflicts.
    pub fn claim_observation(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
        claimed_at: &str,
    ) -> Result<(), DispatchJournalError> {
        validate_timestamp(claimed_at)?;
        let record = ClaimRecord {
            format_version: BENCHMARK_DISPATCH_JOURNAL_VERSION.to_owned(),
            observation_id: observation_id.as_str().to_owned(),
            experiment_id: experiment_id.as_str().to_owned(),
            run_id: run_id.as_str().to_owned(),
            repetition_index,
            claimed_at: claimed_at.to_owned(),
        };
        let path = self.claim_path(observation_id.as_str());
        let _lock = self.lock_capacity()?;
        match write_new_json(&path, &record) {
            Ok(()) => Ok(()),
            Err(DispatchJournalError::AlreadyExists) => {
                let existing: ClaimRecord = read_json(&path)?;
                existing.validate()?;
                if existing.observation_id == record.observation_id
                    && existing.experiment_id == record.experiment_id
                    && existing.run_id == record.run_id
                    && existing.repetition_index == record.repetition_index
                {
                    Ok(())
                } else if existing.observation_id != observation_id.as_str() {
                    Err(DispatchJournalError::IdentityMismatch)
                } else {
                    Err(DispatchJournalError::ObservationClaimConflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Restore a previously persisted claim owner. Corrupt or absent evidence
    /// never implies that an Observation is unclaimed.
    pub fn observation_claim_owner(
        &self,
        observation_id: &ObservationId,
    ) -> Result<Option<(ExperimentId, RunId, u32)>, DispatchJournalError> {
        let path = self.claim_path(observation_id.as_str());
        match read_json::<ClaimRecord>(&path) {
            Ok(record) => {
                record.validate()?;
                if record.observation_id != observation_id.as_str() {
                    return Err(DispatchJournalError::IdentityMismatch);
                }
                Ok(Some((
                    ExperimentId::parse(record.experiment_id)
                        .map_err(|_| DispatchJournalError::Corrupt)?,
                    RunId::parse(record.run_id).map_err(|_| DispatchJournalError::Corrupt)?,
                    record.repetition_index,
                )))
            }
            Err(DispatchJournalError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn reservation_path(&self, reservation_id: &str, suffix: &str) -> PathBuf {
        self.directory
            .join(format!("{reservation_id}.{suffix}.json"))
    }

    fn claim_path(&self, observation_id: &str) -> PathBuf {
        self.directory
            .join(format!("{}.claim.json", digest_fields(&[observation_id])))
    }

    fn authorization_path(&self, authorization_id: &str) -> PathBuf {
        self.directory.join(format!(
            "{}.authorization.json",
            digest_fields(&[authorization_id])
        ))
    }

    fn validate_authorization_index(
        &self,
        scope: &DispatchScope,
        expected_reservation_id: &str,
    ) -> Result<(), DispatchJournalError> {
        let path = self.authorization_path(scope.authorization_id.as_str());
        let record: AuthorizationRecord = read_json(&path).map_err(|error| {
            if error == DispatchJournalError::NotFound {
                DispatchJournalError::Corrupt
            } else {
                error
            }
        })?;
        record.validate()?;
        if record.scope != *scope
            || record.authorization_id != scope.authorization_id.as_str()
            || record.reservation_id != expected_reservation_id
        {
            return Err(DispatchJournalError::IdentityMismatch);
        }
        Ok(())
    }

    fn lock_capacity(&self) -> Result<JournalLock, DispatchJournalError> {
        let path = self.directory.join(JOURNAL_LOCK);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    DispatchJournalError::JournalBusy
                } else {
                    DispatchJournalError::Io
                }
            })?;
        drop(file);
        let lock = JournalLock { path };
        if self.count_records()? >= MAX_JOURNAL_RECORDS {
            return Err(DispatchJournalError::JournalCapacityExceeded);
        }
        Ok(lock)
    }

    fn count_records(&self) -> Result<usize, DispatchJournalError> {
        let mut count = 0usize;
        for entry in fs::read_dir(&self.directory).map_err(|_| DispatchJournalError::Io)? {
            let entry = entry.map_err(|_| DispatchJournalError::Io)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".reservation.json")
                || name.ends_with(".authorization.json")
                || name.ends_with(".claim.json")
            {
                count = count
                    .checked_add(1)
                    .ok_or(DispatchJournalError::JournalCapacityExceeded)?;
                if count > MAX_JOURNAL_RECORDS {
                    return Err(DispatchJournalError::JournalCapacityExceeded);
                }
            }
        }
        Ok(count)
    }

    fn read_reservation(
        &self,
        reservation: &DispatchReservation,
    ) -> Result<ReservationRecord, DispatchJournalError> {
        let record: ReservationRecord =
            read_json(&self.reservation_path(&reservation.reservation_id, "reservation"))?;
        record.validate()?;
        if record.reservation_id != reservation.reservation_id || record.scope != reservation.scope
        {
            return Err(DispatchJournalError::IdentityMismatch);
        }
        self.validate_authorization_index(&reservation.scope, &reservation.reservation_id)?;
        Ok(record)
    }

    fn marker_record(
        &self,
        reservation: &DispatchReservation,
        suffix: &str,
    ) -> Result<Option<MarkerRecord>, DispatchJournalError> {
        let path = self.reservation_path(&reservation.reservation_id, suffix);
        match read_json::<MarkerRecord>(&path) {
            Ok(marker) => {
                marker.validate()?;
                if marker.reservation_id != reservation.reservation_id
                    || marker.scope != reservation.scope
                {
                    return Err(DispatchJournalError::IdentityMismatch);
                }
                Ok(Some(marker))
            }
            Err(DispatchJournalError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

impl BenchmarkDispatchGuard for FileBenchmarkDispatchGuard {
    fn reserve(
        &self,
        scope: &DispatchScope,
        reserved_at: &str,
    ) -> Result<DispatchReservation, DispatchJournalError> {
        validate_timestamp(reserved_at)?;
        let reservation_id = reservation_id(scope);
        let record = ReservationRecord {
            format_version: BENCHMARK_DISPATCH_JOURNAL_VERSION.to_owned(),
            reservation_id: reservation_id.clone(),
            scope: scope.into(),
            reserved_at: reserved_at.to_owned(),
        };
        let authorization_record = AuthorizationRecord {
            format_version: BENCHMARK_DISPATCH_JOURNAL_VERSION.to_owned(),
            authorization_id: scope.authorization_id.as_str().to_owned(),
            reservation_id: reservation_id.clone(),
            scope: scope.into(),
        };
        let path = self.reservation_path(&reservation_id, "reservation");
        let authorization_path = self.authorization_path(scope.authorization_id.as_str());
        let _lock = self.lock_capacity()?;
        if self.state(scope)? != DispatchJournalState::NotReserved {
            return Err(DispatchJournalError::ReservationConflict);
        }
        if self.count_records()? > MAX_JOURNAL_RECORDS - 2 {
            return Err(DispatchJournalError::JournalCapacityExceeded);
        }
        match write_new_json(&authorization_path, &authorization_record) {
            Ok(()) => {}
            Err(DispatchJournalError::AlreadyExists) => {
                let existing: AuthorizationRecord = read_json(&authorization_path)?;
                existing.validate()?;
                return if existing.scope == *scope
                    && existing.reservation_id == reservation_id
                    && existing.authorization_id == scope.authorization_id.as_str()
                {
                    Err(DispatchJournalError::ReservationConflict)
                } else {
                    Err(DispatchJournalError::IdentityMismatch)
                };
            }
            Err(error) => return Err(error),
        }
        match write_new_json(&path, &record) {
            Ok(()) => Ok(DispatchReservation {
                reservation_id,
                scope: scope.clone(),
            }),
            Err(DispatchJournalError::AlreadyExists) => {
                let existing: ReservationRecord = read_json(&path)?;
                existing.validate()?;
                if existing.reservation_id != reservation_id || existing.scope != *scope {
                    Err(DispatchJournalError::IdentityMismatch)
                } else {
                    Err(DispatchJournalError::ReservationConflict)
                }
            }
            Err(error) => Err(error),
        }
    }

    fn acknowledge(
        &self,
        reservation: &DispatchReservation,
        acknowledged_at: &str,
    ) -> Result<(), DispatchJournalError> {
        validate_timestamp(acknowledged_at)?;
        let record = self.read_reservation(reservation)?;
        if parse_utc_timestamp(acknowledged_at)
            .map_err(|_| DispatchJournalError::InvalidTimestamp)?
            < parse_utc_timestamp(&record.reserved_at).map_err(|_| DispatchJournalError::Corrupt)?
        {
            return Err(DispatchJournalError::InvalidTransition);
        }
        if self.marker_record(reservation, "terminal")?.is_some() {
            return Err(DispatchJournalError::InvalidTransition);
        }
        let marker = MarkerRecord {
            format_version: BENCHMARK_DISPATCH_JOURNAL_VERSION.to_owned(),
            reservation_id: reservation.reservation_id.clone(),
            scope: (&reservation.scope).into(),
            recorded_at: acknowledged_at.to_owned(),
            state: MarkerState::Acknowledged,
        };
        self.write_marker(reservation, "ack", &marker)
    }

    fn finish(
        &self,
        reservation: &DispatchReservation,
        outcome: DispatchTerminalOutcome,
        ended_at: &str,
    ) -> Result<(), DispatchJournalError> {
        validate_timestamp(ended_at)?;
        let record = self.read_reservation(reservation)?;
        let ended =
            parse_utc_timestamp(ended_at).map_err(|_| DispatchJournalError::InvalidTimestamp)?;
        let reserved =
            parse_utc_timestamp(&record.reserved_at).map_err(|_| DispatchJournalError::Corrupt)?;
        if ended < reserved {
            return Err(DispatchJournalError::InvalidTransition);
        }
        if let Some(ack) = self.marker_record(reservation, "ack")? {
            if !matches!(&ack.state, MarkerState::Acknowledged)
                || parse_utc_timestamp(&ack.recorded_at)
                    .is_ok_and(|acknowledged| ended < acknowledged)
            {
                return Err(DispatchJournalError::InvalidTransition);
            }
        } else if matches!(
            outcome,
            DispatchTerminalOutcome::ExecutionEnded
                | DispatchTerminalOutcome::Failed
                | DispatchTerminalOutcome::Cancelled
        ) {
            return Err(DispatchJournalError::InvalidTransition);
        }
        let marker = MarkerRecord {
            format_version: BENCHMARK_DISPATCH_JOURNAL_VERSION.to_owned(),
            reservation_id: reservation.reservation_id.clone(),
            scope: (&reservation.scope).into(),
            recorded_at: ended_at.to_owned(),
            state: MarkerState::Terminal(outcome),
        };
        self.write_marker(reservation, "terminal", &marker)
    }

    fn state(&self, scope: &DispatchScope) -> Result<DispatchJournalState, DispatchJournalError> {
        let reservation_id = reservation_id(scope);
        let reservation: ReservationRecord =
            match read_json(&self.reservation_path(&reservation_id, "reservation")) {
                Ok(record) => record,
                Err(DispatchJournalError::NotFound) => {
                    let token = DispatchReservation {
                        reservation_id: reservation_id.clone(),
                        scope: scope.clone(),
                    };
                    let authorization_path =
                        self.authorization_path(scope.authorization_id.as_str());
                    match read_json::<AuthorizationRecord>(&authorization_path) {
                        Ok(record) => {
                            record.validate()?;
                            if record.scope != *scope
                                || record.reservation_id != reservation_id
                                || record.authorization_id != scope.authorization_id.as_str()
                            {
                                return Err(DispatchJournalError::IdentityMismatch);
                            }
                            return Ok(DispatchJournalState::ReservedOutcomeUnknown);
                        }
                        Err(DispatchJournalError::NotFound) => {}
                        Err(error) => return Err(error),
                    }
                    if self.marker_record(&token, "ack")?.is_some()
                        || self.marker_record(&token, "terminal")?.is_some()
                    {
                        return Err(DispatchJournalError::Corrupt);
                    }
                    return Ok(DispatchJournalState::NotReserved);
                }
                Err(error) => return Err(error),
            };
        reservation.validate()?;
        if reservation.reservation_id != reservation_id || reservation.scope != *scope {
            return Err(DispatchJournalError::IdentityMismatch);
        }
        self.validate_authorization_index(scope, &reservation_id)?;
        let reserved_at = parse_utc_timestamp(&reservation.reserved_at)
            .map_err(|_| DispatchJournalError::Corrupt)?;
        let token = DispatchReservation {
            reservation_id,
            scope: scope.clone(),
        };
        let acknowledgment = self.marker_record(&token, "ack")?;
        if let Some(acknowledgment) = &acknowledgment {
            if !matches!(&acknowledgment.state, MarkerState::Acknowledged)
                || parse_utc_timestamp(&acknowledgment.recorded_at)
                    .map_err(|_| DispatchJournalError::Corrupt)?
                    < reserved_at
            {
                return Err(DispatchJournalError::Corrupt);
            }
        }
        if let Some(terminal) = self.marker_record(&token, "terminal")? {
            let MarkerState::Terminal(outcome) = &terminal.state else {
                return Err(DispatchJournalError::Corrupt);
            };
            let terminal_at = parse_utc_timestamp(&terminal.recorded_at)
                .map_err(|_| DispatchJournalError::Corrupt)?;
            if terminal_at < reserved_at
                || acknowledgment.as_ref().is_some_and(|acknowledgment| {
                    parse_utc_timestamp(&acknowledgment.recorded_at)
                        .is_ok_and(|acknowledged_at| terminal_at < acknowledged_at)
                })
                || (matches!(
                    outcome,
                    DispatchTerminalOutcome::ExecutionEnded
                        | DispatchTerminalOutcome::Failed
                        | DispatchTerminalOutcome::Cancelled
                ) && acknowledgment.is_none())
            {
                return Err(DispatchJournalError::Corrupt);
            }
            return Ok(DispatchJournalState::Terminal(*outcome));
        }
        if acknowledgment.is_some() {
            Ok(DispatchJournalState::DispatchAcknowledged)
        } else {
            Ok(DispatchJournalState::ReservedOutcomeUnknown)
        }
    }

    fn claim_observation(
        &self,
        observation_id: &ObservationId,
        experiment_id: &ExperimentId,
        run_id: &RunId,
        repetition_index: u32,
        claimed_at: &str,
    ) -> Result<(), DispatchJournalError> {
        FileBenchmarkDispatchGuard::claim_observation(
            self,
            observation_id,
            experiment_id,
            run_id,
            repetition_index,
            claimed_at,
        )
    }

    fn acquire_execution_slot(
        &self,
        reservation: &DispatchReservation,
    ) -> Result<DispatchExecutionLease, DispatchJournalError> {
        self.read_reservation(reservation)?;
        let path = self.directory.join(ACTIVE_DISPATCH_LOCK);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    DispatchJournalError::ExecutionSlotBusy
                } else {
                    DispatchJournalError::Io
                }
            })?;
        file.write_all(reservation.reservation_id.as_bytes())
            .map_err(|_| DispatchJournalError::Io)?;
        file.sync_all().map_err(|_| DispatchJournalError::Io)?;
        Ok(DispatchExecutionLease { path })
    }
}

impl FileBenchmarkDispatchGuard {
    fn write_marker(
        &self,
        reservation: &DispatchReservation,
        suffix: &str,
        marker: &MarkerRecord,
    ) -> Result<(), DispatchJournalError> {
        let path = self.reservation_path(&reservation.reservation_id, suffix);
        match write_new_json(&path, marker) {
            Ok(()) => Ok(()),
            Err(DispatchJournalError::AlreadyExists) => {
                let existing: MarkerRecord = read_json(&path)?;
                existing.validate()?;
                if existing == *marker {
                    Ok(())
                } else {
                    Err(DispatchJournalError::ConflictingReplay)
                }
            }
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorizationRecord {
    format_version: String,
    authorization_id: String,
    reservation_id: String,
    scope: SerializedDispatchScope,
}

impl AuthorizationRecord {
    fn validate(&self) -> Result<(), DispatchJournalError> {
        if self.format_version != BENCHMARK_DISPATCH_JOURNAL_VERSION
            || self.reservation_id.len() != 64
            || !is_digest(&self.reservation_id)
        {
            return Err(DispatchJournalError::Corrupt);
        }
        let scope = self.scope.parse()?;
        if self.authorization_id != scope.authorization_id.as_str()
            || reservation_id(&scope) != self.reservation_id
        {
            return Err(DispatchJournalError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservationRecord {
    format_version: String,
    reservation_id: String,
    scope: SerializedDispatchScope,
    reserved_at: String,
}

impl ReservationRecord {
    fn validate(&self) -> Result<(), DispatchJournalError> {
        if self.format_version != BENCHMARK_DISPATCH_JOURNAL_VERSION
            || self.reservation_id.len() != 64
            || !is_digest(&self.reservation_id)
        {
            return Err(DispatchJournalError::Corrupt);
        }
        validate_timestamp(&self.reserved_at)?;
        let scope = self.scope.parse()?;
        if reservation_id(&scope) != self.reservation_id {
            return Err(DispatchJournalError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimRecord {
    format_version: String,
    observation_id: String,
    experiment_id: String,
    run_id: String,
    repetition_index: u32,
    claimed_at: String,
}

impl ClaimRecord {
    fn validate(&self) -> Result<(), DispatchJournalError> {
        if self.format_version != BENCHMARK_DISPATCH_JOURNAL_VERSION {
            return Err(DispatchJournalError::Corrupt);
        }
        ObservationId::parse(self.observation_id.clone())
            .map_err(|_| DispatchJournalError::Corrupt)?;
        ExperimentId::parse(self.experiment_id.clone())
            .map_err(|_| DispatchJournalError::Corrupt)?;
        RunId::parse(self.run_id.clone()).map_err(|_| DispatchJournalError::Corrupt)?;
        validate_timestamp(&self.claimed_at)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkerRecord {
    format_version: String,
    reservation_id: String,
    scope: SerializedDispatchScope,
    recorded_at: String,
    state: MarkerState,
}

impl MarkerRecord {
    fn validate(&self) -> Result<(), DispatchJournalError> {
        if self.format_version != BENCHMARK_DISPATCH_JOURNAL_VERSION
            || self.reservation_id.len() != 64
            || !is_digest(&self.reservation_id)
        {
            return Err(DispatchJournalError::Corrupt);
        }
        validate_timestamp(&self.recorded_at)?;
        let scope = self.scope.parse()?;
        if reservation_id(&scope) != self.reservation_id {
            return Err(DispatchJournalError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MarkerState {
    Acknowledged,
    Terminal(DispatchTerminalOutcome),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SerializedDispatchScope {
    experiment_id: String,
    run_id: String,
    repetition_index: u32,
    authorization_id: String,
}

impl From<&DispatchScope> for SerializedDispatchScope {
    fn from(scope: &DispatchScope) -> Self {
        Self {
            experiment_id: scope.experiment_id.as_str().to_owned(),
            run_id: scope.run_id.as_str().to_owned(),
            repetition_index: scope.repetition_index,
            authorization_id: scope.authorization_id.as_str().to_owned(),
        }
    }
}

impl SerializedDispatchScope {
    fn parse(&self) -> Result<DispatchScope, DispatchJournalError> {
        Ok(DispatchScope::new(
            ExperimentId::parse(self.experiment_id.clone())
                .map_err(|_| DispatchJournalError::Corrupt)?,
            RunId::parse(self.run_id.clone()).map_err(|_| DispatchJournalError::Corrupt)?,
            self.repetition_index,
            AuthorizationId::parse(self.authorization_id.clone())
                .map_err(|_| DispatchJournalError::Corrupt)?,
        ))
    }
}

impl PartialEq<DispatchScope> for SerializedDispatchScope {
    fn eq(&self, other: &DispatchScope) -> bool {
        self.experiment_id == other.experiment_id.as_str()
            && self.run_id == other.run_id.as_str()
            && self.repetition_index == other.repetition_index
            && self.authorization_id == other.authorization_id.as_str()
    }
}

impl PartialEq<SerializedDispatchScope> for DispatchScope {
    fn eq(&self, other: &SerializedDispatchScope) -> bool {
        other == self
    }
}

fn reservation_id(scope: &DispatchScope) -> String {
    digest_fields(&[
        scope.experiment_id.as_str(),
        scope.run_id.as_str(),
        &scope.repetition_index.to_string(),
        scope.authorization_id.as_str(),
    ])
}

fn digest_fields(fields: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for field in fields {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_digest(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_timestamp(value: &str) -> Result<(), DispatchJournalError> {
    parse_utc_timestamp(value)
        .map(|_| ())
        .map_err(|_| DispatchJournalError::InvalidTimestamp)
}

fn write_new_json<T: Serialize>(path: &Path, value: &T) -> Result<(), DispatchJournalError> {
    let bytes = serde_json::to_vec(value).map_err(|_| DispatchJournalError::Corrupt)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(DispatchJournalError::RecordTooLarge);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                DispatchJournalError::AlreadyExists
            } else {
                DispatchJournalError::Io
            }
        })?;
    file.write_all(&bytes)
        .map_err(|_| DispatchJournalError::Io)?;
    file.sync_all().map_err(|_| DispatchJournalError::Io)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, DispatchJournalError> {
    let file = File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            DispatchJournalError::NotFound
        } else {
            DispatchJournalError::Io
        }
    })?;
    let metadata = file.metadata().map_err(|_| DispatchJournalError::Io)?;
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(DispatchJournalError::RecordTooLarge);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DispatchJournalError::Io)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(DispatchJournalError::RecordTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| DispatchJournalError::Corrupt)
}

struct JournalLock {
    path: PathBuf,
}

impl Drop for JournalLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Privacy-safe failures from the dispatch journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DispatchJournalError {
    InvalidDirectory,
    InvalidTimestamp,
    Io,
    JournalBusy,
    ExecutionSlotBusy,
    JournalCapacityExceeded,
    RecordTooLarge,
    Corrupt,
    NotFound,
    AlreadyExists,
    ReservationConflict,
    ObservationClaimConflict,
    IdentityMismatch,
    InvalidTransition,
    ConflictingReplay,
}

impl std::fmt::Display for DispatchJournalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidDirectory => "dispatch journal location is not a directory",
            Self::InvalidTimestamp => "dispatch journal timestamp is invalid",
            Self::Io => "dispatch journal persistence failed",
            Self::JournalBusy => "dispatch journal is busy or has an unresolved lock",
            Self::ExecutionSlotBusy => {
                "another or unresolved benchmark dispatch holds the execution slot"
            }
            Self::JournalCapacityExceeded => "dispatch journal capacity is exhausted",
            Self::RecordTooLarge => "dispatch journal record exceeds its size limit",
            Self::Corrupt => "dispatch journal record is corrupt or unsupported",
            Self::NotFound => "dispatch journal record does not exist",
            Self::AlreadyExists => "dispatch journal record already exists",
            Self::ReservationConflict => "execution attempt already has a durable reservation",
            Self::ObservationClaimConflict => {
                "Observation is already claimed by another benchmark run"
            }
            Self::IdentityMismatch => "dispatch journal identity does not match its scope",
            Self::InvalidTransition => "dispatch journal transition is invalid",
            Self::ConflictingReplay => "dispatch journal replay conflicts with accepted evidence",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for DispatchJournalError {}

#[cfg(test)]
mod tests;
