//! Log records on stderr as `logging.basicConfig(format='%(asctime)s %(message)s')` writes them.

use std::io::Write;

use log::{Level, LevelFilter, Metadata, Record};

/// Timestamp layout of the log lines: Python's default asctime, or a `datefmt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFormat {
    /// `2026-10-16 11:29:18,161`
    Milliseconds,
    /// `11:29:18`
    Clock,
}

#[derive(Debug)]
struct Logger {
    level: LevelFilter,
    time: TimeFormat,
}

pub fn timestamp(format: TimeFormat) -> String {
    let now = jiff::Zoned::now();
    match format {
        TimeFormat::Milliseconds => {
            format!("{},{:03}", now.strftime("%Y-%m-%d %H:%M:%S"), now.subsec_nanosecond() / 1_000_000)
        }
        TimeFormat::Clock => now.strftime("%H:%M:%S").to_string(),
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level && metadata.target().starts_with("tobii_")
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let line = format!("{} {}\n", timestamp(self.time), record.args());
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

/// Installs the logger once; later calls change nothing.
pub fn install(level: Level, time: TimeFormat) {
    let logger = Box::leak(Box::new(Logger { level: level.to_level_filter(), time }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level.to_level_filter());
    }
}
