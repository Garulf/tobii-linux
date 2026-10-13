# TTP startup, stream subscription and camera frames

Status: static reconstruction plus offline emulation of request builders. No
request in this document has been sent to an ET5, and no stream message has been
received. Target: `platform_runtime_IS5LEYETRACKER5_service.exe`, SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`, image base
`0x140000000`. It builds on [the protocol notes](usb-protocol.md) and
[the transport report](usb-transport-static.md).

Evidence labels used below:

- **static**: objdump disassembly or focused Ghidra `-noanalysis` decompilation
  (`evidence/private/c-incoming-focused.c`, `c-decode-focused.c`,
  `c-stream-focused.c`, `c-frame-focused.c`).
- **emulated**: original function executed in Unicorn by
  `tools/ttp_commands_oracle.py` with synthetic inputs
  (`evidence/private/ttp-commands-oracle.json`).
- **inferred**: a reading of names, shapes or call order that the code does not
  state directly.

## Message kinds

The TTP parser `0x1402cb510` maps header word 0 to a message class (static):

| Word 0 | ASCII | Class | Handling in `process_incoming_data` (`0x1402be450`) |
|---|---|---|---|
| `0x51` | `Q` | 1 | Request; ignored when received |
| `0x52` | `R` | 2 | Response; copied and handed to the waiter in `0x1402bed50` |
| `0x53` | `S` | 3 | Stream data; decoded by `0x1402ce2a0`, dispatched by `0x1402c2700` |
| `0x4e` | `N` | 6 | Notification; word 3 mapped to a type, dispatched by `0x1402c1920` to configuration slot `+0x168` |
| `0x4c` | `L` | 4 | Ignored |
| `0x43` | `C` | 5 | Ignored |

Word 4 is a status: when nonzero the parser logs and maps it (codes
`0x20000402`..`0x20000516`) instead of decoding the body. For stream data,
word 3 is the wire stream id. Word 1 is copied to the decoded message but is
not used by the stream dispatcher. Notification types: word 3 `0x410`->1,
`0x41a`->2, `0x4d8`->4, `0x4e2`->5, `0x4ec`->6, `0x4f6`->7, `0x4f7`->8,
`0x4f8`->9, `0x582`->10, `0x5aa`->11, `0x668`->12, `0xbcc`->13, `0xbd6`->14,
`0xc26`->15, `0x690`->16, `0xc94`->17, `0xc6c`->18, `0xc80`->19, `0xc8a`->20.
Their meanings are not established.

## Field encoding

Request builders write fields through thunks `0x1403afc00..0x1403afd50`
(static; every encoder below is also emulated). All integers are big-endian.

| Tag | Data after the 4-byte length | Writer |
|---|---|---|
| `0x01` | int32 | `0x1403b0d10` |
| `0x02` | uint32 | `0x1403b0e00` |
| `0x03` | float32 times 65536, truncated to int32 (`cvttss2si`; out of range gives `0x80000000`) | `0x1403b0bd0` |
| `0x04` | float64 times 2^42, truncated to int64 | `0x1403b0c10` |
| `0x05` | struct header: uint32 `member_count << 16 | type`; members follow as sibling fields | `0x1403b0cd0` |
| `0x06` | int64 | `0x1403b0d50` |
| `0x14` | string: uint32 byte count, bytes, no NUL | `0x1403b0d90` |
| `0x15` | blob: uint32 byte count, bytes | `0x1403b0b40` |
| `0x17` | uint32 array: count, elements | `0x1403b0e40` |
| `0x19` | float64 array, each times 2^42 as int64 | `0x1403b0c50` |

Readers mirror this: the struct reader `0x1403b0700` requires length 4, the
nested skipper `0x1403b1f10` consumes `(code >> 16) & 0xfff` members
recursively, and the blob reader `0x1403b0350` uses the inner count.

The generic builder `0x1402c3bc0(request_id, buffer, capacity, schema, command)`
only writes a 24-byte header. Its fourth argument is not field data: when
nonnull it receives the expected reply schema from `0x1402c3c50` (count,
reply item ids, parsed item types). Commands with fields have dedicated builders.

## Startup sequence as far as established

1. **Tracker initialize** (vtable slot `0x8`, `0x1402bc2b0`, static). After it
   creates `tracker_thread` and a transport state check (`0x1403b2f00` result
   words 2 and 0) passes, it sends protocol upgrade 1000, then `unlock_ppu`
   1600, then calls an optional caller callback. If either request fails it
   calls slot `0x10` (`0x1402bff80`) and returns the error.
2. **`unlock_ppu`** (`0x1402c0810`, static). Header-only request 1600. The reply
   must hold three items of parsed type 1 (schema ids 11, 12, 13 from the
   emulated schema; mapping ids to reply positions is inferred). If the first
   item is nonzero and the third is zero, it calls `open_realm_salt(realm =
   second item, key = "aaaaaaaabbbbbbbbccccccccdddddddd", 32 bytes, no extra
   data)`, i.e. commands 1900 then 1910. The key string at
   `0x1412ba7f8` has no other code reference. What the three items mean is
   inferred only from this use.
3. **Head-pose device start** (`0x140261ca0`, static): realm `0x2712`, hardware
   info, close realm. See [the 0x2712 call](#the-0x2712-realm-call).
4. **Stream subscription** (static). The device's stream synchroniser
   `0x140278150` compares a 15-byte set of wanted streams with the current
   state and calls the matching subscribe or unsubscribe wrapper. Flag 9 maps
   to the image stream (slots `0x78`/`0x80`, stream index 2), flag 8 to the
   primary camera image (slots `0x88`/`0x90`, index 17). The wanted set comes
   from a virtual call (`[r14+0x10]` in `0x1402784a0`/`0x1402785f0`); which
   component sets flag 9 for head pose is not traced.
5. **Receive** kind `0x53` messages with word 3 = `0x501` and parse the column
   set below; the dispatcher calls `receive_image` (`0x140266430`).

A minimal Linux sequence suggested by this (not validated on hardware): upgrade
(1000), unlock_ppu (1600, plus a realm open if its reply asks for it), optionally
realm `0x2712` + hardware info (2120) + close realm, then subscribe 1220 with
wire stream `0x501` and read `S` messages.

## Command table

Commands relevant to startup and streaming. Slot is the offset in tracker
vtable `0x1410af508` (126 slots). Names come from the runtime's own log
strings `tracker_usb_ttp_t::<name>`; slots without a log string are marked
inferred. All builders listed are emulated byte-for-byte by the oracle.

| Command | Slot | Function | Builder | Request fields | Reply items (id: parsed type) |
|---|---|---|---|---|---|
| 1000 | (from initialize) | `protocol_upgrade` `0x1402be6d0` | `0x1402c6e40` | uint32 array of versions | 0: 1 |
| 1200 | `0x48` | `enumerate_stream_types` | `0x1402c7190` | none | 3: 14 |
| 1210 | `0x50` | `enumerate_stream_type_columns` | `0x1402c70b0` | uint32 wire stream id | 4: 15 |
| 1220 | stream pairs | subscribe `0x1402c06a0` | `0x1402c71b0` | uint32 wire stream id, uint32 array of columns (empty from every slot) | none |
| 1230 | stream pairs | unsubscribe `0x1402c0760` | `0x1402c72a0` | uint32 wire stream id | none |
| 1330 | `0x20` | `get_device_properties` | `0x1402c53e0` | none | 45: 21 |
| 1350/1351 | `0x30`/`0x38` | `get_capabilities`/`_all` | `0x1402c5bd0`/`0x1402c5bf0` | none | 51: 22 |
| 1420 | `0x18` | `get_device_info` | `0x1402c6250` | none | 7..10: 7 |
| 1490 | `0x28` | `get_device_status` | `0x1402c6230` | none | 46: 21 |
| 1600 | (from initialize) | `unlock_ppu` | `0x1402c6210` | none | 11, 12, 13: 1 |
| 1610 | `0x250` | `get_output_frequency` | `0x1402c5990` | none | 23: 5 |
| 1620 | `0x248` | set output frequency (inferred, no log name) | `0x1402c59b0` | tag 3 fixed-point float | none |
| 1630 | `0x240` | `enumerate_output_frequencies` | `0x1402c5970` | none | 24: 6 |
| 1650 | `0x268` | `get_frequencies` | `0x1402c5aa0` | none | 47: 23 |
| 1660 | `0x270`, `0x278` | unnamed / `set_frequencies_output` | `0x1402c5ac0` | struct `0x2af8` with two uint32 members | none |
| 1670 | `0x258`, `0x260` | `enumerate_frequencies`, `get_frequencies_count` | `0x1402c5a80` | none | 48: 24 |
| 1900 | `0x390` | `open_realm_salt` step 1 | `0x1402c4600` | uint32 realm, uint32 array `[0]` | 28: 1, 29: 1, 30: 13 |
| 1910 | `0x390` | `open_realm_salt` step 2 | `0x1402c47f0` | uint32 item 28, uint32 item 29, blob digest (16) | none |
| 1911 | `0x390` | step 2 with extra data | `0x1402c48f0` | as 1910 plus blob extra | none |
| 1915 | `0x398` | `close_realm` | `0x1402c4730` | uint32 realm | none |
| 2010/2020/2030 | `0x2a0`/`0x2a8`/`0x2b0` | get/set/enumerate illumination mode | `0x1402c56c0`/`0x1402c56e0`/`0x1402c56a0` | set: string | get 34: 7, enumerate 33: 8 |
| 2120 | `0x2c0` | `hardware_configuration_get` | `0x1402c5c10` | none | 54: 27, 55: 12, 56: 2 |
| 3000/3010 | `0x330`/`0x338` | power save activate/deactivate | `0x1402c6c40`/`0x1402c6c60` | none | none |
| 3100 | `0x350`/`0x358` | `pause_device`/`resume_device` | `0x1402c6d80` | uint32 1 (pause) or 0 (resume) | none |

Header-only getters for track box (1400), display area (1430), device name
(1700), extensions (1810/1820), low blink mode (1920), geometry mounting (2110)
and enabled eye (3170) are also encoded and emulated. The full builder map
(about 75 builders, including calibration and persistent files) was produced
by scanning callers of `0x1402c3bc0` and the header writer `0x1403afc40`.
"Parsed type" is the runtime's decoded item type; known correspondences:
1 = uint32 (tag 2), 4 = int64 (tag 6), 13 = blob (tag 0x15). The wire layout
of replies is not reconstructed here.

## Streams

Each stream index has a subscribe/unsubscribe slot pair whose thunk loads the
index into EDX and jumps to `0x1402c06a0`/`0x1402c0760`. The wire id comes from
the jump table in `0x1402c4500` (static, emulated through every subscribe case).
The decoder `0x1402ce2a0` maps wire ids back to the dispatcher index.

| Index | Slots | Wire id | Dispatcher target (configuration offset, callback) |
|---|---|---|---|
| 1 | `0x58`/`0x60` | `0x500` | `0x1402c1c30` -> `+0x110` `receive_gaze_data` |
| 2 | `0x78`/`0x80` | `0x501` | `+0x128` `receive_image` (`0x140266430`) |
| 3 | `0x98`/`0xa0` | `0x502` | `+0x138` (not set by the castor device) |
| 4 | none found | `0x503` | none in the dispatcher |
| 5 | `0x68`/`0x70` | `0x504` | `+0x120` `receive_presence_data` |
| 6 | `0xa8`/`0xb0` | `0x505` | `0x1402c3400` -> `+0x140` `receive_wearable` |
| 7 | none found | `0x50a` | none |
| 8 | `0x138`/`0x140` | `0x1770` | `+0x188` `receive_algo_dbg_data` |
| 9 | `0x3c8`/`0x3d0` | `0x1772` | `+0x198` `receive_device_log` |
| 10 | `0xb8`/`0xc0` | `0x506` | `+0x148` (not set by the castor device) |
| 11, 19 | `0xc8`/`0xd0`, `0xd8`/`0xe0` | `0x50d`, `0x511` | `0x1402c3950` (loads `+0x150` `receive_diagnostics_image` and `+0x158`) |
| 12 | `0xe8`/`0xf0` | `0x507` | `+0x160` |
| 13, 15, 18 | `0x108`/`0x130`, `0x110`/`0x118`, `0x120`/`0x128` | `0x508`, `0x50b`, `0x50f` | `0x1402c22b0` image collection, see below |
| 14 | `0x158`/`0x160` | `0x509` | `+0x1a0` `receive_wearable_foveated` |
| 16 | `0x3b8`/`0x3c0` | `0x50c` | `+0x1a8` `receive_intermediate_algo_data` |
| 17 | `0x88`/`0x90` | `0x50e` | `+0x130` `receive_primary_camera_image` (`0x140266d50`) |
| 20 | `0x148`/`0x150` | `0x1774` | `+0x190` `receive_custom_data` |
| 21 | `0x168`/`0x170` | `0x1771` | `+0x118` `receive_is5_sync_data` |

The configuration offsets are those of the 0x1b0-byte block that
`0x140260380` fills and passes to the tracker constructor wrapper
`0x1402b2de0`; the dispatcher receives the copy at tracker `+0x78`. Callback
names are the runtime's log strings in each callback. Slot pairs for indices
3, 10, 11, 12, 19 come from thunk scanning only.

## Stream data layout

Stream payloads use the common prefix and fields (static,
`0x1403b1b90`, `0x1403b1b40`, `0x1402cf6a0`):

```
00 00                                   payload prefix
05 00000004 (N << 16 | 0x0bb8)          column set, N columns
repeat N times:
  05 00000004 00020bb9                  column header, two members
  02 00000004 <column id>
  <value field>                          may be a struct with nested members
