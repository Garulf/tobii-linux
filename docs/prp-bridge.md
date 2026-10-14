# Native PRP bridge

Head pose from the Linux driver reaches Star Citizen's Tobii DLL under Wine
through two pieces:

- `src/tobii_linux/prp_service.py`: a Linux asyncio service that speaks the
  service side of the platform runtime protocol (PRP) for head pose and, with a
  gaze hub, gaze point, responsive gaze point and gaze origin
  ([gaze-host.md](gaze-host.md)).
- `tools/prp_bridge/prp_bridge.c`: a small Windows program run in the game's
  Wine prefix. It owns the Tobii named pipes and relays every pipe message to
  the service over loopback TCP.

The pipe protocol it serves is described in
[service-transport.md](service-transport.md). Pipe discovery under Wine is
handled separately by a Wine patch
([wine-pipe-enumeration.md](wine-pipe-enumeration.md)); without it the test
client needs an explicit `tobii-prp://` URL.

Evidence labels:

- **live (Windows)**: observed on albedo against the real Tobii service.
- **live (Wine)**: observed on albedo's WSL2 Ubuntu with Wine 9.0.
- **unit**: `tests/test_prp_service.py`, no pipes or Wine.
- **static**: read from disassembly.
- **guess**: a value chosen without evidence of what the client needs.

## Status

| Item | Result | Evidence |
| --- | --- | --- |
| Stream Engine DLL (`tobii_stream_engine.dll`) creates a device from `tobii-prp://ET5-LINUX`, subscribes head pose and receives synthetic samples | works, 91 samples per 3 s run, two runs per bridge session | live (Wine) |
| Star Citizen's `tobii_gameintegration_x64.dll`, its embedded Stream Engine called at static addresses, same sequence | works, 91 samples per 3 s run, two runs per bridge session | live (Wine) |
| Same two client DLLs against the real service on Windows | work | live (Windows) |
| Service responses equal real-service bytes for stream packages, stream start/stop and timesync responses, and the subscription and timesync handshake information | byte-identical | live (Windows), unit |
| Connect response layout | same extended layout and field order as the real one; values partly copied, some guessed | live (Windows) |
| Discovery of `TOBIIPRP-*` under stock Wine | fails; the client falls back to the bare pipe name | live (Wine) |
| Discovery with the pipe-listing patch ([wine-pipe-enumeration.md](wine-pipe-enumeration.md)), both client DLLs, `PRP_URL=discover` | finds `tobii-prp://ET5-LINUX`, connects and receives about 100 samples per 3 s run | live (Wine master + patch) |
| Real tracker through `cli serve`, patched Wine 11.0, discovery, both client DLLs | 361 samples per 10 s client run, timestamp lag median 11 ms; samples invalid (no face in view) | live (Wine 11.0 + patch) |
| Game integration layer through its C exports ([game-integration-layer.md](game-integration-layer.md)): scan, `TrackTracker`, `GetHeadPoses` | works, 330 head poses per 10 s run (every package), two runs per bridge session; `TrackRectangle` works only with `--display-id` | live (Wine 11.0 + patch) |
| Same client against the real service | tracker found and connected; no head poses because nobody was in front of the tracker | live (Windows) |
| Property replies (display area, display id, model, properties 2 and 4, user presence subscribe, refusals, unsubscribe) | byte-identical to the real service except one uninitialised padding byte in the model string | live (Windows), unit |
| Gaze streams 3, 29 and 4 ([gaze-host.md](gaze-host.md)): packages, stream start/stop and the refusals of streams 5, 6 and 33 | byte-identical to the real service | live (Windows), unit |
| Gaze points through both client DLLs and `GetGazePoints` of the game integration layer, synthetic service | every package delivered; the layer returns the same `(2x - 1, 1 - 2y)` as on Windows | live (Wine 11.0 + patch) |

## Architecture

