use std::process::ExitCode;

fn main() -> ExitCode {
    souvenirs_cli::run(std::env::args_os())
}
