# The tet-tcp route

Star Citizen's `tobii_gameintegration_x64.dll` 9.0.3.25 (SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`) also finds a
tracker by connecting to TCP port 4455 on `127.0.0.1`. TCP works in unmodified
Wine and Proton, so a native Linux process on that port would need neither
`prp_bridge.exe` nor the pipe-listing Wine patch. This document records what the
DLL does with such a tracker and whether that route can carry head pose and gaze
to the game.

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
| Head pose on this route | Only from Stream Engine Services over the named pipe `tobii-sesp://streamengineservices`. Without that pipe the module reports head pose unsupported | static, emulated |
| Gaze on this route | Works: 248 of 255 Stream Engine gaze points valid in 8 s; the integration layer returned 262 gaze points in 10 s | emulated |
| Integration layer | Discovers `tet-tcp://127.0.0.1`, `TrackTracker` succeeds, `IsConnected` 1, head stream unsupported, `GetHeadPoses` 0, `GetGazePoints` works | emulated |
| Timesync under Wine | Fails on every Wine version checked: Wine never signals `FD_CONNECT` for a UDP connect. Gaze timestamps stay in device microseconds | emulated, Wine source |
| Discovery | Default mask probes only `127.0.0.1:4455`; a 50 ms connect window; the integration layer rescans every 10 s | static, live (Wine) earlier |

**Recommendation.** The tet-tcp route is feasible for gaze and not feasible for
head pose. A native process on 127.0.0.1:4455 can give Star Citizen gaze with no
bridge and no Wine patch. Head tracking, the main use, still needs a component
inside the Wine prefix, because the only head-pose source of this route is a
named pipe. Keep `prp_bridge.exe` and the Wine patch for head pose. The one
alternative that avoids the Wine patch is a small Wine-side server for the fixed
pipe `streamengineservices` combined with the native tet-tcp server; it needs the
SESP protocol reconstructed and still runs a helper in the prefix (see
[Options](#options)).

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

## Head pose (static, emulated)

The legacy module has no head-pose computation. The chain is:

1. `platmod_stream_head_pose_subscribe` (`0x180039e50`) returns error 3 unless
   the SESP connection flag (`+0xa850`) is set, then calls `0x18005b2d0`,
   `0x18005cc30` and `0x180087fd0`, a head-pose start request to SESP.
2. Samples arrive only through `0x180049aa0`, the callback `platmod_start`
   passes to the SESP connect (`0x18003eeb3`, its only reference). It fills the
   ring at module `+0x26dc8` that `0x18003ff10` hands to the client's head-pose
   callback.
3. The SESP connection (`0x18005c9a0`) opens `tobii-sesp://streamengineservices`
   on channels `0x2711` and `0x2712`. In the transport that scheme is a named
   pipe opened by its fixed name, `streamengineservices`; no listing is
   involved. Its handshake is shorter than the PRP one: hello `{u32 channel,
   reply pipe name}` without the version word, then the `u32 1` acknowledge,
   and no pid or information message (`0x18004e8a0`).

So under Wine a head-pose source has to own that pipe inside the wineserver,
which a native Linux process cannot do ([service-transport.md](service-transport.md)).
The DLL's strings contain no landmark, LBF, ONNX or model names; the `face`
hits are Stream Engine API names. In the emulated runs `tobii_stream_supported`
reported head pose (stream 4) unsupported and `tobii_head_pose_subscribe`
returned `TOBII_ERROR_NOT_SUPPORTED`; the integration layer reported
`head_stream_supported=0` and tracker capabilities `0x99` (`0x9b` with PRP).
A synthetic 1350 reply with capability 3 set to `true` changed nothing
(`evidence/private/tet-tcp/replay/r7-caps3-client.txt`), as expected from the
SESP condition in `platmod_start`.

## Gaze (emulated)

Stream Engine entry points (`tettcp_test_client.exe`, `discover`, 8 s): device
created, gaze point subscribe sent TTP subscribe `0x500`, 255 gaze callbacks,
248 valid. Streams reported supported: 0, 1, 2, 3, 5 and 12.

Integration layer (`tgi_test_client.exe first 10`): one tracker
`tet-tcp://127.0.0.1` (model, generation, serial and firmware from 1420),
`TrackTracker` true, `IsConnected` 1, `IsPresent` 1, 262 gaze points in 10 s,
no monitor rectangle.

The replay feeds about 33 messages a second from a recording, so the counts say
the path works, not what rate or accuracy the game would see.

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
`39345...`. The integration layer kept those points. A native server can make
the timestamps right by rewriting column 1 into Wine's QPC microseconds, as the
PRP service already does for head pose (`--wine-clock`) (inference).

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

Prepared, not run: `evidence/private/tet-tcp/live/tettcp-live.cmd` (albedo,
elevated: bind and attach 8-2, stop and restart the three Tobii services) and
`tettcp-live.sh` (WSL as root: probe with the USB backend, then both test
clients under stock Wine 9.0 with prefix `/home/garulf/.wine-tobii`, and under
GE-Proton 11-9 if `/home/garulf/launch-work/GE-Proton11-9-x86_64` exists).
Copy to `C:\Users\Garulf\tobii-linux-test\tettcp\` with `tettcp-src.tgz`
(`src` and `tools/tet_tcp_probe.py`), both client executables and the DLL; the
output lands in `tettcp\out\`. It answers what the emulation could not: the
real replies to 1420, 1330, 1350, 1200 and 1430, the commands the module sends
after them, and real gaze rate and validity.

## Options

1. **Gaze without bridge or patch.** A native server on 127.0.0.1:4455 inside
   the Linux service, which already owns the tracker session. Needs: answers
   to the start sequence above from the service's own session (cached device
   replies, not forwarding, since several DLL connections come and go),
   fan-out of `0x500` and `0x504` to subscribed connections, column 1
   rewritten to Wine's QPC clock, refusal of all writes, and handling of one
   discovery probe plus one device start every 10 s.
2. **Head pose without the Wine patch.** Option 1 plus a Wine-side server for
   the pipe `streamengineservices`. It needs the SESP protocol (client code
   `sesp_services.cpp`, `0x18004ca20` to `0x18004dfc0`, and its codec around
   `0x180087000`), including the head-pose query `0x18005b2b0` makes, and the
   server running before the game creates the device. It still puts a helper
   process in every prefix, which is what `prp_bridge.exe` already does.
3. **Head pose as today.** `prp_bridge.exe` with the pipe-listing patch
   ([prp-bridge.md](prp-bridge.md), [wine-pipe-enumeration.md](wine-pipe-enumeration.md)).
   If both routes are offered, the game sees two trackers; the integration
   layer tracks the first scanned one unless the game picks by URL.

## Gaps

- Nothing here ran against the real tracker or under GE-Proton.
- Replies for 1420, 1200 and 1430 were synthetic, and 1330, 1350, 1460, 1700,
  3060, 2110 and 1650 were answered with errors. The real device may make the
  module take other paths for gaze; head pose stays tied to SESP whatever the
  device answers (static).
- How Star Citizen chooses between several trackers, and what it does with
  gaze-only trackers, was not examined.
- Whether real Windows posts `FD_CONNECT` for a UDP connect, and so whether
  timesync ever worked on this route, was not checked.
- The SESP protocol is not reconstructed.
