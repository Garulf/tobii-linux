# Star Citizen bundled Tobii interface investigation

Scope: only the shipped `F:\StarCitizen\LIVE\Bin64\tobii_gameintegration_x64.dll`
from albedo, version 9.0.3.25, SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`.
No other projects were consulted. Static analysis does not establish runtime use
by Star Citizen or Wine compatibility.

## Questions and current coverage

- Game API surface: partially answered by PE exports (271 named exports).
- Device discovery: PRP path recovered from native code and corroborated by REA.
- Service handshake: unresolved.
- PRP envelope and head/gaze body serialization: recovered statically; offline codec implemented.
- Non-compound PRP stream packets: reconstructed and checked against emulated original serializer.
- Command operations and live stream transport: unresolved.
- Actual game imported entry points and calling order: unresolved; game executable not analyzed.
- Linux USB tracking/calibration implementation: outside this interface investigation.
- End-to-end parity: untested; there is no NixOS gaming machine at present.

## Supplemental artifact observations

`evidence/private/game-pe.txt` preserves objdump PE imports/exports.
Exports include `GetApi`, `GetApiInternal`, `GetGazePoints`, `GetHeadPoses`,
`GetLatestGazePoint`, `GetLatestHeadPose`, `TrackWindow`, `TrackRectangle`,
`TrackTracker`, `Update`, and `Shutdown`. These are available functions, not proof
that the game calls each of them.

`evidence/private/game-strings.txt` contains embedded source-path/debug strings
for Stream Engine client, platform runtime protocol (PRP), service protocol,
and middleware implementations. Names alone do not establish active paths.

`evidence/private/game-discovery-assembly.txt` records discovery code.
Function at VA `0x180057a50` constructs the Windows pipe path
`\\.\pipe\ETDefaultPIPE` and issues `CallNamedPipeA` with:

- Input: four-byte little-endian integer 1.
- Output buffer capacity: 4096 bytes.
- Timeout: 1000 milliseconds.
- Response first integer must equal 1.
- Second integer is a signed count; returned byte count must exactly equal
  `8 + count * 512`.
- Each entry occupies 512 bytes and supplies a NUL-terminated device name.
  The code prefixes it with `tobii-ttp://` into a 256-byte URL buffer.

This is a legacy discovery branch, not yet established as the Eye Tracker 5
route. A separate routine at `0x180057d60` enumerates `\\.\pipe\*`, matches
`TOBII-` prefixes, removes that prefix and constructs `tobii-ttp://` URLs.
A TCP discovery routine at `0x180058010` conditionally probes `127.0.0.1` and
`172.28.195.1`, using service string `4455`, and constructs `tet-tcp://` URLs.
The artifact also contains `TOBIIPRP-`, `tobii-sesp://`, `tobii-mwsp://` and
`prp-internal://` strings; their code paths need separate tracing.

## Reconstruction implication (inference)

A Wine companion that supplies the service transport expected by the stock
bundled DLL could preserve the DLL's game-facing ABI and filtering. This is an
architecture candidate, not an implemented or tested bridge. Merely returning
a discovery response cannot supply valid gaze/head tracking: handshake,
capabilities, calibration/display mapping, subscriptions, timestamps, stream
payloads and reconnect behavior remain necessary.

## PRP serialization recovered from native code

Supplemental evidence is preserved in `evidence/private/game-prp-*-assembly.txt`
and `game-prp-*-table.txt`. The DLL was not executed in this investigation.
The implementation in `src/tobii_linux/prp.py` covers the envelope, two stream
bodies, and non-compound stream packets. It does not implement a service.

The serializer ending at `0x18004fa79` writes this envelope:

| Offset | Encoding | Meaning |
| --- | --- | --- |
| 0 | 4 bytes | ASCII `TPRP` (u32 LE `0x50525054`) |
| 4 | u32 LE | payload byte count, excluding 12-byte envelope |
| 8 | u32 LE | payload byte count XOR `0x50525054` |
| 12 | bytes | operation payload |

It writes a zero u32 at the end of the operation payload before completing the
envelope. The offline envelope API treats payloads as opaque, because the
complete operation grammar is not yet reconstructed. The XOR field detects
header inconsistency; it is not a payload checksum or authentication.

