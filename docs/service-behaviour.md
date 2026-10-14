# Service behaviour: user presence and robustness

How the Windows runtime (`platform_runtime_IS5LEYETRACKER5_service.exe`, base
`0x140000000`) reports user presence to games and how it behaves when the
tracker goes away, and what `tobii_linux.cli serve` and the browser demo now
do the same way. Protocol framing is in [prp-bridge.md](prp-bridge.md) and
[game-integration-layer.md](game-integration-layer.md).

Evidence labels:

- **static**: read from the runtime or game DLL (focused Ghidra output in
  `evidence/private/presence-*-focused.c`, `robust-*-focused.c`, or
  `tools/pe_xref.py`).
- **emulation**: original machine code run in Unicorn against the native code
  (`tools/presence_oracle.py`, report `evidence/private/presence-oracle.json`).
- **live (Windows)**: albedo, real service, `tools/prp_real_service_probe.py`
  or the runtime's own log (`evidence/private/robust-live/`).
- **live (Wine)**: albedo's WSL2 with the real tracker attached through
  usbipd, `cli serve`, the bridge and test clients under patched Wine 11.0
  (`evidence/private/robust-live/wsl3/`).
- **unit**: `tests/test_prp_service.py`, `tests/test_streams.py`,
  `tests/test_supervisor.py`, `tests/test_demo.py`.
- **inference**: reasoning not checked directly.

## User presence

### Windows

The ET5 decides presence on the device. There is no host-side presence
algorithm for it: nothing in the face tracking, head pose or gaze path feeds
property 0xB.

1. Device stream `0x504` (index 5) carries column 1 (int64 timestamp) and
   column 2 (presence). `parse_presence` `0x1402cba80` reads column 1 as int64
   (field tag 6) and column 2 as **int32** (field tag 1, reader `0x1403af9f0`),
   skips other columns, and fails the whole message on a wrong type. The
   dispatcher `0x1402c2700` case 5 leaves absent columns zero and calls
   `receive_presence_data` `0x140266b60`, which keeps 0, 1 and 2 and turns
   anything else into 0 (static, emulation).
2. Subscribing presence on the device, `platmod_stream_user_presence_subscribe`
   `0x140276dd0`, starts the device stream and, when no presence data has
   arrived on this device yet, immediately calls the subscriber with value 0
   (unknown) stamped `tracker_timestamp_get - 30000` us (static, emulation).
3. The service callback `0x1400fd220` converts each record into property 0xB
   (`0x1400fa2b0`: 0 to 1 unknown, 1 to 2 away, 2 to 3 present) and stores it in
   the property cache `0x14010e930`. Every record except the first one after the
   cache was cleared is queued as an operation 2 notification; there is no
   comparison with the previous value (static, emulation, three sequences).
   The device itself only sends its current value when subscribed and then
   changes (one `0x504` message in a whole Linux capture with a person sitting
   still, `gaze-device/linux-gaze.jsonl`).
4. The subscription sender `0x1400f5080` delivers operation 2 records only to
   clients whose subscription table holds that property (kind 0, static).
   Command 5 for 0xB (`0x1400f0fb0` case 4) subscribes the device side on the
   first subscriber (`0x1400f7a50`) and answers from the cache; the plain
   property get is refused (live (Windows)). The cache is cleared when a
   device is added again (`0x1400f1f90` calls `0x14010e8d0`), so the time in
   the subscribe answer changes after every re-add: `0xcd728c055d`,
   `0xd3d511da8c`, `0xd441c5aaa6`, `0xd4633cb0ea` across this work's runs,
   with value 2 (away, nobody present) each time (live (Windows)).

Notification bytes. The game DLL's serializer `0x18004f730` turns a queued
record into (emulation; `ORIGINAL_PRESENCE_NOTIFICATION` in the unit tests):

```
{12, tag 0, operation 2} {4} {8, 0xB} {4} {0x10, time high, time low, value} {4} 0
```

A live operation 2 from the real service (property 0x11, value 2, captured
while probing subscriptions) has exactly this framing with message tag 0, and
`prp_session.encode_property_notification` reproduces it byte for byte
(live (Windows), `robust-live/subscribe-all.jsonl`). A presence change itself
was not captured: nobody was at the tracker; 4 s and 20 s holds with 0xB
subscribed produced no notification, as expected without a change
(`presence-idle*.jsonl`). `presence-person.cmd` (below) records one.

### Why Linux read 0

The device never reported 0. Linux parsed column 2 as uint32 (tag 2), so the
int32 field the device sends (tag 1) was skipped and the default 0 used. The
only presence message in the person-present Linux capture decodes to 2
(present) with the runtime's types (emulation: the original parser and
dispatcher give `(899969443614, 2)` for that payload). No subscription order
or configuration is involved.

