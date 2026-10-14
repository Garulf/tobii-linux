# The tet-tcp route

Star Citizen's `tobii_gameintegration_x64.dll` 9.0.3.25 (SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`) also finds a
tracker by connecting to TCP port 4455 on `127.0.0.1`. TCP works in unmodified
Wine and Proton, so a native Linux process on that port needs no pipe-listing
Wine patch. This document records what the DLL does with such a tracker, how
`cli serve --tet-tcp` serves it, and what reaches the game: gaze over TCP, and
head pose over the fixed-name pipe `streamengineservices`, which the unchanged
`prp_bridge.exe` serves ([sesp.md](sesp.md)).

Evidence labels:

- **static**: objdump disassembly and focused Ghidra `-noanalysis` decompiles of
  the DLL (`evidence/private/tt-batch1-focused.c`, `tt-batch2-focused.c`, project
  copy `evidence/private/ghidra-copy-tettcp`, script `tools/ghidra/focused-dll.sh`).
- **emulated (Wine)**: the unmodified DLL under local Wine 8.0 (Debian) against
  `tools/tet_tcp_probe.py --backend replay`, which answers with real ET5 replies
  and real stream `0x500` messages recorded earlier on albedo
  (`evidence/private/calibration/probe/*-raw.jsonl`), plus synthetic replies for
  the commands never recorded (marked below). Logs:
  `evidence/private/tet-tcp/replay/`.
- **emulated (Wine), service**: `cli serve --tet-tcp`'s service objects run by
  `tools/tet_tcp_emulation.py` with the real ET5's recorded getter replies
  (`evidence/private/gaze-capture/device-query.jsonl`), recorded `0x500`
  messages and synthetic head pose, `prp_bridge.exe`, and the unmodified DLL
  under this container's unpatched Debian Wine 8.0. Logs:
  `evidence/private/sesp/emulation/`.
- **unit**: `tests/test_tet_tcp_service.py`, `tests/test_sesp.py`,
  `tests/test_supervisor.py`.
