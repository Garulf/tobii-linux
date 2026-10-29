# Tobii Experience: tray icon, on/off, Eye tracking page, quit and guest

What Tobii's own software does for the notification-area icon, the
enable/disable switch, the "Eye tracking" settings page, Quit and the Guest
profile, recorded as a ledger that the `tobii-linux-experience` app copies its
states, strings, styles and rules from. The design is in the spec
(`2026-10-18-experience-app-design.md`); the setup flows that the page launches
are in [setup flow](setup-flow.md) and [gaze calibration](gaze-calibration.md);
the device-side pause is in [service behaviour](service-behaviour.md) and
[USB streams](usb-streams.md).

Every Tobii row below is **static**: read from decompiled .NET assemblies and
their English resources, never executed. Nothing here is emulated or live. The
`Linux` column says what the Linux app does with the row:

- `same`: reproduce as read.
- `differs (why)`: Tobii's row cannot apply as is on Linux.
- `omitted (why)`: not reproduced.
- `(decision)`: fixed by the user or the design. `(proposal)`: my reading, for the
  task that implements the row to confirm.

## Sources and citation key

Tobii Experience Store app `TobiiAB.TobiiEyeTrackingPortal` 1.69.32600.0 and
the offline installer `Tobii.EyeTracker5.Offline.Installer_4.183.0.30025.msi`,
as described in the spec. All paths are under the gitignored
`evidence/private/experience/`. Citations read `KEY:file:line` (or `KEY:line`
for a single-file key); `KEY` stands for the directory below.

| Key | Directory (or file) |
| --- | --- |
| `VM` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.ViewModels/` |
| `VIEW` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Views/` |
| `DEV` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Device/` |
| `CFG` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Configuration/` |
| `EXT` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.ExtensionMethods/` |
| `UTIL` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Utils/` |
| `SIVM` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.ViewModels.SettingItemsViewModels/` |
| `SIV` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Views.SettingItems/` |
| `GT` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.GazeTrace/` |
| `CTRL` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Controls/` |
| `WIN` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF/` |
| `LOC` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/Tobii.EyeTracking.Portal.WPF.Localization.LanguageResources.resx` (page strings, English) |
| `TH` | `portal/decompiled/Tobii.EyeTracking.Portal.WPF.exe/themes/` |
| `BR` | `portal/decompiled/Tobii.EyeX.Branding.dll/themes/` (shared styles) |
| `BRU` | `portal/decompiled/Tobii.EyeX.Branding.dll/Tobii.EyeX.Branding.Utils/` |
| `PROC` | `portal/decompiled/Tobii.Tech.NETCommon.Processes.dll/Tobii.Tech.NETCommon.Processes.ConfigurationApp/` |
| `IO` | `portal/decompiled/Tobii.Tech.NETCommon.IO.dll/Tobii.Tech.NETCommon.IO/` |
| `UTILP` | `portal/decompiled/Tobii.Tech.NETCommon.Processes.dll/Tobii.Tech.NETCommon.Processes/` |
| `TRAY` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea/` |
| `ICON` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea.ExperienceNotifyIcon/` |
| `IMG` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea.IconImage/` |
| `BOOT` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea.Bootstrapping/` |
| `QD` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea.ConfirmationDialog/` |
| `QDVM` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea.ConfirmationDialog.ViewModels/` |
| `QDV` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.NotificationArea.ConfirmationDialog.Views/` |
| `MODPROP` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.Properties/` |
| `MODRES` | `decompiled/Tobii.EyeX.Interaction.Modules.dll/Tobii.EyeX.Interaction.Modules.Properties.Resources.resx` (tray icons) |
| `FRES` | `decompiled/Tobii.EyeX.Interaction.Foundation.dll/Tobii.EyeX.Interaction.Foundation.UI.Common.Localization.LanguageResources.resx` (tray and quit strings, English) |
| `GUEST` | `decompiled/Tobii.EyeX.Interaction.Foundation.dll/Tobii.EyeX.Interaction.Foundation/` |
| `FDEV` | `decompiled/Tobii.EyeX.Interaction.Foundation.dll/Tobii.EyeX.Interaction.Foundation.Device/` |
| `FCORE` | `decompiled/Tobii.EyeX.Interaction.Foundation.dll/Tobii.EyeX.Interaction.Core.Utils/` |
| `INTBOOT` | `decompiled/Tobii.EyeX.Interaction.exe/Tobii.EyeX.Interaction.Bootstrapping/` |
| `SET` | `decompiled/Tobii.EyeX.Interaction.Settings.dll/Tobii.EyeX.Interaction.Settings.Configuration/` |
| `TDC` | `decompiled/Tobii.Tech.NETCommon.TrackingDevice.dll/Tobii.Tech.NETCommon.TrackingDevice.Config/` |
| `TDI` | `decompiled/Tobii.Tech.NETCommon.TrackingDevice.dll/Tobii.Tech.NETCommon.TrackingDevice.Config.Internal/` |
| `TDG` | `decompiled/Tobii.Tech.NETCommon.TrackingDevice.dll/Tobii.Tech.NETCommon.TrackingDevice.GazeTrace/` |
| `CONF` | `decompiled/Tobii.Configuration.exe/Tobii.Configuration/` |
| `CFLOW` | `decompiled/Tobii.Configuration.exe/Tobii.Configuration.ConfigurationFlows/` |
| `CCAL` | `decompiled/Tobii.Configuration.exe/Tobii.Configuration.Calibration/` |
| `CNP` | `decompiled/Tobii.Configuration.exe/Tobii.Configuration.CreateNewProfile.ViewModels/` |
| `CCONT` | `decompiled/Tobii.Configuration.Contract.dll/Tobii.Configuration.Contract/` |
| `CCOM` | `decompiled/Tobii.Configuration.Common.dll/Tobii.Configuration.Common.Calibration/` |
| `CCOMRES` | `decompiled/Tobii.Configuration.Common.dll/Tobii.Configuration.Common.Localization.LanguageResources.resx` |
| `ENG` | `decompiled/Tobii.EyeX.EyeTracking.dll/Tobii.EyeX.EyeTracking/` |
| `ENGDC` | `decompiled/Tobii.EyeX.EyeTracking.dll/Tobii.EyeX.EyeTracking.DevicesCommunication/` |
| `CORE` | `decompiled/Tobii.EyeX.Core.dll/Tobii.EyeX.Core/` |
| `SRV` | `decompiled/Tobii.EyeX.Server.Common.dll/Tobii.EyeX.Server.Common/` |
| `SVC` | `decompiled/Tobii.Service.exe/Tobii.Service/` |
| `MODEL` | `decompiled/Tobii.Interaction.Model.dll/Tobii.Interaction.Framework/` |
| `MSI` | `msi/` |
| `ASSETS` | `assets/tray/` |

Three assemblies were decompiled for this ledger from the unpacked Store app
(`portal/app/Tobii.EyeTracking.Portal.WPF/`) with `ilspycmd` 11.1.0, each into
`portal/decompiled/<assembly>/`: `Tobii.EyeX.Branding.dll` (SHA-256
`fa95551784eea3a5727728f1c4a351e3ccb97cfcce9769b99c51d659a9b56e57`; its BAML
comes out as XAML under `themes/`), `Tobii.Tech.NETCommon.Processes.dll`
(`f77ecba7e590a09582b6fd26843beefcbe7478972d52c0ee8a742b64a5e977e7`) and
`Tobii.Tech.NETCommon.IO.dll`
(`80ee620020f54eab954123957fdbe1a7e3f3cf368212f3ae5e585d329447b68c`). The
portal XAML (`SettingsPage.xaml`, `HomePage.xaml`, `MainWindow.xaml`,
`themes/*.xaml`) is already in the decompiled output; none of it was BAML-only.

Units: WPF lengths are device-independent pixels (1/96 inch), so 1 pt =
4/3 px: 10 pt = 13.33 px, 12 pt = 16 px, 16 pt = 21.33 px.

Where the brief's questions are answered:

| Question | Section |
| --- | --- |
| (a) state, tooltip and icon mapping | 1.1, 1.3 |
| (b) tray menu | 1.4 |
| (c) quit dialog and failure message box | 3 |
| (d) page sections, strings, visibility and sensitivity | 4.2, 4.3 |
| (e) styles | 5 |
| (f) profile list rules | 4.6 |
| (g) Guest, Ctrl+Shift+F10 and `-G` | 6 |
| (h) configuration arguments and exit codes | 4.7 |
| (i) spinner | 1.3 |
| (j) 500 ms delay and busy marker | 4.5 |
| (k) state and status values | 1.2, 4.4 |

## 1. Tray icon

Owner: `Tobii.EyeX.Interaction.exe` (the "Interaction" process) loads
`Tobii.EyeX.Interaction.Modules.dll`. The feature is `NotificationAreaIconFeature`,
registered under the user setting `NotificationArea.Icon`
(`TRAY:NotificationAreaModule.cs:16-19`). The tray strings are in the
Foundation resource `FRES`.

### 1.1 Device status to icon state, icon and tooltip

`NotificationAreaIconFeature.Update` picks the state
(`TRAY:NotificationAreaIconFeature.cs:235-250`): if the EULA is not accepted and
the app is not quitting, GetStarted; otherwise the device status (null when the
engine has no valid value, then `UnknownError`), replaced by `Configuring` while
quitting, goes through `ExperienceNotifyIconStateMapper`
(`ICON:ExperienceNotifyIconStateMapper.cs:53-95` status to state,
`:17-51` state to icon name and tooltip, `:9-15` icon names).

| Icon state | Selected by device status | Icon | Tooltip (key, line) | Linux |
| --- | --- | --- | --- | --- |
| GetStarted | none: only "EULA not accepted and not quitting" (`TRAY:NotificationAreaIconFeature.cs:239-243`) | `GetStarted` | `Tobii Experience - Get started` (`NotificationAreaIcon_Tooltip_GetStarted`, `FRES:45`) | omitted (decision): no EULA on Linux, never shown |
| Configuring | `Configuring`; also forced while quitting (`:245-248`) | `Progress` | `Tobii Experience - Configuring…` (`NotificationAreaIcon_Tooltip_Configuring`, `FRES:52`) | same: shown while setup holds the tracker and while quitting (decision, spec "Changed during planning") |
| Disconnected | `DeviceNotConnected` | `Disabled` | `Tobii Experience - No sensor connected` (`NotificationAreaIcon_Tooltip_Disconnected`, `FRES:48`) | same |
| Error | `ConnectionError`, `NotAvailable`, `TrackingUnavailable`, `UnknownError`; also a null status (`:244`) | `Disabled` | `Tobii Experience - Please troubleshoot` (`NotificationAreaIcon_Tooltip_Error`, `FRES:32`) | same; an unreachable service is the null status (spec decision 3) |
| Initializing | `Initializing` | `Progress` | `Tobii Experience - Loading…` (`NotificationAreaIcon_Tooltip_Initializing`, `FRES:30`) | same |
| InvalidConfiguration | `InvalidConfiguration` | `Disabled` | `Tobii Experience - Action needed` (`NotificationAreaIcon_Tooltip_InvalidConfiguration`, `FRES:53`) | same: no display area on the device (spec) |
| Tracking | `Tracking` | `Enabled` | `Tobii Experience - In use` (`NotificationAreaIcon_Tooltip_Tracking`, `FRES:39`) | same |
| TrackingPaused | `TrackingPaused` | `Disabled` | `Tobii Experience - Disabled` (`NotificationAreaIcon_Tooltip_TrackingPaused`, `FRES:41`) | same |

The separator in every tooltip is an ASCII hyphen with a space on each side.
The enum order of the states is GetStarted, Tracking, Error, TrackingPaused,
InvalidConfiguration, Initializing, Disconnected, Configuring
(`ICON:ExperienceNotifyIconState.cs:5-12`).

When `Update` runs (`TRAY:NotificationAreaIconFeature.cs`):

