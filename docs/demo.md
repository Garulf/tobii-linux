# Browser demo

`python3 -m tobii_linux.demo` (or `tobii-linux demo`) opens one tracker
session, subscribes the gaze stream (index 1, wire `0x500`), the image stream
(index 2, wire `0x501`) and, when the device accepts it, the presence stream
(index 5, wire `0x504`), and serves a single page that shows the camera, head
pose, gaze and device status as they arrive. It is meant both as a showcase
and as a debugging aid.

Everything runs in one Python process. The USB thread splits the streams
(`streams.StreamReader`), gaze messages are decoded by `gaze.py` exactly as the
Windows runtime decodes them ([host-side gaze](gaze-host.md)), and camera
frames go to a processing thread that runs the same face tracking and head-pose
output as `cli headpose`. That thread only ever takes the newest
frame, so head pose never falls behind; skipped frames are counted as dropped.
The server is plain asyncio from the standard library: the page, its script
and stylesheet from `src/tobii_linux/demo/static/`, a Server-Sent Events
stream and a JSON status snapshot. There is no build step and nothing is
loaded from the network.

## Running on Linux

The tracker must be visible to libusb and accessible to the user (the NixOS
module installs a udev rule; otherwise run as root). Head pose needs NumPy,
OpenVINO and `models/tobii-et5`, as for `cli headpose`.

```sh
PYTHONPATH=src python3 -m tobii_linux.demo \
  --display -296.62 326 111.82 300.38 326 111.82 -296.62 10.27 -3.1
```

Then open http://localhost:8765/. Ctrl+C stops the demo; it unsubscribes both
streams and closes the USB channel before exiting. Without `--display` the
display area of the reference host above is used.

| Option | Default | Meaning |
| --- | --- | --- |
| `--host`, `--port` | `127.0.0.1`, `8765` | Where the page is served |
| `--display` | reference host | Display area corners in millimetres, as for `cli serve` |
| `--models` | `models/tobii-et5` | Model directory |
| `--image-rate` | 15 | Camera images sent to the browser per second |
| `--gaze-source` | `device` | Which gaze point the page shows |
| `--no-headpose` | off | Skip face tracking; camera and gaze still stream |
| `--index`, `--channel` | 0, 0 | Tracker index from `probe` and the TTP channel |

If no tracker is visible the page still loads and says so, and the demo
retries every few seconds, so the tracker can be attached afterwards. USB or
protocol errors are shown the same way and also trigger a reconnect.

## Running in WSL2 with usbipd

The demo runs unchanged inside WSL2. With WSL's mirrored networking a Windows
browser reaches the server at http://localhost:8765/; that was checked on the
reference host with `curl` from `cmd.exe` while the demo ran in WSL.

From an elevated Windows terminal, hand the tracker to WSL (the bus id comes
from `usbipd list`; the Tobii services hold the device and must be stopped
first):

```bat
usbipd bind --busid 8-2 --force
net stop TobiiIS5LEYETRACKER5 /y
net stop TobiiGeneric /y
net stop "Tobii Service" /y
usbipd attach --wsl --busid 8-2
```

WSL must be running for `usbipd attach`. Start the demo inside WSL as root (or
with a udev rule for `2104:0313`), then open the page in a Windows browser.
When done, give the tracker back to Windows:

```bat
usbipd detach --busid 8-2
usbipd unbind --busid 8-2
net start "Tobii Service"
net start TobiiGeneric
net start TobiiIS5LEYETRACKER5
```

`usbipd list` should then show the tracker as "Not shared".

## Views

**Camera.** The 280x280 infrared image scaled up, with the face box used for
the frame, the fitted face mesh as a wireframe and the 57 D1 landmarks drawn
on top in the frame's own coordinates, so overlay and image always match. The
wireframe is the `jk_300.wfm` mesh (357 vertices, 640 triangles) deformed with
the frame's fitted shape and animation coefficients and projected with the
fitted rigid state, exactly as the fitter projects its landmark vertices
(`face_wireframe.py`); where it sits on the face, the fit is right. Beside it: tracker state
(searching, recovering, tracking), the D1 landmark quality score and whether
the quality gate accepted it, camera and processing rates, processing time per
frame, dropped frames and a compact head-pose readout. The image can be
mirrored and the overlays hidden.

