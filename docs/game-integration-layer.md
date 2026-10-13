# Game integration layer

Star Citizen's `tobii_gameintegration_x64.dll` 9.0.3.25 (SHA-256
`35a071073c7498fd917b7e29292299f8b3214fe5a7a8cf5d62610d0331146020`) has two
layers: an embedded Stream Engine that speaks PRP to the service, and the game
integration layer ("TGI") above it, which finds trackers, keeps a device per
tracked tracker and buffers head poses for the game. [prp-bridge.md](prp-bridge.md)
covers driving the embedded Stream Engine directly. This document covers the
layer the game itself uses.

Evidence labels: **static** (disassembly and Ghidra `-noanalysis` decompiles of
the DLL), **live (Windows)** (albedo, real Tobii service build
`1.37.4.0_f2b4e724c9`, tracker `IS5FF-100204141463`, nobody in front of it),
**live (Wine)** (albedo WSL2, Wine 11.0 with the pipe-listing patch, synthetic
samples from `prp_service --synthetic`), **unit**, **guess**.

## Result

The layer delivers head pose from the Linux service under Wine:

| Case | Head poses | Notes |
| --- | --- | --- |
| `TrackTracker(url)`, no display options | 330 in 10 s, two client runs per bridge session | every package; timestamp to read 0.8 to 17 ms, median 8.8 ms, with a 16 ms polling loop |
| `TrackTracker(url)`, `--display-area` and `--display-id` | 330 in 10 s | tracker info gains the monitor name, rectangle and size in mm |
| `TrackRectangle`, `--display-area` and `--display-id` | 330 in 10 s | same monitor lookup as `TrackWindow` |
| `TrackRectangle`, display area only | 0, `TrackRectangle` returns false | the tracker has no monitor rectangle |
| stock Wine 9.0 (no pipe listing) | 0, `GetTrackerInfos` reports no tracker after 15 s | the layer only tracks trackers found by discovery |

All live (Wine), log `evidence/private/prp-bridge/gi-wine-final.txt` and
`gi-wine-stock-9.0.txt`. Before this change the service closed the command
connection on the first property get, and later answered property subscribes
with an error; in both states the layer reported the tracker as connected but
returned no head poses (`gi-wine-before-properties.txt`,
`gi-wine-loop-before-subscribe.txt`).

Against the real service on Windows the same client found the tracker,
`TrackTracker` returned true and `IsConnected` became true, but no head poses
arrived, because nobody sat in front of the tracker and the layer drops samples
without any valid field (`0x18000a140`, static). `TrackWindow` failed there:
over SSH the session has no display devices, so the layer could not match the
tracker to a monitor (`gi-real-service-1.txt`, live (Windows)).

`StarCitizen.exe` imports `GetApi` and three head-tracking helper functions
(`SetCenterStabilization`, `SetEyeHeadTrackingRatio`,
`SetHeadMultiAxisSettingsValues`) from this DLL and none of the C exports
(import table read on albedo, live (Windows)). The game therefore uses the C++
interface; which tracking call it makes (`TrackWindow` or `TrackTracker`) was not
established because the game executable was not analysed.

## C API

Every C export loads the API lazily through `0x180005300`:
`LoadLibraryA("tobii_gameintegration_x64.dll")`, `GetProcAddress("GetApi")`,
then `GetApi("TGI C API", 9, 0, 3, 0, 0, 0)`; `SetApplicationName(name)` does the
same with the given name. The exports call methods of the same interface objects
the game gets from `GetApi` (static).

| Export | Interface method | Signature as used |
| --- | --- | --- |
| `UpdateTrackerInfos` | controller slot 2 | `void (void)`: starts a rescan |
| `GetTrackerInfos` | controller slot 3 | `bool (const TrackerInfo **infos, int *count)`; true once the scan finished |
| `GetTrackerInfo` | controller slot 1 | `bool (TrackerInfo *tracked)` |
| `TrackTracker` | controller slot 11 (`0x1801d01b0`) | `bool (const char *url)`; the URL must be in the scanned list |
| `TrackWindow` | controller slot 6 (`0x1801d0300`) | `bool (HWND)` |
| `TrackRectangle` | controller slot 5 (`0x1801d0400`) | `bool (const RECT *)`, screen pixels |
| `StopTracking`, `IsConnected` | controller slots 7, 8 | |
| `IsStreamSupported` | controller slot 10 | `bool (const uint32_t *capability)`; head pose is capability 2 |
| `GetHeadPoses` | streams slot 0 (`0x1801cbed0`) | `uint32_t (const HeadPose **poses)`: the buffered poses, at most 64 |
| `GetLatestHeadPose` | streams slot 1 | `bool (HeadPose *)` |
| `IsPresent` | streams slot 6 | |
| `Update`, `Shutdown`, `IsApiInitialized` | API slots 6, 7, 5 | |