```
 Wine prefix (one wineserver)                       Linux
 +--------------------------------+                +---------------------------+
 | game: tobii_gameintegration.dll|                | prp_service.py            |
 |   writes \\.\pipe\<name>  ---->| prp_bridge.exe |   per relay connection:   |
 |   reads  \\.\pipe\client_* <---|   one TCP      |   handshake, connect,     |
 |   (three channels)             |   connection   |   stream start/stop,      |
 +--------------------------------+   per pipe  -->|   timesync, packages      |
                                      connection   |   HeadPoseHub <- samples  |
                                     127.0.0.1:7760+---------------------------+
```

A native Linux process cannot open a wineserver named pipe
([service-transport.md](service-transport.md), Wine considerations), so the
bridge runs inside the prefix. Wine creates host sockets for Winsock, so the
bridge reaches a native listener on `127.0.0.1`.

The bridge does no protocol work beyond opening the reply pipe: the service
parses the handshake, chooses the subscription and timesync pipe names, and
tells the bridge which reply pipe to open.

## Relay protocol

Every frame on every TCP connection is `u32 type, u32 length, payload`,
little-endian, payload at most 1 MiB.

| Type | Name | Direction | Payload |
| --- | --- | --- | --- |
| 1 | `BRIDGE_HELLO` | bridge to service, first frame of the control connection | u32 relay version (1) |
| 2 | `PIPES` | service to bridge, control connection | NUL-terminated pipe names to serve |
| 3 | `PIPE_OPEN` | bridge to service, first frame of a pipe connection | u32 relay version, u32 client process id (0 if unknown), pipe name, NUL |
| 4 | `REPLY_PIPE` | service to bridge | reply pipe name, NUL; the bridge opens it before handling later frames |
| 5 | `MESSAGE` | both | one pipe message |
| 6 | `DISPLAY_ID` | bridge to service, on the control connection after `PIPES` | UTF-8 device instance id of the primary monitor inside the prefix, no NUL; skipped on Wine 9 and older |

Connections:

1. **Control**: the bridge connects, sends `BRIDGE_HELLO`, receives `PIPES`
   and keeps the connection open. When the service closes it, the bridge exits
   and its pipes disappear.
2. **Pipe**: for each accepted client pipe instance the bridge opens a new TCP
   connection, sends `PIPE_OPEN` with the pipe name and
   `GetNamedPipeClientProcessId`
   (https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid),
   then forwards each whole pipe message read from the client as `MESSAGE`.
   Frames from the service are `REPLY_PIPE` (once, before the handshake ack)
   and `MESSAGE`, each written to the reply pipe as one message. Closing either
   side ends the session: the bridge disconnects and closes both pipes, the
   service stops any stream of that client.