The operation serializer at `0x180051bf0` dispatches operation 1 to a stream
serializer at `0x180052e60`, through the jump table at `0x180052030`. The stream
serializer's jump table is at `0x180053c30`. The diagnostic enum-string table at
`0x18009958c` independently maps stream 2 to `PRP_STREAM_ENUM_HEADPOSE`, stream 3
to `PRP_STREAM_ENUM_GAZE_POINT`. Stream 29 shares the gaze-body branch and has
name `PRP_STREAM_ENUM_RESPONSIVE_GAZE_POINT` in the enum-string table.

Gaze body (24 bytes, serializer branch `0x180052fa4`):

| Offset | Encoding | Field |
| --- | --- | --- |
| 0 | u32 LE | fixed body size 24 |
| 4 | u32 LE | timestamp high 32 bits |
| 8 | u32 LE | timestamp low 32 bits |
| 12 | u32 LE | validity, serialized as 0 or 1 |
| 16 | float32 LE | x |
| 20 | float32 LE | y |

Head body (52 bytes, serializer branch `0x180052f18`):

| Offset | Encoding | Field |
| --- | --- | --- |
| 0 | u32 LE | fixed body size 52 |
| 4, 8 | u32 LE each | timestamp high then low word |
| 12 | u32 LE | position validity |
| 16, 20, 24 | float32 LE each | three position components |
| 28, 32, 36 | u32 LE each | three rotation validity flags |
| 40, 44, 48 | float32 LE each | three rotation components |

Timestamp word order is **not** equivalent to one little-endian int64.
The serializer copies float bit patterns without an explicit conversion.
Coordinates, units, validity behavior upstream of serialization and time-sync
relationships need separate confirmation before game integration.

The writer helper `0x18004fcc0` allocates aligned fixed fields and tracks an
additional length-delimited region after them. Therefore concatenating the
above stream body directly after the envelope does **not** make a complete PRP
subscription packet. `encode_stream_packet` includes the specific intervening blocks recovered for
non-compound streams; it does not implement arbitrary command blocks.

`tests/test_prp.py` uses manually transcribed static byte vectors (not captured
packets), checks high/low timestamp order and independent rotation validity,
and rejects inconsistent header fields, lengths and resource limits. The codec
uses a configurable 16 MiB maximum payload as a local resource limit, not a
claim about vendor limits. Noncanonical validity values are rejected by this
implementation; this stricter policy is not claimed to match the vendor parser.

## Differential check against original machine code

`tools/prp_encoder_oracle.py` binds the input DLL by the SHA-256 above, maps its
PE sections at the preferred base in Unicorn, supplies Win64 arguments and a
bounded scratch/stack region, and executes only selected original serializer
functions. No imports are stubbed, no Windows loader runs, and neither a device
nor a running game is accessed. A nonnull dummy logging context satisfies an
entry guard; the successful serialization path never calls it.

`evidence/private/game-prp-encoder-oracle.json` records six differential cases:
two ordinary gaze, two responsive gaze and two head pose samples. Every body
matches the reconstructed codec byte-for-byte at `0x180052e60`. Every complete
packet matches at `0x18004f730`, including nonzero high timestamp words,
negative timestamps, mixed validity flags and two opaque message-tag values.
This is supplemental emulation evidence, not a REA Evidence ID or a real capture.

For the checked non-compound stream operation, the envelope payload comprises
u32 LE fields below, followed by the stream body described above:

```
12, message_tag, 1, 4, 8, stream_id, 4,
<body, including its initial length word>,
4, 0
```

The complete gaze packet is 72 bytes; head pose is 100 bytes. The meaning and
lifecycle of `message_tag` have not been established, so the encoder requires
an explicit value. The empty extension blocks have length 4; supporting
nonempty extensions, compound streams or negotiation needs more analysis.

The five unit tests cover fixed vectors and malformed inputs. Original-code
emulation adds independent differential validation; neither establishes service
handshake, tracking quality, calibration, game acceptance or Wine compatibility.

## REA evidence ledger

All records below are complete inline REA evidence preserved locally under
`evidence/private/`. Analysis provider: Ghidra 12.1.4. Native code analysis is
static; the separate Unicorn results above are emulation.

