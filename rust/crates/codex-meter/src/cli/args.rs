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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenScope {
    Today,
    Week,
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokensArguments {
    pub database: Option<PathBuf>,
    pub scope: TokenScope,
    pub model: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryArguments {
    pub database: Option<PathBuf>,
    pub today: bool,
    pub week: bool,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: u32,
    pub model: Option<String>,
    pub session_id: Option<String>,
    pub after: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InspectArguments {
    pub database: Option<PathBuf>,
    pub evidence_id: String,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaArguments {
    pub database: Option<PathBuf>,
    pub history: bool,
    pub limit: u32,
    pub after: Option<String>,
    pub json: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CliAction {
    Help,
    SyncHelp,
    EstimateHelp,
    TokensHelp,
    HistoryHelp,
    InspectHelp,
    QuotaHelp,
    Version,
    Sync(SyncArguments),
    Estimate(EstimateArguments),
    Tokens(TokensArguments),
    History(HistoryArguments),
    Inspect(InspectArguments),
    Quota(QuotaArguments),
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
    if arguments[0] == OsStr::new("tokens") {
        return parse_tokens_args(&arguments[1..]);
    }
    if arguments[0] == OsStr::new("history") {
        return parse_history_args(&arguments[1..]);
    }
    if arguments[0] == OsStr::new("inspect") {
        return parse_inspect_args(&arguments[1..]);
    }
    if arguments[0] == OsStr::new("quota") {
        return parse_quota_args(&arguments[1..]);
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
        return Ok(CliAction::EstimateHelp);
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

fn parse_tokens_args(arguments: &[OsString]) -> Result<CliAction, CliError> {
    if has_help(arguments) {
        return Ok(CliAction::TokensHelp);
    }
    let mut seen = HashSet::new();
    let mut database = None;
    let mut model = None;
    let mut json = false;
    let mut scope = None;
    let mut index = 0;
    while index < arguments.len() {
        let flag = utf8_value(&arguments[index], "argument")?;
        if matches!(flag.as_str(), "--today" | "--week" | "--all") {
            if !seen.insert(flag.clone()) {
                return Err(error("invalid_argument", "duplicate time-scope option"));
            }
            if scope.is_some() {
                return Err(error(
                    "conflicting_options",
                    "choose only one of --today, --week, or --all",
                ));
            }
            scope = Some(match flag.as_str() {
                "--today" => TokenScope::Today,
                "--week" => TokenScope::Week,
                _ => TokenScope::All,
            });
            index += 1;
            continue;
        }
        if flag == "--json" {
            set_once(&mut seen, &flag)?;
            json = true;
            index += 1;
            continue;
        }
        let value = next_option_value(arguments, &mut index, &flag, &["--database", "--model"])?;
        if !seen.insert(flag.clone()) {
            return Err(error("invalid_argument", "duplicate tokens option"));
        }
        match flag.as_str() {
            "--database" => database = Some(PathBuf::from(value)),
            "--model" => model = Some(checked_text(value, "model", 256)?),
            _ => unreachable!(),
        }
    }
    Ok(CliAction::Tokens(TokensArguments {
        database,
        scope: scope.unwrap_or(TokenScope::Today),
        model,
        json,
    }))
}

fn parse_history_args(arguments: &[OsString]) -> Result<CliAction, CliError> {
    if has_help(arguments) {
        return Ok(CliAction::HistoryHelp);
    }
    let mut seen = HashSet::new();
    let mut database = None;
    let mut today = false;
    let mut week = false;
    let mut since = None;
    let mut until = None;
    let mut limit = 20;
    let mut model = None;
    let mut session_id = None;
    let mut after = None;
    let mut json = false;
    let mut index = 0;
    while index < arguments.len() {
        let flag = utf8_value(&arguments[index], "argument")?;
        match flag.as_str() {
            "--today" | "--week" | "--json" => {
                set_once(&mut seen, &flag)?;
                match flag.as_str() {
                    "--today" => today = true,
                    "--week" => week = true,
                    _ => json = true,
                }
                index += 1;
            }
            _ => {
                let values = [
                    "--database",
                    "--since",
                    "--until",
                    "--limit",
                    "--model",
                    "--session",
                    "--after",
                ];
                let value = next_option_value(arguments, &mut index, &flag, &values)?;
                if !seen.insert(flag.clone()) {
                    return Err(error("invalid_argument", "duplicate history option"));
                }
                match flag.as_str() {
                    "--database" => database = Some(PathBuf::from(value)),
                    "--since" => since = Some(parse_utc_bound(value, "--since")?),
                    "--until" => until = Some(parse_utc_bound(value, "--until")?),
                    "--limit" => limit = parse_limit(&value)?,
                    "--model" => model = Some(checked_text(value, "model", 256)?),
                    "--session" => session_id = Some(checked_identifier(&value, "session")?),
                    "--after" => after = Some(checked_identifier(&value, "cursor")?),
                    _ => unreachable!(),
                }
            }
        }
    }
    if today && week {
        return Err(error(
            "conflicting_options",
            "choose only one of --today or --week",
        ));
    }
    if (today || week) && (since.is_some() || until.is_some()) {
        return Err(error(
            "conflicting_options",
            "--today and --week cannot be combined with --since or --until",
        ));
    }
    if let (Some(since), Some(until)) = (since.as_deref(), until.as_deref()) {
        use time::{format_description::well_known::Rfc3339, OffsetDateTime};
        let since_value = OffsetDateTime::parse(since, &Rfc3339)
            .map_err(|_| error("invalid_timestamp", "--since is not a valid UTC timestamp"))?;
        let until_value = OffsetDateTime::parse(until, &Rfc3339)
            .map_err(|_| error("invalid_timestamp", "--until is not a valid UTC timestamp"))?;
        if since_value >= until_value {
            return Err(error(
                "invalid_timestamp",
                "--since must be earlier than --until",
            ));
        }
    }
    Ok(CliAction::History(HistoryArguments {
        database,
        today,
        week,
        since,
        until,
        limit,
        model,
        session_id,
        after,
        json,
    }))
}

fn parse_inspect_args(arguments: &[OsString]) -> Result<CliAction, CliError> {
    if has_help(arguments) {
        return Ok(CliAction::InspectHelp);
    }
    let mut seen = HashSet::new();
    let mut database = None;
    let mut evidence_id = None;
    let mut json = false;
    let mut index = 0;
    while index < arguments.len() {
        let value = utf8_value(&arguments[index], "argument")?;
        match value.as_str() {
            "--json" => {
                set_once(&mut seen, &value)?;
                json = true;
                index += 1;
            }
            "--database" => {
                set_once(&mut seen, &value)?;
                index += 1;
                let value = arguments
                    .get(index)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| error("invalid_argument", "--database requires a value"))?;
                database = Some(PathBuf::from(value));
                index += 1;
            }
            option if option.starts_with('-') => {
                return Err(error("unknown_argument", "unknown inspect argument"));
            }
            _ => {
                if evidence_id.is_some() {
                    return Err(error(
                        "invalid_argument",
                        "inspect accepts exactly one event ID",
                    ));
                }
                evidence_id = Some(checked_identifier(&arguments[index], "event ID")?);
                index += 1;
            }
        }
    }
    let evidence_id = evidence_id.ok_or_else(|| {
        error(
            "invalid_argument",
            "inspect requires an opaque event or sample ID",
        )
    })?;
    Ok(CliAction::Inspect(InspectArguments {
        database,
        evidence_id,
        json,
    }))
}

fn parse_quota_args(arguments: &[OsString]) -> Result<CliAction, CliError> {
    if has_help(arguments) {
        return Ok(CliAction::QuotaHelp);
    }
    let mut seen = HashSet::new();
    let mut database = None;
    let mut history = false;
    let mut limit = 20;
    let mut after = None;
    let mut json = false;
    let mut index = 0;
    while index < arguments.len() {
        let flag = utf8_value(&arguments[index], "argument")?;
        match flag.as_str() {
            "--history" | "--json" => {
                set_once(&mut seen, &flag)?;
                if flag == "--history" {
                    history = true;
                } else {
                    json = true;
                }
                index += 1;
            }
            _ => {
                let value = next_option_value(
                    arguments,
                    &mut index,
                    &flag,
                    &["--database", "--limit", "--after"],
                )?;
                if !seen.insert(flag.clone()) {
                    return Err(error("invalid_argument", "duplicate quota option"));
                }
                match flag.as_str() {
                    "--database" => database = Some(PathBuf::from(value)),
                    "--limit" => limit = parse_limit(&value)?,
                    "--after" => after = Some(checked_identifier(&value, "cursor")?),
                    _ => unreachable!(),
                }
            }
        }
    }
    if !history && (seen.contains("--limit") || after.is_some()) {
        return Err(error(
            "conflicting_options",
            "--limit and --after require --history",
        ));
    }
    Ok(CliAction::Quota(QuotaArguments {
        database,
        history,
        limit,
        after,
        json,
    }))
}

fn has_help(arguments: &[OsString]) -> bool {
    arguments
        .iter()
        .any(|argument| argument == OsStr::new("--help"))
}

fn set_once(seen: &mut HashSet<String>, flag: &str) -> Result<(), CliError> {
    if seen.insert(flag.to_owned()) {
        Ok(())
    } else {
        Err(error("invalid_argument", &format!("duplicate {flag} flag")))
    }
}

fn next_option_value(
    arguments: &[OsString],
    index: &mut usize,
    flag: &str,
    allowed: &[&str],
) -> Result<OsString, CliError> {
    if !allowed.contains(&flag) {
        return Err(error("unknown_argument", "unknown command argument"));
    }
    *index += 1;
    let value = arguments
        .get(*index)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| error("invalid_argument", &format!("{flag} requires a value")))?
        .clone();
    *index += 1;
    Ok(value)
}

fn checked_text(value: OsString, label: &str, max_length: usize) -> Result<String, CliError> {
    let value = utf8_value(&value, label)?;
    if value.is_empty() || value.len() > max_length || value.chars().any(char::is_control) {
        return Err(error(
            "invalid_argument",
            &format!("{label} value is invalid"),
        ));
    }
    Ok(value)
}

fn checked_identifier(value: &OsStr, label: &str) -> Result<String, CliError> {
    let value = utf8_value(value, label)?;
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(error(
            "invalid_argument",
            &format!("{label} value is invalid"),
        ));
    }
    Ok(value)
}

fn parse_limit(value: &OsStr) -> Result<u32, CliError> {
    let parsed = utf8_value(value, "limit")?
        .parse::<u32>()
        .map_err(|_| error("invalid_limit", "--limit must be an integer from 1 to 1000"))?;
    if !(1..=1_000).contains(&parsed) {
        return Err(error(
            "invalid_limit",
            "--limit must be an integer from 1 to 1000",
        ));
    }
    Ok(parsed)
}

fn parse_utc_bound(value: OsString, flag: &str) -> Result<String, CliError> {
    let value = utf8_value(&value, flag)?;
    if value.len() > 128
        || crate::telemetry::token_normalization::validate_utc_timestamp(&value).is_err()
        || time::OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
            .is_err()
    {
        return Err(error(
            "invalid_timestamp",
            &format!("{flag} must be a valid UTC RFC 3339 timestamp ending in Z"),
        ));
    }
    Ok(value)
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
    "Codex Meter — local Codex usage evidence and empirical capacity estimates\n\nUSAGE:\n    codex-meter --help\n    codex-meter --version\n    codex-meter sync [OPTIONS]\n    codex-meter tokens [OPTIONS]\n    codex-meter history [OPTIONS]\n    codex-meter inspect EVENT_ID [OPTIONS]\n    codex-meter quota [OPTIONS]\n    codex-meter estimate --database PATH --request FILE --generated-at UTC_TIMESTAMP [OPTIONS]\n\nCOMMANDS:\n    sync                            Ingest supported local Codex rollout evidence\n    tokens                          Summarize persisted per-response token evidence\n    history                         List recent persisted per-response events\n    inspect EVENT_ID                Inspect one token event or quota sample\n    quota                           Show timestamped local quota evidence\n    estimate                        Estimate capacity from an existing dataset\n\nUse `codex-meter COMMAND --help` for command options. Reporting commands read SQLite strictly read-only. No provider API is called.\n"
}

pub fn sync_help_text() -> &'static str {
    "Codex Meter — local Codex usage sync\n\nUSAGE:\n    codex-meter sync [OPTIONS]\n\nOPTIONS:\n    --codex-home PATH    Read a specific local Codex home\n    --database PATH      Use an explicit Codex Meter SQLite database\n    --json               Emit a privacy-safe JSON summary\n    --help               Show this help\n\nSupported input: plain Codex rollout JSONL for the audited Codex CLI 0.157.1 format.\nCompressed .jsonl.zst files are detected and reported as skipped.\n"
}

pub fn estimate_help_text() -> &'static str {
    "Codex Meter — empirical current-capacity estimates\n\nUSAGE:\n    codex-meter estimate --database PATH --request FILE --generated-at UTC_TIMESTAMP [OPTIONS]\n\nOPTIONS:\n    --database PATH                 Existing Codex Meter SQLite database\n    --request FILE                  AnalyticsRequest v1 JSON file\n    --generated-at UTC_TIMESTAMP    Explicit UTC timestamp ending in Z\n    --five-hour-used-percent N      Manually supplied five-hour used percentage points\n    --weekly-used-percent N         Manually supplied weekly used percentage points\n    --python PATH                   Python interpreter executable (default: python)\n    --worker-mode MODE              auto (default), source, or installed\n    --json                          Emit only validated AnalyticsResult v1 JSON\n    --help                          Show this help\n\nCurrent meter percentages are manual and unverified. No provider API is called.\n"
}

pub fn tokens_help_text() -> &'static str {
    "Codex Meter — token usage statistics\n\nUSAGE:\n    codex-meter tokens [--today | --week | --all] [--model MODEL] [--database PATH] [--json]\n\nOPTIONS:\n    --today          Use the current UTC calendar day (default)\n    --week           Use the trailing seven days in UTC\n    --all             Include all persisted per-response events\n    --model MODEL     Match one exact model value\n    --database PATH   Read a specific existing database (default: sync database)\n    --json            Emit versioned JSON only\n    --help            Show this help\n\nOnly per_response records count as consumed usage. Snapshot records are excluded. Component counters preserve source labels; cached input may be included in source input, reasoning output is not added to output, and raw totals are never reconstructed.\n"
}

pub fn history_help_text() -> &'static str {
    "Codex Meter — token usage history\n\nUSAGE:\n    codex-meter history [--today | --week] [--since UTC_TIMESTAMP] [--until UTC_TIMESTAMP] [--limit N] [--model MODEL] [--session SESSION_ID] [--after EVENT_ID] [--database PATH] [--json]\n\nOPTIONS:\n    --today                 Restrict to the current UTC calendar day\n    --week                  Restrict to the trailing seven UTC days\n    --since UTC_TIMESTAMP   Inclusive UTC RFC 3339 lower bound ending in Z\n    --until UTC_TIMESTAMP   Exclusive UTC RFC 3339 upper bound ending in Z\n    --limit N               Rows to show (default 20, range 1–1000)\n    --model MODEL           Match one exact model value\n    --session SESSION_ID    Match one exact opaque session ID\n    --after EVENT_ID        Continue after an event from the previous page\n    --database PATH         Read a specific existing database (default: sync database)\n    --json                  Emit versioned JSON only\n    --help                  Show this help\n\nRows are ordered newest first by UTC timestamp and opaque event ID. --since/--until cannot be combined with --today or --week.\n"
}

