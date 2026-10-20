//! Native command parsing and presentation.

mod args;
mod estimate;
mod render;
mod reporting;
mod sync;

pub use args::{
    parse_args, CliAction, CliError, EstimateArguments, HistoryArguments, InspectArguments,
    QuotaArguments, SyncArguments, TokenScope, TokensArguments,
};

pub fn run<I, S>(arguments: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    match parse_args(arguments)? {
        CliAction::Help => Ok(args::help_text().to_owned()),
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
    }
}
