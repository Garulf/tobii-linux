//! Forwards the core crates' `log` records to Python's `logging`, under `tobii_linux.<module>`.

use log::{Level, LevelFilter, Metadata, Record};
use pyo3::prelude::*;

struct PythonLogger;

fn logger_name(target: &str) -> String {
    let module = target.rsplit("::").next().unwrap_or(target);
    format!("tobii_linux.{module}")
}

fn python_level(level: Level) -> u32 {
    match level {
        Level::Error => 40,
        Level::Warn => 30,
        Level::Info => 20,
        Level::Debug | Level::Trace => 10,
    }
}

impl log::Log for PythonLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.target().starts_with("tobii_")
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        crate::exit::reattach(|py| {
            let logged = py
                .import("logging")
                .and_then(|logging| logging.call_method1("getLogger", (logger_name(record.target()),)))
                .and_then(|logger| logger.call_method1("log", (python_level(record.level()), message)));
            if let Err(error) = logged {
                error.write_unraisable(py, None);
            }
        });
    }

    fn flush(&self) {}
}

static LOGGER: PythonLogger = PythonLogger;

pub fn install() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Debug);
    }
}
