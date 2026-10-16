//! The service without a tracker, as `python3 -m tobii_linux.prp_service` runs it for the Wine
//! checks (tools/prp_bridge/run_wine_test.sh): `prp_service [--host H] [--port P] [--name N]
//! [--synthetic] [--no-gaze] [--display-area X,Y,Z,...] [--display-id ID] [--debug]
//! [--wine-clock auto|boottime|raw]`.

use std::sync::Arc;

use tobii_linux::service::clock::{Clocks, now_us, wine_clock};
use tobii_linux::service::synthetic::publish_synthetic;
use tobii_linux::service::{Corners, Options, PrpService, REAL_SERVICE_PERIOD, list_repr, listen};
use tobii_protocol::gaze::PLATMOD_PRESENT;
use tobii_service::prp_service::{DEFAULT_NAME, DEFAULT_PORT};

const USAGE: &str = "usage: prp_service [--host H] [--port P] [--name N] [--synthetic] [--no-gaze] \
                     [--display-area X,Y,Z,X,Y,Z,X,Y,Z] [--display-id ID] [--debug] [--wine-clock CLOCK]";

struct Arguments {
    host: String,
    port: u16,
    name: String,
    synthetic: bool,
    gaze: bool,
    display_area: Option<Corners>,
    display_id: Option<String>,
    debug: bool,
    wine_clock: String,
}

fn corners(text: &str) -> Option<Corners> {
    let values: Vec<f64> = text.split(',').map(|value| value.trim().parse().ok()).collect::<Option<_>>()?;
    let values: [f64; 9] = values.try_into().ok()?;
    Some(tobii_linux::cli::corners(&values))
}

fn arguments() -> Result<Arguments, String> {
    let mut parsed = Arguments {
        host: "127.0.0.1".into(),
        port: DEFAULT_PORT,
        name: DEFAULT_NAME.into(),
        synthetic: false,
        gaze: true,
        display_area: None,
        display_id: None,
        debug: false,
        wine_clock: "auto".into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| USAGE.to_string());
        match arg.as_str() {
            "--host" => parsed.host = value()?,
            "--port" => parsed.port = value()?.parse().map_err(|_| USAGE.to_string())?,
            "--name" => parsed.name = value()?,
            "--synthetic" => parsed.synthetic = true,
            "--no-gaze" => parsed.gaze = false,
            "--display-area" => parsed.display_area = Some(corners(&value()?).ok_or_else(|| USAGE.to_string())?),
            "--display-id" => parsed.display_id = Some(value()?),
            "--debug" => parsed.debug = true,
            "--wine-clock" => parsed.wine_clock = value()?,
            _ => return Err(USAGE.into()),
        }
    }
    Ok(parsed)
}

async fn serve(arguments: Arguments) -> std::io::Result<()> {
    let clock = wine_clock(&arguments.wine_clock, Clocks::Host).ok_or_else(|| std::io::Error::other(USAGE))?;
    let options = Options {
        name: arguments.name,
        display_area: arguments.display_area,
        display_id: arguments.display_id,
        gaze: arguments.synthetic && arguments.gaze,
        ..Options::default()
    };
    let service = PrpService::new(options, Arc::clone(&clock));
    if arguments.synthetic {
        service.attach_tracker(None);
        service.publish_presence(now_us(&clock), i64::from(PLATMOD_PRESENT));
    }
    let listener = listen(&arguments.host, arguments.port).await?;
    let address = listener.local_addr()?;
    log::info!(
        target: "tobii_linux",
        "PRP service on ('{}', {}), pipes {}",
        address.ip(),
        address.port(),
        list_repr(&service.pipe_names())
    );
    let accept = service.serve(listener);
    if arguments.synthetic {
        tokio::task::spawn_local(publish_synthetic(service.clone(), REAL_SERVICE_PERIOD));
    }
    let _ = accept.await;
    Ok(())
}

fn main() -> std::process::ExitCode {
    let arguments = match arguments() {
        Ok(arguments) => arguments,
        Err(usage) => {
            eprintln!("{usage}");
            return std::process::ExitCode::from(2);
        }
    };
    let level = if arguments.debug { log::Level::Debug } else { log::Level::Info };
    tobii_linux::logger::install(level, tobii_linux::logger::TimeFormat::Milliseconds);
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime");
    match tokio::task::LocalSet::new().block_on(&runtime, serve(arguments)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