### Linux

- `gaze.presence_message` now types the columns as `parse_presence` does and
  returns None where the runtime drops the message; `streams.presence` drops
  it.
- `streams.GazeAndImageStreams` emits the unknown record on subscribe
  (`prp_service.presence_subscribe_time`) and maps device values as
  `receive_presence_data`.
- `prp_service.UserPresence` is the property cache; `PrpService.publish_presence`
  stores each record and pushes `UserPresence.notification()` to every
  subscription that subscribed 0xB with command 5. Command 6 stops it. The
  head-pose-validity fallback is gone.
- `cli serve` subscribes `0x504` beside gaze and images in every session. The
  demo's status shows the game-side value, the device value, its time and how
  many changes would have been pushed.

| Check | Result | Evidence |
| --- | --- | --- |
| 12 raw `0x504` messages (captured, int32 0 to 3 and -1, uint32 value or time, missing columns, unknown and repeated columns) through parser, dispatcher and `receive_presence_data` | all equal to `streams.presence` + `platmod_presence` | emulation |
| Subscribe initial record: new, tracker time below 30 ms, data already received, device not connected, timestamp failure | all equal | emulation |
| Service cache and queue on three record sequences, queued records through the game DLL serializer | same records notified, packets byte-identical | emulation |
| Live service: device presence 1 (nobody present) reaches the game layer | `user presence 2`, game layer `present 0` | live (Wine) |
| Notification only to subscribed clients, stops after command 6 | | unit |

### Property subscriptions

