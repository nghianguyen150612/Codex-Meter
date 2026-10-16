use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use super::*;

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
const RESERVED: &str = "2026-10-16T10:03:00Z";
const ACKNOWLEDGED: &str = "2026-10-16T10:04:00Z";
const ENDED: &str = "2026-10-16T10:05:00Z";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-meter-p025-dispatch-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn ids() -> DispatchScope {
    DispatchScope::new(
        ExperimentId::parse("experiment_a").unwrap(),
        RunId::parse("run_a").unwrap(),
        0,
        AuthorizationId::parse("authorization_a").unwrap(),
    )
}

fn guard(directory: &TestDirectory) -> FileBenchmarkDispatchGuard {
    FileBenchmarkDispatchGuard::open(&directory.0).unwrap()
}

#[test]
fn reservation_is_atomic_and_duplicate_reservation_is_rejected() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();

    let reservation = journal.reserve(&scope, RESERVED).unwrap();
    assert_eq!(reservation.scope(), &scope);
    assert_eq!(reservation.reservation_id().len(), 64);
    assert_eq!(
        journal.state(&scope).unwrap(),
        DispatchJournalState::ReservedOutcomeUnknown
    );
    assert_eq!(
        journal.reserve(&scope, RESERVED).unwrap_err(),
        DispatchJournalError::ReservationConflict
    );
}

#[test]
fn acknowledged_and_terminal_states_survive_guard_recreation() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();
    let reservation = journal.reserve(&scope, RESERVED).unwrap();
    journal.acknowledge(&reservation, ACKNOWLEDGED).unwrap();
    assert_eq!(
        guard(&directory).state(&scope).unwrap(),
        DispatchJournalState::DispatchAcknowledged
    );
    journal
        .finish(&reservation, DispatchTerminalOutcome::ExecutionEnded, ENDED)
        .unwrap();
    assert_eq!(
        guard(&directory).state(&scope).unwrap(),
        DispatchJournalState::Terminal(DispatchTerminalOutcome::ExecutionEnded)
    );
    journal
        .finish(&reservation, DispatchTerminalOutcome::ExecutionEnded, ENDED)
        .unwrap();
    assert_eq!(
        journal
            .finish(&reservation, DispatchTerminalOutcome::Failed, ENDED)
            .unwrap_err(),
        DispatchJournalError::ConflictingReplay
    );
}

#[test]
fn terminal_success_requires_acknowledgement_and_ordered_markers() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();
    let reservation = journal.reserve(&scope, RESERVED).unwrap();
    assert_eq!(
        journal
            .finish(&reservation, DispatchTerminalOutcome::ExecutionEnded, ENDED)
            .unwrap_err(),
        DispatchJournalError::InvalidTransition
    );
    journal.acknowledge(&reservation, ACKNOWLEDGED).unwrap();
    journal
        .finish(&reservation, DispatchTerminalOutcome::ExecutionEnded, ENDED)
        .unwrap();

    let terminal_path = journal.reservation_path(reservation.reservation_id(), "terminal");
    let mut terminal: MarkerRecord = read_json(&terminal_path).unwrap();
    terminal.recorded_at = RESERVED.to_owned();
    fs::write(&terminal_path, serde_json::to_vec(&terminal).unwrap()).unwrap();
    assert_eq!(
        journal.state(&scope).unwrap_err(),
        DispatchJournalError::Corrupt
    );
}

#[test]
fn two_guard_instances_share_the_same_reservation() {
    let directory = TestDirectory::new();
    let first = guard(&directory);
    let second = guard(&directory);
    let scope = ids();
    first.reserve(&scope, RESERVED).unwrap();
    assert_eq!(
        second.reserve(&scope, RESERVED).unwrap_err(),
        DispatchJournalError::ReservationConflict
    );
}

