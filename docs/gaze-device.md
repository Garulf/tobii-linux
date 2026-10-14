# Device-side gaze: startup, calibration and persistent files

Status: the runtime's device start sequence and its calibration handling are
reconstructed statically, every calibration, persistent-file and configuration
request builder is checked byte for byte in emulation, and the existing Linux
session was measured live against the Windows runtime with a person following
on-screen targets. Result: the Linux session already sends the device-side
steps that matter for gaze, the user calibration lives on the device, and
stream `0x500` from Linux is as valid and as accurate as Windows Stream Engine
gaze. Host-side processing of `0x500` into Stream Engine outputs is outside
this document. Calibrating, backing up and restoring the calibration from
Linux are covered in [gaze calibration](gaze-calibration.md).

Target: `platform_runtime_IS5LEYETRACKER5_service.exe`, SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`, image base
`0x140000000`. It extends [the stream notes](usb-streams.md) and
[the reply notes](ttp-replies.md).

Evidence labels:

- **static**: objdump disassembly (`evidence/private/platform-disassembly.txt`)
  or focused Ghidra `-noanalysis` output (`evidence/private/gd-*-focused.c`).
- **emulated**: original builder run in Unicorn by `tools/gaze_device_oracle.py`
  (`evidence/private/gaze-device-oracle.json`).
- **live**: the real tracker on albedo, passed to WSL2 with usbipd-win, or the
  Windows runtime's own log and files on albedo.
- **inferred**: a reading of names, shapes or call order that the code does not
  state directly.

## Tracker vtable map (static)

`evidence/private/gaze-device/map_builders.py` resolves every slot of the
tracker vtable `0x1410af508`, follows subscribe thunks, finds the
`tracker_usb_ttp_t::<name>` log string used in each slot function and the
request builder (a caller of `0x1402c3bc0` or of the header writer
`0x1403afc40`) it calls. `map_wrappers.py` maps the castor platform module's
`tracker_*` wrappers (`0x1402b2be0`..`0x1402b3920`, each a null check and a
`jmp *slot(%rax)`) to slots, and `map_platmod.py` lists which platform-module
function calls which wrapper. Outputs: `builder-map.json`, `wrapper-map.json`
in `evidence/private/gaze-device/`.

## Calibration, persistent-file and configuration commands

Names come from log strings in the slot function, or (marked p) from the
`platmod_command_*` function that is the slot's only caller. Unnamed slots are
marked inferred. Every builder below is emulated (119 cases, all match).
"Reply" is the schema `0x1402c3c50` assigns: item id and parsed type.

| Command | Slot | Name | Builder | Request fields | Reply |
|---|---|---|---|---|---|
| 1010 | `0x1b8` | calibration_start (p) | `0x1402c5120` | none | none |
| 1011 | `0x1c0` | per-eye start, inferred; no caller found | `0x1402c5140` | uint32 eye | none |
| 1020 | `0x210` | calibration_stop (p) | `0x1402c5200` | none | none |
| 1030 | `0x1c8` | calibration_collect_data_2d (p) | `0x1402c4a20` | fixed42 x, fixed42 y, uint32 eye (tracker passes 3) | none |
| 1031 | `0x1d0` | calibration_collect_data_3d (p) | `0x1402c4b30` | fixed42 x, y, z, uint32 eye | none |
| 1032 | `0x1d8` | `calibration_add_point_per_eye` | `0x1402c4c70` | fixed42 x, fixed42 y, uint32 eye | 52: 1 (eye) |
| 1060 | `0x1f8` | calibration_clear (p) | `0x1402c4dc0` | none | none |
| 1070 | `0x200` | calibration_compute_and_apply (p) | `0x1402c4d80` | none | none |
| 1071 | `0x208` | `calibration_compute_and_apply_per_eye` | `0x1402c4da0` | none | 52: 1 |
| 1080 | `0x1e0`, `0x1f0` | calibration_discard_data_2d and `_per_eye_2d` (p) | `0x1402c4e00` | fixed42 x, fixed42 y, uint32 eye | none |
| 1081 | `0x1e8` | calibration_discard_data_3d (p) | `0x1402c4f10` | fixed42 x, y, z, uint32 eye | none |
| 1100 | `0x220` | `calibration_retrieve` | `0x1402c4de0` | none | 2: 13 (blob) |
| 1110 | `0x218` | `calibration_apply` | `0x1402c5050` | blob | none |
| 1120 | `0x3d8` | `get_calibration_stimulus_pts` | `0x1402c5bb0` | none | 58: 28 |
| 1310 | `0x380` | `get_configuration_key` | `0x1402c5300` | string | 32: 18 |
| 1440 | `0x188` | `set_display_area` | `0x1402c6f40` | three fixed42 triples (struct `0x1f41`), empty array of type `0x3039` | none |
| 1800 | `0x2e0` | enable extension, inferred | `0x1402c57a0` | uint32 | none |
| 3060 | `0x378` | `persistent_file_enumerate` | `0x1402c6a20` | none | 42: 8 (string array) |
| 3070 | `0x368` | `persistent_file_write` | `0x1402c6b40` | string name, blob | none |
| 3080 | `0x360` | `persistent_file_read` | `0x1402c6a40` | string name | 43: 7, 44: 13 |
| 3090 | `0x370` | `persistent_file_erase` | `0x1402c6950` | string name | none |
| 3160 | `0x3a8` | set enabled eye (from `set_platmod_enabled_eye_on_tracker`) | `0x1402c5240` | uint32 eye | none |

Eye values are 1 left, 2 right, 3 both (`0x1402b5290`, `0x1402b4490`; the
device reports enabled eye 3). Builders take float32 coordinates and widen them
to double before the fixed42 writer, so the encoders round inputs to float32
first. Timeouts in the tracker functions: collect 15 s, apply and retrieve
15 s, per-eye add 10 s, 1011 3 s. `persistent_file_*` and per-eye calibration
require negotiated protocol `0x10004` and `0x10008` (`+0x264` checks).

`timesync` (slot `0x228`) only logs and returns 2 (unsupported) in this build.
`timestamp_get` (`0x230`) and `sync_token` (`0x238`) do not use TTP builders;
they go through the USB transport helpers `0x1402d1670` and `0x1402d15c0`
("ctrl1 timestamp method" in the runtime log). `sync_token` is called only from
`start_all_trackers_delayed` (`0x140272630`) when two trackers are paired.

`src/tobii_linux/gaze_device.py` implements all encoders, the status and
setting-file decoders below, and `synchronize_calibration`.
`tests/test_gaze_device.py` covers them with emulated bytes as golden vectors.

## Device start sequence of the runtime (static, live log)

`0x140261440` runs once per connected tracker after `initialize`
(upgrade 1000, `unlock_ppu` 1600). In order:

1. `setup_device_info` `0x1402640d0`: 1420 device info, 1330 device
   properties (sub platform, lot id, integration id, calibration version and
   date are optional keys; the ET5 has none of them, as the log says).
2. 1351 capabilities (all), 1490 device status into device `+0x10f40`.
3. `setup_device_capabilities` `0x140263c90`: 1200 stream types and their
   columns.
4. 1430 display area, 1400 track box, 2110 geometry mounting.
5. `start_synchronize_calibrations` `0x140263010`, below.
6. `start_synchronize_screen_plane` `0x140263610`: loads `screenplane.setpm`;
   with the host algorithm present it defers the plane to the algorithm and,
   when the capability list holds entry 0, also sends 1440 `set_display_area`
   with the stored corners. The albedo log shows "Restoring screen plane from
   disk and applying to tracker" on every start.
7. `start_synchronize_adv_gaze_output_freq` `0x140262db0`: only if
   `adv_gaze_output_freq.setpm` exists (it does not on albedo) and capability
   9 is present, sends 1660 `set_frequencies_output`.
8. `0x140263520` copies setting 3 for the host (capability `0x10`), no
   device traffic. `0x140261a70` reads 1650 frequencies (capability 9).
9. `initialize_exclusive_fast_gaze_flag` `0x1402791f0`: 1670 enumerate
   frequencies.
10. `start_host_algo_system` `0x140261ca0`: realm `0x2712`, 2120 hardware
    info, 1915 close realm, 1430 display area (already in the Linux session).

Starting gaze subscribes stream `0x500` only. The stream synchroniser
`0x1402784a0` builds its wanted set with `stream_state_for_gaze_start`
`0x140278a80`: when the device configuration's host-gaze flag (`+0x188`) is
clear it sets just the gaze flag; the host-gaze branch would instead require
intermediate data and the internal algorithm image, which the ET5 does not
enumerate. Before the synchroniser it calls `0x140279320`, which sets the
output frequency only for a stored 60 or 120 Hz advanced-gaze setting.

Compared with Linux (`driver.TrackerSession`): every step that changes device
state is already sent except 1440, which rewrites the display area the device
already holds (live: `screenplane.setpm` corners equal the 1430 reply to the
last digit). Everything else is a getter.

## Where the calibration lives

Static:

- `start_synchronize_calibrations` reads the local calibration from the
  setting manager (`0x140271900`, setting 0) and compares its id with the
  device id at `+0x10f68`. Equal: "Device and local device in sync, doing
  nothing". Local absent but device id nonzero: "clearing device", sends 1010
  then 1020. Different: "Restoring calibration from disk", sends 1110 with the
  stored blob.
- `+0x10f68` is word 10 of the 1490 status block at `+0x10f40`: the status
  parser `0x1402b9360` converts key 7 with `strtoul` into it. Key 7 is
  therefore the device calibration id. The id is also updated by notification
  word 3 `0xc94` (tracker type 17, platform type 13, handler `0x140268050`),
  after which `handle_calibration_id_updated` `0x140267e20` retrieves the
  calibration (1100) and `calibration_receive_and_commit` `0x140267cf0` stores
  id and blob as setting 0.

Live (albedo, read only):

- `C:\ProgramData\Tobii\Tobii Platform Runtime\IS5LEYETRACKER5\IS5FF-100204141463\`
  holds `calibration.setpm` (732,987 bytes), `displayinfo.setpm` (monitor
  device path) and `screenplane.setpm` (three corners, mm). Setting files
  start with uint32 4, uint32 1, uint32 payload size.
- `calibration.setpm` payload: uint32 id `0xbc381a37`, then the 1100 reply
  blob verbatim. `tools/gaze_device_query.py` read the blob from the device
  (1100, 732,971 bytes) and its SHA-256 equals bytes 16 onward of the file.
  Device status key 7 reads `3157793335` = `0xbc381a37`.
- The runtime log (`pr_log0.txt`) shows "Device and local device in sync,
  doing nothing (0xbc381a37)" at every start, so Windows sends no calibration
  command in normal operation. The calibration persists on the device across
  USB re-attachment and the WSL sessions.
- The blob starts with uint32 732,403 (the offset of the point table
  trailer), then uint32 values 110, 84, 1, 0, the id and 732,315, the label
  `human` padded to 64 bytes and opaque data. The trailer holds a word, the
  count 14 and 14 records of 40 bytes: a target, then per eye the mapped gaze
  point and a status. Stream Engine's `tobii_calibration_parse` reads it this
  way (static and emulation, see [gaze calibration](gaze-calibration.md)).
- 1120 returns 14 records of struct `13200`: a target point (struct `8002`,
  z 0), then per eye a status of 2 (valid and used), the bias and the
  precision in normalized display units ([gaze calibration](gaze-calibration.md)).
- 3060 returns an empty file list: there are no persistent files (license
  files are what `platmod_command_license_key_*` would store there).
- No realm or unlock beyond the head-pose realm is involved: `unlock_ppu`
  returns zeros and the calibration commands are not gated by a realm in the
  code paths above.

`gaze_device.synchronize_calibration` mirrors the runtime decision from the
status key and a parsed `calibration.setpm`, sends 1110 only when asked and
the ids differ, and never sends the clear pair.

## Live gaze comparison (2026-10-13)

Setup: user in front of the tracker following
`tools/windows_gaze_targets.py` (centre, four corners at 0.1/0.9, centre,
0.3 and 0.7 on the middle row, 3 s each, 2560x1440). Linux captures with
`tools/gaze_device_capture.py --phase ... --seconds 34`; alignment and
errors with `tools/gaze_device_report.py`: the device timestamp (column 1) is
mapped to the target log's QPC clock by one offset found by grid search (the
WSL wall clock drifts), samples from 0.8 s after a target appears, error of
column 28 (combined relative gaze point) where column 27 is 1. Windows: the
same metric on Stream Engine gaze points (QPC timestamps).

| Run | Records | Combined valid | Both eyes valid | Median error, px (per-target range) |
|---|---|---|---|---|
| Linux, gaze + presence | 1229 | 95.9 % | 94.8 % | 47.7 (32.7 to 104.9) |
| Linux, gaze + presence + image `0x501` | 1228 | 94.2 % | 92.6 % | 51.3 (19.3 to 102.5) |
| Linux, earlier coordinator run (same startup) | 1516 | 96.3 % | 95.6 % | 64.8 (36.7 to 102.8) |
| Windows Stream Engine baseline | 1126 | 98.8 % (Stream Engine validity) | | 52.0 (17.6 to 76.6) |

Gaze origins in the first Linux phase: left (-73.2, -52.5, 720.7) mm, right
(-7.6, -49.1, 724.0) mm, 65.6 mm apart. Image streaming does not change
validity or accuracy beyond run-to-run spread, and the device sent no
notification during any run. The 28 % validity and shared -157 mm origins of
the first capture (`evidence/private/gaze-capture/linux-gaze.jsonl`) came
from a session without targets where the user was not looking at the screen,
not from missing startup. Gaze arrives at 33 Hz in both phases; frequencies
read (132, 33).

Wire semantics observed: per-eye validity (columns 7 and 13) is 0 valid and 4
invalid; the other validity columns (21, 22, 27, 29 to 35, 38, 40) are 1 valid
and 0 invalid; eye openness (41 to 44) stays -1 with validity 0 on this
device.

## Device writes not performed

None of these were sent from Linux:

- 1440 `set_display_area` with new values (the runtime sends it at every start
  with values the device already holds; Linux has since sent exactly that
  rewrite once, see [display setup](display-setup.md)).
- 1110 `calibration_apply` (the runtime skips it while ids match; the device
  already holds `0xbc381a37`).
- 1010/1020 clear, all collect/compute commands, 3070/3090 persistent files,
  1660 frequencies, 3160 enabled eye, 1800.

## Reproduction

```sh
PYTHONPATH=.local/python:.local/inference:src:tools python3 tools/gaze_device_oracle.py
PYTHONPATH=.local/inference:src python3 -m unittest tests.test_gaze_device
python3 tools/gaze_device_report.py capture.jsonl targets.jsonl
```

On albedo, `gaze-device.cmd` (two 34 s phases, each preceded by the target
window through `schtasks /it`; the 5 s countdown covers the Linux startup) and
`gaze-query.cmd` (read-only calibration query) bind and attach the tracker,
run the tool in WSL from a private copy of `src`, then restore the Tobii
services; their sources are in `evidence/private/gaze-device/albedo/`.

## Open questions

- Meaning of 1011, 1800, 1310 and device status keys 0 to 6 and 8.
- Why the device sent no notification at all, while the runtime handles
  `0xc94` for calibration changes; it may only send it after a calibration.
- The calibration blob outside its point table is opaque; see
  [gaze calibration](gaze-calibration.md) for what is known.
- Stream Engine validity is 98.8 % against 94 to 96 % for the device's
  combined column; whether the host fills single-eye records belongs to the
  host-side gaze work.
