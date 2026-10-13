# Temporal pose fitting and eye refinement

The live tracker fits the face model with the caller `0x14007e440`, not with
`0x14007e840` (the staged caller reconstructed in [pose fitting](pose-fitting.md)).
This page covers that temporal caller, the reprojection error that can reject a
frame, and the LBF eye refinement `0x14007d270` that follows a successful fit.

Addresses refer to runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`. Evidence
types are marked: static (raw instructions or decompiler output), emulation
(original code run under Unicorn), file parse. Nothing here is a live
observation. No other ET5 project or web source was consulted.

## Call site (static)

`0x140049450` picks the caller by the type int at caller `+0`; the constructor
`0x14007d070` stores 0, so it calls `0x14007e440(caller, records, w, h, out, gate)`
where `w`/`h` are the working image dimensions and `gate` is the D1 quality gate
result. The gate is always 1 here because a failing gate branches past the fit.
With `gate == 0` the caller would skip fitting and take its failure path.

## Caller `0x14007e440` (static, emulation)

State: `+0x59` initialised, `+0x39` frontal shape refit done, `+0x38` rebuild
fitting objects, plus the parent model pose (`+0x58..+0x6c`) and physical
coefficients (`+0x90` shape, `+0x98` animation).

1. If `+0x38`: `0x14007e330` builds the fitting objects from this frame's
   records (fails the call when no parent), then clears `+0x38`.
2. Copy the records to `+0x60` and flip Y there (`0x14007ac80`: `y = 1 - y`).
3. If `+0x59` is clear: clear `+0x39`, run the initial fit, store its result
   in `+0x59`, set `+0x39` when the output is near frontal. Zero model
   `+0x160/+0x164` (eye offset angles used by mesh finalization).
4. Run the temporal fit (returns false at once when `+0x59` is clear).
5. Failure: zero model `+0xe0/+0xe4` and out `+0x34`, set the model pose to
   `(0, float32(pi), 0, 0, 0, 0)`, output it, out `+0` = 0, return false.
   `+0x59` and the coefficients are left as the fits left them.
6. Success: output; if near frontal and `+0x39` clear, rerun the initial fit
   (into `+0x59`) and the temporal fit, output again, set `+0x39`. The refit
   results are ignored: the call still returns true. Then the output
   postprocessing shared with `0x14007e840` (`0x140054f70`, `0x14007f0f0`,
   `0x14007f190`, `0x14007f310`); none of it writes the model pose.

Output `0x140055470`: `(rx, ry - float32(pi), rz)` and `(tx, ty, tz)`.
Near frontal: `|rx| < 0.15707963705062866` and `|ry - pi| <` the same value.
The test is `COMISS` + `JAE`, so a NaN angle counts as near frontal.

## Initial fit `0x14007ec70` (static, emulation)

Zero both coefficient blocks, reset the pose with `0x140052d70` using yaw
`float(int caller+0x3c)` (3 after `0x14007e270`), copy an external pose from
caller `+0x40..+0x54` when `+0x58` is set (not supported natively; no writer
was traced), then `0x1400dbba0(fitter, 3)` and `0x1400dc3b0(fitter, records, w,
h, hint=NULL, iterations=2)`. Extra parent models are refitted only on success.

## Temporal fit `0x14007ee60` (static, emulation)

At most two attempts while `+0x59` stays set:

1. `facing = ((ry - atan2f(tx, tz)) - 3.141590118408203) * 57.2957763671875`,
   from the parent pose before this attempt's fit.
2. `0x1400dbba0(fitter, 5)`, `0x1400dc3b0(..., hint=NULL, iterations=1)`.
3. `0x14007a090(w, h, records, rect)` on the original (unflipped) records, then
   `error = 0x1400551c0(model, caller+0x60, w, h, float(rect.width))`.
4. Reject when the fit failed, or when `error > 0.10000000149011612` on the
   first attempt (`COMISS` + `JBE`: a NaN error is accepted). On rejection:
   if `-30 < facing < 30`, rerun the initial fit and store its result in
   `+0x59` (clearing it ends the loop); otherwise reset the pose with
   `0x140052d70` and yaw `float32(pi)`. A NaN facing takes the reset branch.
   The recovery also runs after a failed second attempt.

### Fitting object persistence (static, emulation)

`0x1400dbba0(fitter, flag)` rebuilds the fitting object only when none exists
or `flag` differs from the last one. A rebuild re-encodes pose and
coefficients from the parent and copies both physical blocks into the object;
otherwise the object keeps its own latent state from the previous
`0x1400dc3b0`. Decoding (`0x1400dff70`) writes only the active block back to
the parent, and the inactive block used by the update is the copy taken at
build time. Consequences, all reproduced natively and in the oracle:

- Consecutive temporal fits continue from the previous frame's latent state
  (including clamped latents), not from a re-encoding of physical coefficients.
- The facing-branch pose reset in `0x14007ee60` changes the parent pose but not
  the start of the next flag-5 attempt; it only feeds the next facing value.
- After a failed frame the next frame's initial fit, if the object still has
  flag 3, starts from the failed object state instead of the fresh reset.
- The failure-path pose `(0, pi, 0, 0, 0, 0)` affects the output and the next
  facing value (`atan2(0, 0) = 0`), not the next fit start.
- `0x14007e270` sets `+0x38`, so a tracker reset recreates the object.

Inside `0x1400dc3b0` the retry still resets the parent pose and re-encodes it
before three more updates. The function returns true only when the bounds
check passes; mesh finalization (`0x140052710`, which fills the projected
points at model `+0xb8`) runs only then. Fewer than four points returns false.

## Reprojection error `0x1400551c0` (static, emulation)

1. `0x140054dd0` copies the model FDP mapping into a temporary record set;
   `0x140054f70` fills it with the model's finalized 2D points converted to
   normalized image coordinates:
   `x = 0.5 - (px * 0.5) / ax`, `y = (py * 0.5) / ay + 0.5`, with
   `ax = w/h` when `w > h` else 1, `ay = h/w` when `h > w` else 1.
2. Over FDP groups 2..15 except 13 and 15, for every record present in both the
   Y-flipped caller copy and the model set:
   `sum += sqrt((w*rx - w*mx)^2 + (h*ry - h*my)^2) * (1 / rect_width)`.
3. Return `sum / count` (float32). With no shared record this is NaN, which
   the caller accepts.

Model points come from finalization: deformed mesh, then `X * (f / Z)`, which
rounds differently from the fitting prediction `(1/Z) * f`.

## Rectangle `0x14007a090` (static, emulation)

Pixel coordinates are `round(w * x)`, `round(y * h)` with round-half-even
(`cvtps2pd` + `cvtsd2si`). Required records: 3.7, 3.8, 3.11, 3.12, 9.4, 9.5,
8.1; if any is absent the function returns false and the caller uses an
uninitialised width (the native code raises instead).

- `L = round((P3.11 + P3.7) / 2)`, `R = round((P3.12 + P3.8) / 2)`,
  `N = round((P9.5 + P9.4) / 2)`, `E = round((R + L) / 2)`, `M = P8.1`.
- `size = round(sqrt(max(|L-M|^2, |R-M|^2, |L-R|^2)) * 1.7999999523162842)`.
- `x = round(0.2 E.x + 0.8 N.x - size/2)`, likewise y; width = height = size.
- `E.y >= N.y` only triggers a log call.

## Native implementation

`src/tobii_linux/pose_temporal.py`: `TemporalPoseFitter(model).process(points,
w, h)` for complete D1 landmarks in `model.landmark_ids` order. It reuses
`select_pose_landmarks`, `prepare_observations`, `initial_rigid_state`,
`mode_step` and `check_pose_bounds`, and returns validity, the
`0x140055470` output, the parent pose and coefficients, both flags and a
per-fit trace (flag, validity, attempts, facing, error, rectangle).
`reset()` mirrors `0x14007e270` plus the object rebuild. Helpers:
`eye_face_rectangle`, `reprojection_error`, `finalized_projection`,
`facing_degrees`, `pose_output`, `is_near_frontal`.

## Oracle (emulation)

`tools/pose_temporal_oracle.py` builds the original caller and parent model
(as in `pose_caller_oracle.py`), calls `0x14007e270`, then runs original
`0x14007e440` repeatedly with persistent emulator memory. Only the allocator
and free are stubbed; the heap is enlarged for persistent frames. Hooks read
each `0x1400dc3b0` result and update flags, the facing value, the rectangle and
the error. Synthetic landmarks are projections of the installed model at 640x480,
with Gaussian noise (sigma 0.04) on the rejection frames.

Three sequences, 18 frames, 30 original fits:

| Sequence | Frames covered |
| --- | --- |
| tracking | first frame, steady, frontal shape refit, error rejection with reinitialisation, wide yaw (facing > 30) error rejection with pose reset, recovery |
| failure-recovery | tight z limits [0.3, 1]: total failure (pose reset, `+0x59` cleared), reinitialisation that continues from the failed flag-3 object, steady |
| lost-pose-injected | `+0x59` set with the failure-path pose injected, then a fit, an error rejection whose initial refit fails, and recovery |

Every frame matches exactly in return value, `+0x59`, `+0x39`, the sequence of
fits and their results, and the rectangles. Facing differs by at most
`3.8e-5` degrees and the error measure by at most `3.4e-7`. In 17 frames the
state agrees within the fixed tolerance: pose `3.8e-6`, shape `4.5e-5`,
animation `4.2e-5` (coefficient tolerance `1e-4`, pose `1e-5`). In the last
`lost-pose-injected` frame (a flag-3 fit continuing from a failed object state,
then a retry) the differences are pose `2.0e-5`, shape `4.9e-4`, animation
`8.0e-5`. To show these are arithmetic rather than logic differences, the
oracle reruns the native fitter with float32 instead of float64 accumulation;
that alone changes the same frame by pose `2.1e-5`, shape `5.9e-4`, animation
`7.5e-5`. The report passes a frame when each difference is within the fixed
tolerance or within twice that sensitivity, and records the stricter result
separately (`within_fixed_tolerance`). Report:
`evidence/private/pose-temporal-oracle.json/.log`.

The first oracle run exposed the persistence rule: with re-encoding on every
fit, the facing-reset frame diverged by 0.83 rad. A hook on `0x1400df6b0`
showed the original second attempt starting from the post-attempt-0 state, and
`0x1400dbba0` confirmed the rebuild condition.

## Eye refinement `0x14007d270` (static, emulation, file parse)

Called after a successful fit as `0x14007d270(obj, working_image, records,
scale, yaw)` where `obj` is the 0x28-byte object from `0x14007d070`
(`+4` LBF point count, `+8` fa model, `+0x10` fc model), `scale` is
`float(rect.width)` of `0x14007a090` on the tracker records and
`yaw = 180 - (ry - atan2f(tx, tz)) * 57.2957763671875`. Before the call the
tracker copies the 24 group-10 records of out `+0x98` (model projection) into
the records with Y flipped back.

1. `mirrored = yaw > 15`; the point table is fa's `(group, index)` lists.
2. For each point read `records[group][mirrored ? mirror(group, index) :
   index]` (no presence check) and form `x = w * rx`, `y = h * ry` using the
   full image size.
3. `|yaw| >= 15` selects fc with the mirror flag, otherwise fa unmirrored
   (NaN selects fa).
4. `0x1400ac9c0` returns false, without refining, when the IplImage has an ROI
   (`+0x30`) with `x < 0`, `x + w > width`, `y < 0` or `y + h > height`.
   Otherwise it runs the four LBF stages `0x1400acb80` on the ROI pixels
   (axis 0 = y, axis 1 = x) and returns true.
5. On success, only points of group 10 are written: `x/w`, `y/h`, z = 1000,
   present = 1, flag = 1, quality = -1, at the mirrored index when mirrored.

So `0x14007d270` returns false exactly when the image ROI does not fit. In
`0x140079ce0` the ROI is set and reset only on the source frame; the working
image's size fields are written directly (static), so no failure source was
found on the observed path. The installed `fa.lbf` and `fc.lbf` point tables
contain no group-10 point (file parse), so with the shipped models the
refinement writes nothing and its result is discarded.

`mirror(group, index)` is `0x140011900`, which uses the side classifier
`0x140011520` (jump tables over `100 * group + index`). Its values for all
205 FDP ids were tabulated by executing the original function and stored in
`MIRRORED_INDEX`.

`src/tobii_linux/eye_refine.py`: `refine_eye_landmarks(point_ids, records, w,
h, scale, yaw, refine, roi)`, `select_lbf_model`, `roi_inside`,
`mirrored_index`, `tracker_yaw_degrees`, and `lbf_refiner(models, pixels)`
which runs `lbf_predictor.refine_landmarks` on the ROI pixels.

`tools/eye_refine_oracle.py` checks the 205 mirror entries and runs original
`0x14007d270` and `0x1400ac9c0` in 16 cases: two point tables (installed fa,
and a synthetic table with group-10 points), yaw 0, 20, -20, exactly 15 and
NaN, an ROI inside, and two ROIs outside. Model choice, mirror flag, scale,
initial coordinates, pixel offset/size, return value and written records all
match. `0x1400acb80` is replaced by a deterministic coordinate shift on both
sides; its numerics are covered by `lbf_predictor_oracle.py` and
`lbf_gather_oracle.py`. Report: `evidence/private/eye-refine-oracle.json/.log`.

## Evidence files

Raw instruction excerpts: `evidence/private/e-caller-disassembly.txt`,
`e-initial-fit-disassembly.txt`, `e-temporal-fit-disassembly.txt`,
`e-reprojection-error-disassembly.txt`, `e-rectangle-disassembly.txt`,
`e-fit-outer-tail-disassembly.txt`, `e-eye-refine-disassembly.txt`,
`e-lbf-guard-disassembly.txt`, `e-mirror-map-disassembly.txt`,
`e-tracker-fit-call-disassembly.txt`. Focused decompilation:
`e-eye-refine-focused.c`. Earlier focused output used here:
`fitcaller-focused.c`, `fittemporal-focused.c`, `pose-output-focused.c`,
`pose-fitting-focused.c` (`0x1400dbba0`), `pose-update-focused.c`,
`pose-state-focused.c`, `pose-finalize-focused.c`, `trackstate-focused.c`.
The decompiler output of `0x1400551c0` is unusable; the raw instructions are
authoritative.

## Limits

- Synthetic landmarks only: no neural inference, real faces or camera frames.
- Single parent model; extra parent models and the `+0x58` pose hint are not
  implemented.
- Complete D1 records are required; missing records (uninitialised rectangle,
  stale LBF inputs) are not reproduced.
- Non-finite intermediate states raise in the native update instead of
  propagating as in the original; no oracle case reaches them.
- `FaceTracker` still uses the staged caller; wiring `TemporalPoseFitter` and
  `refine_eye_landmarks` into it is not done here.
- Numerical agreement is not bit parity (see the tolerances above).

## Reproduction

```sh
cd tools
PYTHONPATH=.:../.local/python:../.local/inference:../src OPENBLAS_NUM_THREADS=2 python3 pose_temporal_oracle.py
PYTHONPATH=.:../.local/python:../.local/inference:../src python3 eye_refine_oracle.py
cd ..
PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 -m unittest tests.test_pose_temporal tests.test_eye_refine
```

The temporal oracle takes several minutes (up to 600 million instructions per
frame).
