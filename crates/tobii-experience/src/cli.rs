//! The command line of `tobii-linux-experience`. A second instance forwards it to the
//! running one, which acts on `--guest` and `--page` and opens the page when neither is
//! given; the service options only apply to the instance that starts the app.

use std::net::SocketAddr;

pub use tobii_control::DEFAULT_ADDR;

/// The GApplication id; one app per user session holds it.
pub const APP_ID: &str = "org.tobii_linux.Experience";
pub const DEFAULT_SERVICE_UNIT: &str = "tobii-head-pose.service";

pub const USAGE: &str = "\
usage: tobii-linux-experience [options]

Tobii Experience for Linux: the tray icon, the Eye tracking page, on/off and quit. Run it
again while it runs to open its page, or to hand it --guest.

options:
  --guest              Start a temporary guest calibration (Tobii's Ctrl+Shift+F10); only
                       while the tracker is in use or needs setting up
  --page               Open the Eye tracking page
  --service ADDR       Control address of tobii-linux serve (default 127.0.0.1:7760)
  --service-unit NAME  systemd user unit started with the app and stopped by Quit
                       (default tobii-head-pose.service)
  -h, --help           Show this help";

/// What one invocation asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub guest: bool,
    pub page: bool,
    pub service: SocketAddr,
    pub service_unit: String,
}

impl Default for Options {
    fn default() -> Self {
        Options { guest: false, page: false, service: DEFAULT_ADDR, service_unit: DEFAULT_SERVICE_UNIT.into() }
    }
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run(Options),
    Help,
}

/// Parses the arguments after the program name.
pub fn parse<I: IntoIterator<Item = String>>(arguments: I) -> Result<Command, String> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        let (option, inline) = match argument.split_once('=') {
            Some((option, value)) if option.starts_with("--") => (option.to_string(), Some(value.to_string())),
            _ => (argument, None),
        };
        let mut value =
            || inline.clone().or_else(|| arguments.next()).ok_or_else(|| format!("{option}: missing value"));
        match option.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--guest" | "--page" if inline.is_some() => return Err(format!("{option} takes no value")),
            "--guest" => options.guest = true,
            "--page" => options.page = true,
            "--service" => {
                let text = value()?;
                options.service = text.parse().map_err(|_| format!("--service: invalid address '{text}'"))?;
            }
            "--service-unit" => {
                options.service_unit = value()?;
                if options.service_unit.is_empty() {
                    return Err("--service-unit: empty unit name".into());
                }
            }
            _ if option.starts_with('-') => return Err(format!("unknown option {option}")),
            _ => return Err(format!("unexpected argument '{option}'")),
        }
    }
    Ok(Command::Run(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(arguments: &[&str]) -> Result<Options, String> {
        match parse(arguments.iter().map(|argument| argument.to_string()))? {
            Command::Run(options) => Ok(options),
            Command::Help => Err("help".into()),
        }
    }

    #[test]
    fn parses_guest_page_service_unit() {
        assert_eq!(run(&[]).unwrap(), Options::default());
        assert_eq!(Options::default().service.to_string(), "127.0.0.1:7760");
        assert_eq!(Options::default().service_unit, "tobii-head-pose.service");

        let options =
            run(&["--guest", "--page", "--service", "127.0.0.1:5000", "--service-unit", "x.service"]).unwrap();
        assert_eq!(
            options,
            Options {
                guest: true,
                page: true,
                service: "127.0.0.1:5000".parse().unwrap(),
                service_unit: "x.service".into()
            }
        );

        let inline = run(&["--service=[::1]:6000", "--service-unit=y.service"]).unwrap();
        assert_eq!(inline.service, "[::1]:6000".parse().unwrap());
        assert_eq!(inline.service_unit, "y.service");

        assert_eq!(parse(["--help".to_string()]).unwrap(), Command::Help);
        assert_eq!(parse(["-h".to_string()]).unwrap(), Command::Help);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(run(&["--service"]).unwrap_err().contains("--service"));
        assert!(run(&["--service", "nowhere"]).unwrap_err().contains("nowhere"));
        assert!(run(&["--service-unit", ""]).is_err());
        assert!(run(&["--guest=yes"]).unwrap_err().contains("--guest"));
        assert!(run(&["--frobnicate"]).unwrap_err().contains("--frobnicate"));
        assert!(run(&["extra"]).unwrap_err().contains("extra"));
    }
}
