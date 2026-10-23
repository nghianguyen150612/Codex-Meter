//! Private, bounded write-ahead journal for interactive executions.
//!
//! The journal stores operational identities and lifecycle facts only. It never
//! accepts a prepared task, prompt, transcript, path, or App Server payload.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: u32 = 1;
const MAX_RECORDS: usize = 32;
const MAX_JOURNAL_BYTES: usize = 64 * 1024;
const MAX_ID_BYTES: usize = 128;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
enum PersistenceFailurePoint {
    BeforeWrite = 1 << 0,
    DuringWrite = 1 << 1,
    BeforeFlush = 1 << 2,
    AfterFlush = 1 << 3,
    BeforeReplace = 1 << 4,
    AfterReplace = 1 << 5,
    LockAcquisition = 1 << 6,
    RevisionCas = 1 << 7,
    Cleanup = 1 << 8,
    Recovery = 1 << 9,
}

#[cfg(test)]
thread_local! {
    static PERSISTENCE_FAILURES: std::cell::Cell<u16> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn inject_persistence_failures(points: &[PersistenceFailurePoint]) {
    let mask = points.iter().fold(0, |mask, point| mask | *point as u16);
    PERSISTENCE_FAILURES.with(|failures| failures.set(mask));
}

fn persistence_failure(point: PersistenceFailurePoint) -> bool {
    #[cfg(test)]
    return PERSISTENCE_FAILURES.with(|failures| {
        let bit = point as u16;
        let current = failures.get();
        if current & bit == 0 {
            false
        } else {
            failures.set(current & !bit);
            true
        }
    });

    #[cfg(not(test))]
    {
        let _ = point;
        false
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOrigin {
    NewThread,
    ExistingThread,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationIntent {
    None,
    ThreadResume,
    ThreadStart,
    TurnStart,
    ApprovalResponse,
    TurnInterrupt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPhase {
    Prepared,
    ThreadResumeIntentRecorded,
    ThreadResumeMayHaveBeenSent,
    ThreadIdentityKnown,
    ThreadStartIntentRecorded,
    ThreadStartMayHaveBeenSent,
    TurnStartIntentRecorded,
    TurnStartMayHaveBeenSent,
    TurnIdentityKnown,
    Running,
    ApprovalResponseIntentRecorded,
    ApprovalResponseMayHaveBeenSent,
    InterruptIntentRecorded,
    InterruptMayHaveBeenSent,
    InterruptAcknowledged,
    Completed,
    Failed,
    Interrupted,
    OutcomeUnknown,
    RecoveryRequired,
    Cancelled,
}

impl RecoveryPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Prepared => "Prepared",
            Self::ThreadResumeIntentRecorded => "Thread resume intent recorded",
            Self::ThreadResumeMayHaveBeenSent => "Thread resume may have been sent",
            Self::ThreadIdentityKnown => "Thread identity known",
            Self::ThreadStartIntentRecorded => "Thread start intent recorded",
            Self::ThreadStartMayHaveBeenSent => "Thread start may have been sent",
            Self::TurnStartIntentRecorded => "Turn start intent recorded",
            Self::TurnStartMayHaveBeenSent => "Turn start may have been sent",
            Self::TurnIdentityKnown => "Turn identity known",
            Self::Running => "Running",
            Self::ApprovalResponseIntentRecorded => "Approval response intent recorded",
            Self::ApprovalResponseMayHaveBeenSent => "Approval response may have been sent",
            Self::InterruptIntentRecorded => "Interrupt intent recorded",
            Self::InterruptMayHaveBeenSent => "Interrupt may have been sent",
            Self::InterruptAcknowledged => "Interrupt acknowledged",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Interrupted => "Interrupted",
            Self::OutcomeUnknown => "Outcome unknown",
            Self::RecoveryRequired => "Recovery required",
            Self::Cancelled => "Cancelled",
        }
    }

    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Interrupted | Self::Cancelled
        )
    }
}

fn record_is_prunable(record: &ExecutionRecoveryRecord) -> bool {
    !record.recovery_required
        && (record.execution_phase.is_terminal()
            || record.execution_phase == RecoveryPhase::Prepared
            || (record.execution_phase == RecoveryPhase::ThreadIdentityKnown
                && matches!(
                    record.mutation_intent,
                    MutationIntent::ThreadResume | MutationIntent::ThreadStart
                )))
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationState {
    NotConfirmed,
    Confirmed,
    RecoveryWarningAcknowledged,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoritativeStatus {
    InProgress,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryOutcome {
    RecoveredCompleted,
    RecoveredFailed,
    RecoveredInterrupted,
    StillRunning,
    InterruptAcknowledged,
    OutcomeUnknown,
    RecoveryUnavailable,
}

impl RecoveryOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::RecoveredCompleted => "Recovered completed",
            Self::RecoveredFailed => "Recovered failed",
            Self::RecoveredInterrupted => "Recovered interrupted",
            Self::StillRunning => "Still running",
            Self::InterruptAcknowledged => "Interrupt acknowledged; awaiting final state",
            Self::OutcomeUnknown => "Outcome unknown",
            Self::RecoveryUnavailable => "Recovery unavailable",
        }
    }
}

