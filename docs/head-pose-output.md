# Head-pose output path

This traces the fitted face-model pose to the public Stream Engine head-pose
sample. Addresses refer to runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`.
Evidence is focused Ghidra `-noanalysis` decompilation under
`evidence/private/pose-*-focused.c` plus raw disassembly; nothing here has been
emulated or checked against live samples yet.

Reproduce any listing with `tools/ghidra/focused.sh <name> <address>...`.

## Public sample

The Windows probe (`windows-stream-probe.jsonl`) shows public units: position
in millimetres (z about 529) and rotation in radians.

| Step | Address | Observation |
| --- | --- | --- |
| Device result callback | `0x140265420` | Registered per frame by `0x140266430` (`lea r9` at `0x1402665f5`) with the device object as context |
| Head-pose record | `0x140287120` | 48 bytes: timestamp from algo output `+0x11b8`; if `+0x1378` is set, all validity flags = 1, position = `+0x1388/+0x138c/+0x1390`, rotation x = `+0x1380`, y = `+0x137c`, z = `+0x1384` |
| Subscriber | device `+0x11430` | Same callback pointer whose presence enables the algorithm in `0x140266430` |
| Output assembly | `0x14028b310` -> `0x14028b4a0` | Output `+0x11b0` block. Head pose: valid = internal `+0x308`; `+0x137c` = internal `+0x310`, `+0x1380` = `+0x30c`, `+0x1384` = `+0x314`; position = internal `+0x31c/+0x320/+0x324` |

The two reorderings cancel: public rotation x, y, z are the internal values at
`+0x30c`, `+0x310`, `+0x314` in that order. Internal `+0x318` holds the Euler
order code (3). The 0x1c98-byte output is assembled only by copies, so direct
field-store searches do not find the producer.

## Thread and frame entry

- `platmod_algo::start` `0x140286990`; populate hardware info `0x140286410`
  (per-camera records of 0xba ints, copied by `0x1402871e0`; a fallback logs
  "No hardware info available; using the defaults.").
- Thread body `0x140285550` copies the queued frame, calls
  `0x1400e7040(frame, output[0x1c98], settings)` and then the registered
  callback. Display plane setup is `0x140286750` -> `0x1400e7420`.
- `0x1400e7040` dispatches on frame type (0-5) to wrappers such as
  `0x1400e67c0`, which call the pipeline's virtual process method and then
  `0x14028b310`.

## Pipeline objects

The pipeline constructor `0x14028e2f0` wires:

- Face adapter, 0x2158 bytes, constructed by `0x1402901b0`, vtable
  `0x14046ebf0`. It embeds a copy of the VisionSDK tracker result at `+0x20`
  (so result `+0x10` translation is adapter `+0x30`, result `+0x28` rotation is
  adapter `+0x48`) and holds `VisionSDK::Tracker` at `+0x10`.
  - Slot 5, `0x140291830`: head pose. Position = adapter `+0x30/+0x34/+0x38`
    times float32 1000 (no axis change). Rotation matrix = `0x1402b0c90` with
    angles `(+0x48, +0x4c, +0x50)` and order 3, i.e. (rx, ry - pi, rz) from
    `0x140055470`.
  - Slot 8, `0x140290dc0`: eyeball centres FDP 3.5 and 3.6, x1000.
  - Slot 9, `0x140290380`: eye-corner midpoints (3.11/3.7 and 3.8/3.12), x1000.
- Head-pose producer, 0x80 bytes, constructed by `0x140293a20`, vtable
  `0x14046edf8`. Members: `+8` adapter, `+0x10` geometry stage, `+0x18` filter
  stage, `+0x20` validation, `+0x38` settings.

`0x140293fe0` builds a 4x4 matrix with `0x140294230` (adapter head pose plus the
display corners from `0x1400e30d0`), gathers image size/binning and the display
plane block, then calls `0x140293ca0`. That function caches the adapter's
matrix and position (no smoothing in the cache), converts the matrix to
angles with `0x1402b0d30` (order 3 -> `0x1402b1020`), runs the geometry stage
and then the filter stage, and returns valid + 4 rotation floats + 3 position
floats into the internal result `+0x308` block.

### Euler builder

`0x1402b1c00` (order 3) builds three elementary matrices with cos
`0x1403f7240` and sin `0x1403f7740`:

```
A(a) = [[1,0,0],[0,ca,sa],[0,-sa,ca]]
B(b) = [[cb,0,-sb],[0,1,0],[sb,0,cb]]
C(c) = [[cc,sc,0],[-sc,cc,0],[0,0,1]]
```

The stored matrix is `(Ry(b) @ Rx(a) @ Rz(c)).T` in the conventional
column-vector form: the same Y, X, Z composition as the fitter's projection,
laid out for row vectors. Orders 1 and 2 are `0x1402b1210` and `0x1402b1700`.

The order-3 inverse `0x1402b1020` reads the row-major matrix `m` as
`b = atan2(m[6], m[8])`, `a = asin(-m[7])`, `c = atan2(m[1], m[4])`, stores
order 3 in the fourth float, then `0x1402b0dc0` wraps c, a, b by adding or
subtracting float32 2*pi until each lies in [-pi, pi] (NaN passes through).

`pose_output.euler_matrix`, `matrix_euler` and `wrap_angle` implement these.
`tools/pose_euler_oracle.py` runs both original functions on nine angle sets
(single axes, the Windows sample's angles, near-pi yaw, steep pitch): all pass,
seven byte-exact, maximum matrix error 2.98e-8 and angle error 1.19e-7.
Report: `evidence/private/pose-euler-oracle.json`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_euler_oracle.py
```