pub fn inspect_help_text() -> &'static str {
    "Codex Meter — evidence inspector\n\nUSAGE:\n    codex-meter inspect EVENT_ID [--database PATH] [--json]\n\nEVENT_ID may identify one persisted token event or quota sample. IDs are opaque and case-sensitive. Original source payloads, prompts, responses, account IDs, credentials, and filesystem paths are never shown.\n"
}

pub fn quota_help_text() -> &'static str {
    "Codex Meter — local quota evidence\n\nUSAGE:\n    codex-meter quota [--history [--limit N] [--after SAMPLE_ID]] [--database PATH] [--json]\n\nOPTIONS:\n    --history         Include timestamped quota samples\n    --limit N         History page size (default 20, range 1–1000)\n    --after SAMPLE_ID Continue after a sample from the previous page\n    --database PATH   Read a specific existing database (default: sync database)\n    --json            Emit versioned JSON only\n    --help            Show this help\n\nQuota values are persisted account-meter snapshots, not live values or task-specific attribution. A 24-hour freshness label is a presentation policy.\n"
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
        assert_eq!(
            parse(&["estimate", "--help"]).unwrap(),
            CliAction::EstimateHelp
        );
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

    #[test]
    fn reporting_commands_have_scopes_filters_and_local_help() {
        assert_eq!(
            parse(&["tokens"]).unwrap(),
            CliAction::Tokens(TokensArguments {
                database: None,
                scope: TokenScope::Today,
                model: None,
                json: false,
            })
        );
        let CliAction::Tokens(tokens) = parse(&[
            "tokens",
            "--all",
            "--model",
            "gpt-x",
            "--json",
            "--database",
            "local db.sqlite",
        ])
        .unwrap() else {
            panic!("expected tokens")
        };
        assert_eq!(tokens.scope, TokenScope::All);
        assert_eq!(tokens.model.as_deref(), Some("gpt-x"));
        assert_eq!(tokens.database, Some(PathBuf::from("local db.sqlite")));
        assert_eq!(parse(&["tokens", "--help"]).unwrap(), CliAction::TokensHelp);
        assert_eq!(
            parse(&["history", "--help"]).unwrap(),
            CliAction::HistoryHelp
        );
        assert_eq!(
            parse(&["inspect", "--help"]).unwrap(),
            CliAction::InspectHelp
        );
        assert_eq!(parse(&["quota", "--help"]).unwrap(), CliAction::QuotaHelp);
    }

    #[test]
    fn reporting_parser_rejects_conflicts_duplicates_and_bad_limits() {
        assert_eq!(
            parse(&["tokens", "--today", "--week"]).unwrap_err().code,
            "conflicting_options"
        );
        assert_eq!(
            parse(&["tokens", "--json", "--json"]).unwrap_err().code,
            "invalid_argument"
        );
        assert_eq!(
            parse(&["history", "--today", "--since", "2026-10-20T00:00:00Z"])
                .unwrap_err()
                .code,
            "conflicting_options"
        );
        assert_eq!(
            parse(&["history", "--limit", "0"]).unwrap_err().code,
            "invalid_limit"
        );
        assert_eq!(
            parse(&["history", "--limit", "1001"]).unwrap_err().code,
            "invalid_limit"
        );
        assert_eq!(
            parse(&["history", "--since", "not-a-time"])
                .unwrap_err()
                .code,
            "invalid_timestamp"
        );
        assert_eq!(
            parse(&["quota", "--limit", "10"]).unwrap_err().code,
            "conflicting_options"
        );
        assert_eq!(
            parse(&["quota", "--history", "--limit", "10", "--limit", "20"])
                .unwrap_err()
                .code,
            "invalid_argument"
        );
    }

    #[test]
    fn fractional_utc_range_comparison_uses_time_values() {
        let parsed = parse(&[
            "history",
            "--since",
            "2026-10-20T12:00:00.1Z",
            "--until",
            "2026-10-20T12:00:00.11Z",
        ]);
        assert!(parsed.is_ok());
        assert_eq!(
            parse(&[
                "history",
                "--since",
                "2026-10-20T12:00:00.11Z",
                "--until",
                "2026-10-20T12:00:00.1Z",
            ])
            .unwrap_err()
            .code,
            "invalid_timestamp"
        );
    }

    #[test]
    fn inspect_requires_one_opaque_id_and_rejects_duplicate_options() {
        assert_eq!(parse(&["inspect"]).unwrap_err().code, "invalid_argument");
        assert_eq!(
            parse(&["inspect", "event-1", "event-2"]).unwrap_err().code,
            "invalid_argument"
        );
        assert_eq!(
            parse(&["inspect", "event-1", "--json", "--json"])
                .unwrap_err()
                .code,
            "invalid_argument"
        );
    }
}
