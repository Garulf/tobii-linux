# Game DLL service transport and head-pose session

Scope: the client side of the platform runtime protocol (PRP) in Star Citizen's
`tobii_gameintegration_x64.dll` 9.0.3.25, SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`, and what a
service has to present so that this unmodified DLL running under Wine connects and
receives head pose (PRP stream 2). No other projects or web sources about Tobii
internals were consulted. Wine statements cite Wine's own source tree or Microsoft
documentation.

Evidence labels used below:

- **static**: read from disassembly or focused Ghidra decompilation of the DLL.
- **emulated**: original DLL functions executed in Unicorn on synthetic inputs
  (`tools/prp_session_oracle.py`).
- **Wine source**: reading of Wine's source code; nothing was run under Wine.
- **inference**: a conclusion drawn from the above, not observed.

Nothing in this document was observed live. No game, Wine prefix, service or
device was run.

## Findings

1. The PRP client reaches the service only through Windows named pipes. Each
   channel is a pair of one-way, message-type pipes: the client writes to the
   service's pipe and reads from a pipe it creates itself, which the service opens
   in the reverse direction (static).
2. Three channels exist: command (`0x4E21`), subscription (`0x4E22`) and timesync
   (`0x4E23`). The command channel URL comes from discovery; the service hands
   out the other two URLs during the command-channel handshake (static, emulated).
3. Wire framing is a byte stream of `TPRP` envelopes (`src/tobii_linux/prp.py`)
   written in chunks of at most 4096 bytes. A connection handshake of four short
   messages precedes it (static, emulated).
4. To obtain head pose the client sends `connect` (command 1), then
   `stream start` (command 3) with stream id 2, and reads stream packages
   (operation 1) from the subscription channel. Timesync (command 22) uses the
   third channel, during device creation when the service lists command 0x16 and
   whenever the application requests it (static, emulated).
5. A native Linux process cannot own a Wine named pipe. A component running
   inside the game's wineserver is required; it can relay to Linux over loopback
   TCP (Wine source, inference).
6. Discovery depends on listing `\\.\pipe\*`. Wine's source has no directory
   listing for the named pipe device, so the stock DLL is expected to find no PRP
   device under Wine. This is the main open blocker (Wine source, inference,
   untested).

## Discovery

`tobii_enumerate_local_device_urls` (`0x18000cd20`) calls the `_ex` variant with
generation mask `0xFFFFFFFC`. The game integration layer calls it from
`0x180008a50`; every reported URL is tried with `tobii_device_create` (static).

| Routine | Mechanism | Resulting URL | Notes |
| --- | --- | --- | --- |
| `0x1800570b0` | `FindFirstFileA("\\.\pipe\*")`, case-insensitive prefix `TOBIIPRP-` | `tobii-prp://<rest>` | the only PRP discovery source |
| `0x180057d60` | same listing, prefix `TOBII-` | `tobii-ttp://<rest>` | legacy tracker protocol |
| `0x180057a50` | `CallNamedPipeA("\\.\pipe\ETDefaultPIPE")`, input u32 1, 1000 ms | `tobii-ttp://<name>` | see `docs/game-interface.md` |
| `0x180058010` | TCP connect to port `4455`; mask bit 2 selects `127.0.0.1`, bit 1 selects `172.28.195.1` | `tet-tcp://<host>` | default mask probes only `127.0.0.1` |

`device_create` (`0x18002a6a0`) treats `tobii-prp://`, `tprp-tcp://` and
`tobii-qvr-prp://` as PRP URLs. Every other URL creates an in-process
("embedded") platform module that speaks other protocols; that route is outside
this document. `tprp-tcp://` passes the `device_create` prefix check, but the
transport constructor `0x18004e8a0` has no branch for it and returns 3 (invalid
parameter) (static). No TCP route for PRP exists in this build.

## Transport per channel

`0x18004e8a0(transport, url, channel, timeout_us, ..., callback)` builds each
channel. For `tobii-prp://<name>` (static):

