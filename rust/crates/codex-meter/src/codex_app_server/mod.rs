//! Local Codex App Server integration.
//!
//! The client uses Codex's supported newline-delimited stdio transport,
//! initializes once per process, and never starts or steers a model turn.

mod client;
mod errors;
mod models;
mod projects;
mod protocol;
mod sessions;
mod transport;

pub use client::{AppServerInfo, CodexAppServer};
pub use errors::CodexAppError;
pub use models::{ModelCatalog, ModelInfo, ReasoningEffortInfo};
pub use projects::{ProjectAssociation, ProjectAssociationKind, ProjectSummary};
pub use sessions::{
    ArchiveFilter, ForkedSession, HistoryAvailability, RuntimeStatus, SessionDetail,
    SessionListOptions, SessionMetadata, SessionOrigin, SessionSource, TurnMetadata,
    UsageLinkAvailability,
};