Pipe handling in the bridge follows the DLL's own service code
([service-transport.md](service-transport.md)): service pipes are created with
`PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED`, `PIPE_TYPE_MESSAGE |
PIPE_READMODE_MESSAGE`, unlimited instances, input buffer 0x10000; reply pipes
are opened with `GENERIC_WRITE` and switched to message-read mode
(https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea,
https://learn.microsoft.com/en-us/windows/win32/api/namedpipeapi/nf-namedpipeapi-setnamedpipehandlestate).
Reads use overlapped I/O and collect `ERROR_MORE_DATA` parts into one message.

### Pipe names

The service announces four pipes for `--name <name>` (default `ET5-LINUX`):

| Pipe | Purpose |
| --- | --- |
| `TOBIIPRP-<name>` | command channel, the name a working pipe listing finds |
| `<name>` | command channel, the name the client opens when the listing does not show the prefixed pipe (0x18004e8a0, static; observed under Wine) |
| random UUID | subscription channel, URL `prp-internal://<uuid>` |
| random UUID | timesync channel, URL `prp-internal://<uuid>` |

The real service uses the same scheme: random UUID pipes advertised as
`prp-internal://` URLs (live, Windows). For `prp-internal://` the client opens
the name unchanged, so these two pipes need no discovery. Do not choose a name
starting with `TOBII-`: the legacy discovery routine would also report the bare
pipe as a `tobii-ttp://` device.

## Service behaviour

| Step | Service answer | Basis |
| --- | --- | --- |
| hello `{1, channel, reply pipe}` | `REPLY_PIPE`, then ack `u32 1` | real service, DLL service code |
| client pid | compared with the bridge's `GetNamedPipeClientProcessId` when nonzero; mismatch closes | DLL service code |
| info, command channel | new id, subscription URL, timesync URL | real service sent id 9 and two `prp-internal://` URLs |
| info, subscription channel | new subscription id, two empty strings | real service sent `08 00 00 00 00 00` |
| info, timesync channel | id 0, two empty strings | real service sent `00 00 00 00 00 00` |
| connect | status 0, extended layout, see below | |
| connect with unknown subscription id | status 8 (`INVALID_PARAMETER`) | guess |
| stream start 2 | status 0, value 2, starts pushing | real service: status 0, value echoes the stream id |
| stream stop 2 | status 0, value 2, stops pushing | real service |
| stream stop without start | status 5 (`NOT_SUBSCRIBED`) | guess |
| stream start/stop for another stream | status 2 (`NOT_SUPPORTED`) | guess |
| timesync (on any channel) | status 0, service clock in microseconds | real service |
| property get (operation 5), property subscribe and unsubscribe (commands 5 and 6) | see [game-integration-layer.md](game-integration-layer.md#service-behaviour) | real service |
| any other command | status 2 with a 16-byte zero body block | guess; the body size satisfies the client decoder's fixed-size branches (0x180055780, static) |

Stream packages: operation 1, message tag 0, stream 2, one 52-byte head body
(gaze streams: [gaze-host.md](gaze-host.md)),
one TPRP packet per pipe message, as the real service sends them.
`HeadPoseSample.valid` drives all four validity flags. Packages for a client
are dropped while more than 64 KiB are unsent on its TCP connection, so a
stalled game never blocks the producer.

Time base: timesync replies and package timestamps both use one service
clock in microseconds, which must run at the rate of the game's
`QueryPerformanceCounter`: the DLL's client clock is QPC scaled to
microseconds (0x180027650 calls `QueryPerformanceCounter` and multiplies by
1,000,000 over the frequency, static), and it converts package timestamps with
the offset measured by timesync only. Wine's `monotonic_counter` in
`dlls/ntdll/unix/sync.c` reads `CLOCK_MONOTONIC_RAW` up to Wine 10.16 and
`CLOCK_BOOTTIME` from 10.17 on (commit
https://github.com/wine-mirror/wine/commit/3d373e452388b60bea9dca6d5c4cf3f8b20a0c54,
"ntdll: Switch to CLOCK_BOOTTIME for monotonic counters when available";
checked in the 10.0, 10.16, 10.17, 10.20, 11.0 and 11.19 sources).
`prp_service.wine_clock("boottime" | "raw")` and the `--wine-clock` option
select the matching clock; the default is `boottime`. On most Linux hosts the
two run at the same rate, but on the reference host's WSL2 kernel they differ
by 9.1% (below), and the wrong choice makes timestamps run ahead of the game's
clock by that fraction of the elapsed time.

### Real tracker

`python3 -m tobii_linux.cli serve --display ...` runs the USB session, the
head-pose pipeline and this service in one process. Frames are read on their
own thread and only the newest is processed, so a slow pipeline drops frames
instead of letting the device stream back up. Device frame timestamps are
mapped onto the service clock with the smallest receipt delay over the last
64 frames, and never later than the current service time
(`driver.ClockMapper`).

The same session also subscribes the gaze stream (`0x500`) and publishes every
gaze message, never dropped, to the gaze hub as
`gaze.gaze_frame(gaze.gaze_message(message))` restamped onto the service clock
with its own `ClockMapper` (`streams.GazeAndImageStreams`), so head-pose timing
is unchanged. `--no-gaze` serves head pose only, as before.

Live on albedo (tracker attached to WSL2 with usbipd, `cli serve` as root,
bridge and test client under Wine 11.0 with the pipe-listing patch,
`PRP_URL=discover`, `PRP_EXTERNAL_SERVICE=1`, 10 s per client):

| Client DLL | Discovery | Samples | Timestamp to callback |
| --- | --- | --- | --- |
| `tobii_gameintegration_x64.dll` | `tobii-prp://ET5-LINUX` | 361 | 8.6 to 18.6 ms, median 11.3 ms |
| `tobii_stream_engine.dll` | `tobii-prp://ET5-LINUX` | 361 | 8.6 to 17.4 ms, median 11.4 ms |

All samples were invalid because no face was in view. The pipeline took
3.8 ms per frame on the reference host's CPU in that run (replay of 150
recorded frames, 95th percentile 4.3 ms); with the landmark smoothing added
since, 9.6 ms (95th percentile 10.6 ms). Logs: `evidence/private/e2e/`.

### Connect response fields

`device_connect_response` (extended layout, block 0xA8):

| Field | Value | Basis |
| --- | --- | --- |
| build version | `1.37.4.0_tobii_linux` | real `1.37.4.0_f2b4e724c9`; only logged by the client |
| firmware upgrade in progress | 0 | checked by device_create |
| `list_a`, `list_b`, `list_d`, `groups`, `mode` (2), the nine words | copied from the real reply | meaning unknown |
| `list_c` | first 25 real entries | real sends 28; the client keeps 25 |
| `streams` | `(2)` | real `(6, 3, 29, 4, 2, 7, 28, 1, 32)`; only head pose is served |
| `supported_commands` | `(1, 3, 4, 0x16)` | real sends 27 values (two above 0x20, which the client drops); listing 0x16 makes device_create run timesync rounds, observed under Wine |
| strings | serial = `--name`, `IS5_Large_Eyetracker_5`, `IS5`, `tobii-linux`, `Peripheral`, `IS5LEYETRACKER5`, empty | real: serial, model, generation, firmware `02a1a6a977`, then the same three; strings 0 to 3 appear in `tobii_get_device_info` (live) |
| license results | one 0 per license sent | checked by device_create |
| `list_g`, extended strings | empty | real reply has them empty |

The nine words decode as `2` then floats `184.0, 20.0, 0, -0.16, 13.85, 0,
5.38, 9.86`; their meaning was not traced. Whether any game query depends on
the lists, words or the reduced `streams` and `supported_commands` is not
established.

## Running

Build (Linux, mingw-w64):

```sh
make -C tools/prp_bridge
```

This produces `prp_bridge.exe`, `prp_test_client.exe` and `tgi_test_client.exe`,
all static and dependency-free. `tgi_test_client.exe <game dll> [first|window|rectangle|displays|<url>] [seconds]`
drives the game integration layer through its C exports and prints the trackers
it found and every head pose.

Service on Linux, synthetic samples:

```sh
PYTHONPATH=src python3 -m tobii_linux.prp_service --synthetic [--name ET5-LINUX] [--port 7760] [--debug] \
    [--display-area tlx,tly,tlz,trx,try,trz,blx,bly,blz] [--display-id 'DISPLAY\...']
```

`--display-area` (mm) and `--display-id` answer the game integration layer's
display queries; without them those two properties are refused, which is enough
for `TrackTracker` but not for `TrackWindow` or `TrackRectangle`
([game-integration-layer.md](game-integration-layer.md)).

Without `--synthetic` the service publishes nothing. A driver feeds it through
the API:

```python
hub = HeadPoseHub()
server = await PrpService(hub, "ET5-LINUX").serve("127.0.0.1", 7760)
await hub.feed_iterator(samples)  # blocking iterator of HeadPoseSample
```

Bridge in the game's prefix, started before the game:

```sh
WINEPREFIX=... wine tools/prp_bridge/prp_bridge.exe 127.0.0.1 7760
```

Test client in the same prefix:

```sh
WINEPREFIX=... wine prp_test_client.exe <dll> tobii-prp://ET5-LINUX [seconds] [field_of_use]
```

`<dll>` is either `tobii_stream_engine.dll` or the game's
`tobii_gameintegration_x64.dll` 9.0.3.25. The game DLL does not export its
embedded Stream Engine, so the client calls these addresses after checking the
PE timestamp `0x661d6182` and image size `0x664000` (static):

| Function | RVA | How found |
| --- | --- | --- |
| `tobii_error_message` | `0xc9b0` | error-code string switch |
| `tobii_api_create` | `0xcb80` | same argument checks as the exported function in `tobii_stream_engine.dll` |
| `tobii_api_destroy`, `tobii_system_clock`, `tobii_device_create`, `tobii_device_destroy`, `tobii_device_process_callbacks`, `tobii_update_timesync`, `tobii_head_pose_subscribe`, `tobii_head_pose_unsubscribe` | `0xcbc0`, `0xcc70`, `0xd1c0`, `0xd740`, `0xda50`, `0xdef0`, `0xfd80`, `0xfe20` | functions that reference their own name string for logging |
| `tobii_get_device_info` | `0xe310` | [service-transport.md](service-transport.md) |

`tobii_device_create(api, url, field_of_use, &device)` matches the game's own
call at `0x1800089ae`. All of these returned success in the live runs.

`tools/prp_bridge/run_wine_test.sh <dll> [seconds] [outdir]` starts the
service, the bridge and the client and prints the three logs. `PRP_RUNS=n`
repeats the client, `PRP_DEBUG=1` logs every relayed message,
`PRP_CLIENT=tgi` runs `tgi_test_client.exe` (mode from `PRP_TGI_MODE`) and
`PRP_SERVICE_ARGS` adds service options such as `--display-area`.

## Real-service comparison

`tools/prp_real_service_probe.py` runs on Windows with `src/` beside it. It
lists `\\.\pipe\TOBIIPRP-*`, performs the client handshake on the command,
subscription and timesync channels with the `prp_session` codecs, sends
connect (no licenses, empty signer key, client version 4.17.0.6, image name
components from `GetModuleFileNameW`), four timesync rounds, stream start 2,
records every message for a few seconds, four more timesync rounds, stream
stop, and closes. It sends no other commands and stops no services. Output:
`evidence/private/prp-bridge/real-service-probe-1.jsonl` (field_of_use 2) and
`real-service-probe-fou1.jsonl` (field_of_use 1).

```
python.exe tools\prp_real_service_probe.py --seconds 2 [--field-of-use 1]
```

Findings (live, Windows, service build `1.37.4.0_f2b4e724c9`, tracker
`IS5FF-100204141463`, nobody in front of the tracker):

- Pipes listed: `TOBIIPRP-IS5FF-100204141463`. The real service also keeps
  `TOBII-IS5FF-...` (TTP) and per-client UUID pipes.
- Handshake: ack `01000000` on every channel. Command info: id 9,
  `prp-internal://97b8c00b-...`, `prp-internal://00211f9c-...`. Subscription
  info: id 8 with two empty strings. Timesync info: id 0 with two empty
  strings. Sequential reply-pipe counters (1, 2, 3) were accepted.
- Connect: status 0 for field_of_use 1 and 2, extended layout (block 0xA8),
  712 bytes. Decoding and re-encoding with `prp_session` differs only because
  `list_c` (28 entries) and `supported_commands` (27 entries) exceed what the
  client stores; the field order and variable-region order are the same.
- Stream start and stop: status 0, value 2. Byte-identical to the service's
  output for the same tags.
- Timesync: status 0, microseconds. Over 2 s the offset between service time
  and the probe's QPC microseconds stayed within 828165988901 to
  828165989034 us, so the unit is microseconds at rate 1, on an epoch about
  828166 s ahead of QPC.
- Stream packages: message tag 0, one 100-byte TPRP packet per pipe message,
  timestamps in the service clock, period 30.206 to 30.209 ms (33.1 Hz),
  received 4.3 to 5.7 ms after their timestamp. All samples invalid (no user),
  validity flags 0 and values 0. 68 packages in 2 s, none after stop.
  Re-encoding a package with `stream_package` gives identical bytes.
- Timestamp conversion: the Stream Engine and game DLL clients report samples
  in their own QPC microseconds (`timestamp_us` about 10 ms behind QPC at the
  callback, median, 5.1 ms at best, with a 5 ms polling loop), although the
  service clock is about 828166 s ahead of QPC. The client therefore maps
  service time through timesync. Any service clock works as long as timesync
  and packages share it.
- `tobii_get_device_info` returns the first four connect strings.
- The game DLL client once failed twice in a row with
  `PRP_ERROR_ENUM_CONNECTION_FAILED` at `prp_client.cpp(416)` against the real
  service (field_of_use 1), right after other test connections. Five later runs
  with field_of_use 1 and 2 succeeded. The cause is unknown
  (`game-dll-client-real-service.txt`).

Evidence files in `evidence/private/prp-bridge/`:
`stream-engine-client-real-service.txt`, `game-dll-client-real-service-2.txt`.

## Wine test

Environment (live): albedo WSL2 Ubuntu, kernel
`6.18.40.1-microsoft-standard-WSL2`, `wine-9.0 (Ubuntu 9.0~repack-4build3)`,
Python 3.12.3, `WINEPREFIX=/home/garulf/.wine-tobii`, `WINEDEBUG=-all`.
DLL hashes: `tobii_stream_engine.dll`
`f60c90c41a6c29718f0255ab22db160b038f9305fe92d184c9cf3e8e938ec5a3`,
`tobii_gameintegration_x64.dll`
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`.

Command (one WSL session, both DLLs, two client runs each;
`evidence/private/prp-bridge/wine_final.sh` runs it for both DLLs and was
started with `ssh-run.sh albedo 'wsl -d Ubuntu -e bash .../wine_final.sh'`):

```sh
PRP_RUNS=2 bash tools/prp_bridge/run_wine_test.sh se/tobii_gameintegration_x64.dll 3 /tmp/prp-final
```

Excerpt of the game DLL part of the last run (`wine-test-final-2.txt`, made
after the last code change):

```
== service.log
PRP service on ('127.0.0.1', 7760), pipes ['TOBIIPRP-ET5-LINUX', 'ET5-LINUX', '68b47e60-...', '6041c3b1-...']
pipe ET5-LINUX channel 0x4e21 handshake done, pid 244
pipe 68b47e60-... channel 0x4e22 handshake done, pid 244
connect from pid 244 with 0 license(s), subscription 2
pipe 6041c3b1-... channel 0x4e23 handshake done, pid 244
head pose stream started for subscription 2
head pose stream stopped for subscription 2
== bridge.log
ET5-LINUX: client pid 244 connected
ET5-LINUX: opened reply pipe client_TOBIIPRP-ET5-LINUX_000000f4_00006ffffad70029
...
== client.txt
dll Z:\...\tobii_gameintegration_x64.dll entry_points=game_dll_static_rva
log level=2 Connected to platform module with build version "1.37.4.0_tobii_linux"
result device_create code=0 message=No error.
device_info[0] ET5-LINUX
result update_timesync code=0 message=No error.
result head_pose_subscribe code=0 message=No error.
head timestamp_us=1214386299 system_clock_us=1214391896 qpc_us=1214391896 position_valid=1 position=9.144,10.000,600.000 rotation_valid=1,1,1 rotation=-0.1220,-0.1408,0.0000
...
summary head_samples=91 process_calls=533 seconds=3.0
```

Observations (live, Wine):

- Both client DLLs connected through the bridge, ran the handshake on all
  three channels, accepted the connect response, ran timesync (the game DLL
  sent four rounds during device_create plus one from `tobii_update_timesync`,
  as predicted from `supported_commands` containing 0x16), started stream 2,
  delivered the synthetic samples to the callback, stopped the stream and
  closed. 91 samples in each of four 3 s runs; a second client in the same
  bridge session worked each time. The same held in two earlier sessions
  (`wine-test-final.txt`, `wine-test-stream-engine-1.txt`).
- The stream engine and game DLL clients sent no command other than connect,
  timesync, stream start and stream stop (`PRP_DEBUG=1` log in
  `wine-test-game-dll-1.txt`).
- The client opened `ET5-LINUX`, not `TOBIIPRP-ET5-LINUX`: its
  `\\.\pipe\*` listing did not find the prefixed pipe under stock Wine 9.0,
  consistent with [service-transport.md](service-transport.md).
- `GetNamedPipeClientProcessId` works under Wine; the pid matched the one the
  client sent in the handshake.
- Callback `timestamp_us` trailed the client's QPC by 3.4 ms median, 6.5 ms
  at most, including the test client's 5 ms polling interval.
- Sample timestamps advance 33.0 ms per sample although the publisher waits
  30.207 ms on `CLOCK_MONOTONIC`. On this WSL2 kernel (clocksource `tsc`)
  `CLOCK_MONOTONIC_RAW` runs 9.1% faster than `CLOCK_MONOTONIC`
  (3.2732 s against 3.0001 s, measured twice with `clocks.py`, `wsl-clock-rates.txt`).
  Wine 9.0's QPC reads the raw clock, so service and client agreed in these
  runs. With Wine 11.0 and master (`CLOCK_BOOTTIME`) and the raw service
  clock, timestamps ran ahead of the client's QPC by 9% of the elapsed time
  (0.9 s after 10 s); with `--wine-clock boottime` the offset stays flat.

Full logs: `wine-test-stream-engine-1.txt`, `wine-test-game-dll-1.txt` (with
every relayed message, `PRP_DEBUG=1`), `wine-test-final.txt`,
`wine-test-final-2.txt`.

## Tests

`tests/test_prp_service.py` (unit) drives the service over real TCP with a
simulated bridge and the client-side `prp_session` codecs: pipe list, the
handshake on all three channels, connect checks (extended layout, license
count with 0 and 2 licenses, firmware flag, stream 2, timesync listed), unknown
subscription, stream start/package/stop, release of the stream on
disconnect, other streams, timesync, an unknown command, pid mismatch and a
version-0 hello. Real-service vectors check byte equality of packages,
stream start/stop and timesync responses and the handshake information.

## What remains

- **Discovery**: the stock DLL finds `tobii-prp://` devices only through a
  `\\.\pipe\*` listing, which fails under stock Wine (live). The pipe-listing
  patch fixes this (live, both client DLLs); the game's Wine runner has to
  carry it.
- **Game integration layer**: delivers head pose under Wine with the pipe-listing
  patch ([game-integration-layer.md](game-integration-layer.md)). Open: whether
  Star Citizen tracks its window (then the service needs `--display-id` equal to
  the monitor instance id in the game's prefix), `TrackWindow` itself (no Wine
  build with a display driver was available), property notifications, and
  passing the display options through `tobii_linux.cli serve`.
- **Licensed connect**: the game may use `tobii_device_create_ex` with license
  keys (0x180008820). The service returns one result 0 per license, which
  satisfies device_create statically, but this was not run.
- **Valid live samples through Wine**: `tobii_linux.cli serve` feeds tracker
  samples into the service (section "Real tracker"), but nobody was in front
  of the tracker during that run, so only invalid samples reached the game
  DLL. Valid output of the same pipeline was checked offline on recorded
  frames.
- **Startup**: the bridge has to run in the game's prefix and wineserver before
  the game connects, for example from a Lutris or Proton launch script. Proton
  was not tested.
- **Unknown fields**: meanings of `list_a` to `list_d`, `groups`, `mode`, the
  nine words and the extended strings, and how the real service answers
  unsupported streams, repeated starts or unknown commands.
- The transient `CONNECTION_FAILED` of the game DLL against the real service.