- once at start (`:65-80`), on every device-status change and every
  enabled-state change (`:223-233`), on EULA change (`:104-108`) and on display
  settings change (`:98-102`);
- on display settings change the handler first calls `Reset`, which swaps in a
  temporary `Disabled` icon and clears the icon cache
  (`ICON:ExperienceNotifyIconHandler.cs:114-124`). Linux: differs (proposal):
  nothing to rebuild when the icon is a named or pixmap property;
- `SetNotificationState` writes the icon and the tooltip (`NotifyIcon.Text`) on
  every call (`ICON:ExperienceNotifyIconHandler.cs:47-65`).

### 1.2 Where the status comes from

The engine's host state becomes `EyeTrackingDeviceStatus`
(`SRV:EyeTrackingConfigurationProvider.cs:253-278`); the host state comes from
the engine state (`ENG:TrackerComponentHost.cs:512-551`). This is what the
Linux service has to report in order to drive the tray and the page.

| Engine state | Host state | `EyeTrackingDeviceStatus` | Tray state |
| --- | --- | --- | --- |
| `Tracking`, configuration valid or no gaze component | `Tracking` | `Tracking` (5) | Tracking |
| `Tracking`, gaze component with `GazeTrackingValidationStatus` not `ConfigurationValid` | `WaitingForValidConfiguration` | `InvalidConfiguration` (3) | InvalidConfiguration |
| `WaitingForDevice` | `WaitingForDevice` | `DeviceNotConnected` (4) | Disconnected |
| `WaitingForConnection` | `WaitingForConnection` | `Initializing` (1) | Initializing |
| `Idle` (what `DisableEyeTracking` sets, `ENG:EyeTrackingEngine.cs:265-285`) | `WaitingForEyeTrackingToBecomeEnabled` | `TrackingPaused` (6) | TrackingPaused |
| `Configuring` | `Configuring` | `Configuring` (7) | Configuring |
| `Error` | `UnknownError` | `UnknownError` (8) | Error |
| `ConnectionError` | `ConnectionError` | `ConnectionError` (9) | Error |
| `Strobe` | `Strobe` | `TrackingUnavailable` (10) | Error |
| any other engine state (`NotInitialized`, `Initialize`, `ConnectToEyeTracker`, `PreparingForTracking`, `FirmwareUpgrade`) | `Initializing` | `Initializing` (1) | Initializing |
| host not started | `Idle` | `Initializing` (1) | Initializing |

`EyeTrackingHostState.FirmwareUpgrade` also maps to `TrackingUnavailable`
(`SRV:EyeTrackingConfigurationProvider.cs:273-275`); which component sets that
host state was not read. `NotAvailable` (2) is never returned by this mapping.
The values are `MODEL:EyeTrackingDeviceStatus.cs:8-17`; the engine enums are
`ENG:EngineState.cs` and `ENG:EyeTrackingHostState.cs`.

The configuration status that the page also reads is mapped the same way
(`SRV:EyeTrackingConfigurationProvider.cs:281-297`):

| `ConfigurationStatus` (engine) | `EyeTrackingConfigurationStatus` (value) |
| --- | --- |
| `ConfigurationValid` | `Valid` (1) |
| `InvalidMonitorConfiguration`, `IncompleteMonitorConfiguration` | `InvalidMonitorConfiguration` (2) |
| `IncompleteUserProfile`, `NoCalibration` | `InvalidCalibration` (3) |
| `TrackedMonitorNotAvailableForBuiltInTracker` | `TrackedMonitorNotAvailableForBuiltInTracker` (5) |
| anything else (`UnspecifiedError`) | `UnknownError` (4) |

Values: `MODEL:EyeTrackingConfigurationStatus.cs:8-12`;
`TDC:ConfigurationStatus.cs`.

The tray reads two values from the engine: the device status and the enabled
state `global.eyeTrackingEnabled`, both nullable and null when the engine has
no valid value (`FDEV:TrackingDeviceStatusProvider.cs:18-72`).

### 1.3 Icons and the spinner

The four icons are `System.Drawing.Icon` resources of the Modules assembly
(accessors `MODPROP:Resources.cs:46` Disabled, `:56` Enabled, `:66` GetStarted,
`:76` Progress; data `MODRES:28878-28964` Enabled, `MODRES:28964-29050`
Disabled, `MODRES:29050-29136` GetStarted, `MODRES:30-28878` Progress).
`ASSETS` holds the ICO bytes cut from those resources and an enlarged PNG of
each. Looked at directly:

| Icon | ICO directory | Look |
| --- | --- | --- |
| `Enabled` | 1 image, 33 x 33, 32 bpp (`ASSETS:Enabled.ico`) | blue Tobii mark |
| `Disabled` | 1 image, 33 x 33, 32 bpp | the same mark with a red circle and a white cross at the lower right |
| `GetStarted` | 1 image, 33 x 33, 32 bpp | the same mark with a cyan dot (dark outline) at the lower right |
| `Progress` | 10 images, one per size: 16, 24, 32, 48, 64, 96, 128, 192, 256, 512 px, 32 bpp (`ASSETS:Progress.ico`) | ten dots on a circle inside a `#343434` square frame, see below |

The 512 px `Progress` image, measured pixel by pixel (centre 256, 256, circle
radius 175, each dot about 25 px in radius): the dots sit at 30 degree steps,
starting at the top and going anticlockwise through the left, bottom and
right, so 10 of the 12 positions are filled and the upper right is empty. Dot
centres and colours, in order: (256, 81) `#FFFFFF`, (168, 103) `#E8E7E6`,
(104, 168) `#D3D2D0`, (81, 256) `#BFBDBB`, (104, 343) `#ABA8A6`,
(168, 408) `#979492`, (256, 430) `#848180`, (344, 408) `#706E6D`,
(408, 343) `#5B5958`, (431, 256) `#414040`. The frame is `#343434`, about
40 px wide at 512 px.

Spinner animation (question (i)):

- `ISpinnerUpdater` is declared in the Modules assembly
  (`IMG:ISpinnerUpdater.cs:5-12`: event `Changed` carrying an `int`, `Start`,
  `Stop`) and **nothing implements or references it**: a search of every
  decompiled assembly finds only the declaration, and the string occurs only in
  `bin/Tobii.EyeX.Interaction.Modules.dll` among the unpacked binaries of the
  installer and the Store app. So there is no frame count, period or frame
  resource to record for the tray.
- The handler sets the icon once per `SetNotificationState` and never starts a
  timer (`ICON:ExperienceNotifyIconHandler.cs:47-112`). The Windows tray shows
  `Progress` as a static image. Linux: same (proposal): a static icon for
  Configuring and Initializing.
- The only animated spinner in the shipped software is a WPF control in the
  Store app, `CTRL:Spinner.xaml`: 12 ellipses of 3 x 3 on a 24 x 24 canvas
  (`CTRL:Spinner.xaml:143-361`), opacity key frames over 1.2 s repeating
  forever at a desired frame rate of 24 (`CTRL:Spinner.xaml:5-83`). It is used
  on the Home page banner and the overview overlay, not on the Eye tracking page
  and not in the tray (`VIEW:HomePage.xaml:265`, `VIEW:OverviewPage.xaml:25`).

Tray artwork (Linux): the icons in `crates/tobii-experience/assets/tray/` are this project's
own, drawn by `tools/draw_tray_icons.py` with the standard library only; it prints the
SHA-256 of each PNG, and `tests/test_draw_tray_icons.py` checks the committed icons against
it:

    python3 tools/draw_tray_icons.py

They take the roles of Tobii's `Enabled`, `Disabled`, `GetStarted` and `Progress` icons
without copying them: a blue eye for Enabled, the same eye in grey with a slash for Disabled,
the blue eye with an amber dot for GetStarted, and a ring of fading dots for Progress.
`Enabled`, `Disabled` and `GetStarted` are 33 x 33 like Tobii's. A PNG holds one image, so
`Progress` ships as `Progress-16.png` to `Progress-64.png` (16, 24, 32, 48, 64 px: the sizes a
tray draws). They are sizes of one static image, not animation frames.

### 1.4 Interaction and menu

- Left click: `MouseClick` with the left button flag `0x100000` raises
  `MouseLeftClicked` (`ICON:ExperienceNotifyIconHandler.cs:136-144`), which
  calls `NavigateToApp` (`TRAY:NotificationAreaIconFeature.cs:201-221`):
  Windows opens the Store app, or the Store page when it is not installed.
  Linux: differs (why): opens the page window, or raises it when it is open.
- Right button down (flag `0x200000`) rebuilds the menu from the current texts
  (`ICON:ExperienceNotifyIconHandler.cs:146-159`). While a menu exists, a state
  change rewrites the text of items 0 and 1 in place
  (`:68-110`).

Menu, in this order (`ICON:TrayIconContextMenuGenerator.cs:13-24`):

| # | Item | Text (key, line) | Present when | Click | Linux |
| --- | --- | --- | --- | --- | --- |
| 1 | Open | `Open Tobii Experience` (`NotificationAreaIcon_ContextMenuItem_OpenExperienceApp`, `FRES:38`); `Try Tobii Experience` in the GetStarted state (`NotificationAreaIcon_ContextMenuItem_TryExperienceApp`, `FRES:40`) | always | same as a left click (`TRAY:NotificationAreaIconFeature.cs:110-114`) | same text; opens the page window; `Try ...` unused (GetStarted never shown) |
| 2 | Enable or disable | `Disable Tobii Experience` (`NotificationAreaIcon_ContextMenuItem_DisableExperience`, `FRES:54`) when the enabled state is `true`; `Enable Tobii Experience` (`NotificationAreaIcon_ContextMenuItem_EnableExperience`, `FRES:31`) otherwise, including null (`TRAY:NotificationAreaIconFeature.cs:238`) | always | section 2 | same |
| 3 | Quit | `Quit` (`NotificationAreaIcon_ContextMenuItem_Quit`, `FRES:49`) | only when `HasQuitOption` is true (`BOOT:NotificationAreaIconInstaller.cs:18`); that flag lives in the Store app's local settings (`FCORE:ExperienceAppLocalSettings.cs:10-25`) and the Store app sets it to true at every start (`WIN:App.xaml.cs:72`) | section 3 | same; always offered (proposal): on Linux one app writes and reads the flag |
| 4 | Close Interaction | `Close Interaction` (hard-coded, `ICON:TrayIconContextMenuGenerator.cs:22`) | only with the feature licence `09BAD782-5410-4D99-87D9-65986413AC60` (`BOOT:NotificationAreaIconInstaller.cs:11,18`) | stops the Interaction process (`TRAY:NotificationAreaIconFeature.cs:133-139`) | omitted (decision): developer option |

## 2. On/off

The tray item and the page toggle both end in the same two writes: the engine
state `global.eyeTrackingEnabled` and the `SystemEnabled` field of the system
configuration file.

| | Tray item | Page toggle |
| --- | --- | --- |
| Decides the new value from | `EyeTrackingEnabledState == true` then disable, else enable (`TRAY:NotificationAreaIconFeature.cs:116-120`) | the toggle's `IsChecked` (`VM:SettingsPageViewModel.cs:249-259`) |
| Engine state | `EyeTrackingEnabledState` setter calls the observer's `SetValue` (`FDEV:TrackingDeviceStatusProvider.cs:58-71`) | `SetStateAsync("global.eyeTrackingEnabled", ...)` (`DEV:TrackingDeviceEnabler.cs:87-95`), skipped when the value equals the current `SystemEnabled` (`:81-84`) |
| `SystemEnabled` | read, flip, `WriteSystemConfiguration(cfg)` with the folder created if missing (`TRAY:NotificationAreaIconFeature.cs:121-125`) | `WriteSystemConfiguration(cfg, createFolderIfNotExists: false)`; nothing is written when the folder is missing (`DEV:TrackingDeviceEnabler.cs:108-120`, `IO:SystemConfigurationHelper.cs:28-38`) |
| On a write error | warning in the log, no dialog (`:127-130`) | warning in the log, no dialog (`DEV:TrackingDeviceEnabler.cs:116-119`) |
| Busy marker and delay | none | busy marker, plus a 500 ms wait when disabling (4.5) |
| Linux | same | same |

