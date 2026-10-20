//! Small native parser with duplicate-flag rejection and no shell processing.

use crate::analytics::PythonWorkerMode;
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct EstimateArguments {
    pub database: PathBuf,
    pub request: PathBuf,
    pub generated_at: String,
    pub five_hour_used_percent: Option<f64>,
    pub weekly_used_percent: Option<f64>,
    pub python: OsString,
    pub worker_mode: PythonWorkerMode,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncArguments {
    pub codex_home: Option<PathBuf>,
    pub database: Option<PathBuf>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CliAction {
    Help,
    SyncHelp,
    Version,
    Sync(SyncArguments),
    Estimate(EstimateArguments),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CliError {}

pub fn parse_args<I, S>(arguments: I) -> Result<CliAction, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let arguments: Vec<OsString> = arguments.into_iter().map(Into::into).collect();
    if arguments.is_empty() || arguments == [OsStr::new("--help")] {
        return Ok(CliAction::Help);
    }
    if arguments == [OsStr::new("--version")] {
        return Ok(CliAction::Version);
    }
    if arguments[0] == OsStr::new("sync") {
        return parse_sync_args(&arguments[1..]);
    }
    if arguments[0] != OsStr::new("estimate") {
        return Err(error(
            "unknown_command",
            "unknown command; use --help for supported commands",
        ));
    }
    if arguments
        .iter()
        .skip(1)
        .any(|argument| argument == OsStr::new("--help"))
    {
        return Ok(CliAction::Help);
    }

    let mut seen = HashSet::new();
    let mut database = None;
    let mut request = None;
    let mut generated_at = None;
    let mut five_hour_used_percent = None;
    let mut weekly_used_percent = None;
    let mut python = OsString::from("python");
    let mut worker_mode = PythonWorkerMode::Auto;
    let mut json = false;
    let mut index = 1;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or_else(|| error("invalid_argument", "command flags must be valid UTF-8"))?;
        if flag == "--json" {
            if !seen.insert(flag.to_owned()) {
                return Err(error("invalid_argument", "duplicate --json flag"));
            }
            json = true;
            index += 1;
            continue;
        }
        let target = match flag {
            "--database" => 0,
            "--request" => 1,
            "--generated-at" => 2,
            "--five-hour-used-percent" => 3,
            "--weekly-used-percent" => 4,
            "--python" => 5,
            "--worker-mode" => 6,
            _ => return Err(error("unknown_argument", "unknown estimate argument")),
        };
        if !seen.insert(flag.to_owned()) {
            return Err(error("invalid_argument", "duplicate estimate argument"));
        }
        index += 1;
        let value = arguments
            .get(index)
            .ok_or_else(|| error("invalid_argument", "estimate argument requires a value"))?
            .clone();
        if value.is_empty() {
            return Err(error(
                "invalid_argument",
                "estimate argument value cannot be empty",
            ));
        }
        match target {
            0 => database = Some(PathBuf::from(value)),
            1 => request = Some(PathBuf::from(value)),
            2 => generated_at = Some(utf8_value(&value, "generated-at")?),
            3 => five_hour_used_percent = Some(parse_percentage(&value, flag)?),
            4 => weekly_used_percent = Some(parse_percentage(&value, flag)?),
            5 => python = value,
            6 => {
                worker_mode = match utf8_value(&value, "worker-mode")?.as_str() {
                    "auto" => PythonWorkerMode::Auto,
                    "source" => PythonWorkerMode::Source,
                    "installed" => PythonWorkerMode::Installed,
                    _ => {
                        return Err(error(
                            "invalid_argument",
                            "--worker-mode must be auto, source, or installed",
                        ));
                    }
                }
            }
            _ => unreachable!(),
        }
        index += 1;
    }
    let database = database.ok_or_else(|| error("invalid_argument", "--database is required"))?;
    let request = request.ok_or_else(|| error("invalid_argument", "--request is required"))?;
    let generated_at =
        generated_at.ok_or_else(|| error("invalid_argument", "--generated-at is required"))?;
    Ok(CliAction::Estimate(EstimateArguments {
        database,
        request,
        generated_at,
        five_hour_used_percent,
        weekly_used_percent,
        python,
        worker_mode,
        json,
    }))
}

fn parse_sync_args(arguments: &[OsString]) -> Result<CliAction, CliError> {
    if arguments
        .iter()
        .any(|argument| argument == OsStr::new("--help"))
    {
        return Ok(CliAction::SyncHelp);
    }
    let mut seen = HashSet::new();
    let mut codex_home = None;
    let mut database = None;
    let mut json = false;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index]
            .to_str()
            .ok_or_else(|| error("invalid_argument", "command flags must be valid UTF-8"))?;
        if flag == "--json" {
            if !seen.insert(flag.to_owned()) {
                return Err(error("invalid_argument", "duplicate --json flag"));
            }
            json = true;
            index += 1;
            continue;
        }
        let target = match flag {
            "--codex-home" => 0,
            "--database" => 1,
            _ => return Err(error("unknown_argument", "unknown sync argument")),
        };
        if !seen.insert(flag.to_owned()) {
            return Err(error("invalid_argument", "duplicate sync argument"));
        }
        index += 1;
        let value = arguments
            .get(index)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                error(
                    "invalid_argument",
                    "sync argument requires a non-empty value",
                )
            })?
            .clone();
        match target {
            0 => codex_home = Some(PathBuf::from(value)),
            1 => database = Some(PathBuf::from(value)),
            _ => unreachable!(),
        }
        index += 1;
    }
    Ok(CliAction::Sync(SyncArguments {
        codex_home,
        database,
        json,
    }))
}

