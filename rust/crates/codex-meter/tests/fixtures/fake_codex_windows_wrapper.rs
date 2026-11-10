use std::process::Command;

fn main() {
    if let Some(path) = std::env::var_os("CODEX_METER_TEST_WRAPPER_PID_FILE") {
        std::fs::write(path, std::process::id().to_string())
            .expect("the Windows test can record the synthetic wrapper PID");
    }
    let mut arguments = std::env::args_os();
    let _executable = arguments.next();
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("app-server")) {
        std::process::exit(39);
    }
    let fixture = std::env::var_os("CODEX_METER_TEST_FIXTURE")
        .expect("the Windows test provides the fake App Server fixture");
    let mode = std::env::var_os("CODEX_METER_TEST_MODE")
        .expect("the Windows test provides a fixture mode");
    let workspace = std::env::var_os("CODEX_METER_TEST_WORKSPACE")
        .expect("the Windows test provides a workspace");
    let status = Command::new("python")
        .arg(fixture)
        .arg(mode)
        .arg(workspace)
        .status()
        .expect("Python is installed for the native ConPTY integration test");
    std::process::exit(status.code().unwrap_or(1));
}