- **live (Wine)**: albedo WSL2 with the tracker attached. Not run: albedo was
  unreachable for the whole investigation. The scripts are ready (see
  [Live test](#live-test)).
- **Wine source**: Wine's `server/sock.c`.
- **inference**: a conclusion drawn from the above.

## Result

| Question | Answer | Evidence |
| --- | --- | --- |
| What does `device_create` build for `tet-tcp://` | An in-process platform module, "Legacy TTP (4.17.0/6)", platmod API 56, reached by the client through `prp-embedded://<uuid>`. It wraps a legacy tracker client (`legacy_ttp/tracker.cpp`) | static, emulated |
| What it expects over TCP | Device-level TTP on port 4455, the framing `ttp.py` decodes, with no USB envelope. Timesync uses UDP 4457 | static, emulated |
| Does it run tracking itself | No. It never asks for images; gaze is the device's own stream `0x500` passed through; the DLL contains no face, landmark or head-pose model code | static, emulated |
| Head pose on this route | Only from Stream Engine Services over the named pipe `tobii-sesp://streamengineservices`, opened by name. With the bridge serving it, head pose reaches the game | static, emulated (Wine), service |
| Gaze on this route | Works: 268 of 275 Stream Engine gaze points valid in 10 s | emulated (Wine), service |
| Integration layer | Discovers `tet-tcp://127.0.0.1`, `TrackTracker` succeeds, capabilities `0x9b` as on the PRP route, `head_stream_supported=1`, 1104 head poses and 740 gaze points in 35 s, `IsPresent` 1 | emulated (Wine), service |
| Timesync under Wine | Fails on every Wine version checked: Wine never signals `FD_CONNECT` for a UDP connect. The module then passes device timestamps through, so the service rewrites them to the game's QueryPerformanceCounter microseconds | emulated, Wine source |
| Window tracking | No monitor: the module's display id needs SESP display info, which the service does not serve. `TrackTracker` works | static, emulated (Wine), service |
| Discovery | Default mask probes only `127.0.0.1:4455`; a 50 ms connect window; the integration layer rescans every 10 s | static, live (Wine) earlier |

**Result.** The route carries head pose and gaze to the game layer under an
unpatched Wine (emulated (Wine), service): `cli serve --tet-tcp` answers port
4455 from its own tracker session and, through the unchanged bridge, the SESP
pipe. It still needs `prp_bridge.exe` in the prefix, but no Wine patch. It is
off by default until it has run live; see [Which route](#which-route).

## Discovery (static, live (Wine) earlier)

`0x180058010` is called by `tobii_enumerate_local_device_urls` with mask
`0xFFFFFFFC`. Mask bit 2 selects `127.0.0.1`, bit 1 selects `172.28.195.1`, so
only `127.0.0.1` is probed. It opens a non-blocking TCP socket per host
(`socket`, `ioctlsocket(FIONBIO)`, `WSAEventSelect(FD_CONNECT)`, `connect` to
service `"4455"`), waits 50 ms for all events, and reports `tet-tcp://<host>`
for every socket whose `FD_CONNECT` arrived without error. It then shuts the
socket down. Nothing is sent on discovery sockets; they show up as empty
connections (`evidence/private/tet-tcp/replay/record-only-probe.jsonl`).

The integration layer's scan thread enumerates once and every 10 s, creates a
device for each URL, reads its information and destroys it again
([game-integration-layer.md](game-integration-layer.md)). Each scan is therefore
one discovery connection plus one full device start on port 4455. Both
`tobii_gameintegration_x64.dll` and `tobii_stream_engine.dll` report
`tet-tcp://127.0.0.1` under stock Wine 9.0 (live (Wine), earlier run in
`evidence/private/launch-work/test-nopatch-1.txt`).

## Device creation (static)

`device_create` (`0x18002a6a0`) treats every URL other than `tobii-prp://`,
`tprp-tcp://` and `tobii-qvr-prp://` as an embedded device:

1. `0x18003fe60` makes an id, and the module URL becomes `prp-embedded://<id>`.
2. `0x18003e720` builds the platform module context with the device URL, and
   `0x18005a880` starts it (`pr_ipc_service`, `pris_internal_create`, which logs
   "platmod_start with API version 56" on failure).
3. The PRP client connects to the module in-process and runs the normal
   connect sequence. The module answers with build version
   `Legacy TTP (4.17.0/6)` (emulated).

`platmod_start` (`0x18003e860`) then:

1. Creates the tracker client (`0x18005cdd0`, `tracker_create`), which connects
   the device URL (`0x18007dcb0`) and sends protocol upgrade and `unlock_ppu`.
   If `unlock_ppu` asks for a realm it opens it with its own key
   (`1d3ca73b517a4fab80e8a3b5a6ade212`, at `0x180280030`), a different key from
   the service's. The ET5 answers `unlock_ppu` with `0, 0, 0` (recorded reply),
   so no realm is opened.
2. Connects to Stream Engine Services (`0x18005afd0`, three tries 50 ms apart)
   and keeps going if that fails (flag at module `+0xa850`).
3. Builds the stream list it reports to the client. The head-pose entry is
   added only when the SESP connection succeeded and SESP reports head pose
   (`0x18005b2b0(sesp, 0)` at `0x18003f550`); the gaze entries depend on the
   tracker's stream types (`0x18004c7b0`). Capability id 3 of the 1350 reply
   (`0x18005e500`, records `{uint32 id, string "true"/"false"}`) sets flag
   `+0xe659`, which only feeds another list.

## Transport (static)

`0x18004e8a0` maps channels to ports for `tet-tcp://<host>`, and
`0x180090110` opens `socket(AF_INET, type, 0)`:

| Channel | Port | Socket | Use |
| --- | --- | --- | --- |
| 1 | 4455 | TCP | TTP requests, replies and stream data (`0x18007dcb0`) |
| 3 | 4457 | UDP | timesync (`ensure_timesync_connected`, `0x18006ac50`) |
| 4 | 4459 | TCP | not opened in any run |
| `0x4E21`, `0x4E22` | 4453, 4454 | TCP | PRP channel numbers; not used for `tet-tcp://` in the runs |

Each channel connects non-blocking and waits for `FD_CONNECT` (`0x1800904c0`),
100 ms for channels 1 and 3. Bytes are plain TTP messages: the 24-byte
big-endian header and payload of `src/tobii_linux/ttp.py`, exactly what the
tracker carries inside USB channel 0 frames.

## Requests (emulated)

Device start, in order, from `evidence/private/tet-tcp/replay/r6-probe.jsonl`.
"Recorded" replies are real ET5 bytes; "synthetic" ones were made up to get
further and may differ from the device.

| Command | Name | Reply used | Outcome |
| --- | --- | --- | --- |
| 1000 | protocol upgrade, versions `0x10000..0x10008` | recorded | required |
| 1600 | `unlock_ppu` | recorded (`0, 0, 0`) | required |
| 1420 | device info | synthetic: serial, model, generation, firmware strings | required |
| 1330 | device properties | none (error) | tolerated |
| 1490 | device status | recorded | |
| 1350 | capabilities | none (error) | tolerated; no capability flags, so no head pose stream |
| 1200 | enumerate stream types, array of struct `0x41389` (uint32, string, string, uint32) | synthetic: `0x500`, `0x501`, `0x504` | required |
| 1430 | display area | synthetic corners | required |
| 1460 | display area writable | none (error) | tolerated |
| 1700 | device name | none (error) | tolerated |
| 3060 | persistent file enumerate | none (error) | tolerated |
| 2110 | geometry mounting | none (error) | tolerated |
| 1650 | frequencies (integration layer only) | none (error) | tolerated |
| 1220 | subscribe `0x504` presence (integration layer) | none (error) | tolerated |
| 1220 | subscribe `0x500` gaze | recorded | gaze flows |
| 1230 | unsubscribe `0x500` | recorded | |

No write command was sent in any run. The module also has getters for track
box (1400) and others that these runs did not reach. Command 1210 (stream
columns) was never sent.

## Head pose (static, emulated, emulated (Wine))

The legacy module has no head-pose computation; Stream Engine Services gives
it head pose over the pipe `streamengineservices` ([sesp.md](sesp.md) has the
protocol). The chain is:

1. `platmod_start` connects SESP (`0x18005afd0`, three tries 50 ms apart) and
   sets module flag `+0xa850` when the initialize request is answered without
   error. It adds the head-pose stream when SESP also reports capability 1
   (`0x18005b2b0(sesp, 0)` at `0x18003f550`).
2. `platmod_stream_head_pose_subscribe` (`0x180039e50`) returns error 3 unless
   that flag and capability are set, then sends head-pose start
   (`0x18005b2d0`, `0x18005cc30`, `0x180087fd0`).
3. Samples arrive only through `0x180049aa0`, the sink `platmod_start` passes to
   the SESP connect. It fills the ring at module `+0x26dc8` that `0x18003ff10`
   hands to the client's head-pose callback, with the timestamp unchanged and
   the position transformed by the display area ([sesp.md](sesp.md#position-transform-static-emulated)).
4. The pipe is opened by its fixed name: no listing is involved, so the Wine
   patch is not needed. A native Linux process still cannot own it; the bridge
   in the prefix does.

Without the pipe, `tobii_stream_supported` reports head pose (stream 4)
unsupported and `tobii_head_pose_subscribe` returns `TOBII_ERROR_NOT_SUPPORTED`;
the integration layer reports `head_stream_supported=0` and capabilities `0x99`
(emulated). A 1350 reply with capability 3 set to `true` changed nothing
(`evidence/private/tet-tcp/replay/r7-caps3-client.txt`). With the service
serving the pipe, streams 0 to 5 and 12 are supported, head pose included, the
capabilities are `0x9b` and the layer's head stream is supported (emulated
(Wine), service: `e1.client`, `t1.client`).

## Gaze (emulated, emulated (Wine))

Stream Engine entry points (`tettcp_test_client.exe`, `discover`, 8 s) against
the replay proxy: device created, gaze point subscribe sent TTP subscribe
`0x500`, 255 gaze callbacks, 248 valid. Integration layer (`tgi_test_client.exe
first 10`): one tracker `tet-tcp://127.0.0.1` (model, generation, serial and
firmware from 1420), `TrackTracker` true, `IsConnected` 1, `IsPresent` 1, 262
gaze points in 10 s, no monitor rectangle.

Against the production service (emulated (Wine), service, 30 recorded messages
a second): 275 gaze callbacks in 10 s, 268 valid, timestamps 4.4 ms (median)
behind the client's counter; the layer returned 740 gaze points in 35 s. The
counts show the path works, not the rate or accuracy the game would see live.

## Timesync and timestamps (emulated, Wine source)

Timesync runs on a connected UDP socket to port 4457. The client waits for
`FD_CONNECT`. Wine's `IOCTL_AFD_WINE_CONNECT` handler completes a datagram
`connect` immediately and never posts the connect event
(`server/sock.c`, wine-9.0 and master at the time of writing:
https://raw.githubusercontent.com/wine-mirror/wine/wine-9.0/server/sock.c,
https://raw.githubusercontent.com/wine-mirror/wine/master/server/sock.c).
Every run logged `ensure_timesync_connected` failing and no datagram reached
port 4457. `device_create` still succeeds; `tobii_update_timesync` fails.

Without timesync the module passes the device timestamp through unchanged: the
first recorded `0x500` message carries column 1 = `908279389020` and the client
reported gaze at `908286...` microseconds while its system clock read
`39345...`. The integration layer kept those points. On Windows, timesync
would turn device time into the client clock, QueryPerformanceCounter
microseconds (`0x180027650`). The service therefore rewrites column 1 of every
`0x500` and `0x504` message to the game's counter in microseconds and leaves
the other bytes alone; SESP head poses get the same clock
([sesp.md](sesp.md#timestamps-static-emulated-wine)). That clock is the
service clock when it reads the Linux clock behind the counter (the bridge's
reports decide it, as for PRP); for a counter that matches no Linux clock the
service subtracts the smallest recent offset of the bridge's reports
(`qpc_clock.CounterMapping`). Without a bridge it assumes `--wine-clock`'s
source. In the emulated (Wine) run the client saw gaze 4.4 ms and head pose 4.8 ms
(medians; 90th percentiles 7.2 and 10.9 ms) after their timestamps on its own
counter (`e1.client`). The mapping
follows the most recent bridge, so two prefixes on different clocks at once
are not supported (inference).

## Service (`cli serve --tet-tcp`)

`src/tobii_linux/tet_tcp_service.py` with `sesp_service.py`, behind
`cli serve --tet-tcp` (default off; NixOS `headPoseService.tetTcp`). With it the
service serves this route instead of the PRP pipes:

- **Port 4455 only while a tracker session runs.** The service closes it and
  every connection when the session ends and opens it again with the next
  session, as the PRP service refuses pipe clients without a tracker. The
  discovery probe and the full device start that the game layer makes every
  10 s are ordinary connections.
- **Replies from the service's own session, never forwarded.** Before its
  streams start, each session reads the tracker's replies to the getters the
  module sends (1200, 1330, 1350, 1400, 1420, 1430, 1460, 1490, 1650, 1700, 2110,
  3060) and keeps its own protocol-upgrade and `unlock_ppu` replies. Clients get
  those bytes under their request ids, device errors included. Every other
  request, every write among them, gets the device's unsupported-operation
  status `0x20000501` and a log line; nothing a client sends reaches the
  tracker (unit: `test_replies_come_from_the_session_and_the_port_follows_the_tracker`).
- **One device subscription, fanned out.** Subscribe and unsubscribe of `0x500`
  and `0x504` change only which connections receive the session's messages
  and are answered as the device answers them (word 2 = 1, no payload). Other
  streams are refused. A connection whose socket buffer holds more than 64 KiB
  skips messages instead of delaying the others.
- **Timestamps** as above.
- **Head pose** over SESP through the bridge, positions compensated for the
  module's display transform unless `--sesp-positions windows`.

Without a session the bridge's SESP pipe is refused as the PRP pipes are.

`tools/tet_tcp_emulation.py` runs the same service objects without a tracker
(recorded device replies and gaze, synthetic head pose, optional session drop
with `--interrupt-at`); `evidence/private/sesp/emulation/run.sh <tag> <client>
<args>` starts it with the bridge and a test client under local Wine.

### Emulated (Wine) runs, unpatched Debian Wine 8.0

| Run | Client | Result |
| --- | --- | --- |
| `e1` | `tettcp_test_client.exe discover 10` | device created; streams 0 to 5 and 12 supported; 331 head poses and 275 gaze points (268 valid) in 10 s; head positions as sent, so the compensation is undone exactly by the DLL |
| `e2-windows-positions` | same, `--sesp-positions windows` | head position `(x, 10, 600)` arrives as `(x - 1.88, 46.47, 506.04)`, the module's transform |
| `t1`, `t3` | `tgi_test_client.exe first 15`, `first 35` | `TrackTracker` true, capabilities `0x9b`, `head_stream_supported=1`, 455 head poses in 15 s, 1104 in 35 s (31.5 a second), 740 gaze points; presence 1 |
| `t2-interrupt` | `tgi_test_client.exe first 32`, session dropped for 3 s at 15 s | the layer lost the tracker and had head pose again about 12 s later |

The DLL also asked for 1460 and 3060, which the ET5 refuses as well; the game
layer's own SESP client connected with an empty URL and sent statistics,
which the service acknowledges and drops.

## Prototype

- `src/tobii_linux/tet_tcp.py`: `TetTcpServer` listens on TCP 4455, 4457 and
  4459 and records UDP 4457 datagrams. `Router` relays channel 1 requests to a
  backend with its own request ids, returns replies under the DLL's ids, sends
  stream data only to the connections that subscribed, answers a subscribe or
  unsubscribe itself while another connection holds the same stream, and
  unsubscribes streams a connection leaves behind. Requests outside `READ_COMMANDS` (getters,
  enumerations, subscribe and unsubscribe, `unlock_ppu`, realm open and close,
  persistent file enumerate and read) are answered with device error
  `0x20000501` and logged as `blocked`; they never reach the tracker.
  `UsbBackend` reads the tracker on its own thread without pause, so a slow
  client never stalls the USB session. `RawLog` writes every byte in both
  directions and every decoded header as JSON lines.
- `tools/tet_tcp_probe.py`: `--backend none` (record only), `usb` (first ET5
  libusb sees), `replay` (recorded replies, `--replay LOG`, `--reply CMD:HEX`
  for synthetic ones). `--unlock` opens the unlock realm with the service key
  before clients connect; the ET5 does not need it.
- `tools/prp_bridge/tettcp_test_client.c`: `prp_test_client` for this route:
  takes the first `tet-tcp://` URL from discovery, prints
  `tobii_stream_supported` for streams 0 to 12 and subscribes gaze and head pose
  independently. Build: `x86_64-w64-mingw32-gcc -O2 -Wall -Wextra -std=c11
  -D_WIN32_WINNT=0x0601 -o tettcp_test_client.exe tettcp_test_client.c -static -s`.
- `tests/test_tet_tcp.py`: id remapping, blocked writes, stream routing,
  unsubscribe on disconnect, cached upgrade reply.

Replay run, as used for the tables above:

```sh
PYTHONPATH=src python3 tools/tet_tcp_probe.py --backend replay \
  --replay evidence/private/calibration/probe/host-pattern-fixed-raw.jsonl \
  --replay evidence/private/calibration/probe/pause-threshold-raw.jsonl \
  $(PYTHONPATH=src python3 evidence/private/tet-tcp/replay/synth.py) \
  --reply 1420:<device info payload> --raw-log /tmp/replay.jsonl --seconds 60 &
WINEPREFIX=... wine tettcp_test_client.exe 'Z:\path\tobii_gameintegration_x64.dll' discover 8
```

`evidence/private/tet-tcp/replay/go.sh` has the exact arguments.

## Live test

Prepared, not run (albedo unreachable):

- `evidence/private/sesp/live/` (current): `stage.sh` builds `sesp-run.tgz`;
  copy the directory to `C:\Users\Garulf\tobii-linux-test\sesp\`.
  `sesp-windows.cmd` probes the real Windows software's SESP pipe
  ([sesp.md](sesp.md#live-checks)). `sesp-live.cmd` (elevated) binds and attaches
  8-2, stops the three Tobii services and runs `sesp-live.sh` in WSL as root:
  `cli serve --tet-tcp` on the tracker, then the bridge, `tettcp_test_client.exe`
  and `tgi_test_client.exe` under stock Wine 9.0 (`/home/garulf/.wine-tobii`)
  and GE-Proton 11-9; afterwards it detaches, unbinds, starts the services and
  records `usbipd list` in `sesp\out\usbipd-after.txt`. Nobody needs to sit in
  front of the tracker for the protocol checks; valid head poses need a person.
- `evidence/private/tet-tcp/live/` (earlier): the recording proxy with the USB
  backend; superseded by the above for everything but raw traffic logs.

## Which route

| Situation | Route | Why |
| --- | --- | --- |
| Patched Wine or Proton (pipe listing) | PRP (`cli serve`, default) | Live end to end on albedo; window tracking works; the service's timesync keeps time |
| Stock Wine, Proton or GE-Proton | tet-tcp (`cli serve --tet-tcp`) | No Wine patch; head pose and gaze reach the game layer (emulated (Wine), service). Not yet run live, and no monitor for window tracking |
| Gaze only, no helper in the prefix | tet-tcp without the bridge | Port 4455 alone gives gaze; head pose needs the bridge's SESP pipe |

Both routes need `prp_bridge.exe` in the prefix for head pose and run the same
way through `tobii-linux-wine-run` ([game-launch.md](game-launch.md)). The
service offers one route at a time: with `--tet-tcp` it lists only the SESP
pipe to the bridge, so a patched Wine does not see a second tracker (if both
were offered, the layer tracks the first one it scans unless the game picks
by URL).

## Gaps

- Nothing on this route ran against the real tracker, the real Windows SESP
  service, stock Wine 9.0 or GE-Proton; the emulated (Wine) runs used Debian
  Wine 8.0, recorded ET5 replies and gaze, and synthetic head pose.
- Display info over SESP is not served, so window tracking finds no monitor on
  this route.
- With real device replies the module asked for 1460 and 3060, both refused
  by the ET5 too; commands that only a live run reveals are refused and logged.
- How Star Citizen chooses between several trackers, and what it does with a
  tracker whose rectangle is empty, was not examined.
- Whether real Windows posts `FD_CONNECT` for a UDP connect, and so whether
  timesync ever worked on this route, was not checked.