State file (`IO:SystemConfiguration.cs:6-13`, `IO:SystemConfigurationJsonConverter.cs:9-54`,
`IO:SystemConfigurationHelper.cs:28-86`): a JSON file named
`tobii_system_configuration` in `%LocalAppData%\Tobii` (Store app
`EXT:ContainerBuilderExtensions.cs:94`; service `SVC:UserFolderHelper.cs:46`;
file name `IO:TobiiFileNameHelper.cs:5`).

- Fields `SystemQuit`, `SystemEnabled`, `LastConnectedDevice`, each written as a
  string (`"True"` or `"False"`). A missing `SystemEnabled` reads as true; a
  missing file reads as a default object (`SystemEnabled` false) and the
  callers then fall back (below).
- Reads and writes retry 3 times, 50 ms apart; a malformed file is deleted.
- Fallback when the file does not exist: the per-user value `EyeTrackingEnabled`,
  then `DeviceDefaultEnabled` (`DEV:TrackingDeviceEnabler.cs:46-57,71`); the
  service writes the file with that value and `SystemQuit` false
  (`SVC:ExitWatcher.cs:126-135`).
- The page and the service watch the file (`DEV:TrackingDeviceEnabler.cs:122-125`,
  `SVC:ExitWatcher.cs:95-97`).
- Linux: differs (why): the file is Windows-specific. The Linux state file
  under `$XDG_STATE_HOME/tobii-linux/` (spec) carries the same two facts,
  "enabled" (default true when absent) and "quit requested".

Effects of turning tracking off:

| # | Effect | Evidence | Linux |
| --- | --- | --- | --- |
| 1 | Engine: `EyeTrackingEnabler` receives `global.eyeTrackingEnabled`, calls `SetEyeTrackingEnabled` and the host's `DisableEyeTracking`, which puts the engine in `Idle`. Tracking runs only while the setting is on **and** a client is connected. The value is persisted per user as `EyeTrackingEnabled` (registry) each time the host state is applied | `ENG:EyeTrackingEnabler.cs:41-50,73-88,162-169,205-226`; `ENG:EyeTrackingEngine.cs:265-285` | differs (why): no engine; the service applies the persisted value at every session start (spec) |
| 2 | Device: `EyeTrackerToggle.ToggleDevices(enabled)` visits every plugged-in tracker and calls `tobii_resume_device` (on) or `tobii_pause_device` (off); `NOT_SUPPORTED` counts as success. Gated by `features.pauseresumetracker.enabled`, true in the shipped `defaultsettings.txt`. On a connection failure it retries up to 15 times, 5 s apart, then gives up and has the host manager reset the device set | `ENGDC:EyeTrackerToggle.cs:32,35-52,54-109`; `ENG:ConfigurableEyeTracker.cs:238-260`; `MSI:defaultsettings.txt.F15EE609_8BE1_48FF_B34C_D30E5B107FBB:352-354`; `ENG:EyeTrackingEnabler.cs:83-86` | same (decision): the device is paused for every client, games included |
| 3 | A tracker that appears while tracking is off is paused as soon as it is added, when the feature is on and `global.eyeTrackingEnabled` is false (fallback true) | `ENGDC:DeviceManager.cs:238,315-322` | same (decision): the service re-applies the pause at every attach |
| 4 | Service: `Tobii.Service` reads `SystemEnabled` and `SystemQuit`; when disabled it restarts its modules with `StartCondition.Disabled` (stops them instead only with `-KillOnDisable=1`, default false). The engine module then writes `EyeTrackingEnabled` = (condition is Enabled); the driver module disables the virtual camera device `ROOT\TOBIIVCAM` unless Enabled | `SVC:ExitWatcher.cs:99-112`; `SVC:ModulesHandler.cs:171-192,217-233`; `SVC:EnvironmentProxy.cs:60-78`; `SVC:EngineModuleHandler.cs:17-27`; `SVC:DriverHandler.cs:19-22`; `SVC:ModuleHandlerFactory.cs:87` | persisted off state: same; virtual camera: omitted (decision) |
| 5 | Result as seen by clients: the status becomes `TrackingPaused`; the tray shows "Tobii Experience - Disabled" and the page state is `TrackingDisabled` | 1.2 table, 4.4 | same |

Because the pause is on the device, every client including games stops
getting data; in the runtime pause/resume is command 3100 sent when a client
sets property 4 (spec, `docs/usb-streams.md`, `docs/service-behaviour.md`).

## 3. Quit

Menu item 3 (`TRAY:NotificationAreaIconFeature.cs:141-199`). Steps:

1. If a quit dialog is already open, ignore the click (`:143-148`).
2. If the persisted setting `SkipConfirmQuitDialog` is true (default `false`,
   `SET:PersistentSettings.cs:100-111`), go straight to step 5 (`:149-154`).
3. Otherwise open the confirmation dialog with the strings below (`:155-165`).
4. The dialog's Confirm closes the window and then runs the quit; Cancel closes
   it and runs nothing; the title bar's close button only closes it
   (`QDVM:ConfirmationDialogViewModel.cs:194-216`).
5. `ButtonQuit` (`:168-199`): if the dialog's checkbox is checked, persist
   `SkipConfirmQuitDialog = true` (this happens before the write, so it stays
   even if the write fails); set the quitting flag and call `Update`, which
   shows the Configuring icon; read the system configuration, set
   `SystemQuit = true` and write it.
6. If the write throws an I/O error, show the message box below. Yes repeats
   step 5; No clears the quitting flag and calls `Update` (the previous icon
   returns).
7. `Tobii.Service` sees `SystemQuit` and runs its exit flag: it stops the
   engine and the Interaction process (the tray), stops the Store app, the
   Troubleshooter and the Configuration app by process name, stops the platform
   runtime service and stops the virtual camera driver module
   (`SVC:ModulesHandler.cs:171-176`, `SVC:ModuleHandlerFactory.cs:51-65,85,87`,
   `SVC:ModuleHandler.cs:28-31`, `SVC:ExperienceAppModuleHandler.cs:18-28`,
   `SVC:ServiceModuleHandler.cs:32-35`, `SVC:DriverHandler.cs:29-32`). The
   plugin handler does not act on the exit flag
   (`SVC:PluginsModuleHandler.cs:32-40`); the statistics handler was not read.
   The Store app clears `SystemQuit` when it starts again, with a Yes/No retry
   message box on an I/O error (`VM:MainViewModel.cs:145-165`).

Strings (English resx; the line break inside the two multi-line values is
stored as CR LF in the resx and shown as one line break):

| Use | Text | Key, line |
| --- | --- | --- |
| Dialog title | `Are you sure you want to quit Tobii Experience?` | `NotificationAreaIcon_MessageBody_Title`, `FRES:33` |
| Dialog body | `All supported sensors will be disabled.` + line break + `You can launch the app again from the start menu.` | `NotificationAreaIcon_MessageBody_Description`, `FRES:36-37` |
| Confirm button | `Quit Tobii Experience` | `NotificationAreaIcon_MessageBody_ConfirmButtonName`, `FRES:47` |
| Cancel button | `Cancel` | `NotificationAreaIcon_MessageBody_CancelButtonName`, `FRES:44` |
| Checkbox | `Don’t show this again.` | `NotificationAreaIcon_MessageBody_CheckboxName`, `FRES:50` |
| Failure message box body | `There was a problem quitting the app.` + line break + `Do you want to try again?` | `NotificationAreaIcon_MessageBody_IOErrorMessageBody`, `FRES:42-43` |
| Failure message box caption | `Tobii Experience` | `NotificationAreaIcon_MessageBody_IOErrorMessageCaption`, `FRES:51` |

The failure box is a plain `MessageBox.Show(body, caption, YesNo, Error icon)`
(`MessageBoxButtons` 4, `MessageBoxIcon` 16) with no owner window
(`TRAY:NotificationAreaIconFeature.cs:191`).

Dialog geometry and style (`QDV:ConfirmationDialogWindow.xaml:2,191-274`):

| Part | Value |
| --- | --- |
| Window | 600 x 300, no chrome (`WindowStyle` None, transparency allowed), not resizable, not in the taskbar, topmost; title property "Confirmation Dialog" (never visible); style `TobiiSecondaryWindowStyle`: background `#151617`, border 0.7 px `#808080` (`BR:window.xaml:50-80`) |
| Position | shown, then moved next to the taskbar corner with a 10 px margin when the taskbar rectangle and the monitor DPI can be read; otherwise centred on the screen (`QDVM:ConfirmationDialogViewModel.cs:178-192`, `QD:TaskBarLocationProvider.cs:55-77`) |
| Title bar | Branding `TitleBar` with the Tobii logo, 130 px wide, margin `5 0 0 0`; close button bound to the close command |
| Body | a white (`#FFFFFFFF`) panel, height 270, vertically centred, margin `0 30 0 0`, full width |
| Title text | style H3 (16 pt) with foreground `#FF000000`, margin `40 80 40 20`, wraps |
| Body text | style Body (12 pt) with foreground black, margin `40 0 40 0`, wraps |
| Bottom row | docked bottom, margin `0 0 10 10`: checkbox left (margin `20 10 20 0`, scaled 1.5 x 1.5, label Body style 8 pt black, margin `0 2 0 0`); Cancel and Confirm docked right, Cancel declared first so it is the right-most; both 40 px high, 12 pt |
| Cancel | `TobiiButtonStyle` unchanged: padding `25 0`, min size 120 x 33, fill `#004B66`, text `#D9F4FF`; height 40 and 12 pt set on the button (`QDV:ConfirmationDialogWindow.xaml:240`; 5.4, first table) |
| Confirm | `TobiiActionButtonStyle` (`QDV:ConfirmationDialogWindow.xaml:256`): padding, min size and text colour are the same as `TobiiButtonStyle` (`BR:buttons.xaml:355-357` vs `:21-23`); only the fills differ: normal `#008DBF` (`:367`), hover `#00ACE9`, pressed `#00BEFF` (`BR:buttons.xaml:349-682`) |
| Not read | which title bar buttons the dialog shows (`BR:titlebar.xaml`) |

Linux: same strings and quit semantics (decision, spec 7). Linux differs (why)
in the stop: Quit stops the service unit and exits the app, and the persisted
on/off value is untouched. The body text says "start menu" (proposal: keep the
text verbatim as the design asks for the same wording, and tell the user).

Further Linux differences (why), as built in `crates/tobii-experience`:

- The dialog has no Tobii logo in its title strip: the repository has no logo asset.
  The strip keeps its 30 px height and the close button.
- The dialog is neither topmost nor hidden from the taskbar: a GTK 4 window cannot ask
  for either. It is centred, Tobii's fallback position, since no taskbar rectangle exists.
- The failure box is a GTK window titled `Tobii Experience` with an error icon, the body
  and Yes/No buttons, not a modal `MessageBox`; closing it counts as No (a Windows YesNo
  box cannot be closed).
- Step 1 also ignores Quit while the stop runs or the failure box is up (proposal):
  Tobii only checks for its open dialog, and its stop is a synchronous write; with the
  stop on a worker thread and a non-modal box, the extra check keeps two stops or two
  boxes from overlapping.
- The stop is `systemctl --user stop <unit>` (default `tobii-head-pose.service`), and
  launching the app runs `systemctl --user start <unit>` in place of clearing `SystemQuit`;
  a failed start is only logged.

## 4. The Eye tracking page

The Store app's `SettingsPage` (`VIEW:SettingsPage.xaml`) with
`SettingsPageViewModel` (`VM:SettingsPageViewModel.cs`).

### 4.1 Where the page lives, and window geometry