**Head pose.** The sample a game receives: position in millimetres and
rotation in degrees about x (pitch), y (yaw) and z (roll), with its validity.
A 3D view shows the same fitted mesh in display coordinates (origin at the
display centre, x right, y up, z towards the user) at the reported, filtered
head pose, together with the display rectangle, the tracker and the
tracker-to-head distance; drag to orbit, scroll to zoom, double-click to
reset. The mesh is placed with the geometry stage's own position transform
and then turned and moved from the unfiltered to the filtered pose. Below are
10 second plots of x/y/z and pitch/yaw/roll, each axis in its own lane with its
own scale.

**Gaze.** Per eye: validity, 2D gaze point, pupil diameter, eye openness and
gaze origin in tracker millimetres; the combined point, how much of the last
10 s was valid, the gaze rate, the device's user presence and a
pupil-diameter plot. A preview shows the
point on a rectangle with the display area's aspect ratio. "Fullscreen gaze
dot" opens a fullscreen view that draws a smoothed dot with an optional trail
and the raw samples (Esc exits, T toggles the trail, R the raw samples, `[` and
`]` change the smoothing). Gaze points are normalised to the display area, so
the fullscreen view is only accurate when the browser is fullscreen on the
monitor `--display` describes, normally the primary monitor. "All gaze stream
columns" lists every column of the latest message by id and the field name
`gaze.DeviceGaze` gives it.

**Status.** Connection state and messages, the `GET_DEVICE_INFO` reply
(serial number, model, generation, firmware version), stream rates and
totals, dropped frames, gaze messages the runtime would drop, decode errors,
presence records, stream messages from other streams,
the display area, the session details (USB bus and address, camera extrinsic,
or the replayed files) and recent errors.

## Replay

Recorded data can be played instead of a tracker, so the page can be developed
and shown anywhere:

```sh
PYTHONPATH=src python3 -m tobii_linux.demo \
  --replay-frames evidence/private/e2e-person/linux-frames.jsonl \
  --replay-gaze evidence/private/gaze-baseline/linux-gaze.jsonl
```

`--replay-frames` takes the JSON-lines log of `tools/linux_session_probe.py
--until frames`; its frames are read from `frames/` next to the log unless
`--replay-frames-dir` says otherwise, and its calibration step provides the
camera extrinsic for head pose. `--replay-gaze` takes a
`tools/linux_stream_capture.py` log and plays its gaze messages. Either may be
given alone. Each loops at its recorded rate; timestamps keep increasing
across loops so the head-pose filters see a continuous stream.

## Data sent to the browser

`GET /events` is a Server-Sent Events stream with these event types:

| Event | Rate | Content |
| --- | --- | --- |
| `status` | 1 Hz | Everything the Status view shows |
| `mesh_topology` | once | Wireframe edges, display corners and tracker position |
| `frame` | `--image-rate` | PNG image in base64 with the overlay of the same frame, mesh in 0.1 px int16 |
| `pose` | every processed frame | Head-pose sample, tracker state, quality, processing time, mesh in 0.1 mm int16 |
| `gaze` | every gaze message | Selected gaze point, combined point, per-eye values |
| `gaze_columns` | 4 Hz | Every column of the latest gaze message |

Events are written at most 30 times a second; a browser that falls behind
gets only the newest `frame`, `status` and `gaze_columns` and a bounded
backlog of `pose` and `gaze`. `GET /api/status` returns the latest status as
JSON, which is handy for checks with `curl`.

## Gaze columns and sources

Gaze messages go through `gaze.gaze_message` and `gaze.gaze_frame`, the same
decoder `cli serve` uses for games, so a message is dropped exactly where the
runtime drops it (a known column with an unexpected type) and every validity
is the one a game would see: an eye is valid when its validity column (7, 13)
is below 2, the combined point when column 27 is not 0. Eye openness (41, 43)
has been reported invalid in every capture so far, and the presence stream has
only ever reported device value 0 (unknown), even with a person in front.

The page shows the point games receive by default: the tracker's combined
point (column 28). Other sources can be added to `GAZE_SOURCES` in
`src/tobii_linux/demo/pipeline.py`: a source has a `name`, a `description` and
is called with each `gaze.GazeFrame`, returning a normalised `GazePoint` or
`None`. `--gaze-source` then selects it.

## Live check

On the reference host (WSL2 Ubuntu, tracker through usbipd, nobody in front
of the tracker) the demo streamed camera frames and gaze at about 36 Hz each,
processed head pose at about 35 frames per second at 10.6 ms per frame, sent
12 to 15 images per second, and reported the device as
`IS5_Large_Eyetracker_5`. The page, its script, the status snapshot and the
event stream were fetched from Windows at http://localhost:8765/, and Ctrl+C
unsubscribed both streams and exited cleanly.