/// Fields intentionally limited to private operational metadata.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionRecoveryRecord {
    pub schema_version: u32,
    pub execution_id: String,
    pub origin: ExecutionOrigin,
    pub known_thread_id: Option<String>,
    pub known_turn_id: Option<String>,
    pub mutation_intent: MutationIntent,
    pub execution_phase: RecoveryPhase,
    pub confirmation_state: ConfirmationState,
    pub last_authoritative_status: Option<AuthoritativeStatus>,
    pub created_at: i64,
    pub updated_at: i64,
    pub recovery_required: bool,
    pub warning_acknowledged: bool,
    revision: u64,
}

impl ExecutionRecoveryRecord {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn thread_label(&self) -> &'static str {
        if self.known_thread_id.is_some() {
            "Known"
        } else {
            "Unknown"
        }
    }

    pub fn turn_label(&self) -> &'static str {
        if self.known_turn_id.is_some() {
            "Known"
        } else {
            "Unknown"
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RecoveryJournalError {
    UnsupportedPlatform,
    DirectoryUnavailable,
    UnsafeStorage,
    CorruptJournal,
    SchemaMismatch,
    Io,
    TooManyUnresolvedRecords,
    UnacknowledgedRecovery,
    MissingRecord,
    ConcurrentUpdate,
}

impl std::fmt::Display for RecoveryJournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::UnsupportedPlatform => "private recovery storage is unavailable on this platform",
            Self::DirectoryUnavailable => "could not open the private execution recovery directory",
            Self::UnsafeStorage => "execution recovery storage failed its privacy checks",
            Self::CorruptJournal => "the execution recovery journal is corrupt and was preserved",
            Self::SchemaMismatch => "the execution recovery journal uses an unsupported schema",
            Self::Io => "could not durably update the execution recovery journal",
            Self::TooManyUnresolvedRecords => "the execution recovery journal is full of unresolved records",
            Self::UnacknowledgedRecovery => "review or acknowledge unresolved execution recovery before submitting another task",
            Self::MissingRecord => "the execution recovery record is no longer available",
            Self::ConcurrentUpdate => "the execution recovery record changed in another Codex Meter process; refresh and review it",
        };
        f.write_str(value)
    }
}

impl std::error::Error for RecoveryJournalError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(any(unix, windows))]
struct JournalPayload {
    schema_version: u32,
    generation: u64,
    records: Vec<ExecutionRecoveryRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg(any(unix, windows))]
struct JournalEnvelope {
    payload: JournalPayload,
    checksum: String,
}

#[derive(Clone, Debug, Default)]
struct JournalData {
    generation: u64,
    records: Vec<ExecutionRecoveryRecord>,
}

pub struct RecoveryJournal {
    store: platform::Store,
}

impl RecoveryJournal {
    pub fn open_default() -> Result<Self, RecoveryJournalError> {
        Self::open_at(&default_recovery_directory()?)
    }

    pub fn open_at(path: &Path) -> Result<Self, RecoveryJournalError> {
        Ok(Self {
            store: platform::Store::open(path)?,
        })
    }

    #[cfg(all(test, not(any(unix, windows))))]
    pub fn open_memory_for_tests() -> Result<Self, RecoveryJournalError> {
        Ok(Self {
            store: platform::Store::open_for_tests(),
        })
    }

    #[cfg(all(test, windows))]
    pub fn open_memory_for_tests() -> Result<Self, RecoveryJournalError> {
        Ok(Self {
            store: platform::Store::open_for_tests()?,
        })
    }

    pub fn snapshot(&self) -> Result<Vec<ExecutionRecoveryRecord>, RecoveryJournalError> {
        self.store.with_lock(|| Ok(self.store.read()?.records))
    }

    pub fn unresolved(&self) -> Result<Vec<ExecutionRecoveryRecord>, RecoveryJournalError> {
        Ok(self
            .snapshot()?
            .into_iter()
            .filter(|r| r.recovery_required)
            .collect())
    }

    pub fn get(&self, execution_id: &str) -> Result<ExecutionRecoveryRecord, RecoveryJournalError> {
        self.snapshot()?
            .into_iter()
            .find(|record| record.execution_id == execution_id)
            .ok_or(RecoveryJournalError::MissingRecord)
    }

    pub fn has_unacknowledged_recovery(&self) -> Result<bool, RecoveryJournalError> {
        Ok(self
            .unresolved()?
            .iter()
            .any(|record| !record.warning_acknowledged))
    }