1. Server pipe: `TOBIIPRP-<name>`. If `0x18008fb20` cannot find that name in a
   `\\.\pipe\*` listing, the client uses `<name>` unchanged.
2. Reply pipe: `client_<server pipe>_<pid as %08x>_<counter as %016llx>`, where
   pid is `GetCurrentProcessId()` and counter is a process-wide atomic
   (`0x180280028`) incremented per channel.
3. `0x18008fbc0` creates the reply pipe with `CreateNamedPipeA(name,
   PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE |
   PIPE_READMODE_MESSAGE, 1 instance, out 0, in 0x80000, 5000 ms)`.
4. It opens the server pipe with `CreateFileA(GENERIC_WRITE, OPEN_EXISTING,
   FILE_FLAG_OVERLAPPED)`. On `ERROR_PIPE_BUSY` it calls `WaitNamedPipeA(1000)`
   and retries, four attempts in total. It then sets
   `PIPE_READMODE_MESSAGE` with `SetNamedPipeHandleState` and fails if that call
   fails.
5. It starts an overlapped `ConnectNamedPipe` on the reply pipe and waits for the
   service to open it.

Flag values follow Microsoft's CreateNamedPipeA reference
(https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createnamedpipea).
Because the client switches its handle to message-read mode, the service pipe
must be created with `PIPE_TYPE_MESSAGE`. Microsoft documents that message-read
mode requires a message-type pipe
(https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-type-read-and-wait-modes);
Wine's `set_named_pipe_info` handler enforces the same rule (Wine source).

Timeouts (static): `timeout_us / 1000` milliseconds bounds each handshake step;
command and subscription use 1,000,000 us, timesync 100,000 us. Sends wait the
same way (`0x18004f250`). A command response is awaited for 30,000,000 units of the
client clock `0x180027650` (30 s if the unit is microseconds, not established),
polling every 50,000 units.

The DLL also contains the service side of this transport (`pr_ipc_service`,
`server.cpp`), used for in-process modules. Its listener `0x1800aae50` creates
the service pipe as `CreateNamedPipeA(name, PIPE_ACCESS_INBOUND |
FILE_FLAG_OVERLAPPED, PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE, 255, 0,
0x10000, 5000)` and `0x1800aa7e0` opens the client's reply pipe with
`GENERIC_WRITE` and message-read mode (static). The ET5 service executable
carries the same strings, but its code was not compared.

## Framing

- Client to service: each PRP packet goes out in `WriteFile` chunks of at most
  4096 bytes (`0x18004f250`); each chunk is one pipe message (static).
- Service to client: the client reads 4096 bytes at a time and treats
  `ERROR_MORE_DATA` as a partial read (`0x180090ad0`), appends the bytes to a
  buffer and decodes complete envelopes (`0x180021390`, `0x180020df0`) (static).
  Message boundaries therefore do not matter after the handshake. A service may
  write one packet per message.
- Packet: `TPRP` envelope, then a payload made of fixed blocks. Each block starts
  with its own u32 size and is followed by a variable region whose first u32 is
  the region size (including that word). Variable fields are `(length, offset)`
  pairs or bare offsets relative to the region start, 4-byte aligned, with
  NUL-terminated strings. A final u32 0 ends the payload, and it must be the last
  word (`0x18004fcc0`, `0x18004fc30`, `0x18004fd40`, decoder `0x180053d10`)
  (static, emulated).
- Payload layout: header block `{12, tag, operation}`, then the operation block,
  then the command block.

## Handshake (each channel)

From the client transport `0x18004e8a0` and the service state machine
`0x18008f470` (static), with the client-side parsers emulated:

| Step | Direction | Bytes | Rule |
| --- | --- | --- | --- |
| 1 | client to service pipe | u32 1, u32 channel, reply pipe name, NUL | service rejects version 0 and requires 9 bytes or more |
| 2 | service opens the reply pipe | | `CreateFileA(GENERIC_WRITE)` and message-read mode |
| 3 | service to client | u32 1 | client callback `0x18004f300` accepts exactly 4 bytes, and the value must be 1 |
| 4 | client to service | u32 process id | service compares it with `GetNamedPipeClientProcessId` when that call succeeds |
| 5 | service to client | i32 id, subscription URL, NUL, timesync URL, NUL | 256 bytes at most. `0x18004f360` needs both strings, otherwise the channel fails. |