```

Unknown column ids are skipped by the runtime decoder. `frames.stream_columns`
parses this generically; `frames.image_frame` and
`frames.primary_camera_frame` decode the two image streams and reject
duplicates, missing columns and type mismatches (stricter than the runtime).

### Image stream `0x501` (index 2)

| Column | Field | Delivered to `receive_image` at | Meaning |
|---|---|---|---|
| 1 | int64 (tag 6) | `+0x00` | timestamp; microseconds is inferred from the head-pose filter's 1e6/dt rate |
| 2 | uint32 | `+0x08` | pixel format; `receive_image` requires 8 |
| 3 | uint32 | `+0x0c` | width (also passed as stride to the public image subscriber) |
| 4 | uint32 | `+0x10` | height |
| 5 | blob | `+0x18` byte count, pixel pointer as third argument | pixels; count must equal width*height |

`receive_image` copies these into a public image record for the subscriber at
device `+0x114b0`, and when the head-pose algorithm is enabled forwards the
original pixel pointer to `deploy_image` (`0x1402857d0`).

### Primary camera stream `0x50e` (index 17)

Columns 1 int64 timestamp, 2..5 uint32 at `+0x08`, `+0x0c`, `+0x10`, `+0x14`,
6 blob. `receive_primary_camera_image` requires blob size = `+0x10` * `+0x14`;
the head-pose wrapper `0x140285be0` additionally requires `+0x10` = `+0x0c` and
deploys width `+0x0c`, height `+0x14`. Hence `+0x0c` width, `+0x10` stride,
`+0x14` height (inferred names); column 2 has no observed check.

### Image collection streams `0x508`/`0x50b`/`0x50f` (partial)

`0x1402c22b0` reads columns 1 (int64), 2..7 and 9..13 (uint32), 8 (up to 10
records of nine 32-bit words, parsed type 26), 14 (up to 64 records of four
words, parsed type 25) and 15 (blob), then calls `image_collection_receive`
(`+0x180`). If configuration `+0x100` is set, that callback returns 1 and
column 8 holds exactly one record, it also calls `receive_image` with format
from column 2, width record word 5, height record word 6, size width*height.
So head pose can be fed either by stream `0x501` or by an image collection.
Configuration `+0x100` is `device[+0x18c] == 1` in `0x140260380`; which path an
ET5 uses is not established. The wire forms of columns 8 and 14
(`0x1403af980`, `0x1403af9e0`, `0x1403af970`) are not decoded.

## From `deploy_image` to the algorithm

- `0x1402857d0` (static): rejects width*height = 0 or a null pixel pointer,
  copies width*height bytes, then calls `0x140287010(frame, info, copied
  pixels, counter)` with a per-algorithm frame counter (`+0x3df8`,
  post-incremented).
- `0x140287010` fills a zeroed 0x910-byte frame (static): `+0x08` byte 1,
  `+0x10`/`+0x20`/`+0x28` timestamp, `+0x18` frame counter, `+0x34` 0,
  `+0x19c` float 1.0, `+0x1a0` 1, `+0x1a6` byte 1, `+0x1a8` 1, `+0x1b0`
  width (stride), `+0x1b8` pixel pointer, `+0x1c4` uint16 width, `+0x1c6`
  uint16 height, `+0x210` bytes 01 01 01 01, `+0x214` byte 8, `+0x8e8` byte
  1. The pixel format is not copied; `+0x214` is a constant 8 (bits per pixel
  is inferred).
- The "type 0..5" switched on by `0x1400e7040` is the first word of the third
  argument, a settings block copied from algorithm `+0x4928`. `deploy_image`
  writes that word as 0, so camera frames always take type 0 (wrapper
  `0x1400e67c0`, pipeline virtual `+0xa0`). Types 1, 2, 3 and 4/5 select
  `0x1400e5e40` (`+0xb0`), `0x1400e6180` (`+0xa8`), `0x1400e6ae0` and
  `0x1400e64e0` (flag type==4); their producers and meanings are not traced.

## The 0x2712 realm call

`open_realm_salt` (slot `0x390`, `0x1402bd400`) is a challenge response
(static; digest emulated):

1. Send 1900 with the realm id and the array `[0]`; wait up to 3,000,000 units
   in `0x1402bed50`; require zero status and reply item types (1, 1, 13).
2. `0x1402c0ef0(salt, salt_len, key, key_len, out)` computes HMAC with 64-byte
   blocks, 0x36/0x5c pads, MD5 initial constants and a 16-byte result. The
   oracle ran it on four synthetic key/salt pairs (including a key longer than
   64 bytes) and all matched Python HMAC-MD5.
3. Send 1911 (with extra data) or 1910 (without) holding reply items 28 and 29,
   the digest, and the extra blob, through `0x1402bec80`.

`close_realm` (slot `0x398`, `0x1402b5410`) sends 1915 with the realm id.

In the head-pose start routine `0x140261ca0` the call is
`open_realm_salt(0x2712, key = 16 bytes at 0x1412b8e38, extra = 4 bytes at
0x1412b8e34)`. Only when it succeeds does the routine request hardware info
(2120); it then calls `close_realm(0x2712)`. The two constants live in `.data`
and have no other code reference; their bytes are read from the analysed binary
and are not reproduced here. `platmod_internal_command_feature_unlock`
(`0x14026fd80`, `0x14026fa50`) also opens and closes realms `0x2712` or `0x3e9`
with caller-supplied keys, so a realm appears to gate optional features
(inferred). Whether the device refuses hardware info without the realm is
unknown.

`ttp_commands.open_realm_salt`, `realm_digest`, `open_realm_response` and
`close_realm` implement the requests; the caller supplies key and extra data.

## Reproduction

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/ttp_commands_oracle.py
PYTHONPATH=.local/inference:src python3 -m unittest tests.test_ttp_commands tests.test_frames
```

