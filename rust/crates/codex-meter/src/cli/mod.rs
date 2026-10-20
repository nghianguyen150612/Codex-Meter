//! Native command parsing and presentation.

mod args;
mod estimate;
mod render;
mod sync;

pub use args::{parse_args, CliAction, CliError, EstimateArguments, SyncArguments};

pub fn run<I, S>(arguments: I) -> Result<String, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    match parse_args(arguments)? {
        CliAction::Help => Ok(args::help_text().to_owned()),
        CliAction::SyncHelp => Ok(args::sync_help_text().to_owned()),
        CliAction::EstimateHelp => Ok(args::estimate_help_text().to_owned()),
        CliAction::Version => Ok(format!("codex-meter {}\n", env!("CARGO_PKG_VERSION"))),
        CliAction::Sync(arguments) => sync::run(arguments),
        CliAction::Estimate(arguments) => estimate::run(arguments),
    }
}