After step 5 the command channel copies the two URLs into a 256-byte buffer:
the subscription URL at offset 0 and the timesync URL at offset 0x80
(`0x180020d80`). Each URL should therefore stay below 128 bytes (inference from
the copy layout). The subscription channel keeps only the id (`0x1800217e0`).
If the id is still -1 the connect attempt fails (`0x18001fed0`). The service
later receives the same id in the connect request, which lets it pair the
command and subscription channels.

## Session sequence for head pose

1. **Command channel** to the discovered `tobii-prp://` URL, then the
   **subscription channel** to the subscription URL from step 5 (`0x18001fed0`).
2. **connect** (command 1, tag 1). The client waits for a response with the same
   tag. A response carrying a different tag is an error (`0x180021390`).
   Response status must be 0. `device_create` then checks the result
   (`0x18002a6a0`):
   - `license_results` count equals the number of license keys sent. A
     mismatch fails device creation. When the caller asked for validation
     results, a negative value fails it as well.
   - `firmware_upgrade_in_progress` is 0. Otherwise creation fails with error
     0xB.
   - The build version string is logged as "Connected to platform module with
     build version" (`0x18002ee80`), and the 0x130C-byte record is stored in the
     device.
   - If `supported_commands` contains 0x16, creation runs up to four timesync
     rounds right away (`0x180030630`, see step 6).
3. The game integration layer (`0x180008a50`) then asks which streams are
   supported. For PRP devices `0x18002c6d0` searches the connect response's
   `streams` list. Stream Engine stream 4 (head pose) maps to PRP stream 2,
   stream 0 to 3 and stream 12 to 29 (`0x18002c5d0`). Next it reads the display
   area through a property get (`0x1800147d0`, not reconstructed; a failure is
   logged and the tracker is still reported) and the device info.
4. **stream start** (command 3, stream 2) from `tobii_head_pose_subscribe`
   (`0x18000fd80`, then `0x180028560`, `0x18002d270`, `0x1800189f0`). A status of
   3 (connection failed) is tolerated: the subscription is remembered and redone
   on reconnect.
5. The service pushes **stream packages** (operation 1, stream 2) on the
   subscription channel. Layout and encoder: `encode_stream_packet` in
   `prp.py`, previously checked against the original serializer and decoder. The
   subscription reader (`0x180020df0`) dispatches by operation (1 stream, 2
   property notification, 11 compound stream) and does not check the tag.
   Packets are processed only when the application calls
   `tobii_device_process_callbacks`.
6. **timesync** (command 22) is sent when `device_timesync` (`0x18002bec0`)
   runs: from `tobii_update_timesync` (`0x18000def0`), from `0x180015f40`, and
   during device creation as described in step 2. The client opens the timesync channel lazily with a 100 ms budget and
   uses its own tag counter (`client+0x5638`). The response must carry that tag,
   status 0, and a 64-bit timestamp sent high word first. The client waits up to
   1,000,000 clock units (`0x18001ae10`, `0x1800211c0`).
7. **stream stop** (command 4, stream 2) on unsubscribe (`0x180018d80`).
   Destroying a channel disconnects and closes its pipes (`0x1800903b0`).

No keepalive or heartbeat message was found in the analysed client paths (static,
not exhaustive). Failures surface as pipe errors or response timeouts, after
which `prp_client_reconnect` (`0x180018050`) reruns the connect sequence.

## Message table

All tags are u32. Commands use operation 9 (request) and operation 10 (response).