The oracle reports 260 builder cases (24 header-only commands at four request
ids, all 21 streams for subscribe, unsubscribe and column enumeration, realm,
pause, illumination, frequency cases), 4 realm digest cases and 36 reply
schemas; all builder and digest cases match. Its stubs are those of
`tools/vsmat_oracle.py`; inputs are synthetic.

## Gaps

- Nothing here is validated against device traffic; acceptance of every request
  is unverified.
- Which USB channel carries TTP, and whether stream data needs anything beyond
  1220 (for example a frequency or illumination setting) is not established.
- The component that sets the image-stream flag for head pose, and whether an
  ET5 delivers frames on `0x501` or as an image collection.
- Reply wire layout (`0x1402cc1c0`) and the meaning of most reply item ids.
- Meaning of image column 2 values other than 8, of primary camera column 2,
  of notification types, and of `unlock_ppu` reply items.
- Settings types 1..5 of `0x1400e7040`.

## Live observations (WSL2 on the Windows reference)

First run of the Linux session against the real tracker on 2026-10-13, with
the device passed to the reference host's WSL2 Ubuntu through usbipd-win
(Tobii services stopped, then restored and checked afterwards). Logs and frames
are in `evidence/private/live-wsl/`; `tools/linux_session_probe.py` reproduces
the steps.