Reading head poses sets the head-pose request flag, and the device thread then
subscribes the stream (`0x1801cbed0`, `0x180009060`, static). `Update` appears
to clear a request again when it has not been read for a while
(auto-unsubscribe, `0x1801ce9b0`, static, not traced in detail).

Structures (static, `??0`/`??4` exports and the converters; values live (Wine)):

- `HeadPose`, 32 bytes: rotation in degrees at 0 (3 floats), position in mm at
  12 (3 floats), `int64` timestamp in the client's QPC microseconds at 24. With
  the synthetic sweep the rotation floats were `-rotation_y`, `rotation_x`,
  `-rotation_z` of the PRP sample converted to degrees, and the position equals
  the PRP position (fit of 198 samples, residual below 0.004 degrees; the roll
  sign is inferred from `-0.0`).
- `TrackerInfo`, 96 bytes (`0x1801cfd30`): type, capability flags, monitor
  rectangle in pixels (4 ints), display width and height in mm, then pointers to
  the URL, the property 8 string, the monitor device name, model, generation,
  serial and firmware strings, and an attached flag at 88.

`tools/prp_bridge/tgi_test_client.c` uses exactly these exports.

## Tracker scan

A background thread (`0x180007a80`) runs `TobiiDeviceFinder::scanForDevices`
(`0x180008a50`) once and again every 10 s (static). For each URL from
`tobii_enumerate_local_device_urls` it creates a device, checks stream support,
and reads (static, confirmed by the request order live (Wine)):

1. Property 1, display area (`0x1800147d0`, `tobii_get_display_area`): nine
   floats, top-left, top-right and bottom-left corners in mm. Width is
   `top_right.x - top_left.x`, height `top_left.y - bottom_left.y`.
2. Property 0x12, display id (`0x1800154a0`, `tobii_get_display_id`).
3. Device info, then property 8 (`0x180014df0`), then destroys the device.

If both properties succeed, `0x1800084d0` cuts the display id at `##` and looks
for an active monitor whose device instance id (`CM_Get_Device_Interface_PropertyW`
with `DEVPKEY_Device_InstanceId` on the `EnumDisplayDevicesW` interface name)
equals it, and stores that monitor's device name. `EnumDisplayMonitors` then
fills the monitor rectangle. If either property fails, the layer logs "Could not
get display area/id from stream engine" and keeps the tracker without monitor
data (static, live (Wine)).

`TrackWindow` and `TrackRectangle` pick the tracker whose monitor rectangle
overlaps the window or rectangle most (`0x180007fc0`, static). A tracker without
a monitor rectangle is never picked: live (Wine) `TrackRectangle` failed without
`--display-id` and worked with it. `TrackTracker` only needs the URL.

Under Wine without a display driver the only monitor is `\\.\DISPLAY1`,
1024 x 768, instance id `DISPLAY\DEFAULT_MONITOR\0000&0000` (live (Wine)).
`tgi_test_client.exe x displays` prints the instance ids the layer compares
against. The real service sent
`DISPLAY\GSM5B7F\5&5028A6B&0&UID41221##eId:1E6D7F5B0B220100061E#or:1#v:1.0`
(live (Windows)). Ubuntu's Wine 9.0 does not implement
`CM_Get_Device_Interface_PropertyW` and aborts the calling process (live (Wine)
with the test client), so on such a Wine a configured display id would end the
game during the scan (inference); Wine 11.0 implements it.

## Tracking session

After `TrackTracker` the device thread (`0x180009060`) creates a new device and
sends, in this order (live (Wine), with display options):

| Request | Purpose | Real service (live, Windows) |
| --- | --- | --- |
| property get 4 | stored as a flag (`0x18000e8c0`, SE property 2) | status 0, value 0 |
| command 5 for 1, 2, 4, 5 | `tobii_notifications_subscribe` (`0x18000fea0`) | 1, 2, 4: status 0 with the current value; 5 not tried |
| command 6 for 1, 2, 4 | unsubscribe, sent right after property 5 was refused | status 0, echoes the property id |
| command 5 for 0xB | `tobii_user_presence_subscribe` (`0x18000fc60`) | status 0, presence value |
| stream start 2 | head pose | status 0 |
| command 6 for 0xB, stream stop 2 | on `StopTracking`/`Shutdown` | |

