//! Interactive terminal dashboard over Codex Meter's existing local services.

mod app;
mod backend;
pub(crate) mod composer;
mod drafts;
mod layout;
mod render;
mod state;
mod theme;

pub use app::{run, run_composer};
