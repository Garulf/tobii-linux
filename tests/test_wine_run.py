import os
import shutil
import socket
import subprocess
import tempfile
import textwrap
import time
import unittest
from pathlib import Path

WRAPPER = Path(__file__).resolve().parents[1] / "tools" / "prp_bridge" / "tobii-linux-wine-run"

FAKE_WINE = """\
#!/usr/bin/env bash
if [ "$(basename "$1")" = prp_bridge.exe ]; then
    echo "bridge $*" >> "$STATE/log"
    echo "12:00:00.000 serving \\\\\\\\.\\\\pipe\\\\TOBIIPRP-TEST" >&2
    while [ ! -f "$STATE/game-done" ]; do sleep 0.05; done
    echo "bridge exit" >> "$STATE/log"
    exit 0
fi
echo "game $*" >> "$STATE/log"
touch "$STATE/game-done"
exit 7
"""

SKIP_TO_COMMAND = """\
#!/usr/bin/env bash
while [ "$1" != -- ]; do shift; done
shift
exec "$@"
"""

RECORD_ARGUMENTS = """\
#!/usr/bin/env bash
printf '%s\\n' "$(basename "$0")" "$@" > "$STATE/argv"
echo "${STEAM_COMPAT_INSTALL_PATH:-unset}" > "$STATE/install-path"
exit 3
"""