Without display options the layer subscribes property 1 only, which is refused,
and does not try 2, 4 and 5.

When the presence subscribe failed, the client fell back to a property get of
0xB, which the real service also refuses, and the device thread then repeated
"get 0xB, stream start" about every 30 ms without delivering any head pose
(live (Wine), before this change). The reason for the repeat was not traced;
answering command 5 for 0xB as the real service does removed it.

## Property messages

Property get is operation 5 with one block `{8, property}`; the answer is
operation 6 (`decoder 0x180055780`, static; bytes live (Windows) and live (Wine)):

```
request:  {12, tag, 5} {4} {8, property} {4} 0
response: {12, tag, 6} {4} {12, status, property} {4} <value block> <region> 0
```

Command 5 (subscribe) and 6 (unsubscribe) are operation 9 commands whose body is
`{8, property}`. The command 5 response carries `{8, property}` and then the same
value block as operation 6; the command 6 response is `{8, property}`.

Value blocks used here (decoder sizes static, values live (Windows)):

| Property | Block | Real value |
| --- | --- | --- |
| 1 display area | `{0x28, 9 floats}` | -296.62, 326.00, 111.82 / 300.38, 326.00, 111.82 / -296.62, 10.27, -3.10 |
| 0x12 display id, 8 | `{8, 4}`, region `{size, text, NUL, padding}` | see above; 8 is `IS5_Large_Eyetracker_5` |
| 2, 4 | `{8, word}` | 0, 0 |
| 0xB user presence | `{0x10, time high, time low, value}` | 2 with nobody present; get refused |

The presence time was identical in two subscribes made minutes apart, so it is
the time of the last change. The client maps wire value 2 to "away" and 3 to
"present" (`0x180030098`, static).

Refusals carry status 2 and a zero value block of the size the decoder needs for
that property (for example 0x10 for 0xB, 0x1C for 0xA, 0x34 for 0x17, 4 for 0);
the real service's refusals were byte-identical to `prp_service.property_error`
for 0, 3, 0xA, 0xB, 0xD, 0xE, 0x14, 0x17 and 0x18 (unit, live (Windows)).
Properties 0xC and 0x13 were refused with status 17 (`INSUFFICIENT_LICENSE`).
No property notification (operation 2) arrived within one second after
subscribing (live (Windows)).

## Service behaviour

`src/tobii_linux/prp_service.py` answers (unit tests compare with the captured
bytes):

- property 1 from `--display-area x,y,z,x,y,z,x,y,z` (top-left, top-right,
  bottom-left, mm), property 0x12 from `--display-id` or else the id the
  bridge reports, both refused when not available;
- property 8 with `IS5_Large_Eyetracker_5`; the real reply differs only in an
  uninitialised padding byte;
- properties 2 and 4 with 0;
- property 0xB on subscribe only: 3 while the newest sample is valid, 2
  otherwise, with the timestamp of the sample where that last changed;
- command 6 with status 0 and the property id;
- everything else with the refusal above.

`PrpService(hub, name, display_area=DisplayArea(...), display_id=...)` takes the
same values; `tobii_linux.cli serve` passes its `--display` area and an optional
`--display-id`.

The bridge reads the primary monitor's device instance id inside the prefix
(`EnumDisplayDevicesW` with `EDD_GET_DEVICE_INTERFACE_NAME`, then
`CM_Get_Device_Interface_PropertyW` with `DEVPKEY_Device_InstanceId`, as the
layer does) and sends it as relay frame 6. It skips this on Wine before 10.0
(`wine_get_version`), where that call aborts the process. With only
`--display-area` on the service, `TrackRectangle` under Wine 11.0 received the
reported `DISPLAY\DEFAULT_MONITOR\0000&0000` and 329 head poses in 10 s
(live (Wine)); under Wine 9.0 the bridge logged the skip and the Stream
Engine client still received 91 samples in 3 s.

## Open

- Which tracking call Star Citizen makes. `TrackWindow` matches the window's
  monitor against the display id; the bridge reports the primary monitor, so a
  game on another monitor still needs `--display-id`.
- `TrackWindow` itself was not run under Wine: the patched Wine 11.0 build has no
  display driver (`--without-x`), so no window can be created.
- Property notifications: the service never pushes operation 2, so presence and
  display changes after subscribing are not reported. The real encoding of a
  notification was not captured.
- The meaning of properties 2, 4 and 5 and of the "type" and capability bits in
  `TrackerInfo`.
- The layer's own log goes to `OutputDebugStringA` only when its logger is
  enabled (`0x18000b8f0`); the test client's capture saw nothing in any run.