The game layer's `tobii_notifications_subscribe` walks properties 1, 2, 4, 5,
6, 7, 0xF, 0x10, 0x15, 0x16 and drops all of them when one is refused; its
reconnect then retries the refused property every 10 s and never resumes
(live (Wine), runs `wsl1`, `wsl2`). The service now answers commands 5 and 6
for properties 1 to 24 exactly as the real service did (live (Windows),
`tests/data/real-property-subscriptions.json`, unit): 2 power save active,
4 device paused, 5 exclusive mode, 7 calibration active and 0x10 answer 0;
6 is the device's calibration id (read from device status 1490 at session
start, `0xbc381a37` on albedo); 0xF the track box (front plane 450 mm,
back 900 mm, the reference tracker's values); 0x11 the client's field of use;
0x15 and 0x16 the text `ok`; 3, 8, 9 are refused with status 2 and 0xA, 0xC,
0xD, 0xE, 0x13, 0x14, 0x17, 0x18 with 17; unsubscribing a property that is
not subscribed gives status 5. Only two byte ranges differ: the display id's
padding and the refused 0x13 block, both uninitialised memory in the real
service.

## Tracker loss, replug and host suspend

### Windows

The runtime treats unplugging and host sleep alike (live (Windows), its own
log `pr_log2.txt`, `pr_log3.txt`, dozens of sleep cycles 2026-05 to 2026-09):

```
Handling sleep event                                  usb_manager
Device 'IS5FF-100204141463' removed                   module_on_device_changed_handler
Command client 5 with matching subscription client 4 both now queued for destruction
Timesync client 4 now queued for destruction
Server shut down
... (host asleep)
Handling resume event
Device 'IS5FF-100204141463' re-added                  (full startup: calibration sync,
                                                       screen plane, hardware info, host algo)
Server listening for incoming connections
Command client 1 context successfully created          (clients reconnect within a second)
```

So during a loss the real service sends clients nothing: it closes every
command, subscription and timesync connection and its pipes disappear; it
does not send invalid samples. Clients see CONNECTION_FAILED and reconnect
once the server listens again. Statically, sleep (`0x1402d0bb0`) calls the
device-removed path `0x1402d00e0` for every device and resume rescans
(`0x1402d02f0`); removal (`0x1400edef0`, `0x1400f4ad0`) clears the client
subscription table and queues; re-add (`0x1400f1f90`) refreshes the device
information and clears the property cache.

Live pipe-level removal could not be produced on Windows without a person:
`usbipd bind --force` did not take the device away from the running service
(a reboot is pending for the filter), `usbipd attach` refused a device in use,
and `pnputil /disable-device` refused for the same pending reboot
(`robust-live/real-disconnect*.cmd`; the probe kept receiving 33 packages/s).

### Power saving

Commands 3000/3010 (power save) and 3100 (pause/resume) are sent only when a
client sets property 2 or 4 (`0x1400f6e70` from the command dispatcher
`0x1400f3de0`, then `0x140101360`/`0x14027ea30` and `0x140100d50`/`0x14027a8a0`)
and around diagnostic image snapshots (`0x140269390`, "retrieve_using_snapshot").
The vtable slots `0x330`, `0x338`, `0x350`, `0x358` have no other callers. The
runtime does not send them on its own (no display, idle or client-count
trigger) (static). The game layer only reads and subscribes 2 and 4, so the
Linux service never sends them.

### Linux

`cli serve` keeps its process, the PRP listener and the bridge's control
connection up, and mirrors the server shutdown towards clients:

- `supervisor.TrackerLink` opens a session (`driver.usb_session`), runs
  `streams.serve_tracker` and, when the session ends for any reason, waits
  (`supervisor.Backoff`, 0.5 s doubling to 2 s, reset after a 10 s session) and
  starts again from USB enumeration, protocol upgrade, realm and calibration.
  The head-pose models load once (`driver.HeadPoseProcessor`); each session
  restarts tracking state and filters.
- While no session streams, `PrpService.detach_tracker` closes every client
  connection and refuses new pipe connections; `attach_tracker` accepts them
  again. Presence restarts with the unknown record.
- A session ends on USB errors, a request without reply (3 s), no stream data
  for 3 s, a device clock jumping back by more than 1 s (tracker restart),
  or a host suspend (`CLOCK_BOOTTIME` running more than 2 s ahead of
  `CLOCK_MONOTONIC` since the last read, `streams.SuspendWatch`). Such a
  session is marked dead and not asked to unsubscribe.
- The endpoint is always read on its own thread: the tracker goes silent for
  the rest of a session when it is not read for about a second while streams
  are subscribed (live (WSL), `evidence/private/calibration/probe/host-pattern-raw.jsonl`). `driver.head_pose_samples` now
  always reads on a thread, `streams.drained` decouples the demo's consumer,
  and `cli serve` hands results to the event loop without blocking.
- SIGINT and SIGTERM stop the link, unsubscribe the three streams, close USB,
  disconnect clients and the bridge, and exit 0.
- When `--wine-clock auto` moves the service clock to another Linux clock
  while game clients are connected, the service closes them, so that they
  reconnect and timesync on the new clock; the tracker session goes on
  ([prp-bridge.md](prp-bridge.md#time-base)).
- Several clients: every subscription has its own queue per stream; closing a
  client's command channel closes its subscription channel too, as the real
  service destroys both.

The demo's live source uses the same pieces (drained reading, suspend watch,
backoff) and shows sessions started and lost.

| Case | Result | Evidence |
| --- | --- | --- |
| Tracker detached with `usbipd detach` while serving the game layer and a Stream Engine client | session lost on `libusb error -4`, 6 client connections closed, reconnect attempts refused, Stream Engine client gets PRP CONNECTION_FAILED | live (Wine) |
| Tracker attached again | new session 2 s after it is visible; the game layer reconnects 10 s later, re-subscribes 0xB, head pose, gaze and its notifications, `connected 1` | live (Wine), `wsl3` |
| Two clients at once (game layer and Stream Engine) | both stream; the Stream Engine client got 235 head and 232 gaze packages in the 11 s before the detach, a fresh one after the replug 253 and 271 in 10 s | live (Wine) |
| SIGTERM | `unsubscribed presence, image, gaze`, exit 0 | live (Wine) |
| Unplug and replug, stop during streaming, presence after start | scripted devices | unit |
| Stalled tracker, slow consumers (inline, newest-frame and drained readers) | stall detected and session dead; readers keep the endpoint drained | unit |
| Device clock rewind, host resume | session ends | unit |
| Fan-out to two clients, stop for one, command/subscription pairing | | unit |
| Host suspend on real hardware | not run | |

## Person-present check

`evidence/private/robust-live/presence-person.cmd` (staged on albedo as
`C:\Users\Garulf\tobii-linux-test\presence-person.cmd`, with
`presence-person.sh` and `robust-run.tgz`) needs someone at the tracker for
about three minutes: 50 s against the Windows runtime with 0xB subscribed
(records the real operation 2 bytes), then 50 s through `cli serve` with the
game layer under Wine (service log and the layer's `present` lines). Sit in
front for 15 s, leave for 15 s, come back, in each phase. It restores Windows
afterwards.

## Open

- The real service's presence notification bytes for an actual change
  (expected to equal the emulated serializer output with message tag 0).
- Host suspend on a Linux machine with the tracker attached.
- Whether the real service notifies property 6 when the calibration changes.
- The pipes stay listed by the bridge while the tracker is gone; on Windows
  they disappear. Clients get the same failures either way (inference).
