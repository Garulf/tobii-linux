# Display setup

The tracker reports gaze and head pose relative to the monitor, so it has to
know where the monitor is. Tobii's configuration app on Windows has a display
setup step that works this out and stores it on the device as the display
area: the top-left, top-right and bottom-left corners of the visible screen in
millimetres, in the tracker's coordinate frame. This document recovers how the
app computes the area, checks the result against the Windows reference host
`albedo`, and describes `tobii_linux.display_setup`, which does the same on
Linux.

Status: the formula is recovered from the decompiled .NET sources and
reproduces albedo's device display area bit for bit from the monitor's EDID
size and the tracker's mounting geometry; its native Stream Engine form agrees
in emulation; the 1440 request was sent live with the values the device
already held and read back unchanged.

## Evidence

| File | SHA-256 |
| --- | --- |
| `Tobii.Configuration.exe` (display setup wizard) | `4f059d31e6b967f8dd5999b35065d23de32de9e9f94456c69bc90748b06f8900` |
| `Tobii.Experience.Streaming.Domain.dll` (`DisplayAreaUtils`) | `134911154deb2ed4efdffb9df092fa17b2de9f52b75d181d4e3a5b39f762cca1` |
| `Tobii.Experience.Streaming.DeviceAdapter.dll` | `1c27fbdc93129ff227f6c54f33b82357e803b06c82e0e84fe6ff4adff5ae01c1` |
| `Tobii.Tech.NETCommon.Mathematics.dll` (`Vector3`) | `d4c8e50faa984362942a431cddc3cdeb3d60f883ceb8d62851f13156449e1e35` |
| `Tobii.Tech.NETCommon.Displays.dll` (`EdidDataInfoReader`) | `8565dafc43128be54cc3cbef1b26019d26e488c01d21e2c250a229c02f58f1d5` |
| `Tobii.EyeX.EyeTracking.dll` (EyeX engine) | `72c1560d327156edd411db1e6af415631cf27cddf7ba154653214780b5d5d6b9` |
| `Tobii.EyeX.Config.dll` | `b936805ac0677f39389df7cead06e2837f76e809234bbfdda8ef4c52f6c1d82b` |
| `tobii_stream_engine.dll` (base `0x180000000`) | `f60c90c41a6c29718f0255ab22db160b038f9305fe92d184c9cf3e8e938ec5a3` |
| `platform_runtime_IS5LEYETRACKER5_service.exe` (base `0x140000000`) | `0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469` |

The assemblies were copied read-only from `C:\Program Files\Tobii\Tobii EyeX`
on albedo and decompiled with ilspycmd into
`evidence/private/calibration/decompiled/` and
`evidence/private/display-setup/decompiled/`. The live probe output (EDID,
WMI sizes, the `EyeXConfig` registry, the setting files) is
`evidence/private/display-setup/display-probe.txt`.

Labels:

- **static**: decompiled .NET code, objdump of Stream Engine and the runtime.
- **emulation**: original code run in Unicorn by `tools/display_setup_oracle.py`
  (`evidence/private/display-setup-oracle.json`).
- **live**: albedo, the real tracker through usbipd, or Windows files,
  registry and logs.
- **inference**: a conclusion the code does not state directly.

## The wizard (static)

The display setup flow (`SetupDisplayFlow`, `MonitorMappingModel`) selects a
monitor and drives `EyetrackerMonitorMappingDevice`:

1. `EyeTrackerDataLoader.LoadEyeTrackerData` reads the device info and the
   mounting geometry (`tobii_get_geometry_mounting`, device command 2110).
   With a reported width it builds `EyeTrackerSpecifications` from it
   (`TryGetDynamicGeometry`); a static table in
   `EyeTrackerStaticSpecificationsProvider` exists only for older trackers.
2. `DisplaySetupStorage.GetEyeTrackerToMonitorMapping` reads the current
   device display area (`tobii_get_display_area`, 1430) and decomposes it
   (`DisplayArea.Decompose`, below) into a monitor size and a tracker offset.
   If the monitor has an EDID size hint (`IDisplayDevice.SizeHint`), that size
   replaces the decomposed one. Rotation by 90 or 270 degrees swaps width and
   height.