| Item | Tobii | Linux |
| --- | --- | --- |
| Container | not a window: the right-hand sidebar of the Home page. `SettingsSidebar` is a Grid of width and max width `SidebarWidth` = 550 (`TH:generic.xaml:14`), background `#151617` (`BR:colorsfonts.xaml:148`), right-aligned, full height of the content area (`VIEW:HomePage.xaml:533-554`) | differs (decision): one window, 550 px wide, titled "Tobii Experience", holding the sidebar content |
| Dimmer | `SidebarBackground`: black, opacity 0.7, height 800, hit-testable; a mouse press on it closes the sidebar (`VIEW:HomePage.xaml:528`; `VIEW:HomePage.xaml.cs:93-100`) | omitted (why): no surrounding app |
| Open and close | width 0 to 550 in 500 ms with `CubicEase` whose `EasingMode` value is 1 (EaseIn in WPF's numbering, EaseOut = 0; not checked against the framework assembly); close 550 to 0 in 250 ms, linear (`VIEW:HomePage.xaml.cs:63-91`) | omitted (proposal): no slide |
| Opener | ribbon toggle "Settings" in the Home page: text is the current profile name (visible when the profiles section is visible, Body style, margin `0 0 10 0`) and a gear image 24 px high, button margin `40 0 0 0` (`VIEW:HomePage.xaml:115-153`; click toggles the sidebar, `VM:HomePageViewModel.cs:406-417`) | differs (decision): a header row above the page content shows the current profile's name in Body style (margin `45 15 25 0`, so it lines up with the content), visible when the profiles section is; no gear, no toggling |
| Host window | title `Tobii Experience`; no system chrome (`WindowStyle` None, `ResizeMode` CanMinimize, caption height 0), centred; fixed 1330 x 810, scaled down uniformly to fit the work area; title bar row 35 px with the Tobii logo 130 px wide, margin `5 0 0 0`; style `TobiiMainWindowStyle`: background `#000000`, border 0.7 px `#808080`, 1 px `#00ACE9` while active (`WIN:MainWindow.xaml:2-19`; `WIN:MainWindow.xaml.cs:33,35,178-198`; `BR:window.xaml:7-49`) | differs (decision): only the title and the 550 px width are kept |
| Closing | closing the window hides it and shuts the app down (`WIN:MainWindow.xaml.cs:82-86`); the tray lives in another process | differs (decision): closing the page window leaves the tray running; closing it is the sidebar hiding (navigate-from, 4.6), and so is minimizing it where the backend reports `GDK_TOPLEVEL_STATE_MINIMIZED` (X11; restoring it then rebuilds the list as a page show does). On Wayland minimizing is not observable (xdg-shell has no minimized state). Being hidden otherwise (another workspace, occluded: GTK's `suspended`) is not navigate-from |
| "App active" | `StatusMonitor.IsActive` = window active and not minimized, feeds the gaze preview (`WIN:MainWindow.xaml.cs:95-98`) | same, with "window focused" |
| Page insets | root Grid margin `40 0 0 15` (`VIEW:SettingsPage.xaml:3`), `ScrollViewer` (vertical scrollbar Auto, 5.7), inner Grid margin `5 0 0 0` (`:15`), content `StackPanel` margin `0 0 10 0` (`:16`): content is 550 - 40 - 5 - 10 = 495 px wide, 480 px when the scrollbar (own column, 15 px) is showing | same |

The whole page is disabled while the view-model's busy marker is set
(`VIEW:SettingsPage.xaml:4-8`, 4.5). The page registers for state changes when
it becomes visible and unregisters when it hides, is unloaded, or the main
window is minimized (`VIEW:SidebarPage.cs:27-40`, `VIEW:BasePage.cs:43-45,56-94`,
`VM:SettingsPageViewModel.cs:328-376`); on every show it re-reads state, then
`RefreshAvailability` and the profile list (`:337-355`).

### 4.2 Sections, in order

Text sizes and colours are in 5; `H3` is 16 pt white, `Body` 12 pt `#D5D5D5`
wrapping. A margin is `left top right bottom`.

| # | Section | Exact text (resx key, `LOC` line) | Layout (`VIEW:SettingsPage.xaml`) | Shown when | Enabled when | Linux |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | Switch | `Tobii Experience` (`EyeTrackingScreen_Button_EnableDisableEyeTracking`, `LOC:341`); toggle labels `ON` (`General_Status_ON`, `LOC:434`) and `OFF` (`General_Status_OFF`, `LOC:207`) | vertical stack margin `0 25 0 20` (`:17`); a horizontal row (`:18-32`): H3 text, vertically centred, margin `0 -3 0 0`; toggle (5.3), margin `30 0 0 0`, vertically centred; `IsChecked` two-way to `EyeTrackingIsEnabled` | always | `CanEnableDisableEyeTracking` (4.3) | same |
| 1a | RGB explainer | `Enable Tobii sensor. LED light will be on.` (`EyeTrackingScreen_SecondaryHeading_RGBExplainerText`, `LOC:483`) | Body, margin `3 8 0 0` (`:33-39`) | `IsAwareRGBDevice` | n/a | omitted (why): not an Eye Tracker 5 feature |
| 2 | Battery warning | `It's recommended to use a power cord when enabling Tobii Experience.` (`General_Status_BatteryPowerMessage`, `LOC:209`; ASCII apostrophe) | row with a warning icon 24 px high and warning-style Body text, padding `0 0 50 0`, max width 550 (`:41-51`) | `ShowBatteryPowerMessage` = not `DeviceDefaultEnabled` and `KillOnDisable` and no AC power (`VM:SettingsPageViewModel.cs:138-148`) | n/a | omitted (decision): battery messages |
| 3 | Improve my calibration | H3 `Improve my calibration` (`SettingsScreenProfiles_SecondaryHeading_ImproveCalibration`, `LOC:226`); Body `If the light conditions change or if you experience less tracker precision, you might benefit from improving your calibration.` (`SettingsScreenProfiles_Body_ImproveCalibration`, `LOC:162`); button `Improve calibration` (`EyeTrackingScreen_Button_ImproveCalibration`, `LOC:333`) | group margin `0 15 0 0` (`:58`); H3, Body margin `0 5 0 10` (`:65`), button style `GazeEnabledButton` (5.4) | profiles group and `IsCalibrationVisible` | `CanImproveCalibration` | same |
| 4 | Preview my gaze | title H3 `Preview my gaze` (`GazeTrace_SecondaryHeading_Title`, `LOC:51`); Body `Shows you a visual trail of your gaze` (`GazeTrace_Body_Description`, `LOC:272`); no trailing full stop | group margin `0 15 0 0` (`:78`); a `ToggleSettingView` (`SIV:ToggleSettingView.xaml`): root margin `0,0,0,20` (`:2`), title row margin `0,20,0,10` (`:14`) holding H3 (inner margin `0,-4,0,0`, max width 550, `:20-21`) and the toggle (margin `30,0,10,0`, `:31`), then Body margin `0,5,0,10`, max width 640 (`:89`) | profiles group and `IsGazeTraceAvailable` | the state is `Tracking` (`VM:SettingsPageViewModel.cs:393-399`); value is `_gazeTrace.IsEnabled` (4.8) | same |
| 5 | Select eyes to detect | button label H3 `Select eyes to detect` (`SettingsScreenTrackedEyes_SecondaryHeading_SelectEyesToTrack`, `LOC:334`) in the button's foreground; when expanded, Body `If you typically squint or have poor sight in one eye, you can make the eye tracker detect one eye only.` (`SettingsScreenProfiles_Body_TrackOneEyeOnly`, `LOC:71`; a no-break space, U+00A0, sits between `sight` and `in`) and three radios `Both eyes` (`SettingsScreenTrackedEyes_SecondaryHeading_BothEyes`, `LOC:203`), `Left eye only` (`SettingsScreenTrackedEyes_SecondaryHeading_LeftEyeOnly`, `LOC:305`), `Right eye only` (`SettingsScreenTrackedEyes_SecondaryHeading_RightEyeOnly`, `LOC:435`) | button group margin `0 10 0 0` (`:92`), accent-theme button (5.4) containing an arrow grid (vertically centred, margin `0 0 10 0`: `DownArrowImage` width 18 when expanded, `RightArrowImage` height 18 when collapsed, 5.8) and the H3 (`:98-143`). Expanded block (`:145-174`): Body margin `0 5 0 10`, then a horizontal row of three `TobiiRadioButtonStyle` radios (5.5), margin `0 0 15 0` on the first two. The block starts collapsed and the fold state is not reset by navigation (`VM:SettingsPageViewModel.cs:225-235,422-426`) | profiles group and `IsTrackedEyesSectionVisible` (button); `ShowTrackedEyesSelection` (block) | button: `CanSelectTrackedEyes`; the radios are only disabled by the busy marker | same |
| 6 | Change screen | H3 `Change screen` (`SettingsScreenDisplay_SecondaryHeading_MoveYourTracker`, `LOC:393`); Body `If you move the sensor to a different monitor, you’ll need to set up the new display.` (`SettingsScreenDisplay_Body_MoveYourTracker`, `LOC:454`; typographic apostrophe); button `Set up display` (`SettingsScreen_Button_StartDisplaySetup`, `LOC:223`) | stack margin `0 20 0 0` (`:175`); H3, Body margin `0 5 0 10` (`:182`), button `GazeEnabledButton` | profiles group and `ShowDisplaySetup` = integration type `Peripheral` (`VM:SettingsPageViewModel.cs:223`) | `CanSetupDisplay` | same (the Eye Tracker 5 is a peripheral, proposal: report `Peripheral` always) |
| 7 | Change user profile | H3 `Change user profile` (`SettingsScreenProfiles_SecondaryHeading_ChangeUserProfile`, `LOC:81`); Body `Each user needs a unique profile. You may also need separate profiles to track your eyes with or without glasses/contacts. Press Ctrl+Shft+F10 for a temporary guest profile.` (`SettingsScreenProfiles_Body_ChangeUserProfileInfo`, `LOC:168`; "Shft" is Tobii's spelling); button `Create new profile` (`SettingsScreenProfiles_Button_CreateNewProfile`, `LOC:249`); then the profile list | stack margin `0 20 0 0` (`:197`); H3, Body margin `0 5 0 10` (`:204`); a Grid margin `0 20 0 0` with columns 200 and `*` (`:205-209`); column 0: accent-theme button (`:210-240`) with `NewProfileImage` width 35 left-aligned, then the Body text (margin `5 0 0 0`, vertically centred, foreground taken from the button); under the grid the list box (4.6, 5.6) | profiles group and `IsUserProfileVisible` | button: `CanCreateNewProfile`; list: `CanChangeCalibrationProfile` | same; the Body text names a key the app cannot grab (see 6 and 8) |

The sections 3 to 7 live in one stack (`VIEW:SettingsPage.xaml:52-278`) that is
shown when `IsProfilesSectionVisible`. The `SettingsTitle` property of the
view-model (`VM:SettingsPageViewModel.cs:275`, resource
`SettingsScreenProfiles_PrimaryHeading_Hi`) is not bound anywhere in the page.

### 4.3 Visibility and sensitivity rules

Visibility inputs (`VM:SettingsPageViewModel.cs:106-136,223`):

| Property | Rule |
| --- | --- |
| "has gaze" (`HasDeviceWithGazeCapabilityEverBeenConnected`) | gaze capability status is `ReadyToUse`, `AvailableButNotReadyToUse` or `HasBeenAvailable` (`EXT:TrackingDeviceCapabilityProviderExtensions.cs:7-14`) |
| `IsCalibrationVisible`, `IsUserProfileVisible`, `IsTrackedEyesSectionVisible`, `IsGazeTraceAvailable` | "has gaze" |
| `IsProfilesSectionVisible` | "has gaze", or a peripheral device that has had only low-frequency head pose (`:632-639`) |
| `ShowDisplaySetup` | integration type is `Peripheral` |

The capability status comes from the stream capability: `NotAvailable` becomes
`HasBeenAvailable` once a gaze tracker has been seen (the local setting
`GazeTrackerHasBeenConnected` is set whenever gaze becomes
`AvailableButNotReadyToUse` or `ReadyToUse`), else `NotAvailable`
(`DEV:TrackingDeviceCapabilityProvider.cs:243-249,309-326`). The visibility
properties are re-raised only when the page is shown and when the integration
type changes (`VM:SettingsPageViewModel.cs:343-351,600-606`). The integration
type defaults to `Internal_base` until the engine reports one
(`DEV:TrackingDeviceInfoProvider.cs:128,157-168`).

Linux: same. The persisted "a gaze tracker has been connected" flag is the app
setting `gaze_tracker_has_been_connected` (`experience.json`), set the first time a
status reports `attached`, so the sections stay visible while the tracker is
unplugged or the service is down. The capability is `ReadyToUse` while attached in
state `Tracking`, `AvailableButNotReadyToUse` while attached in any other state
(paused included), else `HasBeenAvailable` or `NotAvailable` by that flag. The
`IsProfilesSectionVisible` alternative is omitted (not an ET5 case). The page
re-evaluates visibility on every status, not only on show. `SystemEnabled` is the
status's `enabled`; with no status (Initializing, or the service unreachable, Error)
it reads as true, Tobii's default for a missing value, so the switch shows ON and the
table above makes it insensitive.

`RefreshAvailability` (`VM:SettingsPageViewModel.cs:514-551`) runs when the
page is shown and on every state or gaze-capability change. With gaze
capability `NotAvailable` or `HasBeenAvailable` every flag below except the
toggle is false. Otherwise, with state `S`:

| Flag | Rule |
| --- | --- |
| `CanImproveCalibration` | `S` is `InvalidCalibration` or `Tracking`, and the current profile is not `Guest` (`:531`) |
| `CanCalibrate`, `CanChangeCalibrationProfile` | `S` is `InvalidCalibration` or `Tracking` (`:532-533`) |
| `CanCreateNewProfile` | `S` is `InvalidCalibration`, `NoCalibrationProfile` or `Tracking` (`:534`) |
| `CanSetupDisplayForEyeTracker` | `S` is `InvalidMonitorSetup`, or `CanCreateNewProfile` (`:535`); `CanSetupDisplay` is that, or a peripheral-aware device (omitted) (`:126-136`) |
| `CanSelectTrackedEyes` | the current profile name is not empty (`:536`) |
| `CanEnableDisableEyeTracking` | `SystemEnabled` is false, or `S` is none of `Error`, `Configuring`, `ExclusiveMode`, `Initializing`, `FirmwareUpdateInProgress`, `InvalidMonitorRotation` (`:544`); not re-evaluated when only the file changes |

The same rules over the Tobii page states (4.4). `*` means "and the current
profile is not `Guest`"; `+` means "when `SystemEnabled` is true".

| State `S` | Improve calibration | Profile list (switch, delete, undo) | Create new profile | Set up display | Select eyes | Switch |
| --- | --- | --- | --- | --- | --- | --- |
| `Tracking` | yes* | yes | yes | yes | yes | yes |
| `InvalidCalibration` | yes* | yes | yes | yes | yes | yes |
| `NoCalibrationProfile` | no | no | yes | yes | yes | yes |
| `InvalidMonitorSetup` | no | no | no | yes | yes | yes |
| `TrackedMonitorNotAvailableForBuiltInTracker` | no | no | no | no | yes | yes |
| `TrackingDisabled` | no | no | no | no | yes | yes |
| `Disconnected` | no | no | no | no | yes | yes |
| `Initializing` | no | no | no | no | yes | no+ |
| `Configuring` | no | no | no | no | yes | no+ |
| `Error` | no | no | no | no | yes | no+ |
| `ExclusiveMode` | no | no | no | no | yes | no+ |
| `FirmwareUpdateInProgress` | no | no | no | no | yes | no+ |
| `InvalidMonitorRotation` | no | no | no | no | yes | no+ |

All "yes" cells in the first five columns are also subject to the gaze
capability being `AvailableButNotReadyToUse` or `ReadyToUse` (with
`NotAvailable` or `HasBeenAvailable` every one of them is "no") and, for the
select-eyes column, to a non-empty current profile. "no+" means the switch is disabled while `SystemEnabled` is true and
enabled while it is false. Linux: same for the states the Linux service can
report (4.4); firmware update, display rotation and exclusive mode have no
Linux producer.

### 4.4 Page state and status values

`TrackingDeviceState` (`DEV:TrackingDeviceState.cs`): `Tracking`,
`TrackingDisabled`, `Disconnected`, `Initializing`, `InvalidMonitorSetup`,
`NoCalibrationProfile`, `InvalidCalibration`,
`TrackedMonitorNotAvailableForBuiltInTracker`, `Configuring`, `Error`,
`ExclusiveMode`, `FirmwareUpdateInProgress`, `InvalidMonitorRotation`.
`CapabilityStatus` (`DEV:CapabilityStatus.cs`): `NotAvailable`,
`HasBeenAvailable`, `AvailableButNotReadyToUse`, `ReadyToUse`.
`TrackingDeviceIntegrationType` (`DEV:TrackingDeviceIntegrationType.cs`):
`Unknown`, `Peripheral`, `InternalBase`, `InternalDisplay`, `ExternalDisplay`,
`HeadMounted`. `TrackedEyes` (`TDC:TrackedEyes.cs`): `TrackBothEyes`,
`TrackOnlyLeftEye`, `TrackOnlyRightEye`.

`UpdateState` (`DEV:TrackingDeviceStateProvider.cs:337-435`) evaluates in this
order and the first match wins:

| # | Condition | State |
| --- | --- | --- |
| 1 | engine connection `TryingToConnect` | `Initializing` |
| 2 | engine connection `Disconnected` | `Error` |
| 3 | settings IPC client not connected | `Initializing` while the 2 s wait timer runs, else `Error` |
| 4 | firmware update started; firmware update failed | `FirmwareUpdateInProgress`; `Error` |
| 5 | display rotation invalid | `InvalidMonitorRotation` |
| 6 | by device status, "enabled" meaning the page's tracking-enabled flag (`SystemEnabled`): see the next table | |

| Device status | Enabled | Disabled |
| --- | --- | --- |
| `DeviceNotConnected` | `Initializing` during the 15 s reboot wait, else `Disconnected` | `TrackingDisabled` |
| `Initializing` | `Initializing` | `TrackingDisabled` |
| `NotAvailable` | `Initializing` | `Initializing` |
| `TrackingPaused` | `TrackingDisabled` | `TrackingDisabled` |
| `InvalidConfiguration` | by configuration status (below) | `TrackingDisabled` |
| `Tracking` | `Tracking`, or `Initializing` for 2 s then `Error` while any of gaze, head pose or presence is `AvailableButNotReadyToUse` | `TrackingDisabled` |
| `Configuring` | `Configuring` | `TrackingDisabled` |
| `UnknownError`, `ConnectionError` | `Initializing` during the reboot wait, else `Error` | same |
| `TrackingUnavailable` | `ExclusiveMode` | `ExclusiveMode` |

| Configuration status (`:437-457`) | State |
| --- | --- |
| `InvalidMonitorConfiguration` | `InvalidMonitorSetup` |
| `InvalidCalibration` | `NoCalibrationProfile` when the current profile name is empty, else `InvalidCalibration` |
| `UnknownError` | `Error` |
| `TrackedMonitorNotAvailableForBuiltInTracker` | `TrackedMonitorNotAvailableForBuiltInTracker` |
| `Valid` | state unchanged |

Timers: the 15 s reboot wait starts when the enabled flag turns on, when both
engine connections are first up, and when a firmware update ends; it stops
once the state is no longer `Initializing` (`:282-298,428-431`); the 2 s
capability wait starts when the status becomes `Tracking` with a capability not
ready (`:203-219,486-503`); the 2 s connection wait covers the first connect
(`:186-201,515-532`). The device status read for this is the engine's
`EyeTrackingDeviceStatus` (1.2); its default before a value arrives is
`Initializing`, the configuration status default is `Valid` (`:127-133`).

Linux: differs (why). Rows 1 to 5 and the reboot, capability and connection
timers belong to Tobii's multi-process stack. The Linux page state is derived
from the app's device status (1.2) and the status's `enabled` flag, by the
table above (implemented): no status yet `Initializing`; an unreachable service
`Error`; `TrackingPaused` `TrackingDisabled`; with `enabled` false every other
status `TrackingDisabled`; otherwise `Tracking` to `Tracking`,
`DeviceNotConnected` to `Disconnected`, `Configuring` (released) to
`Configuring`, and `InvalidConfiguration` through the configuration status:
no display area `InvalidMonitorSetup` (checked first), no current profile
`NoCalibrationProfile`. `InvalidCalibration`,
`TrackedMonitorNotAvailableForBuiltInTracker`, `ExclusiveMode`,
`FirmwareUpdateInProgress` and `InvalidMonitorRotation` have no Linux producer
(`calibration_matches_profile` is false while an apply runs, so it is not read as
an invalid calibration).

### 4.5 Busy marker and the 500 ms delay

- `IsPaused` of `BasePageViewModel` is a busy marker, not the device pause
  (`VM:BasePageViewModel.cs:16-26,43-65`). `BusyMarker` sets it on creation and
  clears it on dispose, unless it was already set when the marker was created
  (`UTIL:BusyMarker.cs:13-27`). The page root is `IsEnabled = !IsPaused`
  (`VIEW:SettingsPage.xaml:4-8`), so every control on the page is disabled
  while a marker is held.
- Markers are held around: the switch (`VM:SettingsPageViewModel.cs:608-621`),
  Improve calibration (`:428-437`), Create new profile (`:481-491`), Set up
  display (`:493-512`) and the unbound Guest command (`:469-479`).
- The switch setter returns at once and runs `SetEyeTrackingEnabled`
  (`:249-259`): hold the marker, call `SetIsTrackingEnabled(value)`, and **only
  when value is false wait 500 ms** before releasing; then store the value in
  the view-model (`:608-621`). Enabling releases the page immediately. The
  value shown also follows the persisted state: the file watcher raises
  `TrackingIsEnabledChanged`, which updates the field and the toggle
  (`:623-630`, `DEV:TrackingDeviceEnabler.cs:59,122-125`).
- A configuration run holds the marker until the configuration process has
  exited (4.7), so the page stays disabled for the whole run, then closes.
- Linux: same, and the marker is also held while a profile switch or a
  tracked-eyes change waits for the service's reply: Tobii makes those registry
  writes synchronously on the UI thread, so its page cannot be used meanwhile
  either. A click's value is shown until the reply arrives.

### 4.6 Profiles

| Rule | Value | Evidence | Linux |
| --- | --- | --- | --- |
| Source of the list | every sub-key of `HKLM\Software\Tobii\EyeXConfig\UserProfiles` (32-bit view); the current profile is the value `CurrentUserProfile` of `...\EyeXConfig`. Order: whatever the registry helper's `GetAllSubkeys` returns (not read) | `TDC:ConfigurationManager.cs:352-371,409-422`; `CFG:ConfigurationProvider.cs:114-123` | differs (why): profile files (spec item 4); keep one stable order (proposal: by name) |
| Which profiles are listed | all profiles except the current one and `Guest` | `VM:SettingsPageViewModel.cs:569-578` | same |
| Current profile | not in the list; shown only by the ribbon button of the Home page | 4.1 | differs: see 8 |
| List rebuild | on page show, current profile change, profile list change; each rebuild clears the collection and creates new items (the lookup runs on the cleared collection), so delete marks are discarded | `VM:SettingsPageViewModel.cs:553-598` | same; the change is noticed on page show and on every service status (the store is re-read then), not by watching the profile directory |
| `can_delete` | `!IsSelected && !IsGuest && !IsDeleted`; `IsSelected` is never set anywhere in the portal code, so for listed profiles it is `!IsDeleted` | `VM:CalibrationProfileListItemViewModel.cs:32-42,46-60,84` | same |
| Delete button | marks the item deleted at once, no confirmation, nothing removed yet; the row's profile button is disabled (greyed) and "Undo" replaces the trash icon (`CanUndoDelete = IsDeleted`, `IsEnabled = !IsDeleted`) | `VM:CalibrationProfileListItemViewModel.cs:22,44,62-78,85-94`; `TH:generic.xaml:157-275` | same |
| Undo | clears the mark | `VM:CalibrationProfileListItemViewModel.cs:90-94` | same |
| Marked deletions are really deleted (`DeleteProfilesMarkedForDeletion`) | when the page is hidden (sidebar closed, page unloaded, main window minimized, view-model disposed) and **before** a profile switch, Create new profile and the Guest command; **not** before Improve calibration or Set up display | `VM:SettingsPageViewModel.cs:358-361,371-376,445,475,487,451-467`; `VIEW:SidebarPage.cs:27-40`; `VIEW:BasePage.cs:43-45,80-94` | differs (why): on window close, on minimize where the backend reports it (X11, 4.1), before a switch and Create new profile, and when the app quits with the page open (the view-model disposal; GTK's quit destroys the window without a close request). On Wayland minimize is not observable, so deletions commit on close, switch, new profile and quit only. No Guest command on the page (6) |
| What a real delete does | removes the profile's whole registry key (calibrations for every tracker and monitor, tracked eyes); skipped for the current profile | `TDC:ConfigurationManager.cs:686-702`; `CFG:ConfigurationProvider.cs:133-139` | same: delete the profile and all its calibrations |
| Switching | a click on a listed profile: if it differs from the current one, delete marked profiles, set the current profile, close the sidebar; an equal name does nothing | `VM:SettingsPageViewModel.cs:439-449`; `CFG:ConfigurationProvider.cs:125-131`; `TDC:ConfigurationManager.cs:393-407` | same (the Linux apply sends 3160 then 1110, spec) |
| Profile list control sensitivity | the list box is enabled by `CanChangeCalibrationProfile`, so switching, deleting and undo are disabled together | `VIEW:SettingsPage.xaml:248-252` | same |
| Name rules (in the configuration app) | not empty, at most 255 characters, letters, digits, space, `(`, `)` and `-` only; unique; not `Guest` (`Guest is a reserved name.`) | `TDC:ConfigurationManager.cs:56,424-442,886-889`; `CNP:NewProfileViewModel.cs:102-120`; `CCOMRES:85` | same |
| Tracked eyes | stored per profile as `TrackedEyes` (Both 0, Left 1, Right 2; Both when the profile name is empty or the value unreadable); a radio change is applied at once to the current profile (also `Guest`), only when it differs | `VM:SettingsPageViewModel.cs:261-273,401-407`; `CFG:ConfigurationProvider.cs:141-148`; `TDC:ConfigurationManager.cs:373-391,632-663`; `TDI:DataTypes.cs:24-29`; `TDI:Conversions.cs:18-36` | same; storage differs (spec item 4) |
| Calibration per profile | one blob per profile, tracker and monitor: key `Calibrations\<tracker uri>&<monitor instance id>` with `\` replaced by `*`; fields `Timestamp`, `NumPoints`, `DataSize`, `Data` | `TDC:ConfigurationManager.cs:546-572,891-894` | same (spec item 4) |

The engine also has a `DeleteProfile` command for SDK clients that first moves
the current profile to another one; the page does not use it
(`CORE:EyeTrackingConfigurationObserver.cs:173-190`).

### 4.7 What the buttons launch, and exit codes

`Tobii.Configuration.exe` takes one flag; there is no URI scheme
(`PROC:FlowTypeResolver.cs:7-37`, `CCONT:FlowTypeResolver.cs:8-61`). The page
starts it with no other argument (`PROC:ConfigurationAppLauncher.cs:129-165`).

| Page control | Flow | Argument | Needs | Before | After | Linux |
| --- | --- | --- | --- | --- | --- | --- |
| `Improve calibration` | `Calibrate` | `-C` | `CanImproveCalibration` | busy marker | close the sidebar | `tobii-linux-setup` with `-C` (decision: Tobii's arguments) |
| `Set up display` | `SetupDisplay` when the state is not `InvalidMonitorSetup` | `-S` | `CanSetupDisplay` | busy marker | close the sidebar | `-S` |
| `Set up display` | `ResolveConfiguration` when the state is `InvalidMonitorSetup` | `-R` | `CanSetupDisplay` | busy marker | close the sidebar | `-R` |
| `Create new profile` | `CreateNewProfile` | `-N` | `CanCreateNewProfile` | busy marker, delete marked profiles | close the sidebar | `-N` |
| none on the page (`GuestCalibrationCommand` exists and is bound nowhere) | `GuestCalibration` | `-G` | the global shortcut, section 6 | delete marked profiles (command only) | close the sidebar (command only) | `tobii-linux-experience --guest` runs `tobii-linux-setup --guest`, which setup takes as `-G` (decision) |

Source lines: Improve `VM:SettingsPageViewModel.cs:428-437`, Create
`:481-491`, Set up display `:493-512`, Guest command `:469-479`; XAML bindings
`VIEW:SettingsPage.xaml:66-76,183-194,210-240`. Flows not started from the page:
`-P` (`EyePosition`) and `-Q` (`QuickCalibration`) have no caller in the page
or the tray; the Home page banner button (4.9) and the licence page start `-R`
(`VM:HomePageViewModel.cs:704-712`, `VM:LicenseAgreementViewModel.cs:175`).
Argument recognition: `Tobii.Configuration.exe` scans its arguments for an exact
match in the order `-C`, `-N`, `-P`, `-G`, `-Q`, `-R`, `-S`; with no match it
exits with code 0 (printing usage when `--help` is given and a console is
attached); an optional `-eyeTrackerUrl URL` is parsed too
(`CCONT:FlowTypeResolver.cs:45-61`, `CONF:ApplicationConfiguration.cs:17-34,63-84`).
`tobii-linux-setup` takes `-C`, `-N`, `-S`, `-R` and `-G` (also as `--guest`), at
most one, and when started with one of them exits with Tobii's codes: 0 success, 1
argument error, 2 aborted, 3 configuration error ([setup flow](setup-flow.md#tobii-linux-setup-arguments)).

How the launcher handles the process (`PROC:ConfigurationAppLauncher.cs`):

- One run at a time: a second request while one is active returns
  `AlreadyRunning` at once (`:41-44`).
- Start the process; then wait up to 20 s, polling every 100 ms, for a process
  named `Tobii.Configuration` to exist (any instance, by name); failing that,
  `LaunchError` (`:81-105`). Then poll every 100 ms until no such process
  exists (`:107-127`), read the exit code and map it (`:167-207`).
- Exit code to result: 0 `Success`, 1 `ArgumentError`, 2 `Aborted`, any other
  `ConfigurationError` (`:198-207`). If the code cannot be read the run is
  treated as a success (`:183-186`).
- The configuration app sets the code from `ConfigurationResult`: 0 success,
  1 argument error, 2 aborted, 3 configuration error
  (`CCONT:ConfigurationResult.cs:3-9`, `CONF:MainModel.cs:77-81`); `1` when
  creating the arguments throws (`CONF:App.xaml.cs:111-117`) and `3` when the
  main model cannot be created (`:221-227`).
- The page does not react to the result: the launcher logs an error for any
  result but `Success` (`PROC:ConfigurationFlowLauncher.cs:15-32`) and the page
  closes its sidebar whatever it was (`VM:SettingsPageViewModel.cs:435,477,489,503,509`).
  No message is shown.
- Linux: same (decision): wait for `tobii-linux-setup`, ignore the result,
  close or hide the page. The 20 s start wait and process-by-name check:
  differs (proposal): wait on the child process itself.

### 4.8 Gaze preview, as the page uses it

- The switch's value is `_gazeTrace.IsEnabled`, an in-memory bool, default
  false, never persisted (`VM:SettingsPageViewModel.cs:393-399`;
  `TDG:GazeTraceVisualization.cs:29,35-53`). It is not cleared when the state
  leaves `Tracking`; the toggle just becomes disabled.
- The bubble is shown only while the preview is enabled and the app window is
  active and not minimized (`GT:GazeTraceVisualizationExpApp.cs:35-42`,
  `WIN:MainWindow.xaml.cs:95-98`); showing subscribes to the
  `FilteredCombinedAdvancedGaze` stream, hiding unsubscribes
  (`TDG:GazeTraceVisualization.cs:237-256`).
- Parameters set at start: size 0.075, strength 0.95, decay 0.95, filter 0.75,
  trail 0.7, foreground `Color.Silver`, background unset; native update every
  12 ms while enabled, every 500 ms otherwise (`:23-25,187-214`).
- Data: valid samples only; point = screen origin + normalised gaze point x
  screen size, on the monitor the default tracker is mapped to
  (`:258-286`).
- Linux: differs (why):
  - The bubble also needs state `Tracking` and the tracked monitor to be connected:
    `tobii-linux serve` reports the state, and the monitor can be unplugged under a running
    app. Leaving `Tracking` or losing the monitor hides the bubble and ends the gaze
    subscription. The toggle keeps its value, and the bubble comes back with the state or
    the monitor.
  - The tracked monitor is the one recorded by the last display setup
    (`$XDG_DATA_HOME/tobii-linux/display-setup.json`). The output is the GDK monitor whose
    connector equals the recorded `monitor.connector` after stripping a leading `cardN-`,
    else the one whose connector equals `monitor.key` (the output name setup matched, the
    only name in records written before `connector` existed and in Python setup records),
    else the one whose EDID manufacturer and model match (the EDID monitor name, or the
    product code when the EDID has no name). No match hides the bubble.
  - A record is only as right as the setup that wrote it. Under Wayland, setup used to
    list XWayland's outputs, or DRM connectors without positions, so it could record a
    monitor other than the one it showed on; the bubble then follows that monitor.
    Setup now lists GDK's outputs there (`monitors::placed_monitors`), and running
    "Set up display" again rewrites the record.
  - The overlay is a click-through `wlr-layer-shell` surface (overlay layer, all edges,
    no keyboard). Without layer-shell (X11, or a Wayland compositor without it) the
    preview stays off: X11 offers GTK 4 no way to keep a window above the others on a
    given monitor.
  - Drawing: [gaze-preview.md](gaze-preview.md), "Linux port".

### 4.9 The Home page banner

Tobii shows the tracker problems in a banner along the bottom of the Home page, not
on the Eye tracking page (`RefreshErrorMessages`, `VM:HomePageViewModel.cs:557-646`;
`GlobalErrorArea`, `VIEW:HomePage.xaml:230-306`). It sets one message per state; the
states the user can fix get a button.
"Set up display", "Create profile" and "Recalibrate" all run
`LaunchResolveConfigurationFlow`, `-R` (`VM:HomePageViewModel.cs:704-712`), which
sets up the display when none is set up and then creates a profile with its
calibration, or recalibrates. This banner is how Tobii asks a new user to
calibrate: the page's own "Set up display" runs `-S` whenever a display is
already set up, and `-S` does not calibrate.

| State | Kind | Message (resx `TrackerError_MessageBody_*`) | Button (`TrackingErrorScreen_Button_*`) | Linux |
| --- | --- | --- | --- | --- |
| `Tracking` | none | (ambient light too low: `Unable to detect your presence due to poor camera positioning or low lighting.`) | none | no banner; the light check has no Linux producer |
| `TrackingDisabled` | error | `You have disabled Tobii Experience.` | `Enable`: `SetEyeTrackingEnabled(true)` | same |
| `Disconnected` | error | `No Tobii sensor has been found.` | none | same |
| `Initializing` | waiting | `Getting ready...` | none | same, without the spinner |
| `InvalidMonitorSetup` | error | `Eye tracker not set up for this display.` | `Set up display`: `-R` | same |
| `NoCalibrationProfile` | error | `Create a user profile to get started.` | `Create profile`: `-R` | same |
| `InvalidCalibration` | error | `This profile needs to be recalibrated.` | `Recalibrate`: `-R` | same (no Linux producer yet, 4.4) |
| `TrackedMonitorNotAvailableForBuiltInTracker` | error | `The tracked monitor is unavailable.` | none | same |
| `Configuring` | waiting | `Working on it...` | none | same, without the spinner |
| `Error` | error | `Something's not quite right...` | `Troubleshoot`, plus a help centre link | the message only (why): the troubleshooter and help centre are Windows and web services |
| `ExclusiveMode` | error | `Tobii sensor is in use in another application.` | none | same |
| `FirmwareUpdateInProgress` | waiting | `Updating eye tracker...` | none | same |
| `InvalidMonitorRotation` | error | `Sorry, this mode is not supported. The camera needs to be at the top of the screen.` | none | same |

Before the state, when the device is disabled by default with `KillOnDisable` and
the system configuration is disabled, the banner shows `General_Status_BatteryPowerMessage`
with Enable (`:559-576`); Linux has no such driver account setting and skips it.

Layout (`VIEW:HomePage.xaml:230-306`): a grid docked at the bottom of the window,
background `TobiiStyleGuideBackgroundBrush` `#000000` (`BR:colorsfonts.xaml:132`),
holding one centred horizontal row:

| Part | Value |
| --- | --- |
| Error | warning icon (height 24, margin `0 3 0 0`) and the message in Body style, foreground `TobiiStyleGuideWarningBrush` `#FFB631`, margin `5 0 0 3`; row margin `0 4 0 7` |
| Waiting | the message in Body style, max width 450, and a 40 px spinner; row margin `0 4 0 7` |
| Help centre | Body text around a hyperlink button, margin `15 4 0 7`, only for `Error` |
| Button | `ShowError`, style `ActionButton` (`TobiiActionButtonStyle` with padding `25 0` and 10 pt, `TH:buttons.xaml:12-25`; fills `#008DBF`, hover `#00ACE9`, pressed `#00BEFF`, 5.4), margin `15 4 0 7`, max width 200 |

Linux (decision): the page window has no Home page, so the strip runs along the
bottom of the page window, below the scrolled sidebar, with the same background,
margins and button style; the message is Body text, `#FFB631` for errors, with no
warning icon, spinner or help centre link (`page::banner`, the `tobii-banner` and
`tobii-action-button` CSS classes). It is insensitive while the page is busy, as the
rest of the page is.

## 5. Styles

Fonts: `TobiiFontFamily` = `Segoe UI Light` (`BR:colorsfonts.xaml:115`),
replaced only for Chinese locales (`BRU:CultureHelper.cs:51-71`); no font is
shipped. Linux: differs (why): a substitute is needed: `Selawik Light`
(Microsoft's open Segoe UI replacement, packaged with the app), then the setup
app's fallbacks `Segoe UI Light`, `Segoe UI`, `Selawik`, `Open Sans`,
`Noto Sans`, `DejaVu Sans`, at weight 300.

Linux page CSS (`crates/tobii-experience/src/page/style.rs`) takes the values
below as they are, with these adaptations GTK needs: the `GazeEnabledButton`
pill is the whole button drawn at Tobii's scale (0.95, 0.975, 0.925), so its
label is set at 12 pt / 0.95 to keep 12 pt on screen (it grows 2.6 % on hover,
where Tobii's text does not); the toggle knob's margins are taken inside the 1 px
track border (3 px and 24 px for Tobii's 4 and 25); negative WPF margins on the
two headings beside a toggle become the same shift as a bottom margin on a
centred label; the radio bullet is a content-box fill inside a 4 px padding;
the accent button's 1 px press shift is a `translateY(1px)` of its content.

### 5.1 Palette (`BR:colorsfonts.xaml`)

| Name | Value | Line |
| --- | --- | --- |
| Background | `#021419` | 3 |
| BackgroundHighlight | `#1D4C5F` | 11 |
| BackgroundGray | `#1C1F20` | 19 |
| Accent | `#008DBF` | 27 |
| AccentHighlight | `#00ACE9` | 35 |
| AccentPressed | `#00BEFF` | 43 |
| ControlBackground | `#004B66` | 51 |
| ControlBackgroundHighlight | `#018EC1` | 59 |
| ForegroundHighlight | `#FFFFFF` | 67 |
| ForegroundSemiHighlight | `#D9F4FF` | 75 |
| Foreground | `#AFC4CD` | 83 |
| GreyHighlight | `#637279` | 91 |
| GreyPressed | `#505D62` | 99 |
| GreyDisabled | `#2B3940` | 107 |
| TextBase | `#BFD6E0` | 116 |
| TextHeadline | `#D9F4FF` | 124 |
| StyleGuideBackground | `#000000` | 132 |
| StyleGuideMainWindowBorder | `#808080` | 140 |
| StyleGuideSecondaryWindowBackground | `#151617` | 148 |
| StyleGuideSecondaryWindowBorder | `#808080` | 156 |
| StyleGuideWarning | `#FFB631` | 164 |
| StyleGuideSecondaryAccent | `#30D59B` | 172 |

### 5.2 Text (`BR:texts.xaml`)

| Style | Size | Colour | Other | Line |
| --- | --- | --- | --- | --- |
| Regular (base) | 12 pt | `#BFD6E0` (TextBase) | font `TobiiFontFamily` | 30-40 |
| H3 | 16 pt | `#FFFFFF` | wraps | 70-79 |
| Body | 12 pt | `#D5D5D5` | wraps | 80-88 |
| Warning body | 12 pt | `#FFB631` | wraps | 97-110 |

Toggle labels and radio labels are 10 pt `#AFC4CD`; button labels are 12 pt.
Linux: same.

### 5.3 Toggle (`GazeEnabledToggleStyle`)

`GazeEnabledToggleStyle` is `TobiiToggleButtonStyle` with the labels `ON` and
`OFF` and no gaze behaviour (`TH:buttons.xaml:26-51`; template
`BR:toggle.xaml:14-605`). Linux: same (the gaze states are omitted).

| Part | Value (line in `BR:toggle.xaml`) |
| --- | --- |
| Layout | two columns, `Auto` then 75 (`:42-45`): the switch, then the label; total width 42 + 75 = 117 px |
| Track, off | rectangle 42 x 19, radius 10, fill transparent, stroke `#D9F4FF`, default stroke thickness 1 (`:47-52`) |
| Track, on | rectangle 42 x 19, radius 10, fill `#008DBF`, opacity 0 (off) or 1 (on), stroke transparent (`:54-60`) |
| Knob | ellipse 10 x 10, fill `#D9F4FF`, margin `4 2 2 2` (off) or `25 2 2 2` (on), vertically centred (`:61-66,518-567`) |
| Label | `OFF` or `ON`, 10 pt, `#AFC4CD`, min width 15, margin `10 0 0 0`, vertically centred (`:83-98,572-600`) |
| Animation | 0.1 s for state changes and for the return to normal (`:8,103-109,518-567`) |

| State | Knob fill | Track fill when on | Off stroke | Off fill | Label | Knob scale |
| --- | --- | --- | --- | --- | --- | --- |
| Normal | `#D9F4FF` | `#008DBF` | `#D9F4FF` | transparent | `#AFC4CD` | 1 |
| Hover (`:112-198`) | `#FFFFFF` | `#00ACE9` | `#FFFFFF` | `#637279` | `#FFFFFF` | 1.07 |
| Pressed (`:376-462`) | `#FFFFFF` | `#00BEFF` | `#FFFFFF` | `#AFC4CD` | `#FFFFFF` | 0.93 (by -0.07) |
| Disabled (`:464-516`) | `#637279` | `#2B3940` | `#637279` | transparent | `#2B3940` | 1 |

The scale applies to the knob only (`:61-80`).

### 5.4 Buttons

`TobiiButtonStyle` (`BR:buttons.xaml:15-348`) is the base pill button and is
used as is by the quit dialog's Cancel:

| Part | Value |
| --- | --- |
| Padding | `25 0` (`BR:buttons.xaml:21`) |
| Size | min width 120 (`:22`), min height 33 (`:23`) |
| Font | `TobiiFontFamily` (`:38`); the size is set by the user of the style (the dialog sets 12 pt and height 40) |
| Colours | text `#D9F4FF` (`:24-30`), fill `#004B66` (`:31-37`) |
| Shape, states, transitions | as in the table below (`:60-97,111-162,267-345`); only padding, size and margin differ |

`GazeEnabledButton` (`TH:tobiistyleguide.xaml:3-32`) is `TobiiButtonStyle` with
the overrides below. It is used for "Improve calibration" and "Set up display"
on the page, not in the dialog:

| Part | Value |
| --- | --- |
| Padding | `35 8 35 8` (`TH:tobiistyleguide.xaml:14`) |
| Margin | `-5 10 0 15` (`:15`) |
| Font | 12 pt, `TobiiFontFamily` (`:16`; `BR:buttons.xaml:38`) |
| Size | min width 150 (`:17`), min height 33 (`BR:buttons.xaml:23`), left aligned (`:18`) |
| Colours | text `#D9F4FF` (`BR:buttons.xaml:24-30`), fill `#004B66` (`:31-37`) |
| Shape | pill: the fill rectangle's radius is half its actual height (`:60-80`); the rectangle is drawn at scale 0.95 (`:9,81-94`); the text is not scaled; the text margin is the padding (`:97`) |
| Hover (`:111-162`) | fill `#018EC1`, text `#FFFFFF`, rectangle scale 0.975 |
| Pressed (`:267-313`) | fill `#00BEFF` (instant), text `#FFFFFF`, scale 0.95 - 0.025 |
| Disabled (`:314-341`) | fill `#2B3940`, text `#637279` |
| Transitions | 0.1 s (`:8,101-109`) |

Accent-theme button (`GazeActivatableButtonAccentTheme`,
`TH:buttons.xaml:194-287`, base `:52-99`): used for "Select eyes to detect",
"Create new profile", the profile rows and the trash and undo buttons. No fill,
border 1 px transparent, text and icons take the button's foreground:
`#008DBF` normally, `#00ACE9` on hover (mouse over, not pressed), `#6E6E6E`
when disabled (`TH:buttons.xaml:81`); while pressed the content moves 1 px down
(margin `0 1 0 -1`, `:74-75`). Padding is the control's own (0) unless set.

### 5.5 Radio buttons (`TobiiRadioButtonStyle`, `BR:radiobutton.xaml:14-553`)

| Part | Value |
| --- | --- |
| Min height | 40 (`:29`) |
| Bullet area | 20 x 20 grid, margin `2 2 3 2` (`:48-50`); outline ellipse 1 px `#D9F4FF` (off and on); inner bullet ellipse margin 5 (10 x 10), `#008DBF`, shown when checked (`:51-70,521-546`) |
| Label | margin `5 0`, 10 pt, `#AFC4CD` (`:89-91`) |
| Hover (`:108-195`) | bullet `#00ACE9`, outlines and label `#FFFFFF`, off fill `#637279`, scale 1.05 |
| Pressed (`:372-459`) | bullet `#00BEFF`, outlines and label `#FFFFFF`, off fill `#AFC4CD`, scale 0.95 (by -0.05) |
| Disabled (`:460-518`) | bullet `#2B3940`, outlines `#637279`, off fill transparent, label `#2B3940` |
| Transitions | 0.1 s (`:8`); the scale applies to the bullet |

The page sets the margin `0 0 15 0` on the first two radios
(`VIEW:SettingsPage.xaml:157,162`).

### 5.6 Profile list item (`TH:generic.xaml:107-304`)

| Part | Value |
| --- | --- |
| List box | transparent background, no border, min 5 x 5, no scrolling of its own: its template is a bare items presenter; selection highlight brushes are transparent (`VIEW:SettingsPage.xaml:242-276`) |
| Row | 60 px high, transparent (`TH:generic.xaml:276-304`, height `:291`) |
| Columns | 200 and `*` (`:165-168`) |
| Column 0 | accent-theme button (`CalibrationProfileButton`, `:107-156`; placed at `:160-203`), vertically centred, enabled when the profile is not marked deleted; content: `ProfileImage` 30 px wide, vertically centred, then the name in Body style (12 pt), vertically centred, left, margin `5 0 0 0`, foreground = the button's; click selects the profile |
| Column 1, undo | shown when `CanUndoDelete`: button margin `0 3 0 3`, left aligned, vertically centred, padding 15, content `Undo` (hard-coded English, not from the resource file), style `GazeActivatableText` (accent theme with Body text, `:16-68`) (`:204-235`) |
| Column 1, delete | shown when `CanDelete`: button left aligned, vertically centred, padding 15, content `TrashImage` at 30 px wide (`GazeActivatableSmallIconButton`, `:69-87`) (`:236-272`) |
| Deleted row | name and image `#6E6E6E` (button disabled), "Undo" visible, no trash |

### 5.7 Scrollbar (`TH:scrollbar.xaml`)

The page's `ScrollViewer` uses `ScrollViewerControlTemplate` (`:3-44`): content
column plus a vertical scrollbar column of `Auto` width. The scrollbar is 15 px
wide (`:20`); its thumb is 3 px wide, right aligned, fill `#004B66`
(`:92-115,163-175`). On mouse-over the thumb and the page buttons grow from 3 to
15 px in 0.1 s; after the pointer leaves they shrink back over 0.5 s starting
1 s later (`:189-208`).

### 5.8 Icon path data, verbatim (`TH:images.xaml`)

Fill or stroke is the foreground of the surrounding button (content control for
the arrows). Each icon is a `Viewbox` around a `Canvas`.

| Icon | Canvas | Paths |
| --- | --- | --- |
| `DownArrowImage` (`:115-137`) | 40 x 20 | fill: `M0 0 L40 0 L20 20 L0 0` |
| `RightArrowImage` (`:92-114`) | 20 x 40 | fill: `M0 0 L20 20 L0 40 L0 0` |
| `NewProfileImage` (`:284-351`) | 28 x 30.7, `UseLayoutRounding` false | four unfilled strokes, round line joins, `Stretch=Fill`:<br>1. `Canvas.Left` 0, `Top` 19, thickness 1.52: `F1 M0.76,29.9067 C0.76,29.9067 2.25467,19.7707 10.8947,19.7707 C19.536,19.7707 21.0307,29.9067 21.0307,29.9067`<br>2. `Left` 18.5, `Top` 4.3, thickness 1.28: `F1 M27.7493,4.94266 L19.212,4.94266`<br>3. `Left` 22.8, `Top` 0, thickness 1.28: `F1 M23.484,0.677332 L23.484,9.208`<br>4. `Left` 3.5, `Top` 5.4, thickness 1.52: `F1 M17.936,12.9427 C17.936,16.7133 14.8787,19.7707 11.108,19.7707 C7.33733,19.7707 4.28,16.7133 4.28,12.9427 C4.28,9.172 7.33733,6.11466 11.108,6.11466 C14.8787,6.11466 17.936,9.172 17.936,12.9427` |
| `ProfileImage` (`:352-387`) | 28 x 30.7 | paths 1 and 4 of `NewProfileImage` (`:354,370`), same attributes |
| `TrashImage` (`:218-240`) | 48 x 48, transparent background | fill, non-zero: `M10 9v2h28V9H10z M30 5H18v2h12V5z M12 38c0 2.2 1.8 4 4 4h16c2.2 0 4-1.8 4-4V13H12V38z M14 15h20v23 c0 1.1-0.9 2-2 2H16c-1.1 0-2-0.9-2-2V15z` (the file has two spaces after `v23`) |

Sizes in use: `NewProfileImage` 35 px wide beside "Create new profile";
`ProfileImage` 30 px wide in rows; `TrashImage` 30 px wide in its button; the
arrows 18 px (width for the down arrow, height for the right arrow).

## 6. Guest and Ctrl+Shift+F10

The page has no Guest button. A guest profile is started from the keyboard.

| Fact | Value | Evidence |
| --- | --- | --- |
| Trigger | the global shortcut of `GuestCalibrationLauncher`, a runnable feature of the Interaction process; default shortcut text `CTRL+SHIFT+SC_68` (scan code 68 = 0x44, F10 on a PC keyboard; the shortcut parser was not read) and `Enabled` default `True` | `INTBOOT:FeatureBootstrapper.cs:42`; `SET:GuestCalibrationLauncherSettings.cs:12,25` |
| Key handling | the key combination is acquired from the input engine on key down (the acquire semantics were not read); while a launch is running further presses are ignored | `GUEST:GuestCalibrationLauncher.cs:57-92` |
| When it works | only with a gaze-capable device, and only while the device status is `InvalidConfiguration` or `Tracking` (connected); ignored when `Initializing`, `NotAvailable`, `DeviceNotConnected`, `TrackingPaused`, `Configuring`, `UnknownError`, `ConnectionError` or `TrackingUnavailable` | `SET:GuestCalibrationLauncherSettings.cs:8-9`; `FDEV:TrackingDeviceStatusConstraint.cs:60-108` |
| Action | `LaunchConfigurationFlow(GuestCalibration)`: `Tobii.Configuration.exe -G`; `AlreadyRunning` when a configuration run is active; the result is only logged | `GUEST:GuestCalibrationLauncher.cs:86`; 4.7 |
| `-G` flow | calibration flow of type `GuestCalibration`: profile name `Guest`, tracked eyes both; a 5-point calibration (`NumberOfGuestCalibrationDots = 5`, constants only, use not traced; recalibration and new profile 9, quick 4) | `CONF:MainModel.cs:83-96`; `CFLOW:FlowConfigurationResolver.cs:61-83`; `CCOM:CalibrationHelper.cs:13-23` |
| On success | creates the profile `Guest` when the list lacks it, stores the calibration for the tracker and monitor, then makes `Guest` the current profile; a failed create or store ends with exit code 3, an aborted calibration with 2 | `CFLOW:CalibrationFlow.cs:102-152`; `CCAL:CalibrationDataStorage.cs:23-69` |
| Eye positioning and screens | as the other calibrations, see [setup flow](setup-flow.md) | |
| Effect on the page | `Guest` is never listed, cannot be deleted, and "Improve calibration" is disabled while it is current; the tracked-eyes radios still act on it; "Create new profile" refuses the name `Guest` | 4.3, 4.6 |
| Lifetime | nothing in the read code removes `Guest`; the next `-G` run overwrites its calibration | `TDC:ConfigurationManager.cs:686`; search of all decompiled callers |
| Page text | `Press Ctrl+Shft+F10 for a temporary guest profile.` (4.2 row 7) | `LOC:168` |

Linux: `Ctrl+Shift+F10` becomes `tobii-linux-experience --guest`, which the user
binds in Hyprland (decision, spec 8; Wayland apps cannot grab global keys). The
command starts `tobii-linux-setup --guest`, setup's spelling of `-G`; setup takes
either (decision). Apply the same status gate: only when the
status is `InvalidConfiguration` or `Tracking` (proposal), and the same
re-entrancy rule (an active setup run makes it do nothing). Setup puts both
eyes on the device (3160) before the guest calibration, whatever the current
profile tracks, to match the flow's "tracked eyes both"
([setup flow](setup-flow.md#tobii-linux-setup-arguments)). The page keeps the
resource text, including the key name and the "Shft" spelling (proposal, for
1:1 wording); the user binds the key.

## 7. Fixed Linux decisions for this area

- `GetStarted` is never shown: no EULA, so the "Try Tobii Experience" text is
  unused too.
- `Ctrl+Shift+F10` becomes `tobii-linux-experience --guest`, run from a user
  key binding.
- Windows Hello and the virtual camera, battery messages and RGB device text
  are omitted; so are the Close Interaction menu item and statistics.
- Setup is launched with Tobii's own arguments (`-C`, `-N`, `-S`, `-R`, and
  `--guest` for `-G`) by the binary `tobii-linux-setup`, which takes all of them.
- The page is a 550 px wide window titled "Tobii Experience" with the
  sidebar's content; Tobii's ribbon, dimmer, slide animation and 1330 x 810
  host window are not reproduced (4.1).
- Quit stops the service unit and exits the app and leaves the persisted on/off
  value alone (spec 7).

## 8. Not read, and open points

Not read (where I looked is in brackets):

- The title bar template and its buttons (`BR:titlebar.xaml` was only grepped
  for sizes); how the quit dialog's title bar looks.
- The order `GetAllSubkeys` returns profiles in (`TDC:ConfigurationManager.cs:415`
  calls it; the registry helper is in an assembly not decompiled).
- How the engine decides that an Eye Tracker 5 is `Peripheral` (the page only
  reads the string, `DEV:TrackingDeviceInfoProvider.cs:157-168`).
- `WPF` easing numbering: `EasingMode` value 1 was read from the decompiled call
  (`VIEW:HomePage.xaml.cs:84`); the name EaseIn is from the framework's enum
  numbering, which is not in the sources.
- The shortcut parser (`SC_68`) and the key acquisition in the input engine
  (`GUEST:GuestCalibrationLauncher.cs:69-71`).
- `ISystemLauncher` (the Windows Store launch for "Open") and the EULA flow.
- The native gaze bubble drawing in `GazeNative.dll` (not decompiled).
- Where `EyeTrackingHostState.FirmwareUpgrade` is produced.
- Which numbers of calibration points each flow really uses (the constants in
  `CCOM:CalibrationHelper.cs` are inlined at their uses).
- How the new-profile flow sets the current profile (`CreateNewProfileFlow`);
  the guest and recalibration path does it in `CCAL:CalibrationDataStorage.cs:58-66`.

Open points for the tasks that build on this ledger:

- The current profile is not shown on the page itself: Tobii shows it in the
  Home page ribbon (`VIEW:HomePage.xaml:127-137`) and the list omits it. Linux:
  a header row above the page shows it (4.1).
- The page text and the quit dialog mention a key (`Ctrl+Shft+F10`) and a
  "start menu" that Linux does not have.
- `Segoe UI Light` needs a substitute font.

## 9. Corrections to the first recon note

The planning note `experience-recon/tobii-page.md` was checked against the
files. Differences:

- The vertical scrollbar is not an overlay: it has its own `Auto` column
  (`TH:scrollbar.xaml:8-12,20`), so the content is 480 px wide while it shows
  and 495 px when it does not.
- The sidebar opens with `CubicEase` whose `EasingMode` value is 1, which is
  EaseIn in WPF's numbering, not EaseOut (4.1).
- `ISpinnerUpdater` has no implementation anywhere; the tray's Progress icon is
  static (1.3).
- The pressed toggle scales its knob to 0.93; the hover scale 1.07 is as noted
  (5.3).
- The toggle's labels come from `General_Status_ON` and `General_Status_OFF`
  through the page style, not from the Branding style itself (`TH:buttons.xaml:26-51`).
- The tray quit option exists because the Store app writes `HasQuitOption =
  true` at every start (`WIN:App.xaml.cs:72`).
- The Guest shortcut works in the device statuses `InvalidConfiguration` and
  `Tracking`, not only `Tracking`: the constraint folds both into "Tracking"
  (`FDEV:TrackingDeviceStatusConstraint.cs:76-108`).
- The Progress icon has 10 sizes (16 to 512 px) and shows 10 dots; the earlier
  "10-size icon of grey dots" is both, and neither is an animation (1.3).