    pub fn create(
        &self,
        origin: ExecutionOrigin,
        known_thread_id: Option<String>,
    ) -> Result<ExecutionRecoveryRecord, RecoveryJournalError> {
        self.store.with_lock(|| {
            let mut data = self.store.read()?;
            if data
                .records
                .iter()
                .any(|r| r.recovery_required && !r.warning_acknowledged)
            {
                return Err(RecoveryJournalError::UnacknowledgedRecovery);
            }
            data.records.retain(|record| !record_is_prunable(record));
            if data.records.len() >= MAX_RECORDS {
                return Err(RecoveryJournalError::TooManyUnresolvedRecords);
            }
            if let Some(thread) = &known_thread_id {
                validate_protocol_id(thread)?;
            }
            let now = now_timestamp();
            let record = ExecutionRecoveryRecord {
                schema_version: SCHEMA_VERSION,
                execution_id: next_id(),
                origin,
                known_thread_id,
                known_turn_id: None,
                mutation_intent: MutationIntent::None,
                execution_phase: RecoveryPhase::Prepared,
                confirmation_state: ConfirmationState::Confirmed,
                last_authoritative_status: None,
                created_at: now,
                updated_at: now,
                recovery_required: false,
                warning_acknowledged: false,
                revision: 0,
            };
            data.records.push(record.clone());
            self.store.write(&mut data)?;
            Ok(record)
        })
    }

    /// Compare-and-swap one record under the process lock. Stale in-memory
    /// controllers cannot overwrite newer state from another process.
    pub fn update(
        &self,
        proposed: &ExecutionRecoveryRecord,
    ) -> Result<ExecutionRecoveryRecord, RecoveryJournalError> {
        self.update_inner(proposed, false)
    }

    /// Atomically verifies that every other unresolved record has been
    /// acknowledged before a new externally observable mutation is armed.
    pub fn update_for_new_mutation(
        &self,
        proposed: &ExecutionRecoveryRecord,
    ) -> Result<ExecutionRecoveryRecord, RecoveryJournalError> {
        self.update_inner(proposed, true)
    }

    fn update_inner(
        &self,
        proposed: &ExecutionRecoveryRecord,
        require_acknowledgement: bool,
    ) -> Result<ExecutionRecoveryRecord, RecoveryJournalError> {
        validate_record(proposed)?;
        self.store.with_lock(|| {
            let mut data = self.store.read()?;
            if require_acknowledgement
                && data.records.iter().any(|record| {
                    record.execution_id != proposed.execution_id
                        && record.recovery_required
                        && !record.warning_acknowledged
                })
            {
                return Err(RecoveryJournalError::UnacknowledgedRecovery);
            }
            let Some(index) = data
                .records
                .iter()
                .position(|r| r.execution_id == proposed.execution_id)
            else {
                return Err(RecoveryJournalError::MissingRecord);
            };
            if data.records[index].revision != proposed.revision {
                return Err(RecoveryJournalError::ConcurrentUpdate);
            }
            if persistence_failure(PersistenceFailurePoint::RevisionCas) {
                return Err(RecoveryJournalError::ConcurrentUpdate);
            }
            if data.records[index].schema_version != proposed.schema_version {
                return Err(RecoveryJournalError::SchemaMismatch);
            }
            let current = &data.records[index];
            if current.revision == u64::MAX {
                return Err(RecoveryJournalError::ConcurrentUpdate);
            }
            if current.origin != proposed.origin
                || current.created_at != proposed.created_at
                || current
                    .known_thread_id
                    .as_ref()
                    .is_some_and(|id| proposed.known_thread_id.as_ref() != Some(id))
                || current
                    .known_turn_id
                    .as_ref()
                    .is_some_and(|id| proposed.known_turn_id.as_ref() != Some(id))
                || current.last_authoritative_status.is_some_and(|status| {
                    matches!(
                        status,
                        AuthoritativeStatus::Completed
                            | AuthoritativeStatus::Failed
                            | AuthoritativeStatus::Interrupted
                    ) && proposed.last_authoritative_status != Some(status)
                })
                || (current.recovery_required
                    && !proposed.recovery_required
                    && !matches!(
                        proposed.execution_phase,
                        RecoveryPhase::Completed
                            | RecoveryPhase::Failed
                            | RecoveryPhase::Interrupted
                            | RecoveryPhase::Cancelled
                            | RecoveryPhase::ThreadIdentityKnown
                    ))
            {
                return Err(RecoveryJournalError::CorruptJournal);
            }
            let mut updated = proposed.clone();
            updated.revision = updated.revision.saturating_add(1);
            updated.updated_at = now_timestamp().max(updated.created_at);
            data.records[index] = updated.clone();
            self.store.write(&mut data)?;
            Ok(updated)
        })
    }

    pub fn acknowledge(
        &self,
        execution_id: &str,
    ) -> Result<ExecutionRecoveryRecord, RecoveryJournalError> {
        self.store.with_lock(|| {
            let mut data = self.store.read()?;
            let Some(index) = data
                .records
                .iter()
                .position(|r| r.execution_id == execution_id)
            else {
                return Err(RecoveryJournalError::MissingRecord);
            };
            let mut updated = data.records[index].clone();
            if updated.revision == u64::MAX {
                return Err(RecoveryJournalError::ConcurrentUpdate);
            }
            updated.warning_acknowledged = true;
            updated.confirmation_state = ConfirmationState::RecoveryWarningAcknowledged;
            updated.revision = updated.revision.saturating_add(1);
            updated.updated_at = now_timestamp().max(updated.created_at);
            data.records[index] = updated.clone();
            self.store.write(&mut data)?;
            Ok(updated)
        })
    }
}

