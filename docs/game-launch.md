# Launching a game with head tracking

A Windows game reaches the Linux service through three pieces, and all three
have to line up when the game starts:

1. **The service** (`tobii-linux serve`), a native Linux process that owns the
   tracker and listens on `127.0.0.1:7760`.
2. **The bridge** (`prp_bridge.exe`), which must run in the game's Wine prefix
   under the same wineserver as the game, because a native process cannot
   create Wine named pipes ([prp-bridge.md](prp-bridge.md)).
3. **A Wine or Proton build with the named-pipe listing patch**, because Star
   Citizen's Tobii layer only tracks trackers it finds by listing `\\.\pipe\*`
   ([wine-pipe-enumeration.md](wine-pipe-enumeration.md)). The experimental
   [patch-free route](#without-a-patched-wine) drops this piece.

`tobii-linux-wine-run` takes care of the first two. Put it in front of the
command that starts the game:

```sh
tobii-linux-wine-run [options] <game command>
```

It checks that the service answers (and can start it), places the bridge in
the game's prefix, starts the game, and leaves once the prefix has no programs
left. The third piece, the patched Wine, is a property of the runner you pick;
see [Patched runners](#patched-runners).

## How the wrapper finds the prefix

The wrapper looks at the game command the way Lutris, Steam or a launch script
would run it:

| Command contains | Mode | What happens |
| --- | --- | --- |
| `.../proton <verb> <program>` (Steam) | proton | The bridge is copied to `$STEAM_COMPAT_DATA_PATH/tobii-linux/` and becomes the program Proton runs; it starts the original program itself |
| `umu-run <program>` (Lutris with GE-Proton) | umu | Same, with the copy in `$WINEPREFIX/tobii-linux/`; `STEAM_COMPAT_INSTALL_PATH` is set to the game's directory so the container still sees it |
| anything else | wine | The bridge runs beside the game with the same Wine: the command itself when it starts with `wine`, `wine64` or `wine-*`, otherwise `--wine` or `$WINE` (Lutris sets it) |

In the proton and umu modes the bridge has no terminal; its log is
`tobii-linux/bridge.log` beside its copy, rewritten at every launch. In wine
mode it logs to the wrapper's stderr.

Proton runs the game inside the Steam Linux Runtime container and starts its
own wineserver there, so a bridge started outside could not reach the game's
wineserver, and `waitforexitandrun` would wait for a bridge that was already
running in the prefix. Running the bridge as the Proton program keeps it in
the same container and prefix. The bridge then starts the game as a child process,
before which it has already created its pipes; it is never loaded into the game
process.

In every mode the bridge marks itself as a Wine system process, the way
Proton's own `steam.exe` does, and exits once the last ordinary program of the
prefix has exited. A launcher that closes while its game keeps running (the RSI
Launcher can do this) therefore does not stop tracking, and the bridge never
keeps a prefix or Steam's "running" state alive on its own. If the service
restarts while the game runs, the bridge reconnects and serves the new pipes.

### Options

| Option | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--bridge PATH` | `TOBII_LINUX_BRIDGE` | beside the script, or `../share/tobii-linux/` | `prp_bridge.exe` to use |
| `--host HOST`, `--port PORT` | `TOBII_LINUX_HOST`, `TOBII_LINUX_PORT` | `127.0.0.1`, `7760` | Where the service listens |
| `--wine PATH` | `WINE` | the command itself | Wine that runs the game in wine mode |
| `--start-service` | `TOBII_LINUX_START_SERVICE=1` | off | `systemctl --user start` the unit when the service is down |
| `--service-unit NAME` | `TOBII_LINUX_SERVICE_UNIT` | `tobii-head-pose.service` | Unit for `--start-service` |
| `--service-command CMD` | `TOBII_LINUX_SERVICE_COMMAND` | none | Start CMD when the service is down and stop it after the game |
| `--require-service` | `TOBII_LINUX_REQUIRE_SERVICE=1` | off | Refuse to start the game without the service |
| `--dry-run` | | | Print the mode and the final command |

Without `--require-service`, a missing service is only a warning: the bridge
keeps trying to reach it while the game runs, so the service can be started
later.

## The service clock

The service stamps samples with the clock behind the game's
`QueryPerformanceCounter` ([prp-bridge.md](prp-bridge.md#time-base)), and that
clock depends on the runner:

| Runner | Clock |
| --- | --- |
| Wine 10.17 or later, wine-staging, wine-tkg and LUG runners on those | `CLOCK_BOOTTIME` |
| Proton 9, 10 and 11, GE-Proton 11 | `CLOCK_MONOTONIC_RAW` |
| Wine 10.16 or older | `CLOCK_MONOTONIC_RAW` |

Nothing needs to be set for this. The bridge reports its counter to the
service, which follows the matching clock (`--wine-clock auto`, the default)
and logs its choice:

```text
bridge ('127.0.0.1', 44362): QueryPerformanceCounter follows raw (absolute; ...)
service clock now follows raw instead of boottime (absolute match); moved -550.994 ms
```

The choice is made when the bridge connects, before the game starts. If the
clock changes later while a game is connected, the service disconnects the
game's clients and the game reconnects on the new clock, as after a service
restart. With a bridge older than this report the service stays on
`CLOCK_BOOTTIME`.

`--wine-clock boottime` or `--wine-clock raw` still fixes the clock; the
service then warns when the bridge's counter follows the other one. With the
wrong clock, sample timestamps drift from the game's clock by the rate
difference between the two: about 9% on the WSL2 test host (in a GE-Proton
test run with the old `boottime` default, samples fell from 6 ms to 58 ms
behind the game's clock within 0.6 s), tens of parts per million on a native
kernel, plus the whole suspend time after a host suspend. The NixOS module
sets it with `headPoseService.wineClock`.

## Lutris

Open the game's **Configure** dialog, **System options**, and set **Command
prefix** to:

```text
tobii-linux-wine-run --start-service
```

Lutris inserts the prefix before the Wine (or `umu-run`) command and exports
`WINE` and `WINEPREFIX`, so nothing else is needed. The runner must carry the
patch:

- **Wine runners** (LUG `lug-wine-tkg`, wine-tkg, wine-staging, a system
  Wine): build the runner with the patch (below) and select it under
  **Runner options**, **Wine version**.
- **GE-Proton through umu**: needs a GE-Proton built with the Proton variant of
  the patch. Lutris passes `PROTONPATH`; point it at the patched build.

A **pre-launch script** is not a good fit: it runs before Wine starts and does
not see the game end.

## LUG Helper launch script

The LUG Helper writes `~/Games/star-citizen/sc-launch.sh`. Its last line runs
the RSI Launcher; put the wrapper in front of that line only, not around the
whole script, because the script runs `wineserver -k` first:

```sh
tobii-linux-wine-run --start-service "$wine_path"/wine "C:\Program Files\Roberts Space Industries\RSI Launcher\RSI Launcher.exe" > "$launch_log" 2>&1
```

LUG runners are wine-tkg builds; [their build](#wine-tkg-and-lug-runners)
accepts the patch as a user patch.

## Steam and Proton

Add the RSI Launcher (or the game) to Steam, choose the patched Proton under
**Compatibility**, and set the launch options to:

```text
tobii-linux-wine-run --start-service %command%
```

Steam passes
`STEAM_COMPAT_DATA_PATH`, and `%command%` ends with
`.../proton waitforexitandrun <program>`; the wrapper rewrites that to
`.../proton waitforexitandrun .../tobii-linux/prp_bridge.exe 127.0.0.1 7760 --
<program>`. Steam runs launch options outside the container, so the wrapper
and the service check run on the host. Flatpak Steam runs them inside its
sandbox instead, where a host-installed wrapper is not available.

## Plain Wine

```sh
WINEPREFIX=~/Games/star-citizen tobii-linux-wine-run \
  /opt/wine-tobii/bin/wine "C:\Program Files\Roberts Space Industries\RSI Launcher\RSI Launcher.exe"
```

## NixOS

The module installs `tobii-linux-wine-run` (x86-64 only) with the bridge built
by MinGW-w64 ([nix/bridge.nix](../nix/bridge.nix)), and can run the service as
a user unit:

```nix
hardware.tobii-eye-tracker-5 = {
  enable = true;
  headPoseService = {
    enable = true;
    displayArea = [ (-296.62) 326 111.82 300.38 326 111.82 (-296.62) 10.27 (-3.1) ];
  };
};
```

With the user service enabled, `--start-service` is not needed. Steam's FHS
environment may not have `/run/current-system/sw/bin` on its `PATH`; use the
absolute path in the launch options:

```text
/run/current-system/sw/bin/tobii-linux-wine-run %command%
```

For a Lutris Wine runner, [nix/wine.nix](../nix/wine.nix) is an overlay that
builds nixpkgs' Wine with the patch:

```nix
nixpkgs.overlays = [ (import "${inputs.tobii-linux}/nix/wine.nix") ];
environment.systemPackages = [ pkgs.tobii-linux-wine.staging ];
```

Then choose **System Wine** in Lutris, or point a custom Wine version at the
package's `bin/wine`. `tobii-linux-wine.stable` (11.0) and `.unstable` (11.19)
are also available; `.patchedSourceCheck.<name>` only applies the patch.
All current nixpkgs Wines read `CLOCK_BOOTTIME`.

## Patched runners

Results of `patches/wine/apply.sh` on current sources are in
[wine-pipe-enumeration.md](wine-pipe-enumeration.md#runner-sources). In short:

- Upstream Wine 10.0 or later, wine-staging, wine-tkg and the LUG runners take
  the upstream patch (built and run: 11.0 and master).
- Valve's Proton 10 and 11 and GE-Proton 11 take the Proton variant, which
  `apply.sh` selects by itself (built and run: Proton 11's Wine).
- Proton 9 (Wine 9.0) takes a separate variant (applies; its build was not
  completed).
- Proton 8 and older, and Lutris' old `wine-ge-8` runners (Wine 8.0), have
  neither the pipe root directory object nor Valve's partial listing;
  `apply.sh` refuses them.

### wine-tkg and LUG runners

wine-tkg applies `*.mypatch` files from `wine-tkg-userpatches/` before it runs
`tools/make_requests`, so the patch goes in without its generated hunks:

```sh
patches/wine/apply.sh --emit > wine-tkg-git/wine-tkg-userpatches/tobii-pipe-listing.mypatch
```

For the LUG build script (https://github.com/starcitizen-lug/lug-wine), copy
that file to `patches/wine/tobii-pipe-listing.patch` and build with
`./build-lug-wine.sh -a tobii-pipe-listing`.

### Proton and GE-Proton

Apply the patch to the `wine` submodule of the Proton or proton-ge-custom
checkout after its own preparation, then build as usual:

```sh
patches/wine/apply.sh path/to/proton-ge-custom/wine
```

For proton-ge-custom that means after `patches/protonprep-valve-staging.sh`,
which runs `tools/make_requests` again at its end, so the order does not
matter for the generated files. Install the result in
`~/.steam/root/compatibilitytools.d/` (Steam) or point `PROTONPATH` at it
(umu, Lutris).

## What was tested

On albedo's WSL2 Ubuntu with the synthetic service and Star Citizen's game DLL
driven by `prp_test_client.exe` ([prp-bridge.md](prp-bridge.md#launch-modes)):

- plain Wine (patched 11.0): bridge beside the client and bridge as launcher,
  discovery, a service restart in between, and the bridge leaving on its own;
- `tobii-linux-wine-run .../GE-Proton11-9/proton waitforexitandrun <client>`:
  the rewritten command, `steam.exe` starting the bridge, the bridge starting
  the client, 91 samples per 3 s;
- `tobii-linux-wine-run umu-run <client>` with GE-Proton 11-9 inside umu's
  Steam Linux Runtime 4 container: the bridge copy in the prefix, the relay
  from the container to the host service, 90 samples per 3 s.

Not tested: the Steam client and Lutris themselves, Star Citizen, a NixOS
machine, and the bridge leaving by itself under Proton (in WSL2 a hung display
start kept the prefix busy).

## Without a patched Wine

Stock Wine, wine-staging and Proton cannot list `\\.\pipe\*` for the game's
Tobii DLL, and nothing outside the game process can change that without
replacing at least the wineserver
([wine-pipe-enumeration.md](wine-pipe-enumeration.md#without-a-wine-patch)).
The same DLL also finds a tracker on TCP `127.0.0.1:4455` and then gets head
pose from the fixed-name pipe `streamengineservices`, which needs no listing
([tet-tcp.md](tet-tcp.md), [sesp.md](sesp.md)). Start the service with
`--tet-tcp` and launch the game exactly as above, with the stock runner:

```sh
tobii-linux serve --tet-tcp --display -296.62 326 111.82 300.38 326 111.82 -296.62 10.27 -3.1
tobii-linux-wine-run %command%          # Steam with stock Proton
```

On NixOS set `headPoseService.tetTcp = true`. The bridge is the same program;
the service tells it to serve only `streamengineservices` and itself listens on
port 4455 while the tracker runs.

Status, by evidence:

| Check | Result | Evidence |
| --- | --- | --- |
| Discovery, device creation, head-pose and gaze streams of the unmodified DLL | `tet-tcp://127.0.0.1` found; head pose and gaze flow | emulated (Wine): Debian Wine 8.0, recorded ET5 replies and gaze, synthetic head pose |
| Game integration layer, `TrackTracker` | head pose about 31 a second, gaze, presence; capabilities as on the PRP route | emulated (Wine) |
| Timestamps on the game's clock | yes, without timesync | emulated (Wine) |
| Window tracking (`TrackWindow`, `TrackRectangle`) | no monitor on this route | emulated (Wine) |
| Codec against Tobii's own SESP code | 60 of 60 cases | emulated |
| Service with a session that ends and starts again | port and replies follow the session | unit |
| Real tracker, stock Wine 9.0, GE-Proton, real Windows SESP service | not run | live (Wine), live (Windows): staged in `evidence/private/sesp/live/` |
| Star Citizen | not run | |

Use the PRP route with a patched runner when one is available: it has run live
end to end and supports window tracking. Use `--tet-tcp` when the runner
cannot be patched (stock Proton through Steam, a distribution Wine), knowing
it has not yet run with the real tracker.