3. `MovingLinesScreenPlaneSetupFactory.CreateMovingLinesScreenPlaneSetup`
   uses that mapping when it is valid (nonzero size, same rotation as now,
   aspect ratio known): `MovingLinesScreenPlaneSetup(spec, aspect, size,
   offset)`, aspect taken from the stored size. Only without a valid mapping
   does it estimate from the EDID size hint
   (`TryGetScreenSizeHint`, rejected when its aspect ratio differs from the
   screen's pixel bounds by 0.1 or more) or, failing that, a default width of
   376.3 mm, with the aspect ratio from the pixel bounds.
4. The user step. The screen shows two lines at the tracker's ends: positions
   `left`, `right` as fractions of the screen width from the left
   (`ScreenPlaneSetupControl`). The user drags a line (changes its position
   alone) or the tracker picture (moves both). Positions are clamped to 0.02
   to 0.98 and at least 0.05 apart (`ScreenPlaneSetupViewModel`). On Apply the
   control builds `MovingLinesScreenPlaneSetup(spec, aspect, left, right)`:

   - width = tracker width / (right - left) (184 mm for the Eye Tracker 5)
   - height = width / aspect
   - tracker offset x = ((left + right) / 2 - 0.5) * width

   So the monitor width comes from how wide the tracker looks on screen, and
   the offset from where it sits. There is no vertical or depth adjustment.
5. `EyetrackerMonitorMappingDevice.WriteDisplaySetupToTracker` composes the
   area from the mounting geometry with the external offset increased by the
   tracker offset, sends it (`IConfigurableDevice.SetDisplayArea` ->
   `tobii_set_display_area`), then sends the display id
   (`tobii_set_display_id`: monitor instance id, EDID bytes 8 to 17 in hex,
   rotation, version 1.0).

The EDID size hint is the image size of the first detailed timing descriptor:
`EdidDataInfoReader.TryGetMonitorSizeHintFromEdidData` reads bytes 66, 67 and
68 (width `(b68 & 0xF0) << 4 | b66`, height `(b68 & 0x0F) << 8 | b67`), not
the centimetre fields at 21 and 22. The EDID comes from the monitor's device
registry key (`SetupDiGetClassDevs`, value `EDID`).

## The formula (static, emulation)

`DisplayAreaUtils.ComposeDisplayArea(geometry, size)` with mounting angle `a`,
external offset `E` and internal offset `I`, monitor size `W x H`:

1. A flat rectangle with its bottom edge centred on the origin
   (`FromPhysicalDisplaySize`): top-left `(-W/2, H, 0)`, top-right
   `(W/2, H, 0)`, bottom-left `(-W/2, 0, 0)`.
2. Subtract `E + (offset, 0, 0)` from each corner.
3. Rotate about the x axis by `a`: `y' = y cos a - z sin a`,
   `z' = y sin a + z cos a`.
4. Add `I`.

The mounting values reach the app as float32 (`tobii_geometry_mounting_t`),
the computation runs in double and the result goes back as float32
(`tobii_display_area_t`). The 2110 reply carries guides, width, angle, then
the external and the internal offset triple, in that order; the Eye Tracker 5
reports `(2, 184.0, 20.0, (0, -0.16, 13.85), (0, 5.38, 9.86))` (live). In
words: the mounting places the screen's bottom edge 0.16 mm above and 13.85 mm
behind the tracker's reference point, tilts the screen back by 20 degrees and
moves the result by the internal offset into the camera frame (inference from
the arithmetic).

`DecomposeDisplayArea` inverts it: the tilt is the angle between
`top-left - bottom-left` and the y axis (`Vector3.AngleBetween`, acos of the
clamped normalized dot product), the size is the length of the top edge and
the left edge, and the offset is `-W/2 - x` of the bottom-left corner after
undoing `I`, the tilt and `E`.

Stream Engine also exports the same model natively:
`tobii_calculate_display_area_basic(api, width, height, offset_x, mounting,
area)` at `0x18007da30` (static: constants -0.5 and 0.5 at `0x18023e268` and
`0x18023e258`, `pi/180` at `0x18023e260`, `sin`/`cos`, subtraction of the
external offset and `offset_x`, addition of the internal offset). The app does
not call it. Emulated on seven cases (albedo's monitor, a centred 24 inch, an
off-centre 34 inch ultrawide, portrait, another mounting, no tilt), it agrees
with `compose_display_area` within 0.0001 mm; it computes in float32, so
only the untilted case is bit-equal.

## Albedo (live)

- Monitor: LG 27GL850, 2560 x 1440. EDID first detailed timing: 597 x 336 mm
  (WMI reports 60 x 34 cm from bytes 21 and 22). The size hint passes the
  aspect check (1.777 against 1.778).
- Device display area (1430) and `screenplane.setpm`: top-left
  (-296.62, 326.00, 111.82), top-right (300.38, 326.00, 111.82), bottom-left
  (-296.62, 10.27, -3.10).
- Decomposed with the device mounting: 597.0000 x 336.0000 mm, tilt exactly
  the mounting's 20 degrees, tracker offset -1.87991 mm (the tracker sits
  1.88 mm left of the screen centre).
