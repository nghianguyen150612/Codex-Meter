//! Native command parsing and presentation.

mod args;
mod estimate;
mod models;
mod projects;
mod render;
mod reporting;
mod sessions;
mod sync;

pub use args::{
    parse_args, CliAction, CliError, EstimateArguments, HistoryArguments, InspectArguments,
    ModelsArguments, ProjectsArguments, QuotaArguments, SessionForkArguments, SessionShowArguments,
    SessionsArguments, SyncArguments, TokenScope, TokensArguments,
};

pub fn run<I, S>(arguments: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    match parse_args(arguments)? {
        CliAction::Help => Ok(args::help_text().to_owned()),
        CliAction::SessionsHelp => Ok(args::sessions_help_text().to_owned()),
        CliAction::SessionHelp => Ok(args::session_help_text().to_owned()),
        CliAction::SessionShowHelp => Ok(args::session_show_help_text().to_owned()),
        CliAction::SessionResumeHelp => Ok(args::session_resume_help_text().to_owned()),
        CliAction::SessionForkHelp => Ok(args::session_fork_help_text().to_owned()),
        CliAction::ModelsHelp => Ok(args::models_help_text().to_owned()),
        CliAction::ProjectsHelp => Ok(args::projects_help_text().to_owned()),
        CliAction::SyncHelp => Ok(args::sync_help_text().to_owned()),
        CliAction::EstimateHelp => Ok(args::estimate_help_text().to_owned()),
        CliAction::TokensHelp => Ok(args::tokens_help_text().to_owned()),
        CliAction::HistoryHelp => Ok(args::history_help_text().to_owned()),
        CliAction::InspectHelp => Ok(args::inspect_help_text().to_owned()),
        CliAction::QuotaHelp => Ok(args::quota_help_text().to_owned()),
        CliAction::Version => Ok(format!("codex-meter {}\n", env!("CARGO_PKG_VERSION"))),
        CliAction::Sync(arguments) => sync::run(arguments),
        CliAction::Estimate(arguments) => estimate::run(arguments),
        CliAction::Tokens(arguments) => reporting::tokens(arguments),
        CliAction::History(arguments) => reporting::history(arguments),
        CliAction::Inspect(arguments) => reporting::inspect(arguments),
        CliAction::Quota(arguments) => reporting::quota(arguments),
        CliAction::Sessions(arguments) => sessions::list(arguments),
        CliAction::SessionShow(arguments) => sessions::show(arguments),
        CliAction::SessionResume(thread_id) => sessions::resume(thread_id),
        CliAction::SessionFork(arguments) => sessions::fork(arguments),
        CliAction::Models(arguments) => models::run(arguments),
        CliAction::Projects(arguments) => projects::run(arguments),
    }
}
