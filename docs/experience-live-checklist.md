# Tobii Experience live checklist

Checks for the Tobii Experience app (`tobii-linux-experience`) and the always-running
service (`tobii-linux serve`) on a real Eye Tracker 5. Nothing here has been run: the
apps were built and tested against scripted trackers, Xvfb and a stub service only. Each
item says what to do, what Tobii's software does in the same situation (so a difference
is easy to spot), and what to write down.

The items marked **open question** test something the code had to guess about the
tracker. Their answers decide whether the service needs a change.

## Before you start

The NixOS module with the app enabled, on a Hyprland session:

```nix
hardware.tobii-eye-tracker-5 = {
  enable = true;
  experience.enable = true;
};
```

This runs the `tobii-head-pose` user unit (`tobii-linux serve`) from login and the
`tobii-linux-experience` user unit with `graphical-session.target`. Without that target
(Hyprland started without uwsm), add `exec-once = tobii-linux-experience` to the Hyprland
config ([launching games](game-launch.md#tobii-experience)). The display setup and a
calibration should already be done with `tobii-linux-setup`.

Keep two terminals open while testing:

```sh
journalctl --user -fu tobii-head-pose        # the service log
journalctl --user -fu tobii-linux-experience # the app log (when it runs as a unit)
```

Useful paths:

| What | Where |
| --- | --- |
| On/off state | `~/.local/state/tobii-linux/eye-tracking-enabled` (`0` off, anything else or no file on) |
| Profiles | `~/.local/share/tobii-linux/profiles/` |
| Recorded monitor | `~/.local/share/tobii-linux/display-setup.json` |

Put the notes, logs and screenshots for each item in
`evidence/private/experience/live/<date>/`, one file per item named after its heading.
For every item record the date, the commit, and whether the tracker is plugged in
directly or through a hub.

## 1. Start with the session

Do: log in. Wait 10 s.

Expect: the tray icon appears, shows the `Progress` icon with `Tobii Experience -
Loading…` for a moment, then the blue mark with `Tobii Experience - In use`. Tobii's
tray does the same: Initializing until the engine reports Tracking
([experience-app.md](experience-app.md), section 1.1, and 1.2 for the status table).

Record: `systemctl --user status tobii-head-pose tobii-linux-experience`, the first 30
lines of the service log, and how long the tray took to show `In use`.

## 2. Tray in Waybar, every state

Do: make sure Waybar has the `tray` module. Bring the tracker into each state below and
hover over the icon, then right-click it.

| State | How to get there | Icon | Tooltip | Second menu item |
| --- | --- | --- | --- | --- |
| Tracking | plugged in, on, display set up, a profile | blue mark | `Tobii Experience - In use` | `Disable Tobii Experience` |
| Disabled | `Disable Tobii Experience` in the menu | mark with a red cross | `Tobii Experience - Disabled` | `Enable Tobii Experience` |
| Unplugged | pull the USB cable | mark with a red cross | `Tobii Experience - No sensor connected` | depends on on/off |
| Setup open | `Improve calibration` on the page | `Progress` | `Tobii Experience - Configuring…` | depends on on/off |
| Service stopped | `systemctl --user stop tobii-head-pose` | mark with a red cross | `Tobii Experience - Please troubleshoot` | `Enable Tobii Experience` |
| Disabled and unplugged | disable, then pull the cable | mark with a red cross | `Tobii Experience - No sensor connected` | `Enable Tobii Experience` |

Expect: the table above, which is Tobii's mapping (sections 1.1, 1.2 and 1.4). The menu
always has `Open Tobii Experience`, the enable/disable item and `Quit`. A menu left open
while the state changes should update its first two items in place, as Tobii's does
(section 1.4). The `Progress` icon is a static image on Windows too (section 1.3).

Record: a screenshot of the icon and tooltip in each state; whether Waybar picks the
right icon size (the PNGs are 33 px and 16 to 64 px for `Progress`); whether a left
click opens the page; whether the menu updates while open.

## 3. Disable while a game runs

Do: start a game, or `prp_test_client.exe` under Wine
([prp-bridge.md](prp-bridge.md#launch-modes)), so head pose and gaze flow. Then use
`Disable Tobii Experience` in the tray. After 20 s enable it again.

Expect: as on Windows, turning tracking off pauses the tracker for every client, games
included (section 2, effects 2 and 5): the game loses head and gaze tracking, the
service log shows `tracker paused`, and property 4 (device paused) reads 1, with a
notification to clients subscribed to it ([control-channel.md](control-channel.md),
Requests; [service-behaviour.md](service-behaviour.md#power-saving)). Enabling resumes
the tracker (`tracker resumed`) and the game gets samples again without reconnecting.

Record: what the game shows while disabled; with `prp_test_client.exe`, its sample counts
before, during and after. The client prints samples and a summary but does not read
property 4 itself; if a run with a client that reads `TOBII_STATE_DEVICE_PAUSED` is
possible, record the value. Note whether the game reconnected or showed an error.

## 4. Paused tracker with streams subscribed (open question)

The service pauses a tracker (3100 with 1) at the start of every session while tracking
is off, then subscribes its streams as usual. Whether a real ET5 stays paused after those
subscriptions has not been seen.

Do: disable tracking in the tray. Restart the service:
`systemctl --user restart tobii-head-pose`. Watch the log for 30 s. Then unplug and
replug the tracker and watch another 30 s.

Expect: Tobii pauses a tracker that connects while tracking is off as soon as it is added
(section 2, effect 3). The log should show `eye tracking is disabled; pausing the
tracker`, the session staying up (no `tracker session ended` lines every few seconds),
and the tray at `Disabled`. The LED behaviour of the tracker is not documented;
note it.

Record: the service log for both phases; whether the tracker's LEDs are on; whether any
gaze or image data arrives while paused (the demo or `CONTROL_GAZE` subscribers would see
it); if the session ends and restarts in a loop, how often.

## 5. Pause after the USB session closes, and a crash while disabled (open question)

An enabled session start sends no resume (3100 with 0). If a pause survives the USB
session closing, a tracker left paused by a service that died while tracking was off
would stay silent after an enabled restart, and the service would keep ending and
restarting the session.

Do, clean close:

1. Disable tracking in the tray.
2. `systemctl --user stop tobii-head-pose` (the service unsubscribes and closes USB).
3. `echo 1 > ~/.local/state/tobii-linux/eye-tracking-enabled`
4. `systemctl --user start tobii-head-pose` and watch the log for 30 s.

Do, crash:

1. Disable tracking in the tray.
2. `systemctl --user kill -s KILL tobii-head-pose && systemctl --user stop tobii-head-pose`
   (the second command stops the automatic restart).
3. `echo 1 > ~/.local/state/tobii-linux/eye-tracking-enabled`
4. `systemctl --user start tobii-head-pose` and watch the log for 30 s.

Expect: on Windows the engine resumes the tracker when tracking is turned back on
(section 2, effect 2); it does not have this restart case. If the pause does not survive,
both runs reach `tracker attached` and the tray shows `In use`. If it survives, the log
shows `tracker session ended` on silence again and again; toggling Disable and Enable in the
tray, or replugging the tracker, should end it.

Record: the log of both runs; which recovery worked if the session looped.

## 6. Display area read while paused or missing (open question)

Each session reads the display area from the tracker once (1430), with a 1 s timeout,
unless the module's `displayArea` is set ([rust.md](rust.md), the `serve` entry); the
result is the `display_area` field of the status
([control-channel.md](control-channel.md), Status and events). What a paused tracker
answers has not been seen.

Do: with tracking disabled, restart the service. Look for `display area from the
tracker` or `the tracker holds no display area` in the log. Then enable tracking.

Expect: the tray goes from `Disabled` to `In use`. If the read failed while paused, the
session keeps running without a display area and the tray shows
`Tobii Experience - Action needed` after enabling (no display area maps to
InvalidConfiguration, section 1.2) until the next session.

Also, if a spare tracker or a test setup without a display area is available: start the
service with it. Expect `the tracker holds no display area` and `Action needed`.

Record: the display-area log lines in both runs, and the tray state after enabling.

## 7. Release to setup on nusb

`tobii-linux-setup` asks the service to let go of the tracker before it opens USB
([control-channel.md](control-channel.md), Release lifecycle). This was only tested with
scripted trackers.

Do: open the page (left click on the tray icon) and press `Improve calibration`. Finish
or cancel the calibration.

Expect: the tray switches to `Configuring…`, the service log shows `releasing the tracker
to control client setup` and then `tracker released to control client setup`, and setup
opens the tracker within a second or two without a USB claim error (`Device or resource
busy`). After
setup closes, the service attaches again (`tracker attached`) and the tray returns to
`In use`. Tobii's configuration app runs while the engine shows Configuring the same way
(section 1.2).

Record: both logs (setup prints to the terminal when started by hand:
`tobii-linux-setup -C`); the time between the release reply and setup's first device
message; any USB error.

## 8. Quit and relaunch

Do: note the content of the on/off file. Tray, `Quit`. Read the dialog, then press
`Quit Tobii Experience`. Afterwards run `systemctl --user is-active tobii-head-pose`.
Then start the app again from the application launcher (`Tobii Experience`) or by
running `tobii-linux-experience`. Repeat once with tracking disabled, and once with
`Don’t show this again.` checked.

Expect: the dialog has Tobii's texts (section 3, Strings) and is centred; it is not
topmost (a GTK 4 limit, section 3). After Confirm the icon shows `Configuring…`, then the
app exits and the service unit is `inactive`; games lose the tracker. The on/off file is
unchanged: Quit is not Disable (section 3, spec item 7). Relaunching starts the service
unit again and the tray returns to the state before the quit, `Disabled` included. With
the checkbox ticked, the next Quit goes straight to the stop.

Record: screenshots of the dialog; the unit state after quit and after relaunch; the
on/off file before and after; how long the relaunch took to reach `In use`.

## 9. Profile switch and tracked eyes

Do: with at least two profiles, open the page and click a listed profile. Then open
`Select eyes to detect` and choose `Left eye only`; check the gaze preview (item 10) with
the right eye closed, then the left. Set `Both eyes` again.

Expect: as in Tobii's page (section 4.6): the click makes it current, the list then shows
the other profiles, and the header shows the new name. The service log shows
`profile <name> applied (calibration id ...)` (3160 with the profile's eyes, then 1110,
[control-channel.md](control-channel.md), Requests). With `Left eye only` the gaze point
follows the left eye only.

Record: the log lines; whether the calibration id changed; whether one-eye tracking
behaves as chosen.

## 10. Tracked eyes across a power cycle (open question)

When a session starts, the service skips both writes (3160 and 1110) if the tracker
already reports the profile's calibration id. Whether the tracked-eye setting (3160)
survives the tracker losing power is unknown; if it does not, a one-eye profile would
silently track both eyes after a replug.

Do: set `Right eye only`. Unplug the tracker for 10 s and plug it in again. Check the
gaze preview with each eye closed in turn.

Expect: the log shows `profile <name> already on the tracker` (ids match, nothing
sent). Tracking should
still follow the right eye only.

Record: the apply log line; whether one-eye tracking survived the replug.

## 11. Gaze preview: placement, click-through and monitor

Do: on the page, turn on `Preview my gaze` while the page window is focused. Look around
the monitor the tracker is mounted on. Click on a window under the bubble. Move focus to
another window, then back. With two monitors, check that the bubble stays on the tracked
one. If possible, run the Windows bubble on the same monitor for comparison.

Expect: Tobii draws the bubble only while the preview is on and its window is active and
not minimized (section 4.8). On Linux it also needs `In use` and the recorded monitor
connected. The bubble is a click-through layer-shell overlay on the monitor recorded by
the display setup; clicks reach the window underneath, and keyboard focus stays where it
was ([gaze-preview.md](gaze-preview.md#linux-port)). It disappears when the page loses
focus and comes back when it regains it.

Record: a short screen recording; whether the bubble sits under the gaze point and on the
right monitor; whether clicks pass through; size, colour and trail compared with Windows;
the app log line naming the GL path (`accumulation R32F` or similar).

## 12. Minimizing the page (X11 only)

Wayland (Hyprland included) cannot report a minimized window, so this needs an X11
session or an X11 window manager. Skip it on Hyprland and write "not available".

Do: on X11, mark a profile for deletion with the trash button, then minimize the page.
Restore it.

Expect: as on Windows, hiding the page commits marked deletions; minimizing counts as
hiding where the backend reports it (sections 4.1 and 4.6). After restoring, the deleted
profile is gone from the list and from the profiles directory.

Record: the list before and after, and the profiles directory listing.

## 13. Guest calibration bind

Do: add `bind = CTRL SHIFT, F10, exec, tobii-linux-experience --guest` to the Hyprland
config. Press it while the tray shows `In use`, then while it shows `Disabled`.

Expect: Tobii's shortcut only works in Tracking or InvalidConfiguration (section 6), and
the Linux app applies the same gate. With `In use`, setup starts its guest calibration;
with `Disabled`, nothing happens. Pressing again while setup runs does nothing. When the
app is not running, the bind starts it and the guest run is skipped; press it again
once the tray shows `In use`.

Record: what happened in each state, and the profile list afterwards (`Guest` is never
listed on the page).
