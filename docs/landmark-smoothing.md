# Landmark smoothing and the object fit

After the core frame loop ([face tracking](face-tracking.md)) the runtime
smooths the landmark records over time and only then fits the face model.
This page covers the per-object step `0x14004a530`, its fit `0x14004a2c0`,
the smoothing `0x14007f5a0` and the two history filters `0x14004c480` and
`0x14004bf60`. Addresses refer to runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`.

Evidence labels: static (raw instructions or decompiler output), emulation
(original code under Unicorn), file parse, live (recorded frames), inference.
Sources: `evidence/private/refit-focused.c`, `smooth-focused.c`,
`filter-focused.c`, `c480-focused.c` (decompiled with a 900 s timeout),
`smoothcaller-focused.c`, `d1-consumer-focused.c` and the raw listings
`smooth-f5a0-disassembly.txt`, `smooth-bf60-disassembly.txt`,
`smooth-c480-disassembly.txt`.

## Which fit runs (static, file parse)

`0x140049e60` configures each tracked object. It sets byte `+0x63` when a
face model is configured and builds the fitter caller with type 0, so
`0x14004a2c0` calls `0x14007e440` on the object's record copy `obj[9]` with
the full image width and height stored at `+0x68`/`+0x6c`.

The core's own fit inside `0x140049450` needs the core flag `config+0x15`.
`0x140049e60` clears that flag when any of the 24 FDP group 10 records of the
model mapping is absent. `jk_300_rigid.fdp` maps no group 10 point (groups 2,
3, 4, 8, 9, 12, 14 and 15 only), so the inner fit and the LBF eye refinement
never run. The core vtable slot `+0x30` (`0x1400499c0`) returns the same
flag, so the second smoothing call for group 10 (ears) is skipped as well.
This is why fitting full-frame points matched the Windows depth in the live
comparison: the shipped pose comes from the object fit on frame-normalised,
smoothed records.

## Record flow and Y convention (static, emulation)

1. The core writes its records in working-image coordinates with Y down.
   After a passing gate (or a failing gate while tracking, which still
   returns success and moves the core to recovering) the adjustment undo
   maps them to the frame, then `0x140010560` copies them to `obj[8]` and
   `0x14007ac80` flips them: `y = 1 - y` for present records. `obj[8]` has
   Y up.
2. `0x14004a530` smooths `obj[8]` in place, copies it to `obj[9]` and flips
   again, so the fit sees frame-normalised records with Y down.
3. `0x14007e440` passes those records to the fitter unchanged and flips only
   its private copy `+0x60` for the reprojection error.

`prepare_observations` expects Y down (`1 - 2y` points up in the fitting
plane), which is what `FaceTracker` already supplied; the two flips cancel.
The oracle checks `0x14007ac80` against `flip_y` byte for byte.

## Object state (static)

`0x14004a530` keeps a state at `+0x10`: 3 after a reset, 1 while the fit
succeeds, 2 after a fit failure that followed success. Per frame:

1. All three filters at `obj[3]+8/+0x10/+0x18` get the cursor `+0xc` set to
   0 and the frame counter `+0x10` incremented; `enabled` (`+8`) follows the
   core result.
2. On core success and state other than 1, the frame time becomes the
   smoothing epoch (`+0x50`) and `0x14004bed0` resets the filters. The
   smoothing gets `time - epoch`.
3. In state 2 the first lost frame stores its time; once
   `recovery_timeout <= time - lost_since` the fitter is reset
   (`0x14007e270`), the filters too, and the state returns to 3.
4. `0x14004a2c0` always runs. With a failed core it calls `0x14007e440` with
   gate 0, which fails; any failed fit resets the core (vtable slot 2) and
   the fitter.

The time comes from the tracker field `+0x98`, written by `0x140017ef0`: an
external frame time when supplied, otherwise `_ftime` in milliseconds.

## Group table and parameters (static, file parse)

`0x14004a530` builds 13 entries of four floats (group, first index, last
index, factor). The factors are columns of the `smoothing_factors` matrix of
`Head Tracker.cfg` (parsed by `0x140005440`, stored at config `+0x1a8`,
copied to the tracker at `+0x78` and cloned into `obj[3]` by `0x140049e60`).

| Entry | Records | Column |
| --- | --- | --- |
| eyebrows | 4.1-4.6, 14.1-14.4 | 0 (1.1) |
| mouth | 8.1-8.10, 2.2-2.9 | 1 (0.9) |
| pupils | 3.5-3.6 | 2 (1.1) |
| eyelids | 3.1-3.4, 3.7-3.14, 12.5-12.12 | 3 (1.1) |
| nose | 9.3-9.5, 9.15, 14.22-14.25 | 4 (1.1) |
| contour | 2.1, 15.1-15.17 | 5 (1.2) |
| ears (second call) | 10.1-10.24 | 7 (`-1.0` in the built-in default) |

The constants `0x141135e30/e40/e50/e58/e5c/e60` are 7, 8, 17, 22, 24 and 25.
Each coordinate gain is `factor + (1 - quality) * 0.2` (`0x141135d30`), or the
factor alone for group 15. Absent records are cleared to quality -1, so their
gain is `factor + 0.4`; they are smoothed too, which matters only for the
channel count below. The installed file has seven factors; column 6
(screen-space gaze) is unused here.

## Coordinate filter `0x14004c480` (static, emulation)

The filter object `0x14004bcc0(2000)` holds, per channel, a 31-sample value
ring, a time ring, a position, an initialised byte and two bounds.
`0x14004bed0` rewinds the positions, clears the initialised bytes and sets
every time to -10000000.

For one call with coordinate list `v`, gains `g` and time `t`:

1. Coordinates with a negative gain are dropped; the rest use consecutive
   channels from the cursor.
2. Each channel stores `v` and `t`. Samples with `t - t_i < 310` form the
   window; the count `n` comes from the first channel.
3. With `n <= 4` nothing changes. Otherwise, for each degree `d` of 0, 1, 4
   at position `k` of 0, 1, 2:
   - prediction `e_d = P_d[0] . Y`, where `P_d` is the SVD pseudo-inverse of
     `V[i, j] = (i - n + 1)^j`: the polynomial value at the newest sample;
   - residual `r_d`: per channel sum of squared residuals of a least-squares
     fit (SVD solve) of degree `d` in `T_i = (t_i - t_0) / 30`;
   - weight `w_d = exp(-k * g) / (m * r_d + 1e-8)` with `m` the number of
     kept coordinates.
4. Result `sum(w_d e_d) / (sum(w_d) + 1e-8)`, written back.

The weights prefer the lower degree by `exp(-g)` per step unless a higher
degree fits clearly better, so a steady head is averaged over about 310 ms
while a moving head follows a linear or quartic trend without lag.

## Quality filter `0x14004bf60` (static, emulation)

After the coordinates, every present record with non-negative quality (FDP
order, groups 2 to 15) is filtered with gain 0.5, lower bound 0.05, upper
bound 1.0 and initial upper value 0.5 (stack arguments of the call at
`0x14007f8c7`). A channel becomes initialised at the first call with
`t > 310`; before that the value passes through and the bounds are reset
(upper 0.5, lower 1000000). Afterwards, over the `n` windowed samples:
`d_i = |q - q_i|` updates the lower bound (to `max(d, 0.05)` when smaller) and
the upper bound (to `min(d, 1.0)` when larger),
`s = 2 (clamp(max(g, 0.1) max d_i) - low) / (high - low) + 0.2`, and the result
is the exponentially weighted mean with weights
`exp(j * 30 / (1000 / (310 / n)) * s)`, `j = -(n-1)..0`. The smoothed
quality does not reach the pose fit.

`0x14007f920` uses the same filter on two scalar outputs (`obj[1]+0x50/+0x54`,
derived from records 3.5 and 3.6); it is not reconstructed.

## Implementation

`src/tobii_linux/landmark_smoothing.py`: `LandmarkFilter` (`reset`,
`begin_frame`, `smooth_coordinates`, `smooth_scalar`), `polynomial_blend`,
`index_prediction_row`, `time_residual`, `smooth_records`, `flip_y`,
`coordinate_gain`, `table_entries` and the tables. `FaceTracker` in
`face_tracking.py` now follows `0x14004a530`/`0x14004a2c0`: core records with
the four aliases, Y up, smoothing with the epoch and state logic, copy and
flip, then `TemporalPoseFitter` at frame size. `process(frame, timestamp_ms)`
takes the frame time; without it the monotonic host clock is used.

## Oracle (emulation)

`tools/landmark_smoothing_oracle.py` builds the original filter, record set
and table in persistent emulator memory and calls `0x14007f5a0` per frame
next to `smooth_records`. The OpenCV matrix code needs a TLS block for its
thread-safe static (`gs:0x58`); allocator and free are stubbed as elsewhere.

| Case | Frames | Covers |
| --- | --- | --- |
| flip-y | 1 | `0x14007ac80` on 61 records, two absent |
| tracking | 30 | 13-entry table, 61 present records, jittered 30-31 ms steps |
| reset-gap-absent | 30 | ears entry with factor -1 (skipped), absent records shifting channels, `0x14004bed0` mid-sequence, 400 ms gap |
| coordinates-degenerate | 14 | `0x14004c480` directly: constant, ramp, alternating and quadratic channels, zero and negative gains |

The flip and all smoothed qualities are byte-exact, including the
initialisation phase. Coordinates agree within 2.4e-7 (at most 5 float32
ulps; 72-84% of values byte-exact): the original pseudo-inverse and solve use
OpenCV's float32 SVD, the native code float64. Report:
`evidence/private/landmark-smoothing-oracle.json/.log`.

```sh
cd tools
PYTHONPATH=.:../.local/python:../.local/inference:../src python3 landmark_smoothing_oracle.py
```

## Live replay

`tools/replay_frames.py` passes the recorded frame times. On the 150 recorded
Linux frames (all valid before and after):

| Variant | Position (mm) | Rotation (deg) | RMS frame step |
| --- | --- | --- | --- |
| Before (unsmoothed fit) | (-42.8, -9.8, 690.8) | (11.15, 1.07, 4.43) | 0.215 mm, 0.090 deg |
| Smoothed, as `0x14004a530` | (-42.8, -9.7, 690.8) | (11.18, 1.07, 4.42) | 0.178 mm, 0.054 deg |
| Windows before / after | (-27.9, -8.3, 694.5) / (-13.2, -10.0, 680.2) | (8.3, -0.2, 1.8) / (6.6, -9.7, 1.6) | |

Smoothing lowers frame-to-frame jitter (17% in position, 40% in rotation) but
leaves the means unchanged (live). The 15 mm / 3 degree difference turned out
to be head movement between recordings; a still session agrees with Windows
within 2 mm and 0.4 degrees ([face tracking](face-tracking.md#fitting-coordinates-live-comparison)).

## Open questions

- Unit of the frame time in the service: `_ftime` milliseconds unless the
  caller supplies `+0x1c0`; the supplied value and its unit are not traced.
- `recovery_timeout` (800) reaching object `+0x64` is inferred from the
  tracker copy at `+0x18`; the store into the object was not found.
- `0x14007f920` and the scalar outputs `+0x50/+0x54` are not reconstructed.
- Bit parity of the coordinate filter would need OpenCV's float32 Jacobi SVD.
