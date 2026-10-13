# Eye Tracker 5 USB protocol investigation

Status: partial reconstruction. The implementation does **not** start tracking.
This investigation uses the installed Tobii binaries from `albedo`. No third-party
ET5 driver source or protocol implementation is incorporated.

## Device and software identity

Windows PnP identified USB `2104:0313`, interface `MI_00` using `WinUSB` and
interface `MI_01` using `usbvideo`. The installed `oem2.inf` independently binds
`USB\VID_2104&PID_0313&MI_00` to WinUSB. Its driver version is `2.5.0.5602`.
The software component INF version is `1.133.0.30004` (2023-04-25). It installs
the platform runtime service and middleware `4.72.0.29983`.

The analyzed platform runtime is
`platform_runtime_IS5LEYETRACKER5_service.exe`, version `1.37.4.0_f2b4e724c9`,
19,998,560 bytes, SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`.
Its copied hash matched `Get-FileHash` on the Windows host. The first SFTP copies
were truncated to 204,800 bytes; they were replaced using legacy SCP and are not
the analysis inputs. Local binary artifacts and raw records are in ignored
`evidence/private/`.

## Supplemental static evidence

The following observations come from `objdump` disassembly of that exact binary,
not from a live USB capture. They must not be cited as REA execution evidence.
Addresses use its preferred PE image base `0x140000000`.

| Function address | Observed behavior |
| --- | --- |
| `0x1403af650` | Incremental parser obtains a 24-byte header, reads six 32-bit words, then receives the number of payload bytes in word 5. It preserves incomplete input across calls. A nonempty payload begins with two bytes that the parser checks for zero. |
| `0x1403aff70` | Reads a 32-bit integer by shifting and combining four successive bytes, most significant byte first. |
| `0x1403afd60` | Reads the six header words using that big-endian reader. |
| `0x1403afc40` | Writes six header words through the corresponding integer writer. |
| `0x1403afb50` | Initializes an output buffer, reserving 24 header bytes and two zero payload-prefix bytes when payload capacity exists. |
| `0x1403afbe0` | Computes nonempty payload length as encoded field bytes plus the two reserved bytes. Empty field data yields zero payload length. |
| `0x1403b0e40` | Encodes a uint32 array as tag `0x17`, big-endian byte length `4 + 4*n`, big-endian count, then big-endian elements. |
| `0x1402c6e40` | Builds a request with header words `0x51`, supplied request identifier, `0`, `1000`, `0`, computed payload length. Payload contains the uint32 array. |
| `0x1402be6d0` | Referenced logging string identifies `tracker_usb_ttp_t::protocol_upgrade`; caller increments a request counter and supplies nine values `0x10000` through `0x10008` to the preceding builder. |

The first header word is called `kind`, the second `request_id`, and the fourth
`command` in our code based on those call sites. Meanings of words 2 and 4 are
unresolved. The meaning of the zero payload prefix is unresolved; it is not
assumed to be a status code. Only the array tag is semantically decoded.

`src/tobii_linux/ttp.py` implements this inner framing and the reconstructed
upgrade request. It preserves unknown fields and refuses malformed extents.
Its 4 MiB payload cap is our resource policy, not a discovered hardware limit.
The fixed test vector is derived from the observed instructions, **not** recorded
device traffic. It does not prove that an ET5 accepts the message.

An additional offline check executes the original encoder at `0x1402c6e40`:
all three synthetic cases match our output byte for byte. The reproducible
helper is `tools/ttp_encoder_oracle.py`; retained results are in
`evidence/private/ttp-encoder-oracle.json`. This strengthens the encoder
reconstruction without claiming device acceptance.

## USB access and remaining work

The Linux backend requires descriptor-confirmed bulk IN `0x83` and bulk OUT
`0x05` on interface 0, alternate setting 0, following the original runtime's
streaming path. Additional endpoints are allowed; the separate firmware/image
download pipe `0x04` is unused. Direction-specific outer framing and explicit
channel open/close helpers are implemented. See [the transport report](usb-transport-static.md)
for exact instructions, offline emulation checks and remaining startup gaps.

The bounded Windows descriptor probe failed at `CreateFileW` with error 5
(access denied); no WinUSB handle was initialized. USBPcap is installed, but its
topology enumeration did not return during bounded probes. The USBIP bus label
and PnP port number are not treated as a USBPcap bus/address mapping. No USB trace
has been obtained so far.

Required before a tracking driver can be claimed:

- Validate the reconstructed outer USB envelope and complete the vendor-control startup sequence.
- Verify initialization and upgrade against device traffic.
- Recover stream enumeration, subscriptions, column schemas and timestamps.
- Decode gaze and tracking loss without relying on unverified fixed offsets.
- Recover display geometry, calibration and reconnect behavior.
- Establish the host head-pose processing inputs and algorithm.
- Validate the Linux output against the successful Windows reference probe.

The full head-pose algorithm and whether any device traffic is encrypted
remain unresolved.
