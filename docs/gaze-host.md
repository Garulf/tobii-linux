# Gaze on the host: device stream to game

How the Windows runtime turns the tracker's gaze stream (`0x500`) into what
Stream Engine clients receive, and how the Linux service reproduces it.
Runtime: `platform_runtime_IS5LEYETRACKER5_service.exe`, SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`, base
`0x140000000`. Game DLL: Star Citizen's `tobii_gameintegration_x64.dll`
9.0.3.25, SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`.

Evidence labels:

- **static**: disassembly or focused Ghidra `-noanalysis` decompilation
  (`evidence/private/gaze-*-focused.c`, `gz-*-focused.c`).
- **emulation**: original code run in Unicorn by `tools/gaze_oracle.py`.
- **live (Windows)**: albedo, real service build `1.37.4.0_f2b4e724c9`,
  tracker `IS5FF-100204141463`, a person in front of it.
- **live (Wine)**: albedo WSL2, patched Wine 11.0, synthetic service.
- **unit**: `tests/test_gaze.py`, `tests/test_prp_service.py`.
- **inference**: a conclusion drawn from the above.

## Result

The ET5 computes gaze on the device. The runtime does not filter, combine
eyes or transform coordinates: it copies typed columns into records, turns
validity codes into flags and clamps one stream. The gaze point a game
receives is the device's column 28 ("Relative Combined Gaze Point") whenever
column 27 is nonzero, bit for bit:

| Step | Check | Label |
| --- | --- | --- |
| Device message to platmod records to PRP records to PRP packets | 96 recorded Linux messages from four captures, 576 record and 960 packet comparisons, all byte-exact, plus 9 edge cases | emulation |
| PRP packets to Stream Engine callback | 144 of 144 valid gaze points from `tobii_stream_engine.dll` equal the simultaneously captured stream 3 values bit for bit; one constant timestamp offset | live (Windows) |
| Real service packets for streams 3, 29 and 4 | re-encoded byte for byte by `gaze.stream_packet` | live (Windows), unit |
| Game integration layer `GetGazePoints` | returns `(2x - 1, 1 - 2y)` of the Stream Engine point on Windows and under Wine | live (Windows), live (Wine) |

