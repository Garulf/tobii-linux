use std::process::ExitCode;

fn main() -> ExitCode {
    tobii_linux::cli::main(std::env::args().collect())
}
