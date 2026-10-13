# Head-pose geometry stage

This covers the step between the face adapter's head pose (camera axes) and
the One Euro filter stage in the head-pose producer: the geometry stage at
producer `+0x10` and the inputs the producer hands it. Addresses refer to
runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`.

Evidence types used below:

- **static**: focused Ghidra `-noanalysis` decompilation
  (`evidence/private/b-*-focused.c`) and raw disassembly;
- **emulated**: the original functions run under Unicorn by
  `tools/pose_geometry_oracle.py`
  (report `evidence/private/pose-geometry-oracle.json`).

Nothing here has been compared against live tracker output yet.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_geometry_oracle.py
```

## Producer inputs

`0x140293fe0` (producer slot 0) reads two members of the shared settings
object at producer `+0x38`:

| Settings field | Content | Written by |
| --- | --- | --- |
| `+0x50..+0x8c` | Camera extrinsic 4x4 | Camera model constructor `0x1402b00f0` |
| `+0xe8` flag, `+0xf0` object | Display area (optional) | `0x140296cf0`, called only by the display-area setter `0x140293880` |

If `+0xe8` is clear, `0x140293fe0` returns without producing a head pose
(static). Head pose therefore needs a configured display area.

It builds the display matrix with `0x140294230(out, settings + 0xf0)`, gathers
image size, binning and offsets from the frame into a separate block, and
calls `0x140293ca0(producer, out, frame, sizes)` with two stack arguments:
`param_5` = copy of the settings `+0x50` matrix, `param_6` = the display
matrix. `0x140293ca0` passes them on unchanged as the geometry stage's
`param_4` (camera) and `param_5` (display). The size block only reaches face
adapter slot 4 (`0x1402913e0`, 2D landmark mapping), never the geometry stage.

### Settings object (camera model)

The pipeline constructor `0x14028e2f0` allocates a 0x170-byte shared block
(control vtable `0x14046eba0`) and constructs the object with
`0x140296cb0(object, device)`, which calls `0x1402b00f0(object, *(device + 0x10))`
and clears `+0xe8`. The resulting class has vtable `0x14046e908`; the producer
receives it as its `+0x38`/`+0x40` shared pointer (static).

