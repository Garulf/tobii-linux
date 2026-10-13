# TTP responses and the camera hardware-info reply

Status: the response receive path, the typed payload encoding and the
hardware-info (command `0x848`) reply layout are reconstructed. They are
verified by offline emulation of the original parser on synthetic vectors. No
device response has been observed, so device acceptance and real field values
remain unknown.

Addresses refer to `platform_runtime_IS5LEYETRACKER5_service.exe`, SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`, image base
`0x140000000`. Evidence labels used below:

- **static**: objdump disassembly or focused Ghidra `-noanalysis` output
  (`evidence/private/a-recv-focused.c`, `a-fields-focused.c`,
  `a-readers-focused.c`, `tracker-hwinfo-focused.c`);
- **emulation**: `tools/ttp_reply_oracle.py` running original functions in
  Unicorn (`evidence/private/ttp-reply-oracle.json`).

Nothing here is live observation.

## Receive path (static)

| Address | Role |
| --- | --- |
| `0x1402c0390` | Tracker worker. Pops queue `tracker+0x5df8`; a type-1 record (bytes at `+0x10`, length at `+8`) goes to `0x1402be450`. |
| `0x1402be450` | Feeds the bytes to the parser at `tracker+0x680` (`0x1402cf8c0`), then calls `0x1402cfde0` until it returns 1 (need more data). |
| `0x1402cfa50` | Parser setup. The tracker passes a block at `tracker+0x688` of `0x5740` bytes; `0x1403af550` caps payloads at `0x40000000`; the field arena is the block after `+0x4c0` (`0x5280` bytes). |
| `0x1403af650` | Incremental framing: 24-byte header, words stored at parser `+0x460..+0x474`, payload into the reader at `+0x440` (inline up to `0x400` bytes, allocated above). Returns 0 need more, 1 complete, 2 nonzero payload prefix, 3 payload over the cap (skipped). |
| `0x1402cb510` | `parse_message`. Maps word 0 to a parser kind, word 1 to the request id, maps a nonzero word 4 to a parser status (no fields decoded), and dispatches kind 2 to `parse_response`. |
| `0x1402cc1c0` | `parse_response`. Field types come from a per-command schema, not from the wire. |
| `0x1402c3c50` | Schema lookup by command (header word 3). |
| `0x1402c1390` | Deep-copies each 0x208-byte field into the queued record. |
| `0x1403b2ef0` | Pushes the record to `tracker+0x5e00`; `0x1402bed50` pops it with `0x1403b2f00` / `0x1403b3130`. |

The function names `parse_message` and `parse_response` and the status names
are strings referenced at those sites (`ttp_parser.cpp`). Kind 3 (`0x53`) goes
to `0x1402ce2a0` and kind 6 (`0x4e`) to a command switch inside `parse_message`;
neither is reconstructed here.

The parser message is 32 bytes: request id `+0`, kind `+4`, a subtype `+0xc`
(set only for kind 6), field count `+0x10`, field array `+0x18`. The queued
reply record (`0x1402be5a6`) has type 3 at `+0`, the parser status at `+0x20`
and a copy of the 32-byte message at `+0x28`. That yields the offsets
`0x1402bb550` uses: request id `+0x28`, count `+0x38`, fields `+0x40`.
`0x1402be450` logs and drops parses with status 2 to 6. A waiting request
then times out. Status 0 and statuses 7 and above are queued.

### Header words (static, emulation)

| Word 0 | Parser kind |
| --- | --- |
| `0x51` | 1 (the host's own request kind; a received one with word 4 zero gets status 2) |
| `0x52` | 2, response |
| `0x53` | 3 |
| `0x4c` | 4 |
| `0x43` | 5 |
| `0x4e` | 6 |
| other | 0, refused |

Word 4 of a response is a device error word. Zero means success. Otherwise,
the parser returns one of the `TTP_PARSER_ERROR_*` statuses (string table at
`0x1402c7578`):

| Word 4 | Status |
| --- | --- |
| `0x20000402` | 7 `PROTOCOL_LEGACY_BAD_FORMAT` |
| `0x2000040a` | 8 `PROTOCOL_LEGACY_MISSING_UPGRADE` |
| `0x20000500` | 9 `PROTOCOL_UNKNOWN_OPERATION` |
| `0x20000501` | 10 `PROTOCOL_UNSUPPORTED_OPERATION` |
| `0x20000407`, `0x20000502`, `0x20000511..0x20000514` | 11 `PROTOCOL_OPERATION_FAILED` |
| `0x20000503..0x20000509` | 12 to 18 (`INVALID_PAYLOAD`, `UNKNOWN_ID`, `UNAUTHORIZED`, `EXTENSION_REQUIRED`, `INTERNAL_ERROR`, `STATE_ERROR`, `INVALID_PARAMETER`) |
| `0x20000510` | 19 `PROTOCOL_OPERATION_ABORTED` |
| `0x20000515` | 20 `PROTOCOL_NOT_ENABLED` |
| `0x20000516` | 21 `PROTOCOL_TOO_MANY_SUBSCRIBERS` |
| anything else | 2 `UNKNOWN` (dropped by the tracker) |

## Payload items (static, emulation)

A payload starts with two zero bytes. Then come items in the existing
`ttp.Field` form: a wire-type byte, a big-endian u32 length, and data. Readers
are at `0x1403b0350..0x1403b1c70`; the matching writers are at
`0x1403b0b40..0x1403b1dc0`. All integers are big-endian.

| Wire | Data | Reader / writer | Decoded as |
| --- | --- | --- | --- |
| `0x01` | 4 | `0x1403b0780` / `0x1403b0d10` | signed int32 (writer uses arithmetic shifts) |
| `0x02` | 4 | `0x1403b0a40` / `0x1403b0e00` | uint32 |
| `0x03` | 4 | `0x1403b0400` / `0x1403b0bd0` | signed int32 converted to float32, times 2^-16 (`0x14111a48c`) |
| `0x04` | 8 | `0x1403b0580` / `0x1403b0c10` | signed int64 converted to double, times 2^-42 (`0x14111a490`) |
| `0x05` | 4 | `0x1403b0700` / `0x1403b0cd0` | structure header `item_count << 16 \| struct_id` |
| `0x06` | 8 | `0x1403b0800` / `0x1403b0d50` | signed int64 |
| `0x07` | 8 | `0x1403b0ac0` / none found | 64-bit; signedness unknown |
| `0x14` | `4+n` | `0x1403b0880` / `0x1403b0d90` | u32 `n`, then `n` bytes, string |
| `0x15` | `4+n` | `0x1403b0350` / `0x1403b0b40` | u32 `n`, then `n` bytes, blob |
| `0x17` | `4+4n` | none found / `0x1403b0e40` | u32 count, uint32s (request side only) |
| `0x18` | `4+4n` | `0x1403b0480` / none found | u32 count, wire-3 values; the caller's limit is kept, the rest read and dropped |
| `0x19` | `4+8n` | `0x1403b0600` / `0x1403b0c50` | u32 count, wire-4 values; same limit rule |
| `0x1a` | | `0x1403b0940` / none found | u32 count (above the limit is an error), then per string a u32 length and bytes |

Fixed-length items require the exact length (4 or 8). For variable-length
items, the readers skip the outer length and trust the inner count. The writers
always set the outer length consistently. `ttp_fields.value` refuses a
mismatch rather than follow the runtime's choice. The writers convert floats
with truncation (`cvttss2si` / `cvttsd2si`) after scaling.

Items are not nested by length. A wire-5 header announces how many sibling
items follow it, and the struct readers compare the whole header word exactly.
Struct ids seen in the readers: `0x1f40` (2 wire-4), `0x1f41` (3 wire-4),
`0x1f42` (3 wire-3), `0x1f43` (2 wire-3), `0x332c` (camera, below), `0x2ee0`,
`0x2af8`, `0x2710`, `0x2328`, `0x32c8`, `0x3390`, `0x3388`, `0x3389`, `0x33f4`,
`0x0bb9`, `0x0bb8`, and `0x1770` (a variant container).

An array is a struct header with id `0x100` and item count `n + 1`, followed by
one wire-2 descriptor and then `n` elements (writer `0x1403b1d70`). The reader
(`0x1403b1720`) checks only the low 16 bits of the header and takes the count
from bits 16 to 27. It ignores the descriptor. In one request encoder
(`0x1402c63a0`), the descriptor `0x2ee0` precedes elements with struct id
`0x2ee0`. The descriptor may therefore be the element type id. That is an
inference from one site; another site (`0x1402c6f40`) writes an empty array
with descriptor `0x3039`.

## Response schemas (emulation)

`0x1402c3c50` fills a count, 12 per-field parameters (`+4`) and 12 field types
(`+0x34`). Emulating it for commands `0..0x1fff` gives 48 commands, stored in
`ttp_fields.RESPONSE_FIELD_TYPES`. The full table with parameters is in the
oracle JSON. `parse_response` does not read the parameters. For command
`0x848`, the types are `0x1b, 0x0c, 0x02` and the parameters `0x36, 0x37, 0x38`.

Each parsed field is 0x208 bytes. The type is at `+0`, and the value or count
at `+8`. Arrays keep a pointer at `+0x10` into the parser arena. The
`parse_response` switch (static):

| Type | Wire form | Stored |
| --- | --- | --- |
| 1 | wire 2 | uint32 `+8` |
| 2 | wire 1 | int32 `+8` |
| 3 | wire 7 | 64-bit `+8` |
| 4 | wire 6 | int64 `+8` |
| 5 | wire 3 | float `+8` |
| 6 | wire `0x18`, limit 10 | count, floats |
| 7 | wire `0x14` | string pointer `+8` |
| 8 | wire `0x1a`, limit 10 | count, string pointers |
| 9, `0x10`..`0x12` | struct `0x1770` variant (`0x1403b13b0`) | type rewritten to `0x12`; not reconstructed |
| `0xa` | struct `0x31f41` | 3 floats `+8` |
| `0xb`, `0x13` | refused | |
| `0xc` | array of `0x31f41` | float triples (doubles narrowed) |
| `0xd` | wire `0x15` | length `+8`, pointer `+0x10` |
| `0xe` / `0xf` | arrays of `0x41389` / `0x51388` | 0x88 / 0x8c bytes each |
| `0x14` | array of `0x42328` | 0x4c bytes each |
| `0x15` | `0x22710` items until the payload ends, no array header | 0x7c bytes each |
| `0x16` | array of `0x22710` | 0x7c bytes each |
| `0x17` / `0x18` | `0x22af8` / array of it | 2 uint32 / 8 bytes each |
| `0x19` | array of `0x42ee0` | 16 bytes each |
| `0x1a` | array of `0x932c8` | 36 bytes each |
| `0x1b` | array of `0x1a332c` | 0x2e8-byte camera records |
| `0x1c` | array of `0x73390` | 36 bytes each, two enum ints remapped |

A failed item read makes `parse_response` return status 2. An arena overflow
returns status 3. `parse_response` does not check for trailing payload bytes.
Struct readers OR their member results and keep reading after a failure (seen
in emulation). This only changes the failure position.

## Hardware-info reply (static, emulation)

The reply to command `0x848` is:

```
00 00
array header (camera count)            05 ... ((n+1)<<16 | 0x100), 02 ... descriptor
n x camera                             see below
array header (point count)             05 ..., 02 ...
m x triple                             05 00000004 00031f41, 3 x wire 4
wire 1 int32
```

Camera element (`0x1403b11b0`). The header is `0x1a332c`: 26 items, id
`0x332c`. Items are listed in wire order, which is also the record order. The
decoder's copy at `0x1402cc1c0` (case `0x1b`) and the tracker's copy at
`0x1402bb550` keep the offsets.

| Record offset | Wire | Stored as | Native field |
| --- | --- | --- | --- |
| `0x000` | 1 | int32 | `int32_0x00` |
| `0x004` | 3 | float | `float32_0x04` |
| `0x008` | 3 | float | `float32_0x08` |
| `0x010` | struct `0x31f41` | 3 doubles | `triple_0x10` |
| `0x028..0x098` | 15 x 4 | doubles | `doubles_0x28` |
| `0x0a0`, `0x0a4`, `0x0a8` | 1 | int32 | `int32_0xa0`, `int32_0xa4`, `int32_0xa8` |
| `0x0ac` | | kept count | `series_count_0xac` |
| `0x0b0..0x2b0` | `0x19`, limit 64 | doubles | `series_0xb0` |
| `0x2b0`, `0x2c8` | struct `0x31f41` | 3 doubles each | `triple_0x2b0`, `triple_0x2c8` |
| `0x2e0` | 4 | double | `double_0x2e0` |

The record is 0x2e8 bytes. Evidence: the stride is `0xba` dwords in the
decoder's arena check and copy, `0x2e8` in `0x1402c1390` and `0xba` dwords in
`0x1402bb550`. In `0x1402bb550` the value 0x2d0 is the loop pointer's start
offset (`param_2 + 0xb4` dwords), not the record size. Padding at
`0x00c` and unused series slots are not written.

`0x1402bb550` then copies the reply into its output. It rejects more than 4
cameras or more than 40 points (return 1, after a successful parse). It stores
the camera count at `+0`, records at `+8` (stride 0x2e8), the point count at
`+0xba8`, points widened back to doubles at `+0xbb0`, and the int32 at
`+0xf70`. `camera_info.HardwareInfo` names its members by those output
offsets. The host-side use of record offsets is in
[head-pose output](head-pose-output.md#camera-hardware-info). No wire-level
evidence names any value, so the native names stay neutral. Within the
tracker's limits, the reply needs at most `0x1398` arena bytes, well below
`0x5280`, so the arena limit cannot reject an accepted reply.

## Native implementation

- `src/tobii_linux/ttp_fields.py`: wire types, `value(Field)`, writers that
  mirror the original encoders, the sequential `Reader`, struct and array
  helpers, the response schema table, and the parser-kind and status mapping.
- `src/tobii_linux/camera_info.py`: `read_hardware_info` (parser stage),
  `parse_hardware_info` (adds the kind, command, status and tracker limits) and
  `hardware_info_response`, which inverts the reader for synthetic vectors. No
  device-side encoder exists in this binary.

Fixed-point values match the original bit for bit. The wire-3 values go through
float32 rounding before scaling, and the point triples are narrowed to float32.

## Oracle results (emulation)

`tools/ttp_reply_oracle.py` runs the original `0x1402cfa50` setup, then
`0x1402cf8c0` and `0x1402cfde0` for framing and decoding, `0x1402c18b0` for
the field check, `0x1402c3c50`, and the individual readers and writers. The
logger `0x1402c7360` and the type-mismatch printer `0x14000ffb0` are stubbed
and recorded. Allocation uses the shared `MatrixMachine` stubs. All inputs are
synthetic vectors derived from the instructions.

| Group | Result |
| --- | --- |
| Schema table, commands `0..0x1fff` | 48 commands, equal to `RESPONSE_FIELD_TYPES` |
| Writers (int32, uint32, int64, fixed16, fixed42, struct, array header, fixed42 array, string, blob) | 146/146 byte-identical |
| Readers (all 11 wire readers, including limits and wrong-type and wrong-length cases) | 96/96 |
| Hardware-info replies | 7/7 with identical values: one camera, the unit-test vector, 4 cameras with 40 points and 64-value series, empty arrays, a 70-value series truncated to 64, 5 cameras with 41 points (accepted by the parser), and extreme fixed-point values |
| Malformed payloads | 6/6 rejected by both. The original returns status 2 with log lines `0x9b3` (prefix), `0x84e` (camera element), `0x72b` (point array) and `0x6e4` (int32) |
| Status words and kinds | 33/33 |
| Three replies in 26 random chunks (6413 bytes) | 3 messages, identical |

## Reproduce

```sh
PYTHONPATH=.local/python:.local/inference:src:tools python3 tools/ttp_reply_oracle.py
PYTHONPATH=.local/python:.local/inference:src:tools python3 tools/ttp_reply_oracle.py --quick  # schema scan 0..0x8ff
GHIDRA_PROJECT_DIR=$PWD/evidence/private/ghidra-copy-a tools/ghidra/focused.sh a-fields 0x1402cc1c0 0x1402c3c50
PYTHONPATH=.local/inference:src python3 -m unittest tests.test_ttp_fields tests.test_camera_info
```

The full run takes about three minutes, most of it the schema scan.

## Limits and open questions

- No device reply has been captured. The layout is what this runtime accepts,
  not proof of what an ET5 sends.
- No value in the camera record has a wire-level name. The 15-double block,
  the series and the two trailing triples are unnamed.
- The array descriptor is ignored by the parser. Its value in device replies
  is unknown.
- The signedness of wire type 7 is unknown, because no writer was found.
- The parser kinds 3, 4, 5 and 6, the variant container `0x1770` and the
  element semantics of the other response types are not reconstructed.
- The tracker's interpretation of a nonzero reply status (`0x1402c1110`) is
  not traced.