fn validate_record(record: &ExecutionRecoveryRecord) -> Result<(), RecoveryJournalError> {
    if record.schema_version != SCHEMA_VERSION {
        return Err(RecoveryJournalError::SchemaMismatch);
    }
    if record.execution_id.len() != 64
        || !record.execution_id.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    if let Some(id) = &record.known_thread_id {
        validate_protocol_id(id)?;
    }
    if let Some(id) = &record.known_turn_id {
        validate_protocol_id(id)?;
    }
    if record.updated_at < record.created_at {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    if record.recovery_required && record.execution_phase.is_terminal() {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    Ok(())
}

fn validate_protocol_id(value: &str) -> Result<(), RecoveryJournalError> {
    if value.is_empty() || value.len() > MAX_ID_BYTES || value.chars().any(char::is_control) {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn encode(data: &JournalData) -> Result<Vec<u8>, RecoveryJournalError> {
    if data.records.len() > MAX_RECORDS {
        return Err(RecoveryJournalError::TooManyUnresolvedRecords);
    }
    for record in &data.records {
        validate_record(record)?;
    }
    let mut records = data.records.clone();
    records.sort_by(|left, right| left.execution_id.cmp(&right.execution_id));
    let payload = JournalPayload {
        schema_version: SCHEMA_VERSION,
        generation: data.generation,
        records,
    };
    let payload_bytes =
        serde_json::to_vec(&payload).map_err(|_| RecoveryJournalError::CorruptJournal)?;
    let checksum = hex_digest(&payload_bytes);
    let bytes = serde_json::to_vec(&JournalEnvelope { payload, checksum })
        .map_err(|_| RecoveryJournalError::CorruptJournal)?;
    if bytes.len() > MAX_JOURNAL_BYTES {
        return Err(RecoveryJournalError::TooManyUnresolvedRecords);
    }
    Ok(bytes)
}

#[cfg(any(unix, windows))]
fn decode(bytes: &[u8]) -> Result<JournalData, RecoveryJournalError> {
    if bytes.len() > MAX_JOURNAL_BYTES {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    let envelope: JournalEnvelope =
        serde_json::from_slice(bytes).map_err(|_| RecoveryJournalError::CorruptJournal)?;
    if envelope.payload.schema_version != SCHEMA_VERSION {
        return Err(RecoveryJournalError::SchemaMismatch);
    }
    if envelope.payload.records.len() > MAX_RECORDS {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    let payload_bytes =
        serde_json::to_vec(&envelope.payload).map_err(|_| RecoveryJournalError::CorruptJournal)?;
    if envelope.checksum != hex_digest(&payload_bytes) {
        return Err(RecoveryJournalError::CorruptJournal);
    }
    let mut seen = std::collections::HashSet::new();
    for record in &envelope.payload.records {
        validate_record(record)?;
        if !seen.insert(record.execution_id.clone()) {
            return Err(RecoveryJournalError::CorruptJournal);
        }
    }
    Ok(JournalData {
        generation: envelope.payload.generation,
        records: envelope.payload.records,
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn next_id() -> String {
    let counter = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    hex_digest(format!("{}:{}:{}", std::process::id(), counter, timestamp).as_bytes())
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs().min(i64::MAX as u64) as i64)
}

fn default_recovery_directory() -> Result<PathBuf, RecoveryJournalError> {
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
        {
            return Ok(path.join("codex-meter").join("recovery"));
        }
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join(".local/share/codex-meter/recovery"))
            .ok_or(RecoveryJournalError::DirectoryUnavailable);
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|home| home.join("Library/Application Support/codex-meter/recovery"))
            .ok_or(RecoveryJournalError::DirectoryUnavailable);
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|local| local.join("codex-meter").join("recovery"))
            .ok_or(RecoveryJournalError::DirectoryUnavailable);
    }
    #[allow(unreachable_code)]
    Err(RecoveryJournalError::UnsupportedPlatform)
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    const PRIVATE_MODE: libc::mode_t = 0o700;
    const FILE_MODE: libc::mode_t = 0o600;
    static JOURNAL_THREAD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub struct Store {
        directory: OwnedFd,
    }

    impl Store {
        pub fn open(path: &Path) -> Result<Self, RecoveryJournalError> {
            if !path.is_absolute() {
                return Err(RecoveryJournalError::DirectoryUnavailable);
            }
            let root = CString::new("/").map_err(|_| RecoveryJournalError::DirectoryUnavailable)?;
            let root_fd = unsafe {
                libc::open(
                    root.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                )
            };
            if root_fd < 0 {
                return Err(RecoveryJournalError::DirectoryUnavailable);
            }
            let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };
            let components = path
                .components()
                .filter_map(|component| match component {
                    std::path::Component::RootDir => None,
                    std::path::Component::Normal(value) => Some(Ok(value)),
                    _ => Some(Err(RecoveryJournalError::DirectoryUnavailable)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            if components.is_empty() {
                return Err(RecoveryJournalError::DirectoryUnavailable);
            }
            let uid = unsafe { libc::getuid() };
            for (index, component) in components.iter().enumerate() {
                let name = cstring(component)?;
                let mut next_fd = unsafe {
                    libc::openat(
                        current.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                    )
                };
                if next_fd < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
                {
                    let made =
                        unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), PRIVATE_MODE) };
                    if made != 0
                        && std::io::Error::last_os_error().kind()
                            != std::io::ErrorKind::AlreadyExists
                    {
                        return Err(RecoveryJournalError::DirectoryUnavailable);
                    }
                    if made == 0 && unsafe { libc::fsync(current.as_raw_fd()) } != 0 {
                        return Err(RecoveryJournalError::Io);
                    }
                    next_fd = unsafe {
                        libc::openat(
                            current.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW,
                        )
                    };
                }
                if next_fd < 0 {
                    return Err(RecoveryJournalError::UnsafeStorage);
                }
                let next = unsafe { OwnedFd::from_raw_fd(next_fd) };
                let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
                if unsafe { libc::fstat(next.as_raw_fd(), &mut metadata) } != 0
                    || metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
                {
                    return Err(RecoveryJournalError::UnsafeStorage);
                }
                if index + 1 == components.len()
                    && (metadata.st_uid != uid
                        || unsafe { libc::fchmod(next.as_raw_fd(), PRIVATE_MODE) } != 0)
                {
                    return Err(RecoveryJournalError::UnsafeStorage);
                }
                current = next;
            }
            Ok(Self { directory: current })
        }

        pub fn with_lock<T>(
            &self,
            action: impl FnOnce() -> Result<T, RecoveryJournalError>,
        ) -> Result<T, RecoveryJournalError> {
            if persistence_failure(PersistenceFailurePoint::LockAcquisition) {
                return Err(RecoveryJournalError::Io);
            }
            // flock serializes separate processes. This mutex also serializes
            // independent opens and threads in this process on platforms where
            // flock ownership is tied to an open file description.
            let _thread_guard = JOURNAL_THREAD_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let name = cstring(".lock")?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                    FILE_MODE as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(RecoveryJournalError::UnsafeStorage);
            }
            let lock = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
            if unsafe { libc::fstat(lock.as_raw_fd(), &mut metadata) } != 0
                || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
                || metadata.st_uid != unsafe { libc::getuid() }
                || unsafe { libc::fchmod(lock.as_raw_fd(), FILE_MODE) } != 0
            {
                return Err(RecoveryJournalError::UnsafeStorage);
            }
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(RecoveryJournalError::Io);
            }
            let result = action();
            unsafe {
                libc::flock(lock.as_raw_fd(), libc::LOCK_UN);
            }
            result
        }

        pub fn read(&self) -> Result<JournalData, RecoveryJournalError> {
            if persistence_failure(PersistenceFailurePoint::Recovery) {
                return Err(RecoveryJournalError::Io);
            }
            let name = cstring("journal.json")?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return match std::io::Error::last_os_error().kind() {
                    std::io::ErrorKind::NotFound => Ok(JournalData::default()),
                    std::io::ErrorKind::PermissionDenied => {
                        Err(RecoveryJournalError::UnsafeStorage)
                    }
                    _ => Err(RecoveryJournalError::UnsafeStorage),
                };
            }
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            let file = File::from(owned);
            let metadata = file.metadata().map_err(|_| RecoveryJournalError::Io)?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::getuid() }
                || metadata.len() as usize > MAX_JOURNAL_BYTES
            {
                return Err(RecoveryJournalError::UnsafeStorage);
            }
            use std::os::unix::fs::PermissionsExt;
            if file
                .metadata()
                .map_err(|_| RecoveryJournalError::Io)?
                .permissions()
                .mode()
                & 0o777
                != 0o600
            {
                return Err(RecoveryJournalError::UnsafeStorage);
            }
            let mut bytes = Vec::with_capacity(metadata.len() as usize);
            file.take((MAX_JOURNAL_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| RecoveryJournalError::Io)?;
            if bytes.len() > MAX_JOURNAL_BYTES {
                return Err(RecoveryJournalError::CorruptJournal);
            }
            decode(&bytes)
        }

        pub fn write(&self, data: &mut JournalData) -> Result<(), RecoveryJournalError> {
            data.generation = data.generation.saturating_add(1);
            let bytes = encode(data)?;
            if persistence_failure(PersistenceFailurePoint::BeforeWrite) {
                return Err(RecoveryJournalError::Io);
            }
            // One fixed, private replacement name keeps crash leftovers
            // bounded to a single file. The process lock serializes writers.
            let temporary = ".journal.tmp";
            let temp_c = cstring(temporary)?;
            let fd = unsafe {
                libc::openat(
                    self.directory.as_raw_fd(),
                    temp_c.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_TRUNC
                        | libc::O_CLOEXEC
                        | libc::O_NOFOLLOW,
                    FILE_MODE as libc::c_uint,
                )
            };
            if fd < 0 {
                return Err(RecoveryJournalError::Io);
            }
            let owned = unsafe { OwnedFd::from_raw_fd(fd) };
            let mut metadata = unsafe { std::mem::zeroed::<libc::stat>() };
            if unsafe { libc::fstat(owned.as_raw_fd(), &mut metadata) } != 0
                || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
                || metadata.st_uid != unsafe { libc::getuid() }
                || unsafe { libc::fchmod(owned.as_raw_fd(), FILE_MODE) } != 0
            {
                return Err(RecoveryJournalError::UnsafeStorage);
            }
            let mut output = File::from(owned);
            let write_result = if persistence_failure(PersistenceFailurePoint::DuringWrite) {
                output
                    .write_all(&bytes[..bytes.len() / 2])
                    .and_then(|()| Err(std::io::Error::other("injected journal write failure")))
            } else {
                output.write_all(&bytes)
            };
            let flush_result = if write_result.is_ok()
                && !persistence_failure(PersistenceFailurePoint::BeforeFlush)
            {
                output.sync_all()
            } else if write_result.is_ok() {
                Err(std::io::Error::other("injected journal flush failure"))
            } else {
                write_result
            };
            if flush_result.is_err() || persistence_failure(PersistenceFailurePoint::AfterFlush) {
                drop(output);
                if !persistence_failure(PersistenceFailurePoint::Cleanup) {
                    let _ = self.unlink(temporary);
                }
                return Err(RecoveryJournalError::Io);
            }
            drop(output);
            let target = cstring("journal.json")?;
            if persistence_failure(PersistenceFailurePoint::BeforeReplace) {
                if !persistence_failure(PersistenceFailurePoint::Cleanup) {
                    let _ = self.unlink(temporary);
                }
                return Err(RecoveryJournalError::Io);
            }
            if unsafe {
                libc::renameat(
                    self.directory.as_raw_fd(),
                    temp_c.as_ptr(),
                    self.directory.as_raw_fd(),
                    target.as_ptr(),
                )
            } != 0
            {
                if !persistence_failure(PersistenceFailurePoint::Cleanup) {
                    let _ = self.unlink(temporary);
                }
                return Err(RecoveryJournalError::Io);
            }
            if persistence_failure(PersistenceFailurePoint::AfterReplace) {
                return Err(RecoveryJournalError::Io);
            }
            if unsafe { libc::fsync(self.directory.as_raw_fd()) } != 0 {
                return Err(RecoveryJournalError::Io);
            }
            Ok(())
        }

        fn unlink(&self, name: &str) -> Result<(), RecoveryJournalError> {
            let name = cstring(name)?;
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) } == 0 {
                Ok(())
            } else {
                Err(RecoveryJournalError::Io)
            }
        }
    }

    fn cstring(value: impl AsRef<OsStr>) -> Result<CString, RecoveryJournalError> {
        CString::new(value.as_ref().as_bytes()).map_err(|_| RecoveryJournalError::UnsafeStorage)
    }

    use std::os::unix::fs::MetadataExt;
}