`*(device + 0x10)` is the first per-camera record (0x140 bytes) of the
device configuration that `0x14028a8a0` builds in global `0x141187058`;
`0x140289910` fills it from the algorithm's camera hardware info, moving the
fields from `+0x30` onwards down by 8 bytes. Combined with the field table of
`0x1402871e0` (see [head-pose output](head-pose-output.md#camera-hardware-info)),
the camera model uses (static; the per-device configuration layout is inferred
from matching offsets in `0x140286410`, `0x14028a8a0` and `0x14028a0e0`):

| Camera model input | Camera record | Hardware info | Device record (double) |
| --- | --- | --- | --- |
| Translation x, y, z | `+0x3c..+0x44` | `+0x44..+0x4c` | `+0x10`, `+0x18`, `+0x20` |
| Angles a, b, c | `+0x48..+0x50` | `+0x50..+0x58` | `+0x28`, `+0x30`, `+0x38` |
| Image size | `+0x10`, `+0x12` (u16) | same | `+0xa0`, `+0xa4` |
| Pixel pitch | `+0x14`, `+0x18` | times 1e6 | `+0x90`, `+0x98` |
| Focal length | `+0x1c` | same | `+0x40` |
| Principal offset | `+0x20`, `+0x24` | same | `+0x70`, `+0x78` |

`0x1402b00f0` (emulated for the extrinsic part) stores:

- `+0x08`: intrinsics K, column-major, `fx = focal / pitch_x`,
  `fy = focal / pitch_y`, `cx = (width - 1) / 2 + offset_x / pitch_x`,
  `cy = (height - 1) / 2 + offset_y / pitch_y` (emulated with synthetic
  values). If the record's byte `+1` is set, `0x1402b0310` takes K directly
  from record `+0x28..+0x38` instead; the extrinsic is built the same way.
- `+0x2c`: K inverse (`0x1402b0450`).
- `+0x50`: the extrinsic `[[R, t], [0, 1]]` (`0x1402afe50`) with
  `R = Rx(a) Ry(b) Rz(c + pi)` from `0x1402b2100` and `t` the translation.
  Read as a row-major array, rows 0..2 hold `R` transposed and row 3 holds
  `t`, so it maps camera points to tracker points as `[p, 1] @ C`.
- `+0x90`: the extrinsic's inverse (`0x1402b0660`); `+0xd0`: `t` again;
  `+0xdc`/`+0xe0`: an optional float from the camera record `+0x134/+0x138`.

The "no hardware info" defaults in `0x140286410` copy the three angles into
hardware info `+0x58, +0x54, +0x50`, i.e. in reverse order (static).

### Display object (settings `+0xf0`)

The setter `0x140293880` (slot `+8` of vtable `0x14046ecc0`) takes
`{float *points, int64 count}` and accepts it only when `count == 1`,
`|P1 - P0|^2 >= 1`, `|P2 - P0|^2 >= 1`, `-1 < (P1 - P0).(P2 - P0) < 1`
(millimetres squared, so the edges must be orthogonal),
`P2.x > P0.x`, `P2.x > P1.x`, `P1.y < P0.y` and `P1.y < P2.y`. Those
inequalities fix the corner order as **top-left, bottom-left, top-right**
(static; emulated: every test display, including the albedo one, is
accepted in that order and rejected in Stream Engine's top-left, top-right,
bottom-left order).

`0x1402af800` builds a 0x70-byte object, vtable `0x14046ec88` (base
`0x14046ec50`, constructor `0x1402af610`):

| Offset | Content |
| --- | --- |
| `+0x08`, `+0x14`, `+0x20` | Top-left, bottom-left, top-right |
| `+0x2c` | The tenth input float, copied and not used here |
| `+0x30` | Unit normal `(TR - TL) x (TL - BL)`, towards the viewer |
| `+0x40`, `+0x4c` | Top-left and the normal again (`0x1402b25e0`) |
| `+0x58` | 3x2 pseudo-inverse of the two edges (`0x1402afc30`), used by slots `+8`/`+0x18` |

Vtable `0x14046ec88`: `+0x00` destructor `0x140292ea0`, `+0x08`
`0x1402afa80`, `+0x10` `0x1402afaa0` (ray/plane intersection via
`0x1402b2800`), `+0x18` `0x1402af9e0`, `+0x20` `0x1402afa00`, **`+0x28`
centre `0x1402afad0`**: `((TL - BL) * 0.5 + BL) + (TR - TL) * 0.5`, and
`+0x30` `0x1402afb90`: `(|TR - TL| * s0, |BL - TL| * s1)`.

## Display matrix `0x140294230`

With `right = normalize(TR - TL)`, `up = normalize(TL - BL)`, `n` the stored
normal and `c` the centre from slot `+0x28` (emulated, byte-exact):

```
D = [[right.x, up.x, n.x, 0],
     [right.y, up.y, n.y, 0],
     [right.z, up.z, n.z, 0],
     [-right.c, -up.c, -n.c, 1]]
```

so `[p, 1] @ D = ((p - c).right, (p - c).up, (p - c).n)`: tracker
millimetres to display-centred axes (x right, y up, z towards the viewer).
Each dot product is summed as `(y + z) + x`; lengths are normalized only when
the squared length is positive.

## Stage configuration

`0x14028fc30` (provider slot `+0x10`, constant) gives a 3x3 `G3 = 0x1402b10a0(pi)`
and a 4x4 `G4` with `0x1402b11a0(pi)` in the upper left and zero translation;
`0x1402945f0` copies them to stage `+0x08` and `+0x2c` (emulated, byte-exact).
In the row layout used by the stage, `G3 ~ diag(1, -1, -1)` and
`G4 ~ diag(-1, -1, 1, 1)`, with float32 `sin(pi) = -8.742278e-8` off the
diagonal.

## Stage process `0x140294670`

Inputs: `param_3` = `(a, b, c, order 3)` from `0x1402b0d30` on the adapter's
cached matrix, followed at `+0x10` by the adapter's position in millimetres;
`param_4` = camera extrinsic `C`; `param_5` = display matrix `D`.

- Position `0x140294820(stage, out, position, C, D)`:
  `[p, 1] @ (G4 @ (C @ D))`, divided by w. Matrix entries are summed as
  `((0 + 1) + 2) + 3`; the final row as `w + ((y + x) + z)`.
- Rotation `0x140294ba0(stage, out, angles, C3, D3)` with `C3`, `D3` the
  upper-left 3x3 blocks: `F = E(a, b, c) @ G3 @ C3 @ D3 @ Ry(pi)`, computed
  right to left, where `E` is the order-3 matrix of `0x1402b1c00` and
  `Ry(pi) = 0x1402b1120(pi)`. `0x1402b1020` converts `F` back and the stage
  returns `(-a', b', -c', 0)`.
- Output: rotation in floats 0..3, position in floats 4..6, which the filter
  stage then reads.

In conventional column-vector terms, writing `D'`, `C'` for the transposes
(display axes as rows, camera rotation `Rx(a) Ry(b) Rz(c + pi)`) and `H = E^T`
for the head rotation, the position is `D' * C' * Rz(pi) * p` and the
orientation is `Ry(pi) * D3' * C3' * Rx(pi) * H`. With an upright display and
zero camera angles (where `C3' = Rz(pi)`) the constants cancel and only the
first and third angles change sign (emulated case `upright`/`zero`, and the
unit test `test_aligned_inputs_only_negate_first_and_third_angles`).

## Implementation and verification

`src/tobii_linux/pose_geometry.py`:

| Function | Original |
| --- | --- |
| `display_normal`, `display_center`, `display_matrix` | `0x1402af610`, `0x1402afad0`, `0x140294230` |
| `camera_rotation`, `camera_extrinsic` | `0x1402b2100`, extrinsic part of `0x1402b00f0` |
| `geometry_config`, `x_rotation`, `y_rotation`, `z_rotation` | `0x14028fc30`, `0x1402b10a0`, `0x1402b1120`, `0x1402b11a0` |
| `transform_position`, `transform_rotation`, `geometry_stage` | `0x140294820`, `0x140294ba0`, `0x140294670` |

All arithmetic is float32 in the original grouping. Corners are passed as
top-left, bottom-left, top-right.

The oracle constructs every input natively (camera model from a synthetic
camera record, display object through the real setter, stage from the real
configuration) and runs `0x140294670` on four poses for each of four displays
(upright, tilted and offset, yawed and rolled, and the albedo display area
from `windows-geometry-probe.jsonl`) and three camera extrinsics (zero and two
synthetic): 48 stage results.

| Check | Result |
| --- | --- |
| Display matrix (12 cases) | byte-exact |
| Camera extrinsic (12 cases) | byte-exact |
| Stage configuration | byte-exact |
| Positions (48) | byte-exact |
| Rotations (48) | 46 byte-exact, 2 off by 1 ulp (5.96e-8 rad) |
| Reordered corners (TL, TR, BL) | rejected for all four displays |

The 1-ulp rotation differences match the known `cos`/`sin`/`atan2`/`asin`
differences seen in `pose-euler-oracle.json`.

## Calibration dependence

| Input | Source | Device-specific |
| --- | --- | --- |
| Camera extrinsic `C` (translation and angles) | Camera hardware info from the tracker, device record doubles `+0x10..+0x38` | **Yes, per-device calibration; must be read from the tracker** |
| Intrinsics at settings `+0x08` | Same hardware info (pitch, focal length, principal offset, size) | Yes, but not used by this stage |
| Display corners | Display area configured by the user (Windows: `tobii_get_display_area`) | Per installation, not per device |
| `G3`, `G4`, `Ry(pi)` | Constants | No |

Without the tracker's extrinsic the stage cannot reproduce Windows output: the
camera-to-tracker rotation and offset enter both position and rotation.

## Crop and resize compensation

The tracker fits the face model on a cropped and resized working image. In
the output chain examined here nothing compensates for that crop (static):

- the geometry stage's only inputs are the adapter angles and position, `C`,
  `D` and the constants (confirmed by emulation: outputs are a pure function
  of those inputs);
- `0x140294230` reads only the display object;
- settings `+0x50` is built once from hardware info and never refers to a
  region of interest;
- the frame size, binning and offset block built in `0x140293fe0` goes only
  to adapter slot 4 (`0x1402913e0`), which maps 2D landmarks;
- adapter slot 5 (`0x140291830`) scales the tracker result `+0x10`
  translation by 1000 and builds the matrix from result `+0x28`, with no ROI
  term.

So any correction for crop offset and scale has to happen inside the
VisionSDK tracker before its result is copied into the adapter. That has not
been established. One lead (static, unverified): the pipeline constructor
passes provider slot `+0x48` (`0x14028fe30`), which returns the camera model's
optional float at `+0xe0` or the constant 1.56 at `0x14046ebe4`, into the
tracker configuration object (`0x140297bd0`).

## Open questions

- Where Stream Engine's display area (top-left, top-right, bottom-left) is
  reordered before reaching `0x140293880`; the setter's caller is not traced.
- The units of the hardware-info translation (millimetres is assumed from the
  display corners and the position path) and whether real devices use the
  record's byte `+1` intrinsics branch.
- Whether the VisionSDK tracker result is already expressed in the full
  camera frame (crop compensation), and what the 1.56 tracker setting is.
- The validation object at producer `+0x20` and any later step that sets the
  internal Euler order field `+0x318` (the stage itself writes 0 there).
- End-to-end comparison with live Windows samples, which needs this device's
  camera extrinsic.
