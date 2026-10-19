# Gaze calibration from Linux

How Tobii's Windows software calibrates the Eye Tracker 5, and how
`tobii_linux.calibration` reproduces it: the same device commands in the same
order with the same targets, plus backup and restore of the calibration the
device keeps. It builds on [device-side gaze](gaze-device.md), which covers the
command encoders, the calibration id and `calibration.setpm`.

Status: the procedure is recovered statically from the configuration app, the
Stream Engine DLL and the runtime; the new parsers and the full-size apply
request are checked in emulation; backup, verification and a dry run of the
whole calibration ran live on albedo without any device write. The first live
calibration (2026-10-14) stalled at the first target because the host stopped
reading while the target was shown; see [the tracker needs a reader](#the-tracker-needs-a-reader).
The fix keeps reading and its host pattern ran live without stalling; no
calibration has completed from Linux yet.

## Evidence

| File | SHA-256 |
| --- | --- |
| `platform_runtime_IS5LEYETRACKER5_service.exe` (runtime, base `0x140000000`) | `0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469` |
| `tobii_stream_engine.dll` (base `0x180000000`, same file in `C:\Program Files\Tobii\Tobii EyeX`) | `f60c90c41a6c29718f0255ab22db160b038f9305fe92d184c9cf3e8e938ec5a3` |
| `Tobii.Configuration.exe` (the calibration app) | `4f059d31e6b967f8dd5999b35065d23de32de9e9f94456c69bc90748b06f8900` |
| `Tobii.Configuration.Common.dll` (calibration views and logic) | `c050dd5fb01fcc4d2713360d0bf7c7a188887bbc866e1d1f94dff304668ce3e1` |
| `Tobii.Tech.NETCommon.TrackingDevice.dll` | `5842de22cfb16f905e61650576e8bf8a169b3fc4106643beb514c4cefbb69109` |
| `Tobii.Experience.Streaming.DeviceAdapter.dll` | `1c27fbdc93129ff227f6c54f33b82357e803b06c82e0e84fe6ff4adff5ae01c1` |
| `Tobii.StreamEngine.Net.dll` | `51ec6fd76bd126fd97afe86c920b83750f9700b40d3c0b2e07459b5dff69042a` |

The .NET assemblies were copied read-only from albedo and decompiled with
ilspycmd into `evidence/private/calibration/decompiled/`; the runtime log
`ConfigurationLog.txt` of the app and the runtime's `pr_log*.txt` are in
`evidence/private/calibration/albedo-files/`.

Labels:

- **static**: decompiled .NET code, objdump of the DLL and the runtime,
  focused Ghidra output (`evidence/private/cal-platmod-focused.c`,
  `cal-errors-focused.c`).
- **emulation**: original code run in Unicorn by
  `tools/gaze_calibration_oracle.py` (`evidence/private/gaze-calibration-oracle.json`).
- **live**: albedo, the real tracker through usbipd, or Windows logs and files.
- **inference**: a conclusion drawn from the above that the code does not state.

## Call chain (static)

The calibration window is `Tobii.Configuration.exe` in
`C:\Program Files\Tobii\Tobii EyeX`. The app's `CalibrationProcessViewModel`
uses `CalibrationSubscription`, which calls Stream Engine through
`Tobii.Experience.Streaming.DeviceAdapter` (`StreamEngineWrapper`). Each Stream
Engine export hands one command index to the client dispatcher `0x180089390`;
the service runs a PRP handler that calls a `platmod_command_*` function, which
calls the tracker vtable slot that builds and sends the TTP request.

| App method | Stream Engine export | Index | Service handler | Platmod function | Device |
| --- | --- | --- | --- | --- | --- |
| `Start` | `tobii_calibration_start` `0x18007edf0` | `0x09` | `0x1400fe730` | `0x14026d5f0` | 1010, 3 s |
| `Clear` | `tobii_calibration_clear` `0x18007dfa0` | `0x0f` | `0x1400fdf10` | `0x14026a980` | 1060, 3 s |
| `AddCalibrationPoint` | `tobii_calibration_collect_data_2d` `0x18007e030` | `0x0b` | `0x1400fdf50` | `0x14026ae40` | 1030 (x, y, eye 3), 15 s |
| `RemoveCalibrationPoint` | `tobii_calibration_discard_data_2d` `0x18007e710` | `0x0d` | `0x1400fe3d0` | `0x14026c440` | 1080 (x, y, eye 3), 3 s |
| `Compute` | `tobii_calibration_compute_and_apply` `0x18007e460` | `0x10` | `0x1400fe2e0` | `0x14026bbb0` | 1070, 30 s |
| `Stop` | `tobii_calibration_stop` `0x18007ef20` | `0x0a` | `0x1400fe7d0` | `0x14026da90` | 1020, 3 s, then 1110 if needed (below) |
| `GetCalibration` | `tobii_calibration_retrieve` `0x18007ecc0` | `0x11` | `0x1400fe6c0` | `0x14026d1a0` | 1100, 15 s |
| `SetCalibration` | `tobii_calibration_apply` `0x18007de20` | `0x12` | `0x1400fdec0` | `0x14026a3e0` | 1110, 15 s |
| `GetCurrentStimulusPoints` | `tobii_calibration_stimulus_points_get` `0x18007ee80` | property `0x13` (`0x18008a4c0`) | | property `0x140284620` | 1120, 3 s |
| `GetCalibrationPoints` | `tobii_calibration_parse` `0x18007e9b0` | none, local | | | none |

Timeouts are the values the tracker functions pass to the request waiter
`0x1402bec80` (`0x1402b51f0`, `0x1402b4920`, `0x1402b49c0`, `0x1402b5350`,
`0x1402b4ca0`) or listed in [gaze-device](gaze-device.md).

Coordinates go through unchanged: the collect export stores its two float
arguments, the service handler loads them from the PRP payload (`movss
(%rbx)`, `movss 0x4(%rbx)`) and the tracker function `0x1402b42c0` passes them
with eye 3 to the builder. They are normalized display coordinates, origin at
the top left: the app multiplies them by the window size to draw the target
(`CalibrationProcessView.OnViewModelCalibrationPointsSet`), and the device
reports the same values back as targets (live, below).

The app calls `tobii_calibration_start` with "both eyes", but the export
ignores its second argument (it calls the dispatcher with index 9 and no
arguments), the start handler `0x1400fe730` passes only the device to
platmod, and 1011 (per-eye start) has no caller: no eye selection reaches the
device. The handler records the calling client
(`+0x35fa20`, flag `+0x35fa28`); `0x1400f7100` clears that record when the
client goes away and then calls the platmod stop itself, so a client that
disconnects mid-calibration still produces 1020 (static; the caller of
`0x1400f7100` was not traced).

## The procedure (static, live log)

`Tobii.Configuration.Common.Calibration.PopCalibration` implements the
calibration the app runs for new profiles, recalibration and guest profiles.

Targets. `CalibrationHelper.GetCalibrationPoints` returns 9 points for a new
profile or recalibration, 5 for a guest and 4 for quick calibration, but the
state machine `CalibrationStateManager` only ever uses indices 0 to 6, in
three stages:

| Stage | Targets (x, y) |
| --- | --- |
| `FirstCalibrationSinglePoint` | (0.5, 0.5) |
| `SecondCalibrationThreePoints` | (0.1, 0.9), (0.5, 0.1), (0.9, 0.9) |
| `ThirdCalibrationThreePoints` | (0.1, 0.1), (0.5, 0.9), (0.9, 0.1) |

The values are float32 (0.1 is `0.10000000149011612`). Points 7 and 8 of the
9-point list, (0.1, 0.5) and (0.9, 0.5), are never shown. For a display larger
than 690 mm diagonal or 350 mm high (10 mm tolerance)
`CalibrationAreaCalculator` shrinks the target area to a centred 600 x 340 mm
rectangle and `ScalePoints` maps the targets into it. Albedo's display area
(1430) is 597 x 336 mm, so the targets span the whole screen.

Sequence in `CalibrationProcessViewModel`:

1. Eye positioning screen (user position guide stream only, no calibration
   command).
2. `TryStartCalibration`: `Start` (1010), `Clear` (1060). For a
   recalibration on a tracker generation other than G5 and IS3
   (`ShouldImproveCalibration`) the app first retrieves the current
   calibration (1100) and, after the clear, applies it again (1110) so the new
   samples add to the old ones.
3. Stage 1: as soon as valid gaze arrives the centre target is used without a
   focus test (`UseFirstCalibrationTarget`).
4. For each target: wait 200 ms "for the saccade to end", then
   `AddCalibrationPoint` (1030), which returns once the device has collected
   enough data. If, during the collection, the user's gaze moved to another
   target for more than 150 ms or no valid gaze arrived for 250 ms, the data
   is removed again (`RemoveCalibrationPoint`, 1080) and the target stays to
   be done. In stages 2 and 3 the user chooses the order: the target closest
   to the gaze, within 0.45 times half the smallest distance between targets
   (in pixels, enlarged when the eyes are beyond the track box), gets focus.
5. When every target of the stage is done: `Compute` (1070). After stages 1
   and 2 the app waits for its animation (700 ms after stage 2) and moves on.
   An error from compute aborts, except in quick calibration.
6. After stage 3: `Stop` (1020), then `GetCalibration` (1100) and
   `GetCurrentStimulusPoints` (1120); the precisions and biases go to the
   app's statistics, the blob to the profile store of the EyeX engine
   (`CalibrationDataStorage.TrySetCalibrationData`).

The result is `Success` when compute did not fail and 1100 returned a blob.
A compute error of `TOBII_ERROR_OPERATION_FAILED` is shown as "all points
failed" with a retry, any other error as a calibration error; connection
errors close the window. On every error the app calls `Stop` (1020). It never
calls discard because of a failed compute.

Computing after the single centre point gives the app a usable calibration for
the focus test in the later stages (inference from the order of steps).

The albedo log (`ConfigurationLog.txt`, live) shows both kinds of run: a new
profile on 2026-03-29 (start, clear, centre target, compute, three targets
with two discards for lost focus, compute, three targets, compute, then
"Number of points in calibration data: 7") and a recalibration on 2026-04-06
("Set previous calibration data" right after "Cleared calibration"). Collection
took 0.6 s to 1.1 s per target and compute 0.08 s (one target), 0.76 s (four)
and 1.3 s (seven).

## What the device returns (static, emulation, live)

**Compute (1070)** returns only a status. The tracker maps a nonzero TTP
status with `0x1402c1110`: `PROTOCOL_OPERATION_FAILED` to
`TRACKER_ERROR_OPERATION_FAILED`, `PROTOCOL_STATE_ERROR` to
`TRACKER_ERROR_BAD_STATE`, `PROTOCOL_UNKNOWN_OPERATION`,
`PROTOCOL_UNSUPPORTED_OPERATION`, `PROTOCOL_UNKNOWN_ID` and
`PROTOCOL_EXTENSION_REQUIRED` to `NOT_SUPPORTED`,
`PROTOCOL_INVALID_PARAMETER` to `INVALID_PARAMETER`, `PROTOCOL_UNAUTHORIZED`
to `UNAUTHORIZED`, `PROTOCOL_TOO_MANY_SUBSCRIBERS` to its namesake and the
rest to `INTERNAL`. Platmod (`0x1403abce0`) turns `BAD_STATE` from start into
"already calibrating" and from collect, compute and stop into "not started",
so the device itself tracks whether a calibration is running. That a failed
compute arrives as `PROTOCOL_OPERATION_FAILED` is inferred: it is the only
path to the operation-failed error the app treats as "all points failed".

**Stimulus points (1120)** after a compute describe the active calibration.
The reply is an array of struct 13200 with, per target: a struct 8002 with
x, y, z (fixed16, z 0), then per eye an int32 status and two fixed16 values.
The original parser accepts the status only as int32 (a uint32 field makes it
fail; emulation). `0x1402b7830` copies the fields in that order and maps a
status other than 0, 1, 2 to 0. Stream Engine's
`tobii_calibration_stimulus_point_data_t` names them status, bias, precision
(0 failed or invalid, 1 valid but not used, 2 valid and used). On albedo the
bias equals the distance between the target and the eye's mapped point in the
calibration blob for all 28 eye records, within one fixed16 step (live,
derived), so bias is the calibration's remaining offset at that target and
precision the spread of the samples (0.013 to 0.229, normalized units;
inference). The device returned 14 targets: the 7-target sequence twice,
because the albedo calibration is a recalibration.

**The calibration blob (1100)**, 732,971 bytes on albedo:

| Offset | Content |
| --- | --- |
| 0 | int32 offset of the point table trailer (732,403) |
| 4 to 16 | 110, 84, 1, 0 (unknown) |
| 20 | calibration id, equal to device status key 7 and the `calibration.setpm` id |
| 24 | 732,315, then the label `human` padded to 64 bytes and opaque data |
| trailer | uint32 (`0xfd559c5a` on albedo, not read), int32 record count, then 40-byte records |

A record holds the target (two float32), then per eye the mapped gaze point
(two float32), a status (-1 failed, 0 valid but not used, 1 valid and used)
and an unused word. This is how `tobii_calibration_parse` (`0x18007e9b0`)
reads it; it only checks that the size is at least 8 and the count is not
negative. Everything before the trailer is opaque.

## After the compute (static, live)

The runtime learns the new calibration id from a device notification: word 3
`0xc94` becomes tracker type 17 and platmod type 13; the handler
`0x140268050` stores the id from the notification payload at `+0x10f68` and
flags `handle_calibration_id_updated` (`0x140267e20`), which retrieves the
blob (1100) and has `calibration_receive_and_commit` (`0x140267cf0`) write id
and blob to `calibration.setpm` ("Commited new calibration with ID %u").

The platmod stop (`0x14026da90`) sends 1020 and then runs the same check as at
startup: it reads `calibration.setpm`; equal ids log "No calibration update
needed"; a different, nonzero local id logs "Restoring calibration from disk"
and sends 1110 with the stored blob. In a normal run the notification has
already updated `calibration.setpm`, so stop sends nothing more (inference).

The runtime log on albedo has "Restoring calibration from disk, ID:
3157793335, size: 732971" at 11 service starts between 2026-04-25 and
2026-10-03 (`pr_log1` to `pr_log4`), each followed by a normal start with the
ids in sync (live). The device therefore does come up without that
calibration at times (cause not established) and Windows puts it back from
`calibration.setpm`. A Linux system has to do the same from its own backup;
`restore_calibration` is that step.

## The tracker needs a reader

Live, albedo, 2026-10-14. The first person-present run sent 1220, 1010 and
1060, showed the centre target, and sent 1030; no reply came within 15 s, and
1020, 1230 and 1490 then timed out too. A new USB session right afterwards
worked normally (device status key 7 read 0: the 1060 clear had taken effect),
and Windows restored the calibration at service start.

The same host-side pattern without any calibration write reproduces it
(`tools/gaze_calibration_probe.py host-pattern`, raw USB log in
`evidence/private/calibration/probe/host-pattern-raw.jsonl`): with gaze
subscribed the device delivers one 1,724-byte gaze frame per bulk read every
30 ms; the calibrator then stopped reading for 2.1 s while the page animated
the target, and from the first read after that pause the device sent nothing
at all, neither stream data nor replies, for the remaining 77 s of the
session. Writes were still accepted. `pause-threshold` mode measured the
limit in one run: gaze resumed after unread pauses of 0.25, 0.5, 0.75 and
1.0 s but not after 1.25 s, and the device then ignored 1490.

So a subscribed stream left unread for between 1.0 and 1.25 s silences that
USB session for good; only closing it (channel close, a new session) brings
the device back. Whether the firmware or the usbip path causes it was not
separated. The Windows runtime queues received USB data for its tracker
worker (`0x1402c0390`, [ttp-replies](ttp-replies.md)) independently of any
waiting request, so it presumably never leaves the endpoint unread (inference).
Everything that subscribes a stream on Linux must keep a read pending, or at
least read more often than once a second.

`Calibrator` now runs every UI callback on a worker thread and keeps reading
the device (100 ms reads) until the callback returns; stream data goes to the
focus check as before. The same host pattern with this change ran live for
65 s, seven 2.1 s target pauses included: gaze arrived at 33 Hz throughout and
every 1490 was answered at once (`host-pattern-fixed-raw.jsonl`).
`tests/test_calibration.py` models the device (`StallingDevice`); the old
calibrator fails that test and the new one passes.

## Linux implementation

`src/tobii_linux/gaze_device.py` gains the decoders and writers:
`calibration_setting` (inverse of `parse_calibration_setting`),
`stimulus_points` and `stimulus_points_payload` (1120),
`calibration_points` (the blob's point table, as `tobii_calibration_parse`),
`blob_calibration_id` and `tracker_point_status`.

`src/tobii_linux/calibration.py`:

- `backup_calibration(session)`: reads the id (1490 key 7), the blob (1100)
  and the id again, and refuses a calibration that changed meanwhile. Only
  reads. `save_backup` writes it in the `calibration.setpm` layout (version
  4, kind 1, payload size, id, blob); `verify_backup` compares a file with the
  device's id and blob.
- `restore_calibration(session, calibration)`: 1110 with the stored blob, then
  1490; raises unless the device reports the file's id.
- `Calibrator(session, ui, ...)`: the sequence above, driven by a
  `CalibrationUI` with `show_point(index, x, y)` (return once the eye is on the
  target), `point_done(index, accepted)` and `stage_done(stage)`. Device
  commands: subscribe gaze (1220), 1010, 1060, optionally 1110 with the
  previous calibration (`previous=`, the app's recalibration), then per stage
  1030 for each target and 1070, then 1020, unsubscribe (1230), and the reads
  1100, 1490, 1120. With a `FocusCheck` it applies the app's focus rules to
  the gaze stream received during each collection of stages 2 and 3 and sends
  1080 and collects again when the user looked away (up to three attempts).
  Any error after 1010, and an interrupt, sends 1020 and unsubscribes; the
  error is raised as `CalibrationFailed`. Nothing is ever applied on failure.
  UI callbacks run on a worker thread while the calibrator keeps reading the
  device; they must not use the session themselves.
- `Calibrator(..., choose_by_gaze=True)` with a `FocusCheck`: the app's
  target handling (see [setup flow](setup-flow.md#calibration-calibrationprocessviewmodel-calibrationprocessview)).
  `show_targets` shows a stage's targets at once, `GazeFocus` picks the one
  the gaze rests on (zone, 150 ms, 250 ms rules), `focus` reports it, 200 ms
  later 1030 collects, and the data is discarded (1080) when the gaze left.
  From stage 2 an `AttentionWatch` (4 s without attention and progress, or
  10 s without progress) stops (1020) and starts again (1010, 1060), at most
  `max_stalled_runs` times. `calibration_started`, `computing` and `check`
  (raise to cancel) are further callbacks; the fixed order above stays the
  default.
- `DryRunSession(session)`: forwards read commands (1100, 1120, 1310, 1430,
  1420, 1490, 3060, 3080, 3170) and reports every other command instead of
  sending it.
- `calibration_targets(display_mm)`: the seven targets, scaled for large
  displays like the app.

`src/tobii_linux/calibration_page.py` serves the targets as a fullscreen web
page (standard library only) and implements `CalibrationUI`: the page draws
each target, shrinks it for 1.5 s while the user looks at it, acknowledges,
and the calibrator waits the app's 200 ms before collecting. Once the page
covers the screen (fullscreen, or a kiosk window as large as the screen, which
the server also accepts from the reported sizes) it counts down 3 s and
starts by itself, as Tobii's calibration needs no key; a key or click starts
it at once and Esc aborts (the run then sends 1020). Its texts are written
once and only the countdown digit changes. Under WSL a
Windows browser reaches it at `http://localhost:8775/` through mirrored
networking; on Linux any browser does. This was chosen over a Windows tkinter
window because the page acknowledges every target, so collection never starts
before the target is visible, and the same code serves native Linux.

`tools/gaze_calibrate.py`:

```sh
python3 tools/gaze_calibrate.py backup backup.setpm [--compare calibration.setpm]
python3 tools/gaze_calibrate.py verify backup.setpm
python3 tools/gaze_calibrate.py restore backup.setpm [--dry-run]
python3 tools/gaze_calibrate.py calibrate --before before.setpm --after after.setpm [--improve] [--dry-run] [--ui page|none]
```

`--raw-log FILE` (before the mode) records every USB transfer in hex with its
time (`tobii_linux.traffic_log`); `traffic_log.replay_messages` decodes such a
log offline with the session's own parsers.

`calibrate` reads the display area (1430), saves the current calibration to
`--before` before any write, waits for the page, runs the sequence, and saves
the new calibration to `--after`. It prints the per-target quality from 1120
and, per record of the new blob, each eye's distance from the target in
pixels. After a failure it prints the device id and the restore command.

Differences from the Windows app, all inferences about effect: here targets
within a stage are shown in a fixed order rather than chosen by the user's
gaze; the focus test does not enlarge the zone for users beyond the track
box; there is no eye positioning screen; the result is not added to a profile
store. `tobii-linux setup` ([setup flow](setup-flow.md)) has all of these
except the zone enlargement.

## Checks

Emulation (`PYTHONPATH=.local/python:.local/inference:src:tools python3
tools/gaze_calibration_oracle.py`), 13 of 13 match:

- `tobii_calibration_parse` on seven synthetic blobs (statuses -1, 0, 1 and
  others, an empty table, a negative count, a short blob) and on the albedo
  blob (14 records) against `calibration_points`.
- The runtime's response parser and the 1120 type check `0x1402c1900` on
  replies built by `stimulus_points_payload` (three synthetic sets and the
  values the device returned live) against `stimulus_points`.
- The 1110 builder `0x1402c5050` with the full 732,971-byte albedo blob:
  the 733,006-byte request equals `calibration_apply`
  (SHA-256 `447bdb00...a9ab3`).

Unit tests (`tests/test_calibration.py`) run the real `TrackerSession`
against a scripted device: backup layout and verification, restore and its id
check, the full command order and targets, recalibration, compute failure
(stop, no apply), collect failure, start failure, an interrupt, focus loss
with discard and retry, the dry run (only 1100, 1490 and 1120 reach the
device) and the tool's `calibrate --dry-run`, plus the page's start, target
and abort handling.

Live, read only (albedo, 2026-10-13 23:46, `calibration-readonly.cmd`):

- `backup` returned id `0xbc381a37` and a 732,971-byte blob; the written file
  equals `calibration.setpm` byte for byte (SHA-256 `9d025119...4686`), not
  only from byte 16. `verify` matched id and blob.
- `calibrate --dry-run --ui none` read the display area (597.0 x 336.0 mm,
  whole screen), the calibration and status, reported the 15 commands it
  would send (1220, 1010, 1060, 1030 (0.5, 0.5, 3), 1070, three 1030, 1070,
  three 1030, 1070, 1020, 1230), then read 1100, 1490 and 1120: the device
  was unchanged.
- `restore --dry-run` reported a 733,006-byte 1110 request.
- After the run the Windows runtime logged "Device and local device in sync,
  doing nothing (0xbc381a37)".
- The page loop was checked separately: server in WSL, headless Edge on
  Windows, two targets shown and acknowledged in 1.7 s each.

`tools/gaze_calibration_probe.py` holds the live diagnostics: `host-pattern`
and `pause-threshold` (read only) and `collect` (start, clear, one centre
collect, stop, every received message logged, including replies whose
request id does not match).

## Running it with a person

`evidence/private/calibration/albedo/` (copied to
`C:\Users\Garulf\tobii-linux-test\`) holds the prepared, not yet run,
scripts (`calibration-live.cmd` ran once, see above; it now also writes
`calibration-live\calibrate-raw.jsonl`):

- `calibration-live.cmd`: stops the Tobii services, attaches the tracker to
  WSL, opens the page in Edge kiosk mode on the desktop
  (`open-calibration-page.cmd` through `schtasks /it`), runs
  `calibration-live.sh calibrate`, then a 34 s gaze capture against
  `windows_gaze_targets.py` with the new calibration (read only), and gives the
  tracker back to Windows. Results go to `calibration-live\`.
- `calibration-diagnostic.cmd`: the minimal check, `gaze_calibration_probe.py
  collect --page` with a raw USB log: the page counts down 3 s (or starts on
  a key), shows the centre dot and the person looks at it; the tool sends 1220, 1010, 1060, one
  1030 (0.5, 0.5, 3) and waits up to 15 s plus 20 s for a late reply while
  reading continuously, then 1020 and 1230. Results go to
  `calibration-diagnostic\`.
- `calibration-restore.cmd [file]`: applies `calibration-live\before.setpm`
  (or the given file) from Linux and verifies it.

Device writes of the live calibration, in order: 1220 subscribe gaze; 1010;
1060; 1030 (0.5, 0.5, 3); 1070; 1030 (0.1, 0.9, 3), 1030 (0.5, 0.1, 3),
1030 (0.9, 0.9, 3), each possibly followed by 1080 with the same values and
the same 1030 again; 1070; 1030 (0.1, 0.1, 3), 1030 (0.5, 0.9, 3),
1030 (0.9, 0.1, 3), with the same possible retries; 1070; 1020; 1230
unsubscribe. On failure: 1020 and 1230. Before them the usual Linux startup
(1000, 1600, realm 0x2712 with 2120) and the reads 1430, 1490, 1100, 1490;
after them 1100, 1490, 1120 and, for the verification, 1490, 1100, 1490.

When the services start again, the runtime will find the device id different
from `calibration.setpm` and restore the old calibration itself (1110 from
Windows, logged as "Restoring calibration from disk"). To keep a Linux
calibration on Windows instead, `after.setpm` has the layout of
`calibration.setpm` and could replace it while the services are stopped; that
has not been tried.

## Calibration profiles

The Windows software keeps several user profiles, each with its own
calibration, and switching profile puts that calibration on the device.

### Where Windows keeps them (static, live)

Profiles live in the registry, not in files.
`Tobii.Tech.NETCommon.TrackingDevice.Config.ConfigurationManager` (assembly
`Tobii.Tech.NETCommon.TrackingDevice.dll`) uses
`HKLM\SOFTWARE\Tobii\EyeXConfig` in the 32-bit view, so
`HKLM\SOFTWARE\WOW6432Node\Tobii\EyeXConfig` on a 64-bit system:

| Value | Meaning |
| --- | --- |
| `CurrentUserProfile` | name of the active profile (`GetCurrentUserProfile` cuts it at the first NUL) |
| `UserProfiles\<name>\TrackedEyes` | DWORD 0 both, 1 left, 2 right (`TetConfigTrackedEye`) |
| `UserProfiles\<name>\Calibrations\<key>\Timestamp` | text, `DateTime.Now` in the invariant culture |
| `...\NumPoints` | DWORD, the app's point count (9 for a new profile) |
| `...\DataSize`, `...\Data` | the calibration blob and its length |
| `EyeTrackers\<tracker uri>\MonitorID`, `Mounting` | the monitor a tracker is mapped to, integration type |

`<key>` is `<tracker uri>&<monitor instance id>` with every `\` replaced by
`*` (`EscapeMappingKey`), so a profile holds one calibration per tracker and
monitor. Valid names are 1 to 255 characters of letters, digits, space, `(`,
`)` and `-` (`ValidateUserProfileName`).

After a successful calibration the app's
`CalibrationDataStorage.TrySetCalibrationData` stores the 1100 blob it just
retrieved under the profile, with the monitor from `tobii_get_display_id`,
and makes the profile current (`SetCurrentUserProfile`).

On albedo (live, read only): one profile `Garulf`, current, `TrackedEyes` 0,
with two calibrations: monitor `UID41221` (2026-04-06, 732,971 bytes,
SHA-256 `06b5e67a...604f`) and monitor `UID37125` (2026-03-29, 350,279
bytes, id `0x34e8d612`, 7 points). The `UID41221` blob is byte for byte the
device's 1100 blob and `calibration.setpm` from byte 16: the registry keeps
exactly what the device returns.

### What switching sends (static)

The EyeX engine (`Tobii.EyeX.EyeTracking.dll`) watches the configuration. When
the current profile's mapping (`UserProfileMappingData`: tracker, monitor,
profile name, tracked eyes, calibration) changes,
`EyeTrackingEngine.TrackerComponentOnUserProfileMappingDataChanged` calls
`TrySetCalibrationAndProfileSettings`, which, unless the mapping equals the
last one written:

1. `SetTrackedEyes` -> `tobii_set_enabled_eye` with 2 for both eyes, 0 left,
   1 right (`InternalApiHelper.TrackedEyesToEnabledEye`). Stream Engine sets
   property `0x10` (`0x18007fc40` -> `0x18008aa20`); the runtime's setter
   `0x140281480` maps 0, 1, 2 to device eye 1, 2, 3 and calls the tracker's
   slot `0x3a8`: command 3160 with uint32 eye.
2. `ConfigurableEyeTracker.SetCalibration` -> `tobii_calibration_apply` with
   the profile's `Data`: command 1110 with the blob (see the call chain
   above). A call that times out is reported as `CalibrationTimeout` and the
   engine restarts the device.

The device then reports the new calibration id, and the runtime stores the
blob in `calibration.setpm` (the notification path in "After the compute").
On engine start (`OnEnterInitialize`) the engine restores the last profile
from its own state.

### Linux implementation

`ProfileStore` (`crates/tobii-gaze/src/profiles.rs`, wrapped by
`src/tobii_linux/calibration_profiles.py`) keeps profiles as files under
`$XDG_DATA_HOME/tobii-linux/profiles` (default
`~/.local/share/tobii-linux/profiles`). The layout is this project's choice,
not Tobii's:

| File | Holds |
| --- | --- |
| `NAME.setpm` | the calibration in the `calibration.setpm` layout (version 4, kind 1, size, id, blob); a profile exists when this file does |
| `NAME.json` | `{"tracked_eyes": 0, "calibrations": {"<monitor key>": "<file>.setpm"}}`: Tobii's `TrackedEyes` (0 both, 1 left, 2 right) and which file serves which monitor |
| `NAME@<monitor key>.setpm` | the calibration for another monitor |
| `current` | the name of the active profile |

The monitor key is the record's `monitor.edid_id` (the EDID id in hex) when it
was read, else its `monitor.key` (the output name the desktop reports, `DP-1`),
both as `tobii-linux-setup` records them in
`$XDG_DATA_HOME/tobii-linux/display-setup.json`
(`recorded_monitor_key`, `crates/tobii-gaze/src/profiles.rs`); the record's
`connector` field is not used for it; characters other than letters,
digits, `-` and `_` become `_` in a file name. A profile's first calibration
is `NAME.setpm`, which stays the calibration of the monitor it was made on and
serves every monitor without one of its own. A profile without `NAME.json`
tracks both eyes. Names follow the Windows rules, so `@` and `.` cannot occur
in a name and `NAME@...` files never list as profiles.

- `ProfileStore`: `names`, `load`, `save` (refuses to overwrite unless asked),
  `current`, `set_current`, `tracked_eyes`, `set_tracked_eyes`,
  `calibration_for(name, monitor)` (that monitor's file, else `NAME.setpm`),
  `save_for(name, monitor, ...)`, and `delete`, which removes `NAME.setpm`,
  `NAME.json` and every `NAME@*.setpm` and refuses the current profile and
  `Guest` (Tobii's guest profile is never listed and cannot be deleted,
  docs/experience-app.md section 6).
- `save_from_device(session, store, name)`: the read-only backup (1490, 1100,
  1490) into the store, then the verification reads.
- `switch(session, store, name, eye=Eye.BOTH)`: reads the device id (1490);
  if it differs from the profile's, sends 3160 with the eye and 1110 with the
  blob of `NAME.setpm`, as the engine does; reads the id again and, on a real
  session, requires the profile's id and records the profile as current. With
  a `DryRunSession` the writes are reported instead. Unlike the engine it
  skips both writes when the device already holds the profile's calibration,
  as the runtime's start check does. `switch_with` (Rust) takes the monitor
  key, can force both writes (`force`), and can send 3160 even when the ids
  match (`always_set_eye`); `switch` and the Python binding set neither.
- `tobii-linux serve` applies the current profile with `switch_with` whenever
  a tracker session starts, with the eye from `NAME.json` and the calibration
  for the recorded monitor, and on the control channel's `switch_profile` and
  `set_tracked_eyes` (docs/control-channel.md). It always sends 3160 with the
  profile's eye and sends 1110 only when the device reports another calibration
  id (`set_tracked_eyes` sends both), so a profile that `tobii-linux-setup` or
  `profiles switch` made current with other tracked eyes reaches the device on
  the next attach, as the engine puts calibration and tracked eyes on the
  device at every start.

`tools/gaze_calibrate.py`:

```sh
python3 tools/gaze_calibrate.py profiles list [--directory DIR]
python3 tools/gaze_calibrate.py profiles save NAME [--overwrite]
python3 tools/gaze_calibrate.py profiles switch NAME [--dry-run]
```

`list` does not open the device. A profile can also be made from a Windows
registry `Data` value or a `calibration.setpm` copy, since all three hold the
same blob.

Unit tests (`tests/test_calibration_profiles.py`): name rules, the XDG
location, the store, a read-only save against a scripted device, the switch
order (1490, 3160, 1110, 1490) and its skip when the ids match, and the tool's
`save`, dry-run `switch` and `list`.

Live (albedo, 2026-10-14 00:37, read only, `evidence/private/display-setup/live/`):

- `profiles save Garulf`: id `0xbc381a37`, 732,971-byte blob with the
  registry's SHA-256, verified against the device.
- `profiles list`: `Garulf` and `Garulf-2026-03-29`, the latter made from the
  registry's `UID37125` calibration.
- `profiles switch Garulf-2026-03-29 --dry-run`: device id `0xbc381a37`
  before and after; would send 3160 with eye 3 (35 bytes, SHA-256
  `e92aaea2...8be4`) and 1110 with the 350,279-byte blob (350,314 bytes,
  SHA-256 `dfa0b28c...307a`). Neither was sent.
- `profiles switch Garulf --dry-run`: ids already equal, nothing to send.

## Open questions

- The meaning of blob words 1 to 4, the trailer's first word and the opaque
  body; whether the device takes the id from blob word 5 on 1110 (the
  restore check relies on the reported id, not on this).
- Which notification, if any, the device sends after compute; the calibrator
  logs every notification it reads.
- Whether the reader requirement comes from the firmware or from usbip, and
  whether it holds for native USB on Linux.
- How long a real 1030 takes on Linux and whether it needs anything else; the
  diagnostic script answers this.
- Whether the device collects 1030 data without a gaze subscription; the
  calibrator subscribes as the app's gaze stream does on Windows.
- Why the device came up without its calibration at 11 Windows starts.
- 1031, 1032 and 1071 (3D and per-eye collection and compute) are not used by
  the app and were not traced further.