| Message | Payload blocks after the header | Codec |
| --- | --- | --- |
| request header | `{8, command}` | |
| connect request | `{0x40, sub_id, n, licenses@, key_len, key@, v0, v1, v2, v3, dir_len, dir@, name_len, name@, pid, field_of_use}`; `0x3C` bytes without the last word | `encode_connect_request` |
| stream start/stop | `{8, stream}` | `encode_stream_request` |
| timesync request | `{4}` | `encode_timesync_request` |
| response header | `{12, status, command}` | |
| connect response | `0xA8` bytes, `0x98` without the four extended strings (see below) | `encode_command_response` |
| stream start/stop response | `{8, value}`; the client reads only the status | `encode_command_response` |
| timesync response | `{12, time_high, time_low}` | `encode_command_response` |
| stream package | operation 1, see `docs/game-interface.md` | `prp.encode_stream_packet` |

`@` marks an offset into the variable region. The licenses table holds `n`
`(length, offset)` pairs, followed by the license bytes. Connect request fields
recovered from `0x18001fed0` and `0x18002a6a0` (static):

- `sub_id`: the subscription channel's handshake id.
- `licenses`: the license keys passed to `tobii_device_create_ex`. None are sent
  with `tobii_device_create`. The game integration layer tries the licensed call
  first when it holds a key (`0x180008820`).
- `key`: public key bytes of the process image's Authenticode signer, obtained
  through `CryptQueryObject` and `CertFindCertificateInStore` (`0x1800572d0`);
  empty when unavailable.
- `v0..v3`: constant `4, 17, 0, 6` (`0x1801d9230`), presumably the client API
  version (inference).
- `dir`, `name`: UTF-16LE components of `GetModuleFileNameW(NULL)`, split by
  `0x1801a2464` (`0x180056fa0`). Which component is which is inferred from the
  argument positions.
- `pid`: `GetCurrentProcessId()`.
- `field_of_use`: 1 or 2 from the device_create argument, otherwise 0.

The serializer's version argument only selects between two layouts: positive
gives the extended layout, zero the short one. The client always passes
`0x7FFFFFFF` (`0x18004f6e0`, `0x180053cc0`), so it sends extended requests and
reads extended responses. Which version the real service serializes with is not
established.

### Connect response layout

Word index in the response block, then the stored client record offset in
parentheses (decoder `0x180055780`, serializer `0x180050370`, static and
emulated):

| Word | Field | Client capacity |
| --- | --- | --- |
| 1 | build version string @ (0x000) | 255 characters |
| 2 | firmware upgrade in progress (0x100) | |
| 3-10 | four u32 lists `list_a..list_d` (0x104, 0x16C, 0x1D4, 0x23C) | 25 each |
| 11-12 | `streams`, PRP stream ids (0x2A4) | 32 |
| 13-14 | `supported_commands` (0x328), values above 0x20 dropped | 33 |
| 15-16 | `groups`, 12-byte entries `{kind, n, values@}` with kind < 5 (0x3B0) | 5 entries of 32 values |
| 17-23 | seven strings (0x65C, 0x75C, 0x85C, 0x95C, 0xA5C, 0xD5C, 0xDDC) | 255, 255, 255, 255, 255, 127, 127 |
| 24-32 | nine u32 (0xE5C) | |
| 33-34 | `license_results` (0xE80) | 256 |
| 35 | `mode` (0x1284) | |
| 36-37 | `list_g` (0x1288) | 32 |
| 38-41 | four extended strings (0xB5C, 0xBDC, 0xC5C, 0xCDC) | 127 each |

Variable data follows block order: the build string, the six lists, the group
table and then each group's values, seven strings, the license results,
`list_g`, and the extended strings. `supported_commands` is compared with PRP
command ids: value 0x16 (timesync) triggers the timesync rounds at creation
(`0x180030630`), and `tobii_capability_supported` (`0x18000ee70`) searches it
through `0x18002cd30`. Reading it as a list of supported commands is an
inference. `streams` is also searched for 0xB by capability 9, and `list_c` for
0xB when Stream Engine stream 3 is requested. The meaning
of the unnamed lists, strings and words is not established. They are probably
used by `tobii_get_device_info` (`0x18000e310`) and other queries, which were not
traced. `minimal_connect_response` sets only the four checked items.

## Wine considerations