#[cfg(windows)]
mod platform;

#[cfg(not(any(unix, windows)))]
mod platform {
    use super::*;
    #[cfg(test)]
    use std::sync::{Arc, Mutex};

    #[cfg(test)]
    #[derive(Clone)]
    pub struct Store {
        data: Arc<Mutex<JournalData>>,
    }

    #[cfg(not(test))]
    pub struct Store;

    impl Store {
        pub fn open(_path: &Path) -> Result<Self, RecoveryJournalError> {
            Err(RecoveryJournalError::UnsupportedPlatform)
        }

        #[cfg(test)]
        pub fn open_for_tests() -> Self {
            Self {
                data: Arc::new(Mutex::new(JournalData::default())),
            }
        }

        pub fn with_lock<T>(
            &self,
            action: impl FnOnce() -> Result<T, RecoveryJournalError>,
        ) -> Result<T, RecoveryJournalError> {
            #[cfg(test)]
            {
                // This store exists only for platform-independent controller
                // unit tests. Production Windows access still fails closed.
                action()
            }
            #[cfg(not(test))]
            {
                let _ = action;
                Err(RecoveryJournalError::UnsupportedPlatform)
            }
        }

        pub fn read(&self) -> Result<JournalData, RecoveryJournalError> {
            #[cfg(test)]
            {
                self.data
                    .lock()
                    .map(|data| data.clone())
                    .map_err(|_| RecoveryJournalError::Io)
            }
            #[cfg(not(test))]
            {
                Err(RecoveryJournalError::UnsupportedPlatform)
            }
        }