- Recomposed from the EDID size 597 x 336, the device mounting and that
  offset, all nine float32 values equal the device's bit for bit, and
  `display_setup.screen_plane_setting` writes a file byte-identical to
  albedo's `screenplane.setpm`. With the offset rounded to -1.88 the y and z
  values are still exact and x is off by 9e-5 mm.
- The height 336.0 shows that the stored size came from the EDID hint (or a
  previous area of that size), not from the pixel aspect 16:9, which would
  give 335.81 mm. The configuration log of both display setups on albedo
  (2026-03-29, 2026-04-06) has no "Use width from monitor size hint" line, so
  the wizard took the stored-mapping path both times and the user only dragged
  the tracker sideways (inference).

## How the area is written and stored (static, live)

- `tobii_set_display_area` (`0x18007f9f0`) sets Stream Engine property 1
  through `0x18008aa20`. In the runtime, `platmod_property_display_area_set`
  (`0x14027c120`) hands the plane to the host algorithm (`0x140286750`),
  calls the tracker's `set_display_area` (slot `0x188`, wrapper
  `0x1402b37c0`, builder `0x1402c6f40`, command 1440: three fixed42 triples
  and an empty array of type `0x3039`), then commits the setting ("Commited
  new screen plane").
- `screenplane.setpm` in
  `C:\ProgramData\Tobii\Tobii Platform Runtime\IS5LEYETRACKER5\<serial>\`:
  uint32 4, uint32 1, uint32 36, then bottom-left, top-left, top-right as
  float32. The runtime restores it at every start
  (`start_synchronize_screen_plane`, "Restoring screen plane from disk and
  applying to tracker"), which re-sends 1440 with the same corners.
- `displayinfo.setpm`: the display id string, here
  `DISPLAY\GSM5B7F\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0`,
  zero padded. It is host-side only; no TTP command carries it (static: no
  builder in the tracker vtable map).
- The EyeX engine (`EyeTrackingEngine.WriteDisplayAreaToTrackerAndRegistry`)
  keeps a copy in the registry,
  `HKLM\SOFTWARE\WOW6432Node\Tobii\EyeXConfig\MonitorMappings\<tracker uri>&<monitor id>\ActiveDisplayArea`
  (text, two decimals) with `AspectRatioWidth`/`Height` 16 and 9, and writes
  the area to the device only when it is not already there within
  `DisplayArea.AlmostEquals` (0.1 mm²) and the integration type asks for it.
  Its black-bars mode (a resolution with a different aspect ratio than the
  panel) adjusts the area; this is not reproduced.

## The on-screen step (static)

Sources: `ScreenPlaneSetupControl` (.xaml and .xaml.cs),
`DisplaySetupLineHandleView`, `ScreenPlaneSetupViewModel`,
`MonitorMappingViewModel`, `MonitorMappingModel` and
`ResolveConfigurationFlow` in `Tobii.Configuration.exe`; texts from
`Tobii.Configuration.Common.Localization.LanguageResources`.

Where it sits in the app's flow. `ResolveConfigurationFlow.InitAndRunFlow`
runs the display setup when the configuration status is
`InvalidMonitorConfiguration`; when that flow succeeds it waits up to 2 s for
the status to change and then runs profile creation or recalibration when the
status is `IncompleteUserProfile` or `NoCalibration`. So display setup comes
first and calibration follows it. Albedo's log shows the same order on
2026-03-29 (display setup finished at 21:12:04, calibration of the new
profile right after). Inside the display setup, `MonitorMappingViewModel` goes
Loading (device info, 2110, the stored mapping), Mapping (the lines),
SavingConfiguration (1440 and the display id) and TrackboxVisualization (a
screen asking the user to sit upright in front of the screen) before it
reports success. Before Mapping it warns, with a yes/no box offering the
display settings, when the screen's resolution does not have the panel's
native aspect ratio (`VirtualScreen.MonitorHasRecommendedAspectRatio`).

Intro. Only when no monitor has been mapped yet (`ShowIntro` is set when the
persisted monitor id is empty), the page asks where the eye tracker is, tells
the user to attach it now if they have not, and offers a "This display"
button plus previous/next arrows to move to another monitor. Choosing the
display animates the tracker picture down into place.

Layout. The control fills the monitor. A picture of the tracker model (for
the Eye Tracker 5 the `EyeTracker5` vector drawing, 812.7 x 72.2 units, white
outline on black) sits with its bottom 150 px above the bottom of the screen
(`MarginFromEyeTrackerToBottomOfScreen`), centred between the lines and
`TotalWidth / EyeTrackerWidth` = 286 / 184 times as wide as their distance
(`EyeTrackerToLineWidthProportion`, `TrackerLeft`, `TrackerWidth`). Because
that proportion is above 1, `LineIsOnSide` is false and the lines start a
quarter of the picture's height below its top
(`OffsetFromEyeTrackerTopToDragLineTop`) and run down to the bottom edge of
the screen. Each line handle is a 1 px dashed line (dash 5) in the foreground
colour, highlighted while the mouse is over it, with a small filled triangle
on each side and a "V" arrowhead at the bottom pointing at the real tracker.
The instruction for two-guide trackers (Eye Tracker 5, PCEye 5, G6 large, H5)
asks the user to move the lines to the marks on top of the eye tracker; one
guide (G6 small): to its sides. A Done button (the default button, so Enter)
confirms. When the step opens, the lines slide in from 0.01 outside their
positions over 1.5 s.

Initial positions. From `MovingLinesScreenPlaneSetupFactory` (see "The
wizard"): from the stored mapping when valid, otherwise centred with the
EDID width or 376.3 mm.

Interaction. Mouse only (`WindowPreviewMouseDown/Move/Up`): pressing within
half a handle's width of a line, below the line's top, drags that line;
pressing on the tracker picture drags both, keeping the distance from the
pointer to each line. The cursor turns into a horizontal resize cursor over a
line or the picture. Leaving the window ends a drag. There is no keyboard
handling and no step size: positions follow the pointer in device-independent
pixels and are stored as fractions of the control width. Limits come from the
view model setters: each line in 0.02 to 0.98 of the width and the lines at
least 0.05 apart (`MinRelativeLinePosition`, `MaxRelativeLinePosition`,
`MinRelativeDistanceBetweenLines`). There is no minimum or maximum monitor
width and no aspect check at this point: Done always takes width = 184 /
(right - left), height = width / aspect, where the aspect is the stored or
estimated one from the start of the step.

## Linux implementation

`src/tobii_linux/display_setup.py`:

- `compose_display_area(size_mm, mounting, tracker_offset_mm)` and
  `decompose_display_area(area, mounting)`: the formula above.
- `size_from_tracker_lines(left, right, tracker_width, aspect)` and
  `tracker_lines(...)`: the wizard's moving lines.
- `edid_size_mm`, `edid_native_resolution`, `size_hint_is_plausible`,
  `drm_monitors()` (EDIDs under `/sys/class/drm/*/edid`) and
  `size_from_diagonal(inches, aspect)`.
- `display_area_from_reply` (1430), `mounting_from_reply` (2110),
  `read_display_area`, `read_mounting`, `apply_display_area` (1440 then 1430).
- `screen_plane_setting`, `save_display_area` and `load_display_area`: files
  in the `screenplane.setpm` layout (a copy of the Windows file works), or a
  text file with the nine numbers.

`tools/display_setup.py`:

```sh
python3 tools/display_setup.py show
python3 tools/display_setup.py compute --connector card0-DP-1 --offset -1.88 --save area.setpm
python3 tools/display_setup.py compute --diagonal 27 --aspect 16:9 --lines 0.34 0.66
python3 tools/display_setup.py apply --edid /sys/class/drm/card0-DP-1/edid --offset -1.88 [--dry-run]
python3 tools/display_setup.py apply --file area.setpm [--dry-run]
python3 tools/display_setup.py apply --from-device
tobii-linux serve --display $(python3 tools/display_setup.py args area.setpm)
```

`show` reads 1430 and 2110 and prints the size and offset they decompose to.
`compute` is offline and uses the Eye Tracker 5 mounting unless `--mounting`
gives another; `apply` reads the mounting from the device. The tracker offset
is the horizontal position of the tracker centre relative to the screen
centre, positive to the right; `--lines` takes the wizard's line positions
instead and, like the wizard, derives the width from them. `args` prints a
saved area in the order `--display` takes, so `serve`, `headpose` and the
demo use it without writing anything to the device.

### The setup page

`tobii-linux setup` ([setup flow](setup-flow.md)) runs this step inside the
guided setup with Tobii's layout and asks before writing; the tool below
remains for scripted use.

```sh
python3 tools/display_setup.py setup [--edid FILE | --connector NAME] [--output display-area] [--apply] [--then-calibrate]
```

`setup` reads 1430 and 2110, serves `src/tobii_linux/display_setup_page.py`
on port 8776 (open `http://localhost:8776/` fullscreen on the display the
tracker is under; under WSL a Windows browser reaches it through mirrored
networking) and waits for the user:

- The page asks "Where is your eye tracker?" only when the device holds no
  display area (`display_setup.is_mapped`), as Tobii shows its intro only
  while no monitor is mapped; "This display" or Enter continues. Otherwise it
  goes straight to the lines with the instruction from
  `display_setup.line_instructions`. It reports its viewport,
  `devicePixelRatio` and the screen size in device pixels; the canvas is
  drawn at device resolution. Done (or Enter) is refused unless the page
  covers the whole screen (fullscreen, or a kiosk window as large as the
  screen), because line positions are fractions of the page width.
- Initial lines: `display_setup.initial_lines`, the factory's rules: the
  stored area decomposed with the device mounting (size replaced by the EDID
  size when `--edid` or `--connector` gives one), else an estimate from the
  EDID size (only if its aspect matches the screen's pixels) or 376.3 mm
  wide, centred, with the screen's pixel aspect.
- The drawing follows the control's layout (tracker 150 px above the bottom,
  286/184 as wide as the line distance, dashed lines with side triangles and
  a downward arrowhead); the tracker is a plain rounded bar, not Tobii's
  artwork. Dragging works as in the app, with the same limits: pressing near
  a line (the nearest one when both are close) below its top drags only that
  line and selects it; pressing on the tracker, which is labelled "drag to
  move both arrows", drags both. As a Linux addition the arrow keys move the
  selected line, which starts as the left one and is drawn solid and blue,
  by one device pixel (Shift: ten); Tab, L and R select a line, Enter
  confirms and Esc or Cancel cancels. Earlier the page started with both
  lines selected, so the keys moved both. The page shows the resulting size,
  diagonal and tracker offset live.
- On confirm, `display_setup.area_from_lines` computes size, offset and area
  as Done does; the tool saves `OUTPUT.setpm` (screenplane.setpm layout) and
  `OUTPUT.json` (the corners, `display_args` and the full `--display` option),
  prints the 1440 request it would send, and sends it only with `--apply`,
  reading 1430 before and after. `/auto` confirms the initial lines without a
  person, for automated checks.
- `--then-calibrate` then runs `tools/gaze_calibrate.py calibrate` (dry run
  unless `--apply`) and the page switches to the calibration page on port
  8775 once it answers, as Tobii runs calibration after display setup.

## Checks

Emulation (`PYTHONPATH=.local/python:.local/inference:src:tools python3
tools/display_setup_oracle.py`), 8 of 8 match: the seven
`tobii_calculate_display_area_basic` cases above, and the 1440 builder
`0x1402c6f40` with the exact corners albedo's device returned, against
`gaze_device.set_display_area` (188-byte request, byte-identical).

Unit tests (`tests/test_display_setup.py`): the albedo reproduction from the
EDID bytes, decomposition, round trips, the moving-lines arithmetic, EDID and
sysfs parsing, the setting file against albedo's bytes, the 1430/2110 reply
decoders on the live payloads, and the tool's `apply` against a scripted
device (identical rewrite, and a dry run that sends no 1440).

Live (albedo, 2026-10-14 00:36, `evidence/private/display-setup/albedo/display-run.cmd`,
output in `evidence/private/display-setup/live/`):

- `show` before and after: 1430 and 2110 as above, decomposed to 597 x 336 mm
  and -1.87991 mm.
- `apply --from-device`: 1440 with the 1430 corners (188 bytes, SHA-256
  `1f769209...a09b`), then 1430: unchanged, equal to the request bit for bit.
  This is the write the runtime makes at every start.
- `apply --edid <albedo EDID> --offset -1.879913330078125 --dry-run`: the
  area computed from the EDID and the device mounting equals the device's
  (`identical_to_device: true`, same request digest); not sent.
- `apply --diagonal 27 --dry-run`: a different area (597.73 x 336.22 mm,
  centred), reported, not sent.
- `screenplane.setpm` had the same SHA-256 before and after, and the runtime
  logged its normal start ("Restoring screen plane from disk and applying to
  tracker", calibration in sync) when the services came back.

Setup page:

- Unit tests (`tests/test_display_setup.py`): the initial lines from albedo's
  stored area and EDID size (left 0.342747, right 0.650955) reproduce the
  device area bit for bit; lines placed for 597 mm and -1.88 mm give 597 x
  336 mm and the device's y and z exactly; the estimate paths (EDID accepted
  and rejected by the aspect check, default 376.3 mm); the line limits; the
  page server on port 0 (page, `/setup`, a rejected and an accepted
  `/result`, `/status`, cancel); and `setup` against a scripted device, which
  saves the files and sends no 1440.
- Local browser check (headless Chromium driven over the DevTools protocol,
  1280 x 720 at device pixel ratio 2): intro, key press, lines at the
  initial positions, R then three Shift+Right moved the right line by 30
  device pixels (0.0234 of the width), the readout and the confirmed result
  agreed (554.8 x 312.3 mm, +4.75 mm), and the drawing was checked on
  screenshots.
- After the 2026-10-14 fixes (`evidence/private/ui-fix/old-pages-check.mjs`,
  2560 x 1440 and 1280 x 720): straight to the lines without the intro for a
  mapped tracker, Shift+Right moved only the left line, Tab then Left only the
  right line; screenshots `old-*-display-page*.png`.
- Live, without a person (albedo, 2026-10-14 02:02,
  `evidence/private/display-setup/albedo/setup-check.cmd`, output in
  `evidence/private/display-setup/setup-check.out`): headless Edge on Windows
  opened `http://localhost:8776/auto`; the tool read 1430 and 2110, the page
  fetched the initial lines (0.3427472, 0.6509549, source "stored display
  area") and confirmed them; the computed area was identical to the device's
  (`identical_to_device: true`), the saved `display-area.setpm` equals
  albedo's `screenplane.setpm` byte for byte, the 1440 request was only
  reported (188 bytes, SHA-256 `1f769209...a09b`, the same as the live
  identical rewrite) and a `show` afterwards read the same area. Headless Edge
  reported an 800 x 600 screen, which the stored-mapping path does not use.
- `evidence/private/display-setup/albedo/setup-display.cmd` is the
  person-present version: it opens the page in Edge kiosk mode on the
  desktop through `schtasks /it`, runs `setup` without `--apply` (results in
  `setup-person\`) and gives the tracker back to Windows. Not run yet.

## Open questions

- The black-bars adjustment (`_blackBars.AdjustDisplayArea`, monitors driven
  at a non-native aspect ratio) was not recovered.
- When `TrackerWriterHelper.ShouldWriteDisplayArea` lets the EyeX engine
  write the area was not traced; the runtime's own start-up write covers the
  Eye Tracker 5 either way.
- 1460 (`display_area_writable`, slot `0x190`) is not queried before 1440.