- Channel 0 carries TTP. Protocol upgrade 1000 is answered with response kind
  `0x52`, word 2 = 1, status 0 and uint32 `0x10008`.
- `unlock_ppu` 1600 returns three uint32 zeros, so no unlock realm is opened.
- Realm `0x2712`: the salt reply is uint32 `0x2712`, uint32 0 and a 256-byte
  blob. Command 1911 with the HMAC-MD5 digest and the 4 extra bytes is accepted;
  hardware info 2120 then returns a 0x40f-byte reply and close 1915 succeeds.
- Hardware info, camera 0: 2240x2240 pixels, two values of 1.75e-3 (pixel
  pitch in mm, inferred), 5.22 (focal length in mm, inferred), translation
  (0, 0, 4.14), zero angles, header values 16 and 100. Camera 1 is empty apart
  from header values 62 and 100. Points: (0, 0, 0) and (+-130, 0.76, 1.62)
  (illuminators, inferred). 5.22 / (2240 * 0.00175 / 2) = 2.66, close to the
  configured `camera_focus 2.6`.
- Subscribe 1220 to `0x501` is accepted and stream messages `0x53` follow:
  280x280 8-bit frames (the sensor binned by 8, inferred) every 30.2 ms
  (about 33 Hz), microsecond timestamps. Unsubscribe 1230 is accepted.
- With nobody in front of the tracker the frames show the room at levels 8 to
  20, with or without a gaze subscription. Illumination commands 2010/2030 and
  output-frequency commands 1610/1630 are refused (unsupported or unknown
  operation). The Windows runtime reported no valid head pose at the same time.
- The device enumerates streams gaze `0x500`, image `0x501`, presence `0x504`,
  image collection `0x508`, primary camera image `0x50e` and others.

Not yet observed: frames with a person present, and therefore any live head
pose from the Linux pipeline.
