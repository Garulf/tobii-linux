# Setup flow

What a user meets when Tobii's Windows software sets up an Eye Tracker 5,
and how `tobii-linux setup` gives the same experience on Linux on one
browser page. The device side of each step is in
[display setup](display-setup.md) and [gaze calibration](gaze-calibration.md).

Sources (static): the decompiled `Tobii.Configuration.exe` and
`Tobii.Configuration.Common.dll` in `evidence/private/calibration/decompiled/`,
their English `LanguageResources`, and the theme of `Tobii.EyeX.Branding.dll`
(`themes/colorsfonts.xaml`, `buttons.xaml`, `texts.xaml`). Labels as in
[gaze calibration](gaze-calibration.md#evidence): everything below about
Tobii is **static** unless marked.

## Tobii's flow (static)

### How it starts

`Tobii.Configuration.exe` takes one flag (`FlowTypeResolver`, exact match in
this order): `-C` recalibrate, `-N` new profile, `-P` eye positioning only,
`-G` guest, `-Q` quick, `-R` resolve configuration, `-S` display setup;
optionally `-eyeTrackerUrl URL`. Exit codes (`ConfigurationResult`): 0
success, 1 argument error, 2 aborted, 3 configuration error. There is no URI
scheme. `Tobii.EyeX.ConfigLauncher` starts it with one flag and waits for
the exit code.

`ResolveConfigurationFlow` (`-R`) is the new-user path:

1. `InvalidMonitorConfiguration` (no monitor mapped): display setup
   (`SetupDisplayFlow`). On success it polls the configuration status for up
   to 2 s and dispatches again.
2. `IncompleteUserProfile` or `NoCalibration`: with no current profile name,
   `CreateNewProfileFlow` (eye positioning, calibration, then naming the
   profile); otherwise `CalibrationFlow(Recalibration)`.

Every screen is a borderless, topmost window covering the tracked monitor.
Esc, Alt+F4 and locking the session cancel (`WindowPresenter`); the mouse
cursor hides after 2 s without movement (`HideInactiveCursorBehavior`).

### Display setup (`MonitorMappingViewModel`, `ScreenPlaneSetupControl`)

States: Loading (spinner, no text), Mapping, SavingConfiguration (spinner),
TrackboxVisualization, Error ("Oops. No connection." with Try again and
Cancel).

- Intro, only when no monitor id is stored (`ShowIntro`): "Where is your eye
  tracker?" and "If you haven't yet attached it to the display, now is the
  time." over a monitor icon with a flat tracker on its bezel, a "This
  display" button and arrows to move to the next monitor. "This display"
  moves the tracker picture into place (1 s), cross-fades to the detailed
  drawing (0.5 s), then fades in the instruction and Done.
- Mapping: "Move the lines to the marks on top of your eye tracker." (one
  guide trackers: "to the sides of"). The lines slide in from 0.01 outside
  over 1.5 s with a back ease. Text sits at the bottom of the second grid
  row (rows 0.35:1:1:2), Done in the third row, the lines in the fourth.
- Lines: 1 px dashed (dash 5) `#AFC4CD`, white while the mouse is over the
  60 px handle, a V arrowhead at the bottom, white side triangles. The
  tracker drawing's two slanted marks end where the lines start. Pressing
  within 30 px of a line below its top drags that line; pressing on the
  tracker drags both. No keyboard, no live size readout, no confirmation:
  Done writes 1440 and the display id, then shows TrackboxVisualization.
- TrackboxVisualization: "Sit in an upright position in front of the
  screen." over a line drawing of a person and the track box, with Done. It
  uses no tracker data.
- When the screen resolution does not have the panel's aspect ratio, a box
  ("We're sorry...", Yes opens the display settings) comes before Mapping.

### Eye positioning (`EyesPositioningViewModel`, `EyesPositioningView`)

- A faint track box (white at 3 %) of golden ratio: height
  min(width, height) / 2.25. The eyes are circles from the user position
  guide (gaze columns 37 to 40), mirrored (x becomes 1 - x).
- `EyesPositioningParametersCalculator`: the distance is the mean z of the
  valid eyes averaged over 3 samples; eye size 100 at z <= 0.2, 50 between
  0.4 and 0.65, 25 at z >= 0.8, linear in between. A missing eye is
  extrapolated from its last two positions within 11 samples. Status, first
  match: no eyes, size <= 25 "Move closer", size >= 100 "Lean back", x <= 0.1
  "Move right", x >= 0.9 "Move left", y <= 0.1 "Move down", y >= 0.9 "Move
  up", else good. Grey level: white at size 50, dark grey at 25 or 100;
  opacity fades over the outer 0.2 of the box (`EyePositioningUtils`).
- Hints change at once, except that after a good position a problem must
  last 11 samples (49 when the eyes are lost). Lost eyes show nothing, then
  "Are you there?" after 2 s and "Adjust your seating position" after 5 s.
- Intro, always: "These are your eyes..." (0.5 to 3 s), the eyes fade in
  (3 to 4 s), "See how much you can move around." (4 to 7 s); hints show
  from 7 s.
- Advancing: for every calibration it closes by itself once the screen has
  been up 6 s, the user has been present more than 3 s (gaps under 0.5 s do
  not count) and the position is good; a key or click advances only while
  the position is good (`ClosedIfValidEyePosition`). The right eye blinks
  (0.4 s) and the screen fades out (1 s).

### Calibration (`CalibrationProcessViewModel`, `CalibrationProcessView`)

- Black screen. For a new profile, "Let's calibrate for your eyes..." (2.5 s).
  Then "Look at each dot until it explodes!" (2.5 s) while a 15 px accent dot
  grows and shrinks in the centre; the first target appears 0.75 s in.
- Targets are 20 accent-coloured (`#008DBF`) particles, the largest 18 px,
  sitting on one point. They fly out from the centre (1.5 s for one target,
  1 s for three). Stage 1 is the centre, then (0.1, 0.9), (0.5, 0.1),
  (0.9, 0.9), then (0.1, 0.1), (0.5, 0.9), (0.9, 0.1).
- Stages 2 and 3 show their three targets at once and the user chooses the
  order by looking: the closest waiting target within 0.45 x half the
  smallest target distance gets focus, focus moves only after 150 ms, 250 ms
  without gaze drops it. On focus the particles squash and spin; 200 ms
  later the tracker collects (1030). If the gaze left during collection the
  data is removed (1080) and the target returns to rest; otherwise it
  explodes. After a stage the device computes (1070) with a spinner after
  0.5 s; 0.7 s later the next stage flies out.
- `CalibrationAssessor`, from stage 2: when for 4 s the gaze neither moved to
  a waiting target nor exploded one, or for 10 s nothing exploded, the whole
  calibration starts again with "Look at each dot until it explodes!" (no
  limit).
- A compute error of operation failed (or no calibration) is "Oops. No
  detection." with four tips, Try again (default, back to the calibration,
  not to positioning) and Cancel. Esc at any time aborts.