fn parse_percentage(value: &OsStr, label: &str) -> Result<f64, CliError> {
    let value = utf8_value(value, label)?;
    let parsed = value.parse::<f64>().map_err(|_| {
        error(
            "invalid_manual_meter_percentage",
            "meter percentage must be numeric",
        )
    })?;
    if !parsed.is_finite() || !(0.0..=100.0).contains(&parsed) {
        return Err(error(
            "invalid_manual_meter_percentage",
            "meter percentage must be finite and between 0 and 100",
        ));
    }
    Ok(parsed)
}

fn utf8_value(value: &OsStr, label: &str) -> Result<String, CliError> {
    value.to_str().map(str::to_owned).ok_or_else(|| {
        error(
            "invalid_argument",
            &format!("--{label} value must be valid UTF-8"),
        )
    })
}

pub fn help_text() -> &'static str {
    "Codex Meter — local Codex usage evidence and empirical capacity estimates\n\nUSAGE:\n    codex-meter --help\n    codex-meter --version\n    codex-meter sync [OPTIONS]\n    codex-meter estimate --database PATH --request FILE --generated-at UTC_TIMESTAMP [OPTIONS]\n\nCOMMANDS:\n    sync                            Ingest local Codex rollout evidence into SQLite\n    estimate                        Estimate current capacity from an existing dataset\n\nUse `codex-meter sync --help` or `codex-meter estimate --help` for command options.\nNo provider API is called.\n"
}

pub fn sync_help_text() -> &'static str {
    "Codex Meter — local Codex usage sync\n\nUSAGE:\n    codex-meter sync [OPTIONS]\n\nOPTIONS:\n    --codex-home PATH    Read a specific local Codex home\n    --database PATH      Use an explicit Codex Meter SQLite database\n    --json               Emit a privacy-safe JSON summary\n    --help               Show this help\n\nSupported input: plain Codex rollout JSONL for the audited Codex CLI 0.157.1 format.\nCompressed .jsonl.zst files are detected and reported as skipped.\n"
}

fn error(code: &'static str, message: &str) -> CliError {
    CliError {
        code,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(values: &[&str]) -> Result<CliAction, CliError> {
        parse_args(values.iter().copied())
    }

    #[test]
    fn handles_help_version_and_estimate_help() {
        assert_eq!(parse(&["--help"]).unwrap(), CliAction::Help);
        assert_eq!(parse(&["--version"]).unwrap(), CliAction::Version);
        assert_eq!(parse(&["estimate", "--help"]).unwrap(), CliAction::Help);
    }

    #[test]
    fn rejects_unknown_commands_arguments_and_missing_required_values() {
        assert_eq!(parse(&["wat"]).unwrap_err().code, "unknown_command");
        assert_eq!(
            parse(&["estimate", "--bad"]).unwrap_err().code,
            "unknown_argument"
        );
        assert!(parse(&[
            "estimate",
            "--request",
            "request.json",
            "--generated-at",
            "2026-10-16T00:00:00Z"
        ])
        .unwrap_err()
        .message
        .contains("--database"));
        assert!(parse(&[
            "estimate",
            "--database",
            "db.sqlite",
            "--generated-at",
            "2026-10-16T00:00:00Z"
        ])
        .unwrap_err()
        .message
        .contains("--request"));
    }

    #[test]
    fn validates_percentages_and_rejects_duplicate_flags() {
        let prefix = [
            "estimate",
            "--database",
            "db",
            "--request",
            "req",
            "--generated-at",
            "2026-10-16T00:00:00Z",
        ];
        for flag in ["--five-hour-used-percent", "--weekly-used-percent"] {
            for percentage in ["-1", "101", "NaN", "Infinity", "not-a-number"] {
                let mut args = prefix.to_vec();
                args.extend([flag, percentage]);
                assert_eq!(
                    parse(&args).unwrap_err().code,
                    "invalid_manual_meter_percentage"
                );
            }
        }
        let mut duplicate = prefix.to_vec();
        duplicate.extend(["--weekly-used-percent", "20", "--weekly-used-percent", "30"]);
        assert!(parse(&duplicate).unwrap_err().message.contains("duplicate"));
    }

    #[test]
    fn retains_spaces_in_paths_and_percentage_points() {
        let args = parse(&[
            "estimate",
            "--database",
            "a db.sqlite",
            "--request",
            "a request.json",
            "--generated-at",
            "2026-10-16T00:00:00Z",
            "--five-hour-used-percent",
            "24.25",
            "--json",
        ])
        .unwrap();
        let CliAction::Estimate(args) = args else {
            panic!("expected estimate")
        };
        assert_eq!(args.database, PathBuf::from("a db.sqlite"));
        assert_eq!(args.five_hour_used_percent, Some(24.25));
        assert_eq!(args.worker_mode, PythonWorkerMode::Auto);
        assert!(args.json);
    }

    #[test]
    fn worker_mode_selects_a_deterministic_strategy() {
        let prefix = [
            "estimate",
            "--database",
            "db",
            "--request",
            "req",
            "--generated-at",
            "2026-10-16T00:00:00Z",
        ];
        let mut installed = prefix.to_vec();
        installed.extend(["--worker-mode", "installed"]);
        let CliAction::Estimate(arguments) = parse(&installed).unwrap() else {
            panic!("expected estimate")
        };
        assert_eq!(arguments.worker_mode, PythonWorkerMode::Installed);

        let mut automatic = prefix.to_vec();
        automatic.extend(["--worker-mode", "auto"]);
        let CliAction::Estimate(arguments) = parse(&automatic).unwrap() else {
            panic!("expected estimate")
        };
        assert_eq!(arguments.worker_mode, PythonWorkerMode::Auto);

        let mut invalid = prefix.to_vec();
        invalid.extend(["--worker-mode", "unexpected"]);
        assert_eq!(parse(&invalid).unwrap_err().code, "invalid_argument");
    }
}