So a Linux gaze point differs from Windows only where the device's column 28
differs. With the same person and targets, the published Linux gaze point
scores median 48 to 65 px against Windows' 52 px (see
[accuracy](#accuracy-against-targets)); the remaining spread is device-side.

## Chain

| Stage | Address | What it does |
| --- | --- | --- |
| TTP parser | `0x1402cfa50`, `0x1402cf8c0`, `0x1402cfde0` | stream decoder `0x1402ce2a0` case `0x500` reads each known column with a fixed reader; a wrong wire form fails the message (status 2), unknown columns are skipped |
| Column dispatcher | `0x1402c1c30` | zeroes a 0x150-byte record, copies columns by id after a parsed-type check, calls configuration `+0x110` |
| `receive_gaze_data` | `0x140265d70` | builds six platmod records and calls the subscribers at device `+0x11388` and up |
| Stream descriptors | `0x140277c20`, `0x140277a40` | map platmod stream types to subscriber slots; every gaze family shares the gaze stream's sync functions `0x140278a80`/`0x140278c00`, which subscribe device stream 1 (`0x500`) |
| Service subscribe | `0x1400f7a50` | maps PRP stream ids to platmod subscribe functions and service callbacks |
| Service callbacks | `0x1400fc170`, `0x1400fcdd0`, `0x1400fc040`, `0x1400faf30` (`0x1400f9f00`), `0x1400fae20`, `0x1400fd0f0` | fill a queued PRP record; a validity becomes true only when it equals 1 |
| Queue | `0x14010db20` allocate, `0x14010e1a0` push | sender thread serializes; no value change |
| Client decoder | game DLL `0x180053d10`, `0x180056230` | PRP stream decode |

All static; every stage from parser to queue is also run in emulation.

### Columns (dispatcher `0x1402c1c30`)

Parsed types: 1 uint32, 4 int64, 5 float (wire fixed16, `float32(raw) *
2^-16`), 9 two floats (struct `0x1f40` of two fixed42), 10 three floats
(struct `0x1f41` of three fixed42). Fixed42 is `double(raw) * 2^-42`
narrowed to float32.

| Column | Name in the device's column list | Record offset | Type |
| --- | --- | --- | --- |
| 1 | Timestamp | `0x00` | 4 |
| 2, 8 | Left/Right Gaze Origin Tracker | `0x08`, `0x4c` | 10 |
| 34, 36 | Left/Right Gaze Origin Display | `0x14`, `0x58` | 10 |
| 29, 30 | Left/Right Eye 3D Relative Position Validity | `0x20`, `0x64` | 1 |
| 3, 9 | Left/Right Eye 3D Relative Position | `0x24`, `0x68` | 10 |
| 4, 10 | Left/Right Eye 3D Gaze Point | `0x30`, `0x74` | 10 |
| 5, 11 | Left/Right Eye 2D Gaze Point | `0x3c`, `0x80` | 9 |
| 6, 12 | Left/Right Eye Pupil Diameter | `0x44`, `0x88` | 5 |
| 7, 13 | Left/Right Eye Validity | `0x48`, `0x8c` | 1 |
| 28, 27 | Relative Combined Gaze Point, Validity | `0x90`, `0x98` | 9, 1 |
| 32, 31 | Remote Gaze Point Responsive Combined, Validity | `0x9c`, `0xa4` | 9, 1 |
| 38, 37 | Left User 3D Position Guide Validity, Guide | `0xa8`, `0xac` | 1, 10 |
| 40, 39 | Right User 3D Position Guide Validity, Guide | `0xb8`, `0xbc` | 1, 10 |
| 14, 17, 20 | unnamed (17 is 4, 20 counts frames in captures) | `0xc8`, `0xe0`, `0xf8` with a presence flag | 1 |
| 22, 33, 21, 35 | Left/Right Gaze Origin Tracker/Display Validity | `0x100`, `0x108`, `0x110`, `0x118` with presence flags | 1 |
| 23, 24 | Left/Right EyeBall Center position | `0x120`, `0x130` with presence flags | 10 |
| 41, 42, 43, 44 | Left/Right Absolute Eye Openness, Validity | `0x140` to `0x14c` | 5, 1 |

Columns 25 and 26 (2D position relative to the sensor) are sent but never
read. A repeated column overwrites the earlier one. Absent columns stay zero.
In the emulated parser, 40 columns in one message fail with status 3 while
the device sends 39; this depends on the parser buffer the oracle sets up
(`0x5740` bytes, as `tools/ttp_reply_oracle.py`) and is not reproduced natively.

Validity codes differ by column (live, Linux captures): eye validity (7, 13)
is 0 when valid and 4 when not; every other validity column is 1 or 0.

### Platmod records (`receive_gaze_data` `0x140265d70`)

| Record | Subscriber slot | Contents |
| --- | --- | --- |
| Gaze point | `+0x11388` | timestamp, valid = column 27 != 0, column 28 |
| Responsive gaze point | `+0x11398` | timestamp, valid = column 31 != 0, column 32 |
| Eye openness | `+0x114e0` | valid = 42 != 0, 41, valid = 44 != 0, 43 |
| Gaze origin | `+0x113a8` | valid = 33 == 1, column 34, valid = 35 == 1, column 36 (display coordinates, mm) |
| Gaze data | `+0x113b8` | per eye: origin (22/21 == 1, column 2/8), relative position (29/30 == 1, 3/9), eye valid = 7/13 < 2 (unsigned), 3D point 4/10, 2D point 5/11, eyeball valid = eye valid when column 23/24 was present, eyeball 23/24, pupil valid = eye valid, pupil 6/12 |
| User position guide | `+0x11440` | valid = 38 == 1, column 37 clamped, valid = 40 == 1, column 39 clamped; clamp: below 0 becomes 0, above 1.0 (`0x141135d74`) becomes 1.0, NaN passes |

The first five are produced only when the device settings word `+0x188` is 0.
With it set, `receive_gaze_data` publishes only the user position guide, and
the head-pose result callback `0x140265420` publishes gaze point, responsive
gaze point and gaze data from the host algorithm instead; the gaze sync
function then logs "Gaze requested from host algobox, but intermediate data or
internal algo image is missing" unless intermediate data and the internal
image are enabled (static). The ET5 takes the device branch: the device refuses
the intermediate stream (`0x50c`, `PROTOCOL_UNKNOWN_ID`, live Linux capture),
and on Windows the gaze timestamps never coincide with head-pose (image)
timestamps (0 of 332 shared, median 9.6 ms apart, live (Windows)). Inference
from both.

### PRP streams

Service subscription `0x1400f7a50` and the stream name table `0x1400f79c0`
(static), statuses from the real service (live (Windows), no license keys):

| PRP stream | Name | ET5 runtime source | Real service answer |
| --- | --- | --- | --- |
| 2 | HEADPOSE | head-pose algorithm | status 0 |
| 3 | GAZE_POINT | gaze point record | status 0 |
| 4 | GAZE_ORIGIN | gaze origin record | status 0 |
| 5 | EYE_POSITION_NORMALIZED | `0x14026e730`, returns 3 | status 2 (not supported) |
| 6 | ADVANCED_GAZE | gaze data record | status 17 (insufficient license) |
| 29 | RESPONSIVE_GAZE_POINT | responsive record | status 0 |
| 33 | ABSOLUTE_EYE_OPENNESS | eye openness record | status 17 |
| compound 2 | USER_POSITION_GUIDE_XYZ | user position guide record, operation 11 | not tried |

The connect response lists streams 6, 3, 29, 4, 2, 7, 28, 1, 32. Eye position
normalized is therefore not available from an ET5 over PRP; the closest data is
the compound user position guide.

Stream Engine to PRP mapping in the game DLL (`0x18002c5d0`, static): stream 0
to 3, 1 to 4, 2 to 5, 4 to 2, 5 to 6, 7 to 22, 11 to 19, 12 to 29; compound
map `0x18002c660`: 8 to 2, 9 to 3, 10 to 4.

### PRP records and bodies

Service records (emulation) and the bodies the game DLL serializer
`0x180052e60` writes (emulation, version 0 and `0x7fffffff`):

| Stream | Native record | Body |
| --- | --- | --- |
| 3, 29 | timestamp, bool at +8, x, y | 24 bytes, see [game-interface.md](game-interface.md) |
| 4, compound 2 | timestamp, bool, xyz, bool, xyz | 44 bytes: size, time high, time low, left valid, left xyz, right valid, right xyz |
| 6 | timestamp, two 0x50-byte eyes | 164 bytes, or 172 for a peer version above 0 with both relative-position validities appended; per eye 19 words: origin valid, origin, relative position, eye valid, 3D point, 2D point, eyeball valid, eyeball, pupil valid, pupil |
| 33 | timestamp, bool, float, bool, float | no serializer case in this DLL |

Packets use the head-pose framing with operation 1 (11 for compound streams)
and message tag 0: 72 bytes for 3 and 29, 92 for 4. The real service sent one
packet per pipe message, 33.1 Hz, streams 3, 29 and 4 sharing one timestamp
per device frame (live (Windows), `evidence/private/gaze-host/gaze-probe-fou1.jsonl`,
1325 packages in 10 s). Invalid points carry the device's values: gaze point
`(-1, -1)` and gaze origin zeros, as in the Linux captures.

### Timestamps

The runtime passes the device timestamp (column 1, microseconds) through.
The real service's gaze packets carry the device clock, and its timesync
answers on the same clock: Stream Engine timestamps were service time minus
828166710697 us for all 144 matched points; the service's packets read about
900357 to 900402 s, and Linux captures of the same device taken minutes
earlier read 898734 to 900013 s in column 1 (live). The Linux service keeps its Wine-rate
clock and moves device timestamps onto it as head pose does
(`driver.ClockMapper`, `GazeFrame.with_timestamp`).

### User presence

`receive_presence_data` `0x140266b60` keeps device values 0, 1 and 2 of stream
`0x504` column 2 (an int32) and turns anything else into 0; service callback
`0x1400fa2b0` builds a property 0xB record with operation 2 and value 1, 2 or
3 (unknown, away, present), and `0x1400fd220` queues every record after the
first as a notification (static, emulation). The client maps 2 to away and 3
to present ([game-integration-layer.md](game-integration-layer.md)). The whole
path, the earlier "device reports 0" (a parsing error on Linux) and the Linux
service's notifications are in [service-behaviour.md](service-behaviour.md).

## Game side

- The game integration layer subscribes, per request bit of its device thread
  `0x180009060` (static): bits 4, 8, 16 gaze point (Stream Engine 0, PRP 3,
  `0x18000f920`); bit 0x80 responsive gaze point (12, PRP 29, `0x180010730`),
  and gaze data (5, PRP 6, `0x180015b20`) only when the responsive subscribe
  fails. `GetGazePoints` and `GetLatestGazePoint` set bit 8 (`0x1801cc100`,
  `0x1801cc430`).
- Its gaze point callback `0x18000a0a0` drops invalid points, NaN and any
  point 0.75 or more from 0.5 on either axis (`0x18025da60`, `0x18025da70`).
- Under Wine the layer subscribed streams 2 and 3 and nothing else
  (`evidence/private/gaze-host/wine/tgi-service.log`, live (Wine)).
- TrackerInfo capabilities read 0x9b with the gaze streams listed, the same as
  against the real service; 0x3 with head pose only (live).

## Linux implementation

`src/tobii_linux/gaze.py`:

- `decode_gaze_message` mirrors parser and dispatcher (None when the runtime
  drops the message); `platmod_records`, `prp_record`, `stream_packet` mirror
  the three later stages; `gaze_frame` bundles one message's outputs.
- `recorded_gaze_messages`, `convert_messages` and `python3 -m tobii_linux.gaze
  <capture.jsonl>` convert a capture from `tools/linux_stream_capture.py` or
  `tools/gaze_device_capture.py` into the published outputs, one JSON line per
  device message.
- `platmod_presence`, `prp_presence`, `presence_message` for stream `0x504`.

`src/tobii_linux/prp_service.py`: `GazeHub` (fan-out of `GazeFrame`),
`PrpService(..., gaze_hub=...)` serves streams 3, 29 and 4, answers 6 and 33
with status 17 and others with 2, and lists 6, 3, 29, 4, 2 in the connect
response. Without a gaze hub it behaves as before. `--synthetic` publishes a
gaze figure eight beside the head sweep (`--no-gaze` turns it off).

`tobii_linux.cli serve` feeds live gaze: `streams.GazeAndImageStreams`
subscribes `0x500` beside the image stream in the same session and publishes
`gaze.gaze_frame(gaze.gaze_message(message)).with_timestamp(host_time)` for every
message the runtime would keep, with device time mapped by its own
`driver.ClockMapper`; head pose keeps its own mapper and newest-frame dropping.
`cli serve --no-gaze` serves head pose only. Presence (`0x504`) is subscribed
in every session and feeds property 0xB ([service-behaviour.md](service-behaviour.md)).

## Checks

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/gaze_oracle.py
PYTHONPATH=.local/inference:src python3 -m unittest tests.test_gaze tests.test_prp_service
python3 tools/gaze_target_check.py --windows GAZE TARGETS --linux CAPTURE TARGETS
```

`tools/gaze_oracle.py` (report `evidence/private/gaze-oracle.json`) feeds raw
TTP messages to the original parser, dispatcher and `receive_gaze_data` with
recording subscribers, each platmod record to the original service callback
with the queue calls recorded, and each queued record to the game DLL's packet
serializer `0x18004f730`. It selects every validity pattern plus evenly spaced
messages from four Linux captures (`gaze-capture`, `gaze-baseline`,
`gaze-device` twice): 96 messages, all records and packets byte-exact. Edge
cases: validity codes 1, 2, 3, 7, clamping below 0 and above 1, missing
eyeball columns, fixed-point rounding, a known column with the wrong type and
an int32 validity (both dropped by the parser), missing columns, an unknown
column, the host gaze setting, and the presence mapping. `tests/test_gaze.py`
holds one message's original-code vectors and five real-service packets.

### Real service

`tools/prp_real_service_probe.py --stream N` starts and records any streams.
Runs on albedo (`evidence/private/gaze-host/`): `gaze-probe-1.jsonl` (3 s),
`gaze-probe-fou1.jsonl` (10 s, field_of_use 1), `pair-prp.jsonl` with
`pair-se.jsonl` (PRP and Stream Engine at the same time), `pair-se-client.txt`
with `pair-tgi-client.txt` (Stream Engine and game layer at the same time).
Stream start, stop and refusal responses for streams 3, 29, 4, 6, 5, 33 and 2
are byte-identical to the Linux service's (unit).

### Wine

`tools/prp_bridge/prp_test_client.c` now also subscribes gaze point
(`tobii_gaze_point_subscribe`, game DLL RVA `0xf920`/`0xf9b0`) and
`tgi_test_client.c` reads `GetGazePoints`. Patched Wine 11.0, synthetic
service, `PRP_URL=discover` (live (Wine), `evidence/private/gaze-host/wine/`):

| Client | Result |
| --- | --- |
| game DLL embedded Stream Engine, 5 s | 166 head poses, 166 valid gaze points, callback 1.0 to 6.3 ms after the timestamp, median 3.5 |
| `tobii_stream_engine.dll`, 5 s | 165 head poses, 165 valid gaze points, median 3.6 ms |
| game integration layer, `TrackTracker`, 10 s | 330 head poses, 330 gaze points, read 1.2 to 17.5 ms after the timestamp, median 9.5 (16 ms polling) |
| Stream Engine and game layer at once (`wine2/`) | all 197 layer points equal `(2x - 1, 1 - 2y)` of the Stream Engine point, with and without `--display-area` |

## Accuracy against targets

`tools/gaze_target_check.py` scores the published gaze point (Linux: captures
through `tobii_linux.gaze`, device time mapped with one fitted offset; Windows:
Stream Engine points, no fit), median pixel error per target from 0.8 s after
it appears, 2560 x 1440 display (`evidence/private/gaze-host/target-check.jsonl`):

| Run | Median | Range over 8 targets |
| --- | --- | --- |
| Windows, `gaze-baseline/windows-gaze.jsonl` | 52.0 px | 17.6 to 76.6 |
| Linux, `gaze-baseline/linux-gaze.jsonl` | 64.6 px | 36.7 to 102.7 |
| Linux, `gaze-device/linux-gaze.jsonl` | 47.7 px | 32.7 to 104.9 |
| Linux, `gaze-device/linux-gaze-image.jsonl` | 51.3 px | 19.3 to 102.6 |

The fitted offsets (-828166.61 s, -828166.22 s, -828166.22 s) are within 0.5 s
of the offset Windows timesync measured; with that measured offset fixed the
medians are 65.7, 48.9 and 51.8 px. The published point is column 28, so these
equal the raw column 28 figures; any remaining gap to Windows is in the device.

## Open

- Whether licensed clients get streams 6 and 33 from the real service; the
  game layer requests 6 only when 29 fails.
- A real operation 2 presence notification for an actual change; the framing
  is known (a real operation 2 for property 0x11 and the emulated serializer,
  [service-behaviour.md](service-behaviour.md)).
- Live gaze through `cli serve` with the game: wired and unit tested, not yet
  run against a game under Wine with a real tracker.
- What sets the device settings word `+0x188`; the ET5 behaviour is inferred
  from the two observations above, not from the setter.
- Columns 14, 17 and 20 and the parser's column capacity.