def resolve_env_shebang(text):
    """Nix build sandboxes have no /usr/bin/env, so point the shebang at the interpreter itself."""
    first, _, rest = text.partition("\n")
    if first.startswith("#!/usr/bin/env "):
        first = "#!" + shutil.which(first.split()[1])
    return first + "\n" + rest


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class WineRunTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.state = self.root / "state"
        self.state.mkdir()
        self.bridge = self.root / "share" / "prp_bridge.exe"
        self.bridge.parent.mkdir()
        self.bridge.write_bytes(b"MZ bridge")
        self.port = free_port()
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", self.port))
        self.listener.listen()

    def tearDown(self):
        self.listener.close()
        self.temp.cleanup()

    def tool(self, name, text):
        path = self.root / "bin" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(resolve_env_shebang(text))
        path.chmod(0o755)
        return path

    def run_wrapper(self, *arguments, env=None, timeout=20):
        environment = {
            "PATH": os.environ["PATH"],
            "HOME": str(self.root),
            "STATE": str(self.state),
            "TOBII_LINUX_BRIDGE": str(self.bridge),
            "TOBII_LINUX_PORT": str(self.port),
            "TOBII_LINUX_SERVICE_WAIT": "3",
        }
        environment.update(env or {})
        return subprocess.run([str(WRAPPER), *map(str, arguments)], env=environment,
                              capture_output=True, text=True, timeout=timeout)

    def test_steam_proton_command_runs_the_bridge_as_the_proton_program(self):
        compat = self.root / "compatdata" / "123"
        proton = self.tool("proton", RECORD_ARGUMENTS)
        reaper = self.tool("reaper", SKIP_TO_COMMAND)
        entry_point = self.tool("_v2-entry-point", SKIP_TO_COMMAND)
        game = "/games/Star Citizen/RSI Launcher.exe"
        result = self.run_wrapper(
            reaper, "SteamLaunch", "AppId=123", "--", entry_point,
            "--verb=waitforexitandrun", "--", proton, "waitforexitandrun", game, "-flag",
            env={"STEAM_COMPAT_DATA_PATH": str(compat)})
        self.assertEqual(result.returncode, 3, result.stderr)
        placed = compat / "tobii-linux" / "prp_bridge.exe"
        self.assertEqual(placed.read_bytes(), b"MZ bridge")
        argv = (self.state / "argv").read_text().splitlines()
        self.assertEqual(argv, ["proton", "waitforexitandrun", str(placed), "127.0.0.1",
                                str(self.port), "--", game, "-flag"])

    def test_dry_run_prints_the_rewritten_steam_command(self):
        compat = self.root / "compatdata" / "123"
        result = self.run_wrapper(
            "--dry-run", "/steam/reaper", "SteamLaunch", "--", "/proton dir/proton", "run",
            "C:\\game.exe", env={"STEAM_COMPAT_DATA_PATH": str(compat)})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.splitlines(), [
            "mode proton", "/steam/reaper", "SteamLaunch", "--", "/proton dir/proton", "run",
            str(compat / "tobii-linux" / "prp_bridge.exe"), "127.0.0.1", str(self.port), "--",
            "C:\\game.exe"])

    def test_proton_without_compat_data_path_is_refused(self):
        result = self.run_wrapper("--dry-run", "/p/proton", "waitforexitandrun", "game.exe")
        self.assertEqual(result.returncode, 2)
        self.assertIn("STEAM_COMPAT_DATA_PATH", result.stderr)

    def test_umu_command_runs_the_bridge_and_keeps_the_game_directory_visible(self):
        prefix = self.root / "prefix"
        umu = self.tool("umu-run", RECORD_ARGUMENTS)
        game = self.root / "Games" / "RSI Launcher.exe"
        game.parent.mkdir()
        game.write_bytes(b"MZ")
        result = self.run_wrapper(umu, game, "--arg", env={"WINEPREFIX": str(prefix)})
        self.assertEqual(result.returncode, 3, result.stderr)
        placed = prefix / "tobii-linux" / "prp_bridge.exe"
        argv = (self.state / "argv").read_text().splitlines()
        self.assertEqual(argv, ["umu-run", str(placed), "127.0.0.1", str(self.port), "--",
                                str(game), "--arg"])
        self.assertEqual((self.state / "install-path").read_text().strip(), str(game.parent))

    def test_umu_winetricks_is_not_wrapped(self):
        result = self.run_wrapper("--dry-run", "umu-run", "winetricks", "d3dx9")
        self.assertEqual(result.returncode, 2)

    def test_plain_wine_starts_the_bridge_before_the_game_and_waits_for_it(self):
        wine = self.tool("wine", FAKE_WINE)
        result = self.run_wrapper(wine, "C:\\game.exe", "-x")
        self.assertEqual(result.returncode, 7, result.stderr)
        log = (self.state / "log").read_text().splitlines()
        self.assertEqual(log, [
            f"bridge {self.bridge} --until-prefix-exits 127.0.0.1 {self.port}",
            "game C:\\game.exe -x",
            "bridge exit",
        ])

    def test_script_command_uses_the_wine_from_the_environment(self):
        wine = self.tool("wine-ge", FAKE_WINE)
        script = self.tool("sc-launch.sh", '#!/usr/bin/env bash\n"$WINE" "C:\\\\launcher.exe"\n')
        result = self.run_wrapper(script, env={"WINE": str(wine)})
        self.assertEqual(result.returncode, 7, result.stderr)
        self.assertIn("game C:\\launcher.exe", (self.state / "log").read_text())

    def test_unknown_wine_is_an_error(self):
        script = self.tool("launch.sh", "#!/bin/sh\nexit 0\n")
        result = self.run_wrapper(script)
        self.assertEqual(result.returncode, 2)
        self.assertIn("--wine", result.stderr)

    def test_missing_service_is_fatal_only_when_required(self):
        self.listener.close()
        wine = self.tool("wine", FAKE_WINE)
        result = self.run_wrapper("--require-service", wine, "game.exe")
        self.assertEqual(result.returncode, 2)
        self.assertIn("no Eye Tracker 5 service", result.stderr)
        result = self.run_wrapper(wine, "game.exe")
        self.assertEqual(result.returncode, 7)

    def test_service_command_is_started_and_stopped(self):
        self.listener.close()
        service = self.tool("fake-service", textwrap.dedent(f"""\
            #!/usr/bin/env python3
            import os, socket, time
            open(os.environ["STATE"] + "/service-pid", "w").write(str(os.getpid()))
            server = socket.create_server(("127.0.0.1", {self.port}))
            while True:
                server.accept()[0].close()
            """))
        wine = self.tool("wine", FAKE_WINE)
        result = self.run_wrapper("--service-command", f"exec {service}", wine, "game.exe")
        self.assertEqual(result.returncode, 7, result.stderr)
        pid = int((self.state / "service-pid").read_text())
        for _ in range(50):
            if not Path(f"/proc/{pid}").exists():
                break
            time.sleep(0.1)
        self.assertFalse(Path(f"/proc/{pid}").exists(), "service still running")


if __name__ == "__main__":
    unittest.main()
