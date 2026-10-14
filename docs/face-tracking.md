# Face tracking loop

`VisionSDK::VNNTrackerCore` (vtable `0x141132a08`, constructor `0x140048fc0`)
runs once per camera frame in `0x140049450`. Addresses refer to runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`. Evidence:
`evidence/private/trackstate-focused.c`, `adjust-focused.c`,
`trackmap-focused.c`, `d1-consumer-focused.c` and emulation.

## States

| State | Meaning | Face box source | D1 gate |
| --- | --- | --- | --- |
| 0 | searching | full-frame detector ([face detection](face-detection.md)), first box | score > 0.5 |
| 1 | recovering | previous box, then re-detection inside the working image | score > 0.5 |
| 2 | tracking | square around the previous frame's landmarks | score > 0.35 |

Reset (`0x140049400`, vtable slot 2) sets state 0, the box to (-1, -1, -1, -1),
turns roll compensation off and reinitialises the fitter caller (initial yaw
3, zero deformations). It runs when detection finds nothing, re-detection
fails, the fit fails, the eye refinement fails, or the gate fails outside
state 2. A gate failure in state 2 moves to state 1 and keeps the current box.
A passing gate moves to state 2 with the box from `0x14007a4b0`.

## Working view (`0x140079ce0`)

The tracker never runs D1 or the fit on the raw frame. The constructor sets a
target face width of 96 and an expansion of 2.5, and preallocates a 240x240
working image.

1. Expand the integer box around its centre: `w' = floor(2.5 w)`,
   `x' = floor((w/2 + x) - w'/2)` (float32), likewise for y.
2. Clamp to the frame (`0x14007a740`) and shift the face box into the region
   (CroppingAdjustment, vtable `0x141133dd8`).
3. Scale the face box and region by `96 / w` with round-half-even
   (ResizingAdjustment, vtable `0x141133df8`).
4. Resize the region into the working image with the bilinear resize, through a
   2x intermediate when the face is at least 768 pixels wide.

In state 1, `0x1400499d0` runs the detector on the working image (sensitivity
0.95, no size limits) and replaces the box with the detection of highest
inclusive-pixel overlap, if that overlap exceeds 0.1.

D1 runs on the working image. After the gate passes, the core's own pose fit
would run on working-image coordinates, but the shipped configuration disables
it (see [landmark smoothing](landmark-smoothing.md)). Records are then mapped
back by the adjustments' inverses (`0x14007aa30`:
`x = (region_w * x' + region_x) / frame_w`), copied to the caller and flipped
to Y up (`0x14007ac80`).

## Temporal roll

After each D1 run `0x14007dc50` stores, at D1 object `+0x14`,
`atan2(dy, dx) * 57.2957763671875 - 90` degrees, where (dx, dy) runs from the
midpoint of FDP 4.1/4.2 to the midpoint of FDP 9.4/9.5 in working-image pixels.
The next frame's crop rotates by it when the tracker was in state 2 and
`|roll| > 5`. The value survives resets; it starts at 0.

## Implementation and checks

`src/tobii_linux/face_tracking.py`: `working_view`, `redetect`,
`landmark_box`, `temporal_roll`, `to_frame_points`, `frame_records` and
`FaceTracker`, which combines the detector, D1 and gate of this loop with the
per-object step `0x14004a530`: landmark smoothing, then the temporal fitter of
[pose-temporal](pose-temporal.md) (`0x14007e440`) on frame-normalised records
([landmark smoothing](landmark-smoothing.md)). A failed gate while tracking
still returns success: the core moves to recovering and its records are
smoothed and fitted.

`tools/face_tracking_oracle.py` runs the original `0x140079ce0` on five face
boxes (centred, clamped at the top-left and bottom-right, downscaled 97 to 96,
upscaled 41 to 96). The working image bytes, scaled face box and clamped region
are byte-exact in all five. Report: `evidence/private/face-tracking-oracle.json`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/face_tracking_oracle.py
```

Not yet covered: the two-stage resize for faces of 768 pixels or more and an
emulated check of re-detection and the landmark box. The LBF eye refinement
`0x14007d270` is omitted: it fails only for a source-image region outside the
image, which this loop never sets, and writes nothing with the installed
models. A public-domain portrait (NASA astronaut photo from scikit-image's
sample data) runs through detection, D1, the gate and the temporal fit and
holds a stable frontal pose over repeated frames; this is a sanity check only.

## Fitting coordinates (live comparison)

Static reading of `0x140049450` suggests the fit receives working-image
coordinates and dimensions. A live comparison contradicts that. Frames
captured by the Linux session with a person in front of the tracker
(`evidence/private/live-compare/`), with the Windows runtime recorded
immediately before at the same head position:

| Variant | Position (mm) | Rotation (deg) |
| --- | --- | --- |
| Windows runtime | (-27.9, -8.3, 694.5) | (8.3, -0.2, 1.8) |
| Working view, focal 2.6 | (-5.9, -8.6, 400.4) | (4.0, 3.4, 3.4) |
| Working view, focal 3.0 | (-6.0, 14.3, 463.4) | (3.9, 3.5, 3.3) |
| Full frame, focal 2.6 | (-42.8, -9.5, 690.8) | (11.6, 1.0, 4.3) |
| Full frame, focal 3.0 | (-42.8, 26.7, 790.4) | (10.5, 1.4, 4.2) |

Only full-frame landmarks with the `Head Tracker.cfg` focal 2.6 reproduce the
Windows depth (0.5% apart) and height. The static mechanism is now known
(static, file parse): `0x140049e60` disables the core fit because the model
FDP map has no group 10 point, and the object step `0x14004a530` fits the
smoothed, frame-normalised records with the full frame size through
`0x14004a2c0` ([landmark smoothing](landmark-smoothing.md)). Adding the
smoothing leaves the means unchanged (live).

The 15 mm and 3 to 4 degree differences in that first comparison were head
movement between recordings. A second session, with the person holding still
through a Windows recording, a Linux recording and the Linux service, agrees
within 2 mm and 0.4 degrees (live, `evidence/private/e2e-person/`):

| Source | Position (mm) | Rotation (deg) | Std. dev. position (mm) | Std. dev. rotation (deg) |
| --- | --- | --- | --- | --- |
| Windows runtime, just after | (-27.4, 3.1, 620.4) | (11.85, 0.73, 2.21) | (0.34, 0.22, 0.93) | (0.18, 0.07, 0.16) |
| Linux, 150 recorded frames replayed | (-27.7, 3.0, 622.1) | (11.51, 1.14, 2.27) | (0.67, 0.53, 2.12) | (0.14, 0.15, 0.11) |
| Linux through Wine, game DLL (Stream Engine path) | (-26.7, 3.0, 622.3) | (11.54, 0.61, 2.13) | | |
| Linux through Wine, `tobii_stream_engine.dll` | (-27.1, 3.2, 623.2) | (11.37, 0.39, 2.27) | | |

A Windows recording just before was taken while the person was still settling
(-13 degrees of pitch) and is not comparable.

Frame-to-frame jitter (RMS of successive differences) is in the same range:
Windows (0.016, 0.024, 0.088) mm and (0.061, 0.021, 0.033) degrees, the
replayed Linux frames (0.021, 0.036, 0.157) mm and (0.023, 0.026, 0.016)
degrees, and the Linux service run through Wine (0.006, 0.005, 0.028) mm. The
recordings were taken one after another, so these figures mix tracker noise
with the person's own movement and cannot rank the two implementations.
