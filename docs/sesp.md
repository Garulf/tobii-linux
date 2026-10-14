# Stream Engine Services protocol (SESP)

The game DLL's legacy TTP module, which every `tet-tcp://` device gets
([tet-tcp.md](tet-tcp.md)), computes no head pose. It asks Stream Engine
Services for it over the named pipe `streamengineservices`. This document
reconstructs that protocol from both ends: the client in Star Citizen's
`tobii_gameintegration_x64.dll` 9.0.3.25 (SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`) and the
service in `ses_windows.dll` from albedo's `C:\Program Files\Tobii\Tobii EyeX\`
(SHA-256 `2f7ee110e784aaad75df09cb6a71f35fd5875c445a72ec3004e83e5712f6ff6e`).

Evidence labels:

- **static**: focused Ghidra `-noanalysis` decompiles
  (`evidence/private/sesp-{a..f}-focused.c` for the game DLL, `ses-{a,b}-focused.c`
  for `ses_windows.dll`, project copy `evidence/private/ghidra-copy-sesp`, where
  `ses_windows.dll` was imported without analysis) and objdump.
- **emulated**: original code run in Unicorn by `tools/sesp_oracle.py` (service
  builders, client builders, client parser and the module's position transform),
  60 of 60 cases agree (`evidence/private/sesp-oracle.json`).
- **emulated (Wine)**: the unmodified game DLL under this container's unpatched
  Debian Wine 8.0 against `tools/tet_tcp_emulation.py`
  (`evidence/private/sesp/emulation/`).
- **live (Windows)**, **live (Wine)**: not run; albedo was unreachable. The
  probes are staged (see [Live checks](#live-checks)).
- **inference**: a conclusion drawn from the above.

## Summary

| Question | Answer | Evidence |
| --- | --- | --- |
| Pipe | `\\.\pipe\streamengineservices`, opened by name; no listing | static, emulated (Wine) |
| Framing | Not PRP. Per message: `"sesp"`, u32 size, u32 size XOR `0x70736573`, then a FlatBuffers buffer built with flatcc | static, emulated |
| Handshake | Hello `{u32 channel, reply pipe name, NUL}`; the service opens the reply pipe and writes one u32, its slot index for the connection | static, emulated (Wine) |
| Head pose | Initialize with capability 1, head-pose start, then head-pose messages on the subscription channel | static, emulated, emulated (Wine) |
| Time base | None in the protocol; the module hands the timestamp to the game unchanged, so it must already be the game's QueryPerformanceCounter microseconds | static, emulated (Wine) |
| Position | The module turns every position `p` into `A p + t` (display tilt about x, minus the display centre) | static, emulated |
| Game layer | With SESP the tet-tcp tracker reports head pose (capabilities `0x9b`) and `GetHeadPoses` returns about 31 poses a second | emulated (Wine) |
| Windows owner | `ses_windows.dll` is the service side; which process loads it on albedo, and whether the pipe exists there, is not observed | static |

## Where the code is

| Side | Code | Addresses |
| --- | --- | --- |
| Client service object (`sesp_services.cpp`) | connect, reconnect, head pose, display info, stream status, statistics | `0x18005afd0` create, `0x18005c9a0` connect, `0x18005b800` reconnect, `0x18005b2d0` head-pose start, `0x18005b5b0` display info, `0x18005b910` stream status, `0x18005bc50` statistics |
| Client codec | flatcc builder and verifier, message builders, parser | `0x180087530` context, `0x180087110` root table, `0x180087020` frame and send, `0x1800882d0` initialize, `0x180087fd0` head-pose start, `0x180088150` stop, `0x18008e8a0` parse, `0x18008a4e0` root verifier, `0x18008b3a0` dispatch |
| Client handlers | command channel `0x18005c130`, subscription channel `0x18005c790`, response wait `0x18005ccc0` | |
| Game layer's own client (`sesp_mwc.cpp`) | `MiddlewareService` of `TobiiDeviceManager`, `0x18004ca20`, `0x18004dfc0` | display info, statistics |
| Service (`ses_windows.dll`) | builders, command handler, head-pose receiver | `0x1800b71d0` context, `0x1800b91b0` initialize response, `0x1800b8e50` / `0x1800b9000` head-pose start / stop responses, `0x1800bb490` head-pose message, `0x1800b9d50` stream status response, `0x1800ba1b0` statistics response, `0x1800baa20` invalid operation, `0x1803ac3c0` command handler, `0x18039fd60` head-pose receiver |

## Transport (static, emulated (Wine))

`0x18004e8a0` handles `tobii-sesp://` (and `tobii-mwsp://`) like PRP pipes, with
the server pipe name taken unchanged from the URL. Each channel:

1. The client creates its reply pipe `client_streamengineservices_<pid %08x>_<counter %016llx>`
   (inbound, message type, one instance) and opens `\\.\pipe\streamengineservices`
   for writing in message-read mode, as for PRP
   ([service-transport.md](service-transport.md#transport-per-channel)).
2. It writes `u32 channel, reply pipe name, NUL`: 0x2711 for the command
   channel, 0x2712 for the subscription channel. There is no version word.
3. The service opens the reply pipe and writes one u32. The client checks only
   its size. The service's own transport (`0x18008f470` with its scheme flag 0)
   writes its slot index for the connection, accepts channels 0x2711 to 0x2714,
   and sends no pid or URL message.
4. For the subscription channel the client's id callback (`0x1800217e0`) keeps
   that u32 as the subscription id it later sends in the initialize request.

Each channel gets 1 s for these steps. Client messages go out in pipe writes
of at most 4096 bytes; the client reassembles service messages from any
number of reads, so message boundaries do not matter after the hello.

## Messages (static, emulated)

Every message is `"sesp"` (`0x70736573`), u32 size, u32 size XOR `0x70736573`,
then a FlatBuffers buffer of that size. The client verifies it with flatcc's
verifier before dispatching on the type. The root table has three fields:
0 sequence (u32), 1 type (u8), 2 payload table. Responses carry the request's
sequence; the client rejects a response with another sequence. Head-pose
messages count from 1 per subscription connection.

flatcc lays a buffer out as the root offset, the tables in reverse creation
order, then the vtables in creation order, sharing identical ones; fields
equal to their default are left out. `sesp.py` builds buffers this way, and
every service message it encodes is byte for byte what the original builder
produces for the same input (emulated, 30 cases).

| Type | Message | Payload fields (default 0 unless noted) |
| --- | --- | --- |
| 1 | invalid operation (service) | 0 error |
| 2 | initialize request | 0 device URL (string), 1 subscription id |
| 3 | initialize response | 0 error, 1 to 15 capability flags (u8) |
| 4, 6 | head-pose start, stop | none |
| 5, 7 | their responses | 0 error |
| 8 | head pose (service, subscription channel) | 0 timestamp high word, 1 low word, 2 to 4 position (float, mm), 5 to 7 rotation (float, rad), 8 position validity (default 1), 9 to 11 rotation validities (default 1) |
| 9, 10 | display info request, response | response: 0 error, 1 monitor device id substring, 2 and 3 u32 |
| 0x1a, 0x1b | notify stream status, response | request: stream list; response: empty table |
| 0x32, 0x33 | send statistics, response | response: 0 error |

Other types exist (device lists, profiles, device-list changes, firmware-upgrade
permission, user presence, gaze point, properties); the game DLL sends none of
them on this route. Errors on the wire are 0 to 7; the service maps every
other internal error to 1 (`0x1800c2d10`). 6 is "not supported", 7 "not found".

Head-pose fields are `tobii_head_pose_t` as the service receives it from Stream
Engine (`0x18039fd60`): it forwards timestamp, position and rotation unchanged
and sends each validity as 1 when it equals 1, else 0.

### Capabilities (static)

The client keeps eight flags of the initialize response (`0x18005c130`, at
service object `+0x2a24`); the module checks only the first:

| Flag | Client byte | Gates |
| --- | --- | --- |
| 1 | `+0x2a24` | head-pose start; the platmod head-pose stream (`0x18005b2b0(sesp, 0)` at `0x18003f550`) |
| 2 | `+0x2a25` | display info, which backs the platmod display id (`0x1800472e0`) |
| 3 | `+0x2a26` | notify stream status (`0x18004b010` on every stream change) |
| 9 | `+0x2a29` | device-list changes |
| 7, 8, 10, 13 | `+0x2a2b`, `+0x2a27`, `+0x2a28`, `+0x2a2a` | not used by the module |

The real service sets flag 3 always and derives the others from its device
list, looked up by the request's URL; a URL it does not know, or a
subscription id with no matching connection (paired by client pid and slot),
gets error 7 (`0x1803ac3c0`, static). The game layer's own client sends an
empty URL.

## Session (static, emulated (Wine))

1. `platmod_start` (`0x18003e860`) calls `0x18005afd0` up to three times, 50 ms
   apart, with the device URL (`tet-tcp://127.0.0.1`) and the head-pose sink
   `0x180049aa0`. That connects both channels, sends initialize and waits up to
   3 s (`0x18005ccc0`). Module flag `+0xa850` is set only when this returns 0,
   which needs an initialize response with error 0.
2. The stream list the module reports includes head pose when that flag and
   capability 1 are set (`0x18003f550`).
3. `tobii_head_pose_subscribe` reaches `platmod_stream_head_pose_subscribe`
   (`0x180039e50`), which sends head-pose start and waits up to 3 s for its
   response, then notifies stream status if capability 3 is set.
4. Head-pose messages go to `0x18005c790`, which calls the sink for type 8 (and
   the device-list callback for type 0x25; every other type is an error). The
   sink copies timestamp, rotation and validities into the ring at module
   `+0x26dc8` and replaces the position by `A p + t`.
5. Unsubscribe sends head-pose stop. Destroying the device closes both channels.

`sesp_services_reconnect` (`0x18005b800`) reconnects both channels, initializes
again and restarts head pose if it was running; what triggers it was not
traced. In the emulated (Wine) run with the service dropping every connection
for 3 s, the game layer lost the tracker and tracked it again about 12 s later,
with head pose flowing again (`t2-interrupt.client`).

### Position transform (static, emulated)

The module builds `A` and `t` from the display area it reads from the device
(1430) when the device starts or reports a change (`0x18004ac80`): `A` rotates
about x by minus the angle `atan2(tl.z - bl.z, tl.y - bl.y)` and `t` is minus
the midpoint of bottom-left and top-right. `0x180048d30` applies the 4x4 matrix
to every SESP position. Stream Engine's head position is already in display-
centred axes ([pose-geometry.md](pose-geometry.md#display-matrix-0x140294230)),
so a service that forwards it unchanged, as `ses_windows.dll` does, makes the
game see a different position on this route than on the PRP route: for
albedo's display, `(0, 0, 600)` becomes `(-1.88, 37.08, 509.46)`. Rotation is
not changed.

### Timestamps (static, emulated (Wine))

SESP has no timesync and the sink does not convert the timestamp, so the game
gets it as sent. The Stream Engine client clock is QueryPerformanceCounter in
microseconds (`0x180027650`); the service therefore has to send the game's
counter in microseconds.

## Linux implementation

- `src/tobii_linux/sesp.py`: framing, the flatcc-shaped builder, every message
  above that the service sends, and the readers for requests and head poses.
- `src/tobii_linux/sesp_service.py`: the service side over the bridge's relay.
  Slot indices count from 1; initialize pairs the command and subscription
  connections by slot, answers capabilities 1 and 3, or error 7 for an unknown
  slot. Head-pose start subscribes the head-pose hub; stream status and
  statistics are acknowledged (statistics are dropped); anything else gets the
  invalid-operation signal with error 6, as the real service's default branch.
  Positions are sent as `A^T (p - t)` with the module's `A` and `t` for the
  device display area, so the module turns them back into the PRP route's
  positions (`cli serve --sesp-positions windows` sends them unchanged).
  Timestamps are the game's counter (`qpc_clock.CounterMapping`).
- `prp_bridge.exe` needs no change: the service lists `streamengineservices`
  as the one pipe to serve, and the bridge relays it like the PRP pipes
  ([prp-bridge.md](prp-bridge.md)).
- `tools/sesp_oracle.py`: the differential checks; `tests/test_sesp.py` carries
  the original builders' outputs as vectors.

## Live checks

Staged in `evidence/private/sesp/live/` (copy to
`C:\Users\Garulf\tobii-linux-test\sesp\` after `stage.sh`):

- `sesp-windows.cmd` (Tobii services running): `tools/sesp_real_service_probe.py`
  reports whether the pipe exists, its owner process, the initialize answers for
  `tet-tcp://127.0.0.1`, the empty URL and each listed `tobii-prp://` device,
  and 3 s of head-pose messages. It sends no request that changes the service.
- `sesp-live.cmd` / `sesp-live.sh`: the tracker in WSL2, `cli serve --tet-tcp`,
  and both test clients under stock Wine 9.0 and GE-Proton 11-9; restores
  Windows afterwards.

## Open

- Whether albedo's Tobii software serves the pipe, and its exact answers
  (capabilities, error for an unknown URL), are not observed.
- Display info (capability 2) is not served, so on this route the module
  reports no display id and window tracking finds no monitor (emulated
  (Wine): `monitor_rect=0,0,0,0`). Its response's integer fields are not
  identified.
- The trigger of `sesp_services_reconnect`.