`pose_output.adapter_head_pose` reproduces slot 5's millimetre scaling and
matrix; its emulated comparison is still pending.

### Geometry stage (`+0x10`, vtable `0x14046ee08`, process `0x140294670`)

Configured from a 0x60-byte settings block (pipeline settings getter `+0x10`)
holding a 3x3 at `+8..+0x28` and a 4x4 at `+0x2c..+0x68`.

- Position `0x140294820`: homogeneous `p' = [p,1] * (Cfg4x4 * B * C)`,
  divided by w. `B` is the `0x140294230` matrix, `C` the display-plane block.
- Rotation `0x140294ba0`: rebuilds the matrix with `0x1402b1c00`, applies
  `0x1402b1120(pi)` (a 180 degree Y rotation, diag(-1,1,-1)) and the 3x3
  configuration and the same matrices, then converts back.

### Filter stage (`+0x18`, vtable `0x14046ee18`, process `0x140295790`)

One Euro filters keyed on microsecond timestamps (rate = 1e6 / dt):

- Three scalar filters (`0x140295460`), one per angle. Note the routing:
  filter 0 takes angle 1, filter 1 takes angle 0, filter 2 takes angle 2.
  `alpha = 2*pi*fc / (2*pi*fc + rate)`, derivative cutoff at object `+0xc`,
  `fc = min_cutoff(+4) + beta(+8) * |dx|`.
- Position filter (`0x1402955d0`, 0x50-byte object): same form using the
  Euclidean norm of the 3D derivative.

Filter parameters come from pipeline settings getters `+0x20`/`+0x28` and a
third 0x30-byte object; their values are not yet recovered.

## Configured geometry on the Windows reference

`tools/windows_geometry_probe.py` calls only getters (`tobii_get_display_area`,
`tobii_get_geometry_mounting`, `tobii_get_track_box`) and then captures head
pose for two seconds in the same session. Output:
`evidence/private/windows-geometry-probe.jsonl` (66 samples, e.g. position
(-54.1, -15.8, 541.5) mm, rotation (0.160, -0.126, 0.012) rad).

| Item | Value |
| --- | --- |
| Display top-left (mm) | (-296.62, 326.00, 111.82) |
| Display top-right (mm) | (300.38, 326.00, 111.82) |
| Display bottom-left (mm) | (-296.62, 10.27, -3.10) |
| Geometry mounting, raw words | int 2, then floats 184.0, 20.0, 0.0, -0.16, 13.85, 0.0, 5.38, 9.86 |
| Track box | `TOBII_ERROR_NOT_SUPPORTED` (3) |

The panel is 597 x 336 mm and tilted 20 degrees, matching the mounting's
second float. The mounting buffer was oversized and only the first nine words
were nonzero; a plausible reading is guides, width_mm, angle_deg, then two
offset vectors, but that field naming is not yet confirmed from code.
These values are user configuration on one machine, not device constants.

## Not yet established

- Contents of the configured 3x3/4x4 (likely camera-to-tracker extrinsics from
  the device hardware info) and the `0x140294230` / display-plane matrices.
- One Euro parameter values and the validation object at producer `+0x20`.
- A native implementation and an emulated comparison of this chain.
