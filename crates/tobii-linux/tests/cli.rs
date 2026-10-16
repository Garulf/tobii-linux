//! The command line as a process: usage errors, exit codes and help match what argparse
//! prints for `python3 -m tobii_linux.cli` run as `tobii-linux`.

use std::process::Command;

const SERVE_USAGE: &str = "\
usage: tobii-linux serve [-h] --display TLX TLY TLZ TRX TRY TRZ BLX BLY BLZ
                         [--models MODELS] [--channel CHANNEL] [--index INDEX]
                         [--host HOST] [--port PORT] [--name NAME]
                         [--display-id DISPLAY_ID]
                         [--wine-clock {auto,boottime,raw}] [--no-gaze]
                         [--tet-tcp] [--sesp-positions {prp,windows}]
";
const MAIN_USAGE: &str = "usage: tobii-linux [-h] {probe,demo,setup,headpose,serve} ...\n";

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_tobii-linux")).args(args).output().unwrap();
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap();
    (output.status.code(), text(output.stdout), text(output.stderr))
}

fn usage_error(args: &[&str]) -> String {
    let (code, stdout, stderr) = run(args);
    assert_eq!((code, stdout.as_str()), (Some(2), ""), "{stderr}");
    stderr
}

#[test]
fn a_command_is_required_and_must_be_known() {
    assert_eq!(
        usage_error(&[]),
        format!("{MAIN_USAGE}tobii-linux: error: the following arguments are required: command\n")
    );
    assert_eq!(
        usage_error(&["bogus"]),
        format!(
            "{MAIN_USAGE}tobii-linux: error: argument command: invalid choice: 'bogus' \
             (choose from 'probe', 'demo', 'setup', 'headpose', 'serve')\n"
        )
    );
    assert_eq!(
        usage_error(&["probe", "extra"]),
        format!("{MAIN_USAGE}tobii-linux: error: unrecognized arguments: extra\n")
    );
}

#[test]
fn serve_checks_its_options_before_starting() {
    let error = |message: &str| format!("{SERVE_USAGE}tobii-linux serve: error: {message}\n");
    assert_eq!(usage_error(&["serve"]), error("the following arguments are required: --display"));
    assert_eq!(usage_error(&["serve", "--display", "1", "2", "3"]), error("argument --display: expected 9 arguments"));
    let display = ["serve", "--display", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
    assert_eq!(
        usage_error(&[&display[..], &["--wine-clock", "x"]].concat()),
        error("argument --wine-clock: invalid choice: 'x' (choose from 'auto', 'boottime', 'raw')")
    );
}

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let (code, stdout, stderr) = run(&["serve", "--help"]);
    assert_eq!((code, stderr.as_str()), (Some(0), ""));
    assert!(stdout.starts_with(SERVE_USAGE), "{stdout}");
    assert!(stdout.contains("  --tet-tcp             Serve the route that needs no Wine patch"), "{stdout}");
    let (code, stdout, _) = run(&["--help"]);
    assert_eq!(code, Some(0));
    assert!(stdout.starts_with(MAIN_USAGE) && stdout.contains("Native Eye Tracker 5 driver (experimental)"));
}

#[test]
fn demo_options_are_checked_by_the_demo() {
    let stderr = usage_error(&["demo", "--gaze-source", "bogus"]);
    assert!(stderr.starts_with("usage: tobii-linux demo [-h] [--host HOST] [--port PORT]\n"), "{stderr}");
    assert!(
        stderr.ends_with(
            "tobii-linux demo: error: argument --gaze-source: invalid choice: 'bogus' (choose from 'device')\n"
        ),
        "{stderr}"
    );
}