| Mechanism used by the DLL | Expected under Wine | Evidence |
| --- | --- | --- |
| Named pipes between processes in one Wine prefix | works; the pipes are wineserver objects | Wine source: `server/named_pipe.c` and the `\Device\NamedPipe` device in `server/directory.c` |
| Native Linux process opening such a pipe | not possible by path; names live in the wineserver object namespace, not the filesystem | Wine source, inference |
| `FindFirstFileA("\\.\pipe\*")` | expected to fail: the pipe directory uses a pseudo fd without a Unix fd, and `NtQueryDirectoryFile` needs a Unix directory fd | Wine source, inference, untested |
| Message-type pipes, `ERROR_MORE_DATA`, `SetNamedPipeHandleState` | implemented | Wine source: `message_queue_read`, `set_named_pipe_info` |
| Winsock TCP | wineserver creates and connects host sockets, so loopback reaches native listeners | Wine source: `init_socket`, `connect` in `server/sock.c` |

Wine source excerpts and the exact commit are in
`evidence/private/d-wine-source-notes.txt`. They come from the official mirror at
commit `081042ede6a165a36bf9988350dd584b440d7a3d`:

- https://raw.githubusercontent.com/wine-mirror/wine/081042ede6a165a36bf9988350dd584b440d7a3d/server/named_pipe.c
- https://raw.githubusercontent.com/wine-mirror/wine/081042ede6a165a36bf9988350dd584b440d7a3d/server/fd.c
- https://raw.githubusercontent.com/wine-mirror/wine/081042ede6a165a36bf9988350dd584b440d7a3d/dlls/ntdll/unix/file.c
- https://raw.githubusercontent.com/wine-mirror/wine/081042ede6a165a36bf9988350dd584b440d7a3d/dlls/kernelbase/file.c
- https://raw.githubusercontent.com/wine-mirror/wine/081042ede6a165a36bf9988350dd584b440d7a3d/server/sock.c
- https://raw.githubusercontent.com/wine-mirror/wine/081042ede6a165a36bf9988350dd584b440d7a3d/server/directory.c
- wineserver man page source, which describes wineserver as providing "roughly the
  same services that the Windows kernel provides on Windows":
  https://raw.githubusercontent.com/wine-mirror/wine/master/server/wineserver.man.in

Proton and other Wine forks were not examined.

## What a Linux-side service needs

A native service alone cannot satisfy the DLL. The minimum split (inference):

1. **Wine-side bridge**, a Windows program started in the game's prefix and
   wineserver:
   - Listen on `\\.\pipe\<name>` and on `\\.\pipe\TOBIIPRP-<name>` as overlapped,
     inbound, message-type pipes with message-read mode. The client falls back
     to `<name>` when the listing fails.
   - Do the same for the subscription and timesync pipe names it advertises in
     handshake step 5, for example `tobii-prp://<name>_sub` and
     `tobii-prp://<name>_ts`, each URL below 128 bytes.
   - For each client: read the hello, open the reply pipe for writing, set
     message-read mode, then send the ack, read the pid and send the handshake
     information.
   - Relay the TPRP byte streams to and from the Linux service, for example
     over `127.0.0.1` TCP.
2. **Linux service**, speaking PRP through the bridge or embedded in it:
   - Answer connect with status 0, the same number of license results as
     licenses received (0 means valid), `firmware_upgrade_in_progress = 0`, and
     `streams` containing 2 (and 3 or 29 for gaze). Leaving 0x16 out of
     `supported_commands` avoids the timesync rounds at creation; whether that
     harms timestamp handling later is not established.
   - Answer stream start and stream stop with status 0.
   - Push operation 1 head-pose packages on that client's subscription channel
     without blocking when the game falls behind.
   - Answer timesync with the service clock.
   - Answer every other command or property request with a nonzero status
     instead of staying silent, since the client waits up to 30 s for a tag
     match.
