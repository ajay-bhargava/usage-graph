//! Binary entry point for `usage`.

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = color_eyre::install() {
        eprintln!("{error}");
        return ExitCode::from(1);
    }
    match usage_cli::run(std::env::args_os()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