#[test]
fn concurrent_reservation_race_has_one_winner() {
    let directory = TestDirectory::new();
    let scope = ids();
    let barrier = Arc::new(Barrier::new(12));
    let workers: Vec<_> = (0..12)
        .map(|_| {
            let journal = guard(&directory);
            let barrier = Arc::clone(&barrier);
            let scope = scope.clone();
            thread::spawn(move || {
                barrier.wait();
                journal.reserve(&scope, RESERVED).is_ok()
            })
        })
        .collect();
    let successes = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .filter(|success| *success)
        .count();
    assert_eq!(successes, 1);
}

#[test]
fn crash_before_or_after_adapter_ack_recovers_as_uncertain_or_acknowledged() {
    let directory = TestDirectory::new();
    let scope = ids();
    let journal = guard(&directory);
    journal.reserve(&scope, RESERVED).unwrap();
    assert_eq!(
        guard(&directory).state(&scope).unwrap(),
        DispatchJournalState::ReservedOutcomeUnknown
    );

    let reservation = DispatchReservation {
        reservation_id: reservation_id(&scope),
        scope: scope.clone(),
    };
    journal.acknowledge(&reservation, ACKNOWLEDGED).unwrap();
    assert_eq!(
        guard(&directory).state(&scope).unwrap(),
        DispatchJournalState::DispatchAcknowledged
    );
    assert_eq!(
        journal.reserve(&scope, RESERVED).unwrap_err(),
        DispatchJournalError::ReservationConflict
    );
}

#[test]
fn authorization_id_cannot_be_reused_for_a_different_run() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let original = ids();
    journal.reserve(&original, RESERVED).unwrap();
    let reused = DispatchScope::new(
        original.experiment_id.clone(),
        RunId::parse("run_other").unwrap(),
        original.repetition_index,
        original.authorization_id.clone(),
    );
    assert_eq!(
        journal.reserve(&reused, RESERVED).unwrap_err(),
        DispatchJournalError::IdentityMismatch
    );
}

#[test]
fn interrupted_authorization_index_write_fails_closed_after_restart() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();
    let id = reservation_id(&scope);
    let record = AuthorizationRecord {
        format_version: BENCHMARK_DISPATCH_JOURNAL_VERSION.to_owned(),
        authorization_id: scope.authorization_id.as_str().to_owned(),
        reservation_id: id,
        scope: (&scope).into(),
    };
    write_new_json(
        &journal.authorization_path(scope.authorization_id.as_str()),
        &record,
    )
    .unwrap();
    assert_eq!(
        guard(&directory).state(&scope).unwrap(),
        DispatchJournalState::ReservedOutcomeUnknown
    );
    assert_eq!(
        guard(&directory).reserve(&scope, RESERVED).unwrap_err(),
        DispatchJournalError::ReservationConflict
    );
}

#[test]
fn reservation_without_its_authorization_index_is_corrupt() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();
    journal.reserve(&scope, RESERVED).unwrap();
    fs::remove_file(journal.authorization_path(scope.authorization_id.as_str())).unwrap();
    assert_eq!(
        journal.state(&scope).unwrap_err(),
        DispatchJournalError::Corrupt
    );
}

#[test]
fn identity_digest_binds_experiment_run_repetition_and_authorization() {
    let original = ids();
    let changed_run = DispatchScope::new(
        original.experiment_id.clone(),
        RunId::parse("run_b").unwrap(),
        original.repetition_index,
        original.authorization_id.clone(),
    );
    let changed_repetition = DispatchScope::new(
        original.experiment_id.clone(),
        original.run_id.clone(),
        1,
        original.authorization_id.clone(),
    );
    let changed_auth = DispatchScope::new(
        original.experiment_id.clone(),
        original.run_id.clone(),
        original.repetition_index,
        AuthorizationId::parse("authorization_b").unwrap(),
    );
    assert_ne!(reservation_id(&original), reservation_id(&changed_run));
    assert_ne!(
        reservation_id(&original),
        reservation_id(&changed_repetition)
    );
    assert_ne!(reservation_id(&original), reservation_id(&changed_auth));
}

