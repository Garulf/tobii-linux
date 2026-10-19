//! Tobii's guest calibration launcher (docs/experience-app.md section 6): `--guest` stands in
//! for Ctrl+Shift+F10 and runs `tobii-linux-setup --guest`, Tobii's `-G`.

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::status::DeviceStatus;

pub const SETUP_BINARY: &str = "tobii-linux-setup";
pub const GUEST_ARGUMENTS: [&str; 1] = ["--guest"];

/// Whether Tobii's `TrackingDeviceStatusConstraint` lets the guest shortcut run.
pub fn guest_allowed(status: DeviceStatus) -> bool {
    matches!(status, DeviceStatus::Tracking | DeviceStatus::InvalidConfiguration)
}

/// Tobii's `ConfigurationAppLauncher` result of a configuration run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupResult {
    Success,
    ArgumentError,
    Aborted,
    ConfigurationError,
}

/// Exit code 0 is a success, 1 an argument error, 2 aborted, any other a configuration
/// error; an exit without a readable code (a signal, a failed wait) counts as a success, as
/// in Tobii's launcher.
pub fn setup_result(exit: &io::Result<ExitStatus>) -> SetupResult {
    match exit.as_ref().ok().and_then(ExitStatus::code) {
        None | Some(0) => SetupResult::Success,
        Some(1) => SetupResult::ArgumentError,
        Some(2) => SetupResult::Aborted,
        Some(_) => SetupResult::ConfigurationError,
    }
}

/// Called with the setup run's exit once it ends.
pub type OnExit = Box<dyn FnOnce(io::Result<ExitStatus>) + Send>;

/// Why a setup run did not start.
#[derive(Debug)]
pub enum LaunchError {
    /// A setup run started by this app has not ended yet (Tobii's `AlreadyRunning`).
    AlreadyRunning,
    Spawn(io::Error),
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LaunchError::AlreadyRunning => f.write_str("a setup run is already active"),
            LaunchError::Spawn(error) => write!(f, "cannot start {SETUP_BINARY}: {error}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Starts `tobii-linux-setup` runs, one at a time.
pub trait SetupLauncher: fmt::Debug {
    fn launch(&self, arguments: &[&str], on_exit: OnExit) -> Result<(), LaunchError>;
}

/// The real launcher: one child process at a time, waited for on its own thread. Every run
/// gets `--service` with the app's service address, so setup borrows the tracker from the
/// same `tobii-linux serve` the app talks to.
#[derive(Debug)]
pub struct SetupProcess {
    binary: PathBuf,
    service: SocketAddr,
    running: Arc<AtomicBool>,
}

impl SetupProcess {
    pub fn new(binary: PathBuf, service: SocketAddr) -> SetupProcess {
        SetupProcess { binary, service, running: Arc::new(AtomicBool::new(false)) }
    }

    /// The command line of a run with `arguments`.
    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(&self.binary);
        command.args(arguments).arg("--service").arg(self.service.to_string());
        command
    }
}

impl SetupLauncher for SetupProcess {
    fn launch(&self, arguments: &[&str], on_exit: OnExit) -> Result<(), LaunchError> {
        if self.running.swap(true, Ordering::AcqRel) {
            return Err(LaunchError::AlreadyRunning);
        }
        let mut child = match self.command(arguments).spawn() {
            Ok(child) => child,
            Err(error) => {
                self.running.store(false, Ordering::Release);
                return Err(LaunchError::Spawn(error));
            }
        };
        let running = self.running.clone();
        let waiter = std::thread::Builder::new().name("tobii-experience-setup".into()).spawn(move || {
            let status = child.wait();
            running.store(false, Ordering::Release);
            on_exit(status);
        });
        if let Err(error) = waiter {
            self.running.store(false, Ordering::Release);
            return Err(LaunchError::Spawn(error));
        }
        Ok(())
    }
}

/// `tobii-linux-setup` next to this executable when it is there (a build tree or one install
/// prefix), else the bare name for a `PATH` lookup.
pub fn setup_binary(current_exe: Option<&Path>) -> PathBuf {
    current_exe
        .and_then(Path::parent)
        .map(|directory| directory.join(SETUP_BINARY))
        .filter(|sibling| sibling.is_file())
        .unwrap_or_else(|| PathBuf::from(SETUP_BINARY))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;
    use crate::cli::DEFAULT_ADDR;

    fn launcher(binary: &str) -> SetupProcess {
        SetupProcess::new(PathBuf::from(binary), DEFAULT_ADDR)
    }

    #[test]
    fn guest_runs_only_while_tracking_or_needing_setup() {
        use DeviceStatus as D;
        let allowed = [D::Tracking, D::InvalidConfiguration];
        for status in [
            D::Initializing,
            D::Tracking,
            D::TrackingPaused,
            D::DeviceNotConnected,
            D::InvalidConfiguration,
            D::Configuring,
            D::Error,
        ] {
            assert_eq!(guest_allowed(status), allowed.contains(&status), "{status:?}");
        }
    }

    #[test]
    fn exit_codes_map_like_tobiis_launcher() {
        use std::os::unix::process::ExitStatusExt;
        let exited = |code: i32| Ok(ExitStatus::from_raw(code << 8));
        assert_eq!(setup_result(&exited(0)), SetupResult::Success);
        assert_eq!(setup_result(&exited(1)), SetupResult::ArgumentError);
        assert_eq!(setup_result(&exited(2)), SetupResult::Aborted);
        assert_eq!(setup_result(&exited(3)), SetupResult::ConfigurationError);
        assert_eq!(setup_result(&exited(101)), SetupResult::ConfigurationError);
        assert_eq!(setup_result(&Ok(ExitStatus::from_raw(9))), SetupResult::Success, "killed: no code");
        assert_eq!(setup_result(&Err(io::Error::other("wait failed"))), SetupResult::Success);
    }

    #[test]
    fn guest_command_is_setup_with_guest() {
        let command = launcher(SETUP_BINARY).command(&GUEST_ARGUMENTS);
        assert_eq!(command.get_program(), "tobii-linux-setup");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--guest", "--service", "127.0.0.1:7760"]);
    }

    #[test]
    fn every_run_names_the_apps_service() {
        let other: SocketAddr = "127.0.0.1:7761".parse().unwrap();
        let command = SetupProcess::new(PathBuf::from(SETUP_BINARY), other).command(&["-C"]);
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["-C", "--service", "127.0.0.1:7761"]);
    }

