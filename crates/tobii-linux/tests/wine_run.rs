//! tools/prp_bridge/tobii-linux-wine-run starting and stopping this binary as its service,
//! as tests/test_wine_run.py does with a fake service: the wrapper waits for `serve` to
//! listen, runs the bridge and the game under a fake Wine, then stops the service with
//! SIGTERM, which it answers with a clean shutdown.

use std::path::{Path, PathBuf};
use std::process::Command;

const FAKE_WINE: &str = r#"#!/usr/bin/env bash
if [ "$(basename "$1")" = prp_bridge.exe ]; then
    echo "bridge $*" >> "$STATE/log"
    echo "12:00:00.000 serving \\\\.\\pipe\\TOBIIPRP-TEST" >&2
    while [ ! -f "$STATE/game-done" ]; do sleep 0.05; done
    echo "bridge exit" >> "$STATE/log"
    exit 0
fi
echo "game $*" >> "$STATE/log"
touch "$STATE/game-done"
exit 7
"#;
const DISPLAY: [&str; 9] = ["-296.62", "326", "111.82", "300.38", "326", "111.82", "-296.62", "10.27", "-3.1"];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn the_wrapper_starts_serve_for_the_game_and_stops_it_afterwards() {
    let models = tobii_linux::paths::default_models();
    if !models.join("support").is_dir() {
        eprintln!("skipped: no models in {}", models.display());
        return;
    }
    let root = std::env::temp_dir().join(format!("tobii-linux-wine-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (state, bridge, wine) = (root.join("state"), root.join("share/prp_bridge.exe"), root.join("bin/wine"));
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(bridge.parent().unwrap()).unwrap();
    std::fs::write(&bridge, b"MZ bridge").unwrap();
    executable(&wine, FAKE_WINE);
    let port = free_port();
    let service = format!(
        "exec {} serve --display {} --port {port} --models {}",
        env!("CARGO_BIN_EXE_tobii-linux"),
        DISPLAY.join(" "),
        models.display()
    );
    let output = Command::new(repository().join("tools/prp_bridge/tobii-linux-wine-run"))
        .args(["--service-command", &service])
        .arg(&wine)
        .arg("game.exe")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &root)
        .env("STATE", &state)
        .env("TOBII_LINUX_BRIDGE", &bridge)
        .env("TOBII_LINUX_PORT", port.to_string())
        .env("TOBII_LINUX_SERVICE_WAIT", "60")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(7), "{stderr}");
    assert!(stderr.contains(&format!("PRP service on 127.0.0.1:{port}, pipes ['TOBIIPRP-ET5-LINUX'")), "{stderr}");
    assert!(stderr.contains(" stopping\n") && stderr.contains(" stopped: 0 session(s), 0 lost\n"), "{stderr}");
    let log = std::fs::read_to_string(state.join("log")).unwrap();
    assert!(log.contains("game game.exe") && log.contains("bridge exit"), "{log}");
    std::fs::remove_dir_all(root).unwrap();
}
