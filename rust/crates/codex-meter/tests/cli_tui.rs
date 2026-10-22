use std::process::Command;

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_codex-meter"))
}

#[test]
fn tui_help_is_available_without_a_terminal() {
    let output = cli().args(["tui", "--help"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("codex-meter tui [--database PATH]"));
    assert!(!stdout.contains('\u{1b}'));
}

#[test]
fn compose_help_is_available_without_a_terminal_and_keeps_no_arg_help() {
    let output = cli().args(["compose", "--help"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("codex-meter compose [--database PATH]"));
    assert!(stdout.contains("fresh preflight and explicit confirmation"));
    assert!(stdout.contains("Prompts and execution output remain transient"));
    assert!(!stdout.contains('\u{1b}'));
}

#[test]
fn redirected_composer_fails_before_emitting_terminal_control_sequences() {
    let output = cli().args(["compose"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("tui_requires_tty"));
    assert!(stderr.contains("codex-meter compose"));
    assert!(stderr.contains("requires stdin and stdout to be interactive terminals"));
    assert!(!stderr.contains('\u{1b}'));
}

#[test]
fn redirected_tui_fails_before_emitting_terminal_control_sequences() {
    let output = cli()
        .args(["tui", "--database", "local evidence.sqlite3"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("tui_requires_tty"));
    assert!(stderr.contains("requires stdin and stdout to be interactive terminals"));
    assert!(!stderr.contains('\u{1b}'));
}

#[test]
fn no_arguments_remain_the_existing_help_command() {
    let output = cli().output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("USAGE:"));
}