        pub fn write(&self, data: &mut JournalData) -> Result<(), RecoveryJournalError> {
            #[cfg(test)]
            {
                data.generation = data.generation.saturating_add(1);
                self.data
                    .lock()
                    .map(|mut current| *current = data.clone())
                    .map_err(|_| RecoveryJournalError::Io)
            }
            #[cfg(not(test))]
            {
                let _ = data;
                Err(RecoveryJournalError::UnsupportedPlatform)
            }
        }
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Barrier};

    fn temp_path(label: &str) -> PathBuf {
        #[cfg(windows)]
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .expect("the Windows test runner provides a per-user local data directory");
        #[cfg(not(windows))]
        let base = std::env::temp_dir().canonicalize().unwrap();
        base.join(format!(
            "codex-meter-recovery-{label}-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn journal_round_trips_only_bounded_operational_metadata() {
        let path = temp_path("round-trip");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let mut record = journal.create(ExecutionOrigin::NewThread, None).unwrap();
        record.mutation_intent = MutationIntent::ThreadStart;
        record.execution_phase = RecoveryPhase::ThreadStartMayHaveBeenSent;
        record.recovery_required = true;
        let record = journal.update(&record).unwrap();
        assert_eq!(journal.unresolved().unwrap(), vec![record]);
        let bytes = fs::read(path.join("journal.json")).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        for canary in [
            "PRIVATE_PROMPT_CANARY",
            "PRIVATE_OUTPUT_CANARY",
            "/private/workspace",
            "command body",
        ] {
            assert!(!text.contains(canary));
        }
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn corruption_and_truncation_fail_closed_without_deleting_the_journal() {
        let path = temp_path("corrupt");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let record = journal.create(ExecutionOrigin::NewThread, None).unwrap();
        let mut unresolved = record.clone();
        unresolved.recovery_required = true;
        unresolved.execution_phase = RecoveryPhase::TurnStartMayHaveBeenSent;
        unresolved.mutation_intent = MutationIntent::TurnStart;
        journal.update(&unresolved).unwrap();
        fs::write(path.join("journal.json"), b"{\"payload\":").unwrap();
        assert_eq!(
            journal.snapshot(),
            Err(RecoveryJournalError::CorruptJournal)
        );
        assert!(path.join("journal.json").exists());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn schema_mismatch_is_distinguished_from_corruption() {
        let mut bytes = encode(&JournalData::default()).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["payload"]["schema_version"] = serde_json::json!(SCHEMA_VERSION + 1);
        bytes = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            decode(&bytes),
            Err(RecoveryJournalError::SchemaMismatch)
        ));
    }

    #[test]
    fn possible_submission_survives_opening_a_new_process_store() {
        let path = temp_path("restart");
        let execution_id = {
            let journal = RecoveryJournal::open_at(&path).unwrap();
            let mut record = journal.create(ExecutionOrigin::NewThread, None).unwrap();
            record.mutation_intent = MutationIntent::TurnStart;
            record.execution_phase = RecoveryPhase::TurnStartMayHaveBeenSent;
            record.recovery_required = true;
            journal.update(&record).unwrap().execution_id
        };
        let reopened = RecoveryJournal::open_at(&path).unwrap();
        let record = reopened.get(&execution_id).unwrap();
        assert_eq!(
            record.execution_phase,
            RecoveryPhase::TurnStartMayHaveBeenSent
        );
        assert!(record.recovery_required);
        assert!(record.known_thread_id.is_none());
        assert!(record.known_turn_id.is_none());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn unresolved_records_are_never_pruned_when_the_journal_is_full() {
        let path = temp_path("bounded-unresolved");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let mut ids = Vec::new();
        for _ in 0..MAX_RECORDS {
            let mut record = journal.create(ExecutionOrigin::NewThread, None).unwrap();
            record.mutation_intent = MutationIntent::TurnStart;
            record.execution_phase = RecoveryPhase::OutcomeUnknown;
            record.recovery_required = true;
            let record = journal.update(&record).unwrap();
            journal.acknowledge(&record.execution_id).unwrap();
            ids.push(record.execution_id);
        }
        assert_eq!(
            journal.create(ExecutionOrigin::NewThread, None),
            Err(RecoveryJournalError::TooManyUnresolvedRecords)
        );
        let snapshot = journal.snapshot().unwrap();
        assert_eq!(snapshot.len(), MAX_RECORDS);
        assert!(ids
            .iter()
            .all(|id| snapshot.iter().any(|item| &item.execution_id == id)));
        fs::remove_dir_all(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unix_journal_refuses_symlinks_and_public_file_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let path = temp_path("symlink");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        journal.create(ExecutionOrigin::NewThread, None).unwrap();
        let external = temp_path("external");
        fs::write(&external, b"leave this file untouched").unwrap();
        fs::remove_file(path.join("journal.json")).unwrap();
        symlink(&external, path.join("journal.json")).unwrap();
        assert_eq!(journal.snapshot(), Err(RecoveryJournalError::UnsafeStorage));
        assert_eq!(fs::read(&external).unwrap(), b"leave this file untouched");
        fs::remove_file(path.join("journal.json")).unwrap();
        fs::write(path.join("journal.json"), b"not a journal").unwrap();
        fs::set_permissions(path.join("journal.json"), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(journal.snapshot(), Err(RecoveryJournalError::UnsafeStorage));
        fs::remove_dir_all(path).unwrap();
        fs::remove_file(external).unwrap();
    }

    #[test]
    fn resolved_history_is_pruned_on_next_create() {
        let path = temp_path("resolved-retention");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let mut record = journal
            .create(ExecutionOrigin::ExistingThread, None)
            .unwrap();
        record.execution_phase = RecoveryPhase::Cancelled;
        journal.update(&record).unwrap();
        let new_record = journal.create(ExecutionOrigin::NewThread, None).unwrap();
        assert_eq!(journal.snapshot().unwrap().len(), 1);
        assert_eq!(
            journal.snapshot().unwrap()[0].execution_id,
            new_record.execution_id
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn stale_updates_conflict_and_acknowledgement_does_not_clear_uncertainty() {
        let path = temp_path("cas");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let mut record = journal
            .create(ExecutionOrigin::ExistingThread, Some("thread-known".into()))
            .unwrap();
        record.execution_phase = RecoveryPhase::TurnStartMayHaveBeenSent;
        record.mutation_intent = MutationIntent::TurnStart;
        record.recovery_required = true;
        record = journal.update(&record).unwrap();
        let stale = record.clone();
        let acknowledged = journal.acknowledge(&record.execution_id).unwrap();
        assert!(acknowledged.recovery_required);
        assert!(acknowledged.warning_acknowledged);
        assert_eq!(
            journal.update(&stale),
            Err(RecoveryJournalError::ConcurrentUpdate)
        );
        assert!(!journal.has_unacknowledged_recovery().unwrap());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn concurrent_creates_serialize_and_keep_unique_records() {
        let path = temp_path("concurrent");
        let journal = Arc::new(RecoveryJournal::open_at(&path).unwrap());
        let barrier = Arc::new(Barrier::new(5));
        let mut workers = Vec::new();
        for _ in 0..4 {
            let journal = Arc::clone(&journal);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                journal.create(ExecutionOrigin::NewThread, None).unwrap()
            }));
        }
        barrier.wait();
        let created = workers
            .into_iter()
            .map(|worker| worker.join().unwrap().execution_id)
            .collect::<std::collections::HashSet<_>>();
        let snapshot = journal.snapshot().unwrap();
        assert!(!snapshot.is_empty());
        assert!(snapshot.len() <= 4);
        assert!(snapshot
            .iter()
            .all(|record| created.contains(&record.execution_id)));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn deterministic_persistence_failures_keep_recovery_fail_closed() {
        let path = temp_path("fault-injection");
        let journal = RecoveryJournal::open_at(&path).unwrap();
        let initial = journal.create(ExecutionOrigin::NewThread, None).unwrap();

        for point in [
            PersistenceFailurePoint::BeforeWrite,
            PersistenceFailurePoint::BeforeFlush,
            PersistenceFailurePoint::AfterFlush,
            PersistenceFailurePoint::BeforeReplace,
        ] {
            inject_persistence_failures(&[point]);
            assert_eq!(
                journal.create(ExecutionOrigin::NewThread, None),
                Err(RecoveryJournalError::Io)
            );
            assert_eq!(journal.snapshot().unwrap(), vec![initial.clone()]);
        }

        inject_persistence_failures(&[
            PersistenceFailurePoint::DuringWrite,
            PersistenceFailurePoint::Cleanup,
        ]);
        assert_eq!(
            journal.create(ExecutionOrigin::NewThread, None),
            Err(RecoveryJournalError::Io)
        );
        assert!(path.join(".journal.tmp").exists());
        assert_eq!(journal.snapshot().unwrap(), vec![initial.clone()]);
        let after_stale_temp = journal.create(ExecutionOrigin::NewThread, None).unwrap();
        assert!(!path.join(".journal.tmp").exists());
        assert_eq!(journal.snapshot().unwrap(), vec![after_stale_temp.clone()]);

        inject_persistence_failures(&[PersistenceFailurePoint::LockAcquisition]);
        assert_eq!(journal.snapshot(), Err(RecoveryJournalError::Io));
        inject_persistence_failures(&[PersistenceFailurePoint::Recovery]);
        assert_eq!(journal.snapshot(), Err(RecoveryJournalError::Io));

        inject_persistence_failures(&[PersistenceFailurePoint::AfterReplace]);
        assert_eq!(
            journal.create(ExecutionOrigin::NewThread, None),
            Err(RecoveryJournalError::Io)
        );
        let after_replacement = journal.snapshot().unwrap();
        assert_eq!(after_replacement.len(), 1);
        assert_ne!(
            after_replacement[0].execution_id,
            after_stale_temp.execution_id
        );

        let record = after_replacement[0].clone();
        let mut stale = record.clone();
        stale.execution_phase = RecoveryPhase::Cancelled;
        inject_persistence_failures(&[PersistenceFailurePoint::RevisionCas]);
        assert_eq!(
            journal.update(&stale),
            Err(RecoveryJournalError::ConcurrentUpdate)
        );
        assert_eq!(journal.get(&record.execution_id).unwrap(), record);
        inject_persistence_failures(&[]);
        fs::remove_dir_all(path).unwrap();
    }
}

#[cfg(all(test, not(any(unix, windows))))]
mod unsupported_platform_tests {
    use super::*;

    #[test]
    fn private_journal_fails_closed_when_platform_security_is_unavailable() {
        let path = std::env::temp_dir().join("codex-meter-recovery-unsupported");
        assert!(matches!(
            RecoveryJournal::open_at(&path),
            Err(RecoveryJournalError::UnsupportedPlatform)
        ));
    }
}
