# Supplemental static USB transport findings

These are **objdump instruction observations and bounded offline emulation**, not
REA Evidence IDs and not USB captures. Target: installed
`platform_runtime_IS5LEYETRACKER5_service.exe`, SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`,
preferred image base `0x140000000`. No other driver projects were consulted.

Reproduction material:

- `evidence/private/usb-transport-assembly.txt`: focused original instruction ranges.
- `tools/usb_framing_oracle.py`: hash-bound original function emulation.
- `evidence/private/usb-framing-oracle.json`: ten successful synthetic comparisons.

The oracle executes original framing functions `0x1402d3490` and `0x1402d3740`.
Its USB-write stub records the outgoing arguments and **returns chosen status 0**
(success); transfer-result stub supplies chosen bytes and **returns chosen status
0**. Mutexes and callbacks return chosen 0; memcpy is replaced by emulator memory
copying. This verifies emitted framing and received-byte interpretation, **not
actual device success, initialization, or whole-program behavior**.

## Framing is asymmetric

| Direction | Offset 0, LE uint32 | Offset 4, LE uint32 | Following bytes |
|---|---|---|---|
| Host OUT, pipe `0x05` | Channel index, originally widened from uint16 | Payload length | Payload |
| Device IN, pipe `0x83` | Recipient bitmask | Total record length, including eight-byte header | Payload |

Transmit builds the header at `0x1402d3580..0x1402d35f8`, copies payload at
`0x1402d35ff`, calls the pipe writer with endpoint 5 at `0x1402d3613..3615` and
length `payload+8` at `0x1402d3608`. Maximum payload per transfer is 4087 (`0xff7`).
If a selected payload chunk is divisible by 512, it is shortened by one byte;
remaining payload is sent in the next record. This is a literal observed choice,
not a claim about why the original author chose it. Empty input emits no records.
Emulated 512-byte input produces transfer lengths 519 and 9; 8192-byte input
produces lengths 4095, 4095, 26.

Receive accumulates an eight-byte header at `0x1402d3879..38c8`, decodes the two
LE words at `0x1402d38ca..3924`, then subtracts eight from the second word at
`0x1402d3936`. Recipient dispatch tests the low bit, shifts, and advances a channel
slot at `0x1402d37e0..3833`; at most twelve slots exist. Mask `0x805` sends identical
payload to slots 0, 2, and 11 in the emulated observation. Reads may split headers
or payloads. The original delivers payload fragments to channel consumers as they
arrive; our bounded parser buffers complete records before returning payload.
This is a timing/buffering design difference, not asserted 1:1 runtime parity.

The startup branch `0x1402d2f30` allocates four 16384-byte asynchronous reads on
pipe `0x83` at `0x1402d3100..3136`. It uses a channel-slot array initialized to
indices 0..11 at `0x1402d2f90..2fa0`. Host-local handles start at 100 and are distinct
from on-wire channel indices (`0x1402d2fff`, `0x1402d3326..3332`).

## Control setup fields

`0x1403b4640` copies the caller's eight-byte setup structure without changing fields
into `WinUsb_ControlTransfer` (`0x1403b4672..46bd`). The accompanying payload pointer
is at caller structure +8. Thus the following table follows instruction writes,
not string-name guesses. Vendor/interface OUT is `0x41`; IN is `0xc1`.

| Caller | bmRequestType | bRequest | wValue | wIndex | wLength | Interpretation/limits |
|---|---:|---:|---:|---:|---:|---|
| `0x1402d3300..3316` | 0x41 | 0x41 | channel 0..11 | 0 | 0 | Open channel slot; slot is marked active only after success |
| `0x1402d340e..3420` | 0x41 | 0x42 | channel | 0 | 0 | Close channel; also used during destructor at `0x1402d39cd..39e2` |
| `0x1402d40c6..40dc` | 0xc1 | 0x46 | 1 | 0 | 8 | New timestamp query; caller log calls this `ctrl1` timestamp method |
| `0x1402d4113..4128` | 0x41 | 0x45 | 1 | 0 | 22 | Legacy timestamp request |
| `0x1402d413c..4159` | 0xc1 | 0x45 | 1 | 0 | 22 | Legacy timestamp response |
| `0x1402d4427..4473` | 0x41 | 0x30 | 0 | 0 | 24 | Optional metadata SCP request |
| `0x1402d44a1..44be` | 0xc1 | 0x30 | 0 | 0 | 512 | Optional metadata SCP response |
| `0x1402d307c..308a` | 0x41 | 0x10 | 3 | 0 | 0 | Version-mismatch branch; **not implemented or executed** |

Metadata request is six LE uint32 values `[1,0,1,4,0,0]` initialized at
`0x1402d443e..4468`; response code uses a length field at offset 20 and text at
offset 24. The parent initialization only performs metadata matching if an
embedded-version pointer is present (`0x1402d300a`). It can transition state on a
mismatch; reproducing that requires further verification and is deliberately not
part of the experimental channel helper.

The selected normal branch requires interface number 0 through `0x1403b4fe0`
called with EDX=0 at `0x1402d30b0`; the checked descriptor member was populated by
`WinUsb_QueryInterfaceSettings(..., alternate=0)` at `0x1403b4901..4911` after
`WinUsb_Initialize` at `0x1403b48c3`. This is an interface-number check, not a command
that sets configuration or alternate setting.

## Separate firmware path must stay separate

Pipe `0x04` is used at `0x1402d45b4..45c3` after vendor request `0x41/0x01` in a
separate upload path. Nearby original mapped strings identify `Trying to download
new image` (`0x1410b14e8`), `Failed to transfer image header` (`0x1410b15d0`), and
`dfu_transfer_thread` (`0x1410b1658`). It is not used by the implemented channel
helper. Vendor requests `0x01`, `0x04`, `0x06`, `0x10` are not issued by that helper.

The runtime chooses normal/other branches from an object field at +0x28 through
`0x1402d2960..2978`. We have not yet traced that field completely back to real ET5
state. Endpoint presence is checked against actual descriptors before opening,
but complete device-mode selection is unresolved.

## Implemented scope and remaining limits

`usb.py` now requires descriptor-confirmed bulk endpoints `0x83` and `0x05` on
interface 0, whose only alternate setting is 0. It accepts other endpoints without
using them and rejects missing/duplicate expected pipes. Bounded control IN/OUT
methods are explicit low-level APIs, with no automatic actions during `probe`.
`usb_channel.py` adds an explicit single-use channel context and direction-specific
stream parser. Failed or partial writes are not retried. Read exceptions fail the
experimental channel closed; libusb partial-transfer details remain on the error.
The caller owns overall receive deadlines and higher-level protocol negotiation.

The one-megabyte framing bound and 4096-byte control bound are defensive local
limits, not recovered firmware constants. The parser rejects invalid lengths
rather than reproducing unchecked native arithmetic. NixOS hardware behavior,
USB descriptors on Linux, real control success, clock negotiation, encrypted
tracking, and complete startup remain unvalidated. This is not a functioning
full-parity eye/head tracking driver yet.

## Bounded image-to-headpose inspection

This additional static check follows the image buffer into the host algorithm;
it does not complete the USB-to-image decoder chain. `platmod_algo::deploy_image`
(`0x1402857d0`, named by log string `0x14046e5c0`) receives image metadata in RDX
and a pixel pointer in R8. It multiplies metadata uint32 fields +0x0c and +0x10
at `0x14028581d..824`, then copies exactly that many bytes from the original
pixel pointer through vector helper `0x1402853d0` called at `0x140285a1e`.
That helper allocates and calls memcpy at `0x140285468`; it contains no cipher or
pixel transform. The copied pixel pointer reaches `0x140287010` at
`0x140285a90`, and the image is queued for the host worker.

Upstream callback `0x140266430` checks a format field equals 8 and byte count
equals width*height (`0x140266475..486`); it forwards the original pixel pointer
unchanged to `deploy_image` at `0x140266609`. Callback `0x140266d50` checks a
stride*height byte count and forwards the original pointer at `0x140266f9d` via
metadata wrapper `0x140285be0`. That wrapper checks dimensions and count, then
calls `deploy_image` at `0x140285d55`. No decryption or byte transform was found
on these inspected callback-to-queue paths. Both callbacks are placed in tracker
configuration by `0x140260380`, passed to `0x1402b2de0` at `0x14026052c`.

**Unresolved:** the earlier device-message decoder and its indirect callback
invocation have not been completely traced here.
