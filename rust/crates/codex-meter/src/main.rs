fn main() {
    match codex_meter::cli::run(std::env::args_os().skip(1)) {
        Ok(output) => print!("{output}"),
        Err(error) => {
            eprintln!("error [{}]: {}", error.code, error.message);
            std::process::exit(2);
        }
    }
}