| File | Evidence ID | Finding |
| --- | --- | --- |
| game-prp-stream-dossier.json | `ev_2139be9fae5ebf57678d2da4f017182aae8c64c8c1425ecf04362f0187042827` | stream serializer head/gaze bodies |
| game-prp-envelope-dossier.json | `ev_b187b9cd50020bcca282a61db51dfea880bd19e77370441db8e9b087e9e6907b` | TPRP header and operation serializer entry |
| game-prp-operation-dossier.json | `ev_b025dc3cdf7ad1742435e79a87853126e48debb4ab6b577d6922710cc8735881` | operation 1 selects stream serialization |
| game-prp-decode-dossier.json | `ev_8451652ea998897200fa28db2b67939e1155902768db46c3071ae617f0a1073b` | validates envelope and block sequence then decodes operation |
| game-prp-operation-decode-dossier.json | `ev_974741b3e8478158677b00bc7237fb1b39edbd2354130957ccc0c3f25bf9432e` | operation 1 calls stream decoder |
| game-prp-discovery-dossier.json | `ev_ce7f2718f0d9a2c0a0bd1c9643c77c5889495f876b4ca9549aa640b23e45d91e` | TOBIIPRP- Windows pipe discovery into tobii-prp:// URLs |
| game-prp-enums-dossier.json | `ev_914af7e9755d3fd6fc4e856eb5561b6f136b95e0c483c8284576b0e79763d931` | stream enum diagnostic names |

The original full decoder at `0x180053d10` also accepted all six reconstructed
packets in the oracle, consumed their exact lengths, and reconstructed the
original native tag/type/stream/data structures byte-for-byte.

## Head-pose encryption boundary

The checked PRP head-pose packet contains plaintext timestamp, validity flags,
position and rotation. The original DLL decoder accepts it without any key or
cipher context. Its exact static call path is:

```
0x180053d10  PRP envelope/block validation
  -> 0x180055780  operation dispatch (operation 1)
     -> 0x180056230  stream dispatch (stream 2)
```

In stream 2, the decoder checks a body size greater than 0x33, combines timestamp
words, converts integer flags to booleans and copies six 32-bit fields. There
are no function calls in that branch. REA evidence:
`ev_09c2553630eb9aef0a16352cef002e48bd0633febdf88d7e6e2782a89d16a74a`
in `game-prp-stream-decode-dossier.json`, together with the envelope and operation
dossiers above. This establishes a **plaintext consumer-side boundary**. It does
not rule out encryption in USB frames, upstream platform drivers, device
firmware, or another transport before this PRP layer.

The DLL also contains licensing logic and a large embedded crypto library.
Presence of those routines does not establish head-pose encryption. A search
for a bare `CryptDecrypt` symbol was unresolved in REA (name-not-found), which
is not evidence that the PE import or a caller is absent.

The qualified import `ADVAPI32.DLL::CryptDecrypt` was subsequently resolved.
Its native caller at `0x1800ce600` identifies itself as `capi_rsa_priv_dec()`
and cites `engines\\e_capi.c`; it performs RSA private decrypt through Windows
CryptoAPI. REA evidence `ev_cbf06f0d64a9c6a8127a65849bfef52c329d804f8e5490e8f45c6cdac8adf87d`
(`game-cryptdecrypt-caller-dossier.json`). This routine's presence is not evidence
of its use on head-pose data, and no connection to the checked PRP head branch
was established. The import xrefs are retained in
`game-cryptdecrypt-import-xrefs.json` with evidence
`ev_23caaef78ce7b45cb443c6a85c38c061213dc9c1584da70c640ef088580b0062`.

The actual `prp_client_stream_start` entry (`0x1800189f0`) constructs operation 9,
command 3, with the requested stream ID in its command body. It increments a
client request counter and uses it as the leading message field, sends with a
1,000,000-unit timeout parameter, then waits for a matching response. Units of
that timeout parameter are not established here. It can propagate
`INSUFFICIENT_LICENSE` (0x11) and `UNAUTHORIZED` (0x13) service errors; these error
paths do not themselves implement head-pose encryption. Evidence:
`ev_ad2339b59532b922535feb6e94a311c8ab1f5ecaeec0a0fff2a8e41cfeb287e6`
(`game-prp-start-entry-dossier.json`). This identifies the command correlation
field; the corresponding field's semantics in unsolicited stream messages still
need confirmation.

The PRP send entry at `0x180021a40` serializes through `0x18004f6e0` (wrapper
for `0x18004f730`), then supplies the resulting bytes and length to transport
`0x18004f250`. No encryption call appears between these operations in this
function. An internal-mode branch instead passes a pointer-bearing 24-byte
message; it is distinct from the serialized external transport. Evidence:
`ev_0fbfd038df8b5010bf2dfe8c9146b9c5707a51437691acf1edd612343279079e`
in `game-prp-send-dossier.json`. The final transport implementation and any
upstream device encryption remain outside this bounded conclusion.