- Success: a new profile goes to the naming screen ("Calibration
  successful!", a 15 character name of letters, digits, spaces, brackets
  and dashes, not "Guest", not an existing one, Save); a recalibration shows
  "Calibration successful!" for about 7.5 s and closes. A recalibration of
  an Eye Tracker 5 re-applies the old calibration after clearing (1110), so
  the new points refine it.
- The strings "Your calibration may be improved", "Improve" and "Use anyway"
  exist in the resources but no code uses them: there is no result screen
  with per-point quality.

## Linux flow

```sh
PYTHONPATH=src python3 -m tobii_linux.setup [--display auto|always|never] [--no-calibration]
    [--save-display] [--dry-run] [--profile NAME] [--port 8777] [--no-browser] [--simulate mapped|unmapped]
```

(also `tobii-linux setup`). The command starts the page server first, then
opens the browser (on WSL Edge in kiosk mode through `cmd.exe /c start`, so
the page fills the screen without a click; on Linux `xdg-open`), then opens
the tracker. The page shows "Connecting to the eye tracker..." until the
tracker answers, so it is never blank. Steps:

1. **Display setup** (`--display auto`: only when the device holds no
   display area, as Tobii's intro rule). The intro asks where the tracker is
   (only then), "This display" or Enter moves the tracker into place and the
   lines slide in. Linux additions: the left arrow starts selected (white,
   solid, with a light band), the arrow keys move only the selected one by a
   device pixel (Shift: ten), Tab, L and R switch, dragging the tracker moves
   both; a small readout shows the screen size and tracker offset. Done (or
   Enter) needs the page to cover the screen (fullscreen, or a kiosk window
   as large as the screen). An unchanged area writes nothing. A changed one
   asks "Save the new display setup?" unless `--save-display` was given;
   the old area is saved to `$XDG_DATA_HOME/tobii-linux/backups/` first.
   Tobii's separate "Sit in an upright position" screen becomes a line at the
   top of the positioning screen.
2. **Eye positioning**: `eye_positioning.py` ports the calculator and the
   hint and advance rules above; the flow subscribes gaze and reads it
   continuously, the page draws the eyes 30 times a second. It advances by
   itself under Tobii's rule, or on a key or click while the position is
   good. Linux addition: while the position is good it says so ("Good. Stay
   like this, the calibration starts in a moment, or press any key.").
3. **Calibration**: `Calibrator(choose_by_gaze=True)` shows each stage's
   targets at once, focuses by gaze (`GazeFocus`), waits 200 ms, collects,
   discards when the gaze left, and restarts when the `AttentionWatch` sees no
   progress (at most 5 times, then the failure screen). The page draws the
   particles, focus, explosion, spinner and restart as above. Before the
   first write the device calibration is read and saved to the backups
   directory. A recalibration of the current profile (or `--profile`)
   re-applies the old calibration after clearing, as Tobii does.
4. **Failure**: "Oops. No detection." with the tips, Try again (Enter) and
   Cancel.
5. **Result**: a new profile is named on the page and saved with
   `calibration_profiles` (and made current); a recalibration is saved to
   its profile. The last screen says what happened and Close (or Enter)
   closes the window.

Leaving after the calibration started (Esc, the X, Cancel, a closed page)
puts the previous calibration back (1110) and says so. `--dry-run` reads
from the device and reports every write instead (gaze subscriptions are
allowed so positioning and target choice still work). The server never
leaves the gaze stream unread: the positioning loop reads every 100 ms and
the calibrator keeps its reading loop.

Page and server: `setup/server.py` publishes screens with a sequence number;
the page acknowledges each screen once drawn (calibration events after their
animation), and sends actions with the screen they belong to. Texts are set
once per screen and never cleared and rewritten, so nothing flickers. The
page reports its size, the screen size and fullscreen state with every event.

Differences from Tobii, all deliberate: one page instead of separate
windows; no monitor switching (the browser window is on one display); no
aspect-ratio warning; the display area is only written after the user
confirms (or `--save-display`); the size readout and keyboard handling on the
display step; the "Good" line on the positioning screen; the restart limit;
the previous calibration put back on cancel (Tobii's runtime restores it
from `calibration.setpm` when the services start); the proximity modifier of
the focus zone and the 3 s extra delay before "Are you there?" at the very
start are not reproduced.

## Checks

- Unit tests: `tests/test_eye_positioning.py` (sizes, mirror, status order,
  extrapolation, colours, hint delays, lost-eye messages, the advance rule)
  and `tests/test_setup_flow.py` (the whole flow against
  `simulated_tracker.SimulatedTracker` with a scripted page: first setup with
  display, mapped tracker, unchanged and declined display areas,
  `--save-display` with backup, recalibration with 1110 after 1060, all points
  failed and Try again, cancel restoring the calibration, cancel while
  positioning writing nothing, profile name rules, dry run; the page channel;
  the browser launch command; gaze-chosen target order and the attention
  watch).
- Headless Chromium (2026-10-14, `evidence/private/ui-fix/flow-check.mjs`
  and `failure-check.mjs` against `--simulate unmapped` and a failing
  simulated tracker, 2560 x 1440 and 1280 x 720, screenshots in
  `evidence/private/ui-fix/`): keyboard focus without a click, the kiosk-size
  window accepted as fullscreen, Shift+Right moved only the left line by 30
  px, Tab then Left only the right one, pressing near the left line dragged
  only it, dragging the tracker moved both, pressing above a line's top
  grabbed nothing, "Move closer" while the simulated user sat too far, the
  calibration started 9.7 s after positioning appeared without input, the
  profile field had focus and refused "Guest", no static text was written
  twice, Try again had focus and Esc afterwards put the calibration back.
- Not yet run with the tracker or a person.