    #[test]
    fn setup_binary_prefers_the_sibling() {
        let directory = std::env::temp_dir().join(format!("tobii-experience-sibling-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let exe = directory.join("tobii-linux-experience");
        assert_eq!(setup_binary(Some(&exe)), PathBuf::from("tobii-linux-setup"));
        std::fs::write(directory.join("tobii-linux-setup"), "").unwrap();
        assert_eq!(setup_binary(Some(&exe)), directory.join("tobii-linux-setup"));
        assert_eq!(setup_binary(None), PathBuf::from("tobii-linux-setup"));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_second_run_is_refused_while_the_first_one_runs() {
        let launcher = launcher("sh");
        let (exited, exits) = mpsc::channel();
        let report = |exited: mpsc::Sender<bool>| -> OnExit {
            Box::new(move |status: io::Result<ExitStatus>| {
                let _ = exited.send(status.unwrap().success());
            })
        };
        launcher.launch(&["-c", "sleep 0.3"], report(exited.clone())).unwrap();
        assert!(matches!(launcher.launch(&["-c", "true"], report(exited.clone())), Err(LaunchError::AlreadyRunning)));
        assert!(exits.recv_timeout(Duration::from_secs(5)).unwrap());
        launcher.launch(&["-c", "true"], report(exited)).unwrap();
        assert!(exits.recv_timeout(Duration::from_secs(5)).unwrap());
    }

    #[test]
    fn a_missing_binary_does_not_count_as_running() {
        let launcher = launcher("/nonexistent/tobii-linux-setup");
        assert!(matches!(launcher.launch(&GUEST_ARGUMENTS, Box::new(drop)), Err(LaunchError::Spawn(_))));
        assert!(matches!(launcher.launch(&GUEST_ARGUMENTS, Box::new(drop)), Err(LaunchError::Spawn(_))));
    }
}
