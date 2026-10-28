use std::io::Write;
use std::process::ExitCode;

use log::{LevelFilter, Metadata, Record};
use tobii_experience::app::{Deps, Shell};
use tobii_experience::cli::{self, Command, USAGE};

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.target().starts_with("tobii_")
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let line = format!("tobii-linux-experience: {}\n", record.args());
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

fn main() -> ExitCode {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
    let argv: Vec<String> = std::env::args_os().map(|argument| argument.to_string_lossy().into_owned()).collect();
    let options = match cli::parse(argv.iter().skip(1).cloned()) {
        Ok(Command::Run(options)) => options,
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{}\nerror: {error}", USAGE.lines().next().unwrap_or_default());
            return ExitCode::from(2);
        }
    };
    ExitCode::from(Shell::new(Deps::system(options.service)).run(argv).get())
}