3. **Discovery**: the stock DLL learns `tobii-prp://` URLs only from a pipe
   listing, which is expected to fail under Wine. Without a remedy the game never
   tries the bridge. Candidate remedies, none evaluated: a Wine change that lists
   named pipes; a game-side hook; or the legacy routes in the discovery table.
   The legacy routes lead to the embedded module, whose head-pose subscription
   reaches `sesp_request_headpose_start` (`0x180039e50`, then `0x18005b2d0`,
   `0x18005cc30`, `0x180087fd0`). That uses the `tobii-sesp://streamengineservices`
   pipe and the tracker protocol, neither of which is reconstructed here (static).

## Codec, oracle and tests

`src/tobii_linux/prp_session.py` contains pure codecs for pipe names, the
handshake messages, chunking and envelope reassembly, the four commands, their
responses, and `minimal_connect_response`. It is stricter than the client in
places: it rejects unterminated strings and out-of-region fields, which the
original decoder does not check.

`tools/prp_session_oracle.py` maps the hash-bound DLL in Unicorn as
`tools/prp_encoder_oracle.py` does and runs only original functions: serializer
`0x18004f730`, decoder `0x180053d10`, handshake parsers `0x18004f300` and
`0x18004f360`, and channel callbacks `0x180020d80` and `0x1800217e0`. Output:
`evidence/private/prp-session-oracle.json`, 22 cases, all matching:

- 8 requests: connect with and without licenses and strings, both layouts;
  stream start/stop for streams 2 and 29; timesync. The original serializer
  output equals the Python encoder byte for byte. The original decoder
  reproduces every native request field, compared through pointer contents.
- 12 responses: three connect variants, including status 17, plus stream start,
  stream stop with status 5, and timesync, each in both layouts. Bytes are equal,
  and the original client decoder's stored records equal the native inputs byte
  for byte, including the whole 0x130C-byte connect record.
- 2 handshake cases. The ack is accepted, and a 5-byte variant is rejected. The
  handshake information is accepted with 3 entries: the command callback gets
  both URLs and the subscription callback gets the id. A message missing the
  second string is rejected.

A mutation check (swapping the two image-name fields, or changing a stream id)
makes the oracle fail. That run is not stored. `tests/test_prp_session.py` holds
oracle-derived vectors plus round-trip and malformed-input tests.

## Evidence files

| File | Content |
| --- | --- |
| `evidence/private/d-transport-assembly.txt` | objdump of the transport constructor, client pipe open, service handshake, discovery order, stream map, timesync and stop requests |
| `evidence/private/d-transport-import-callers.txt`, `d-game-iat.txt` | socket and pipe import call sites and the IAT map |
| `evidence/private/d-batch1-focused.c` ... `d-batch6-focused.c` | Ghidra 12.1.4 `-noanalysis` focused decompilation, with logs; project in `evidence/private/ghidra-agent-d-dlls/` |
| `evidence/private/d-wine-source-notes.txt` | Wine source excerpts with commit |
| `evidence/private/prp-session-oracle.json` | emulation results |

Focused decompilation batches: 1 transport, connect, device_create, send/receive,
stream start; 2 command serializers, operation decoder, handshake callbacks;
3 service handshake helpers, pipe server, client receive, TCP discovery;
4 stream support, capability and device-info entry points, subscribe; 5 stream map
consumers, subscription and timesync processing; 6 service handshake state
machine, game integration enumeration.

## Gaps

- Pipe enumeration under Wine has not been tested, nor has any part of this
  route run under Wine or against the real service.
- The real service's handshake timing, its subscription and timesync URL
  naming, its serializer version and its values for the unnamed connect-response
  fields are unknown. Only the DLL's built-in service code was read.
- Property get and notifications (display area, device info and others) were
  not reconstructed. The game integration layer requests the display area during
  enumeration (`0x1800147d0`).
- The game's later tracking connection after enumeration, and its call pattern
  for `tobii_update_timesync` and `tobii_device_process_callbacks`, were not
  traced.
- The meaning of `message_tag` in pushed stream packages is still open. The
  client does not check it.
- The embedded legacy route (tracker protocol and SESP) was not reconstructed.
- Clock units of `0x180027650` are not established.
