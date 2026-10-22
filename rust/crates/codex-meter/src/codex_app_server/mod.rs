//! Local Codex App Server integration.
//!
//! The P033 management client uses Codex's newline-delimited stdio transport
//! and retains its read-only execution boundary. P036's separate interactive
//! controller is version-gated and owns the explicitly confirmed turn flow.

mod approvals;
mod client;
mod errors;
mod execution;
mod execution_events;
mod execution_protocol;
mod execution_recovery;
mod models;
mod projects;
mod protocol;
mod sessions;
mod transport;

pub use approvals::{ApprovalChoice, ApprovalKind, ApprovalView};
pub use client::{AppServerInfo, CodexAppServer};
pub use errors::CodexAppError;
pub use execution::{
    interrupt_recovery_record, reconcile_recovery_record, ControllerEvent, ExecutionPreflight,
    InteractiveExecution,
};
pub use execution_events::{ExecutionMachine, ExecutionPhase};
pub use execution_protocol::{ApprovalPolicy, SandboxMode, VerifiedSecurity};
pub use execution_recovery::{
    AuthoritativeStatus, ExecutionOrigin, ExecutionRecoveryRecord, MutationIntent, RecoveryJournal,
    RecoveryJournalError, RecoveryOutcome, RecoveryPhase,
};
pub use models::{ModelCatalog, ModelInfo, ReasoningEffortInfo};
pub use projects::{ProjectAssociation, ProjectAssociationKind, ProjectSummary};
pub use sessions::{
    ArchiveFilter, ForkedSession, HistoryAvailability, RuntimeStatus, SessionDetail,
    SessionListOptions, SessionMetadata, SessionOrigin, SessionSource, TurnMetadata,
    UsageLinkAvailability,
};