#[test]
fn corrupt_records_fail_closed() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();
    let id = reservation_id(&scope);
    fs::write(
        journal.reservation_path(&id, "reservation"),
        br#"{"format_version":"99.0.0"}"#,
    )
    .unwrap();
    assert_eq!(
        journal.state(&scope).unwrap_err(),
        DispatchJournalError::Corrupt
    );
    assert_eq!(
        journal.reserve(&scope, RESERVED).unwrap_err(),
        DispatchJournalError::Corrupt
    );
}

#[test]
fn identity_mismatch_in_a_valid_json_record_fails_closed() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let scope = ids();
    journal.reserve(&scope, RESERVED).unwrap();
    let id = reservation_id(&scope);
    let path = journal.reservation_path(&id, "reservation");
    let mut record: ReservationRecord = read_json(&path).unwrap();
    record.scope.run_id = "run_other".to_owned();
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert_eq!(
        journal.state(&scope).unwrap_err(),
        DispatchJournalError::IdentityMismatch
    );
}

#[test]
fn filesystem_failure_prevents_reservation() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    fs::remove_dir_all(&directory.0).unwrap();
    fs::write(&directory.0, b"not a directory").unwrap();
    assert!(matches!(
        journal.reserve(&ids(), RESERVED),
        Err(DispatchJournalError::Io)
    ));
}

#[test]
fn durable_observation_claims_reject_cross_experiment_reuse_and_restore_owner() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let observation = ObservationId::parse(format!("obs:{}", "a".repeat(64))).unwrap();
    let experiment = ExperimentId::parse("experiment_a").unwrap();
    let run = RunId::parse("run_a").unwrap();
    journal
        .claim_observation(&observation, &experiment, &run, 0, ENDED)
        .unwrap();
    journal
        .claim_observation(&observation, &experiment, &run, 0, "2026-10-16T10:06:00Z")
        .unwrap();
    assert_eq!(
        guard(&directory)
            .observation_claim_owner(&observation)
            .unwrap(),
        Some((experiment.clone(), run.clone(), 0))
    );
    assert_eq!(
        journal
            .claim_observation(
                &observation,
                &ExperimentId::parse("experiment_b").unwrap(),
                &RunId::parse("run_b").unwrap(),
                0,
                ENDED,
            )
            .unwrap_err(),
        DispatchJournalError::ObservationClaimConflict
    );
}

#[test]
fn stale_journal_lock_fails_closed() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    fs::write(directory.0.join(JOURNAL_LOCK), b"").unwrap();
    assert_eq!(
        journal.reserve(&ids(), RESERVED).unwrap_err(),
        DispatchJournalError::JournalBusy
    );
}

#[test]
fn active_dispatch_slot_prevents_overlapping_and_stale_reentry() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    let first_scope = ids();
    let first_reservation = journal.reserve(&first_scope, RESERVED).unwrap();
    let lease = journal.acquire_execution_slot(&first_reservation).unwrap();

    let second_scope = DispatchScope::new(
        ExperimentId::parse("experiment_b").unwrap(),
        RunId::parse("run_b").unwrap(),
        0,
        AuthorizationId::parse("authorization_b").unwrap(),
    );
    let second_reservation = journal.reserve(&second_scope, RESERVED).unwrap();
    assert_eq!(
        journal
            .acquire_execution_slot(&second_reservation)
            .unwrap_err(),
        DispatchJournalError::ExecutionSlotBusy
    );
    drop(lease);
    let released = journal.acquire_execution_slot(&second_reservation).unwrap();
    drop(released);

    fs::write(directory.0.join(ACTIVE_DISPATCH_LOCK), b"stale").unwrap();
    assert_eq!(
        journal
            .acquire_execution_slot(&second_reservation)
            .unwrap_err(),
        DispatchJournalError::ExecutionSlotBusy
    );
}

#[test]
fn invalid_timestamps_are_rejected_before_persistence() {
    let directory = TestDirectory::new();
    let journal = guard(&directory);
    assert_eq!(
        journal.reserve(&ids(), "not-a-timestamp").unwrap_err(),
        DispatchJournalError::InvalidTimestamp
    );
    assert_eq!(
        journal.state(&ids()).unwrap(),
        DispatchJournalState::NotReserved
    );
}
