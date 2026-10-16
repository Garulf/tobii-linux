# Face assets and pose reconstruction checkpoint

This is a chronological evidence ledger. Later checkpoints supersede earlier
unresolved items. Current state: LBF refinement and D1 crop/landmark components
run natively; pose, live image acquisition and tracking remain unfinished.

The supporting files now have native readers in
`crates/tobii-models/src/face.rs` (wrapped by `src/tobii_linux/face_assets.py`). These parse observed file layouts and validate
counts, finite coordinates, references and exact binary extents. They do not
implement a pose solver or establish coordinate units.

## Verified asset structure

| File under `models/tobii-et5/support/` | Decoded structure |
| --- | --- |
| `jk_300.wfm` | 357 vertices, 357 texture coordinates, 640 triangles, 23 sparse animation-unit arrays, 40 sparse shape-unit arrays |
| `jk_300_rigid.fdp` | Text landmark ID to mesh/vertex mapping; all 57 neural landmark IDs resolve to mesh 0, valid vertex indices |
| `bdtsdata/NN/vnn_nir/landmarks.bin` | LE uint32 count 57, then 57 pairs of LE uint32 landmark identifiers; exactly 460 bytes |
| `bdtsdata/NN/vnn_nir/init_shape.bin` | Two LE uint32 dimensions (57, 2), followed by 114 LE float32 values; exactly 464 bytes |
| `bdtsdata/NN/is.bin` | Three LE uint32 dimensions (9, 35, 2), followed by 630 LE float32 values; exactly 2532 bytes |

All decoded floats are finite. The mesh's triangles and sparse deformation
indices are within the vertex array. Face indices include zero, and all file
references can be joined as zero-based indices without leaving the mesh.
FDP tokens such as `0.191` are preserved as `(mesh=0, vertex=191)`, not parsed
as a floating-point coordinate.

This establishes usable static 2D/3D asset correspondence. It does not prove
that the default undeformed mesh is sufficient for accurate head pose, or that
its units or orientation match public Stream Engine coordinates.

## Relation to neural outputs

The `d1` XML graph has a 114-value dense output and a 57-value sigmoid output,
concatenated to 171 values. The shape asset has 57 two-coordinate points and
the identifier table has 57 entries. This is consistent with 57 landmark
coordinate pairs plus 57 confidence values, **an inference pending runtime
consumer verification**. Do not yet assign x/y order, crop normalization or
confidence thresholds from counts alone.

## Current reverse-engineering work

Focused Ghidra analysis completed successfully and is retained in
`evidence/private/pipeline-focused.c` and `pipeline-focused.log` for functions
`0x1400abb00`, `0x1400ac7d0`, `0x1400aa2b0`, `0x1400569b0`, `0x140056cb0`
and `0x1400aa4a0`. The helper writes completed functions incrementally. These
are supplemental decompiler results, not REA Evidence IDs.

The D1 constructor at `0x1400abb00` reads `landmarks.bin` and
`init_shape.bin` using plain file reads. It sets the crop width to 96 and calls
`0x1400ac560` on the loaded initial shape before creating the D1 network.
**Raw initial-shape coordinates are not yet the runtime's normalized shape.**
The normalizer's exact reductions, scale and shift still need reconstruction.
The constructor is called at `0x140056f5a` and stored at parent offset `+0x38`.

## LBF files: codebook compression

The `fa.lbf` and `fc.lbf` read paths use ordinary binary reads and codebook
expansion. Native parser `src/tobii_linux/lbf.py` now accounts for every byte in
both files and rejects trailing/truncated data (the readers are now
`crates/tobii-models/src/lbf.rs`). The separate `LBF/lv` layout now has its own `read_lv` reader (see continuation below).

Evidence chain in the hash-bound platform runtime:

- `0x1400ac7d0`: reads version byte, landmark count, stage count and matrices per
  stage; both NN files have `(4, 35, 4, 3)`.
- `0x1400de8d0`: per-landmark ID, presence flag, two float coordinates, and
  optional forest. `0x1400e1f10` reads a submodel count and calls `0x1400e1c50`.
- `0x1400e1c50`: three metadata words including tree count; each tree contains
  a metadata word (optional `0x000b00b5`/`0x000c00b5` marker), a bit-count word,
  and `4 * 2**bits` payload bytes. Tree payload semantics remain unresolved.
- `0x1400dd1b0`: three matrices per stage. Each has two dimensions on disk.
- `0x1400ddbb0`: first matrix uses 16 float32 codebook entries per output,
  followed by packed 4-bit indices, low nibble first. The first decoded value
  is the bias; remaining values are weights. Later matrices contain plain
  float32 biases followed by weights.

The first matrix is stored as per-output codebooks/indices and decoded into
`weights[input, output]`. Its file extent per output is
`64 + ceil((inputs + 1) / 2)` bytes. Later matrices occupy
`4 * outputs * (inputs + 1)` bytes. All decoded floats are finite.

| File | Bytes | Trees | Forest ends at | Stage matrix shapes |
| --- | ---: | ---: | ---: | --- |
| fa.lbf | 1,019,189 | 1,980 | 145,093 | 7920→32→64→70, 7920→40→80→70, 7920→48→96→70, 7920→56→112→70 |
| fc.lbf | 713,181 | 1,260 | 92,525 | 5040→32→64→70, 5040→40→80→70, 5040→48→96→70, 5040→56→112→70 |

Decoded float32 coefficients and per-file SHA-256 manifests are in
`models/tobii-et5/decoded-lbf/{fa,fc}/`. Original plaintext/compressed forest
bytes remain in `support/bdtsdata/NN/{fa,fc}.lbf`; no tree behavior is invented.
The runtime selects an additional int8 quantization branch for the first
matrix. Float exports preserve values before that branch; separate `.i8` weights
and `.f32` scale exports now reproduce the recovered conversion described below.
Full predictor parity remains unverified.

Reproduce static checks and exports with:

```sh
PYTHONPATH=src python3 tools/check_face_assets.py
PYTHONPATH=.local/python:src python3 tools/lbf_coeff_oracle.py
```

The oracle emulates `0x1400ddbb0` with its float-output branch selected and
explicit stubs for allocation, file reads, memset, free and pow. It compares
original first columns from both files and synthetic column/boundary cases.
**The expanded suite passes 25 comparisons byte-for-byte**: 13 LV matrices,
two real fa/fc float columns, two real int8 columns, and eight synthetic
float/int8 packing-boundary cases. Its result is retained in `evidence/private/lbf-coeff-oracle.json`; it covers the bounded int8 cases described below but does not validate a
complete predictor or live pose.
Raw disassembly: `lbf-stage-reader.txt`, `lbf-coeff-reader.txt`,
`lbf-tree-reader.txt`, and `lbf-matrix-reader.txt` under `evidence/private/`.
These are supplemental static/emulation evidence, not REA Evidence IDs.

Next: inspect model-run consumers to establish preprocessing and output
interpretation, then recover the pose solver's camera model, deformation and
coordinate transforms. The asset parser makes those inputs available without
inventing the fitting behavior.

## LV continuation

`LBF/lv` has now been traced to its own loader `0x1400aadc0`, called at
`0x14005735e` from the `/LBF/lv` path use. It is a different layout from
`fa`/`fc`: one version byte (4), one record count (13), then each record has
an integer landmark pair, presence flag, a single forest, and one compressed
matrix. There are no initial XY values, submodel-count word, or stored biases.
All 13 records have 50 trees and a 6400×1 coefficient matrix. The parser reaches
exact EOF at byte 380853. This read path uses plain file reads and the same
codebook expansion function.

Native reader: `read_lv` in `src/tobii_linux/lbf.py`. Expanded matrices and
hash manifests are exported to `models/tobii-et5/decoded-lbf/lv/` by
`tools/check_face_assets.py`. Retained static evidence: `lv-loader.txt` under
`evidence/private/`. Tree semantics and the predictor's role still need tracing.
The earlier statements that the LV layout was unresolved are superseded by
this continuation.

## Int8 coefficient conversion

The first matrix in each `fa`/`fc` stage is quantized by `0x1400ddbb0` when
object offset `+0x20` is nonzero; the LV loader sets that flag to zero.
For each output, the code finds the maximum absolute value across the entire
16-entry float32 codebook, including unused entries. It computes
`scale = trunc(float32(127 / max_abs))`, stores that scale as float32, and writes
`int8(trunc(float32(weight * scale)))`. The bias remains the decoded float32
value. The truncation helper is `0x1403fa1bc`, calling `0x14040c624`; the oracle
executes both original functions, without replacing them with Python math.

`quantize_matrix` implements this finite, nonzero-codebook case. All eight
installed first matrices (one per stage) export successfully, with `.i8` weights and
per-output `.f32` scales recorded in each manifest. Zero-codebook/special-value
behavior is deliberately unsupported. Six int8 oracle cases match the original
code, including real first columns from fa/fc. These checks establish conversion
behavior for those inputs, not complete landmark-prediction equivalence.

Current further trace: supplemental `fitting-focused.c/.log` for
`0x1400ac560` (shape normalization), `0x1400ac9c0` and `0x1400acb80`
(LBF predictor path), and `0x1400ab6a0` (candidate related routine).

## Tree traversal checkpoint

`0x1400e1970` evaluates a forest; its border helper is `0x1400e18a0`.
The four bytes at each tree node are signed pixel offsets `(a0,a1,b0,b1)`.
Centers and scale are truncated from float32 to integers. A sample is addressed
as `(center0 + ((offset0 * scale) >> 8)) * stride + center1 +
((offset1 * scale * axis1_sign) >> 8)`, with axes clamped to image bounds.
The node branches right when pixel A minus pixel B is **strictly greater**
than its signed-byte threshold. The observed loader initializes every threshold
to zero. Traversal starts at node 0, follows `2*node + 1/2` for `depth` levels,
and returns `node - 2**depth + 1`.

Native implementation: `src/tobii_linux/lbf_features.py`, bounded to sensible
image sizes and zero thresholds as loaded. This does not establish whether
some later path mutates thresholds, or map these buffer axes to camera axes.
It always uses the clamped formulation; original code has a faster interior
branch with equivalent outputs in the checked cases.

`PYTHONPATH=.local/python:src python3 tools/lbf_tree_oracle.py` compares selected
installed fa/fc/LV forests against **unmodified original traversal code with no
import stubs**. All 18 cases passed, covering fractional centers/scales, padded
rows, both axis signs, border/outside positions and zero scale. Report:
`evidence/private/lbf-tree-oracle.json`; instructions:
`evidence/private/lbf-tree-evaluator.txt`.

The higher-level LBF loop at `0x1400acb80` calls `0x1400debc0` to gather leaf
features, `0x1400dd410` for the three-matrix predictor, then adds paired outputs
to the current landmarks, multiplying by scale and applying the axis sign to
the second coordinate. Exact feature indexing and matrix activations are being
traced in `predictor-focused.c/.log`; no complete predictor parity is claimed.

`fitting-focused.c/.log` completed successfully. The D1 normalizer subtracts a
per-axis reduction with mode 1 (mean is currently inferred) and scales/shifts
toward half the 96-pixel crop width. Its reduction helper and extent selection
still need confirmation before implementing that transform.

## Complete LBF refinement checkpoint

`src/tobii_linux/lbf_predictor.py` now connects tree features and the three
matrices into all four refinement stages. Callers must supply image geometry,
initial landmark coordinates and face scale; these are not inferred from a
live camera yet.

Feature gathering at `0x1400debc0` selects the stage-specific forest, multiplies
input scale by its stored float factor, and concatenates leaves with a 16-slot
block per depth-4 tree. fa produces 495 indices into 7920 features; fc produces
315 into 5040. All four stage factors are `0.4, 0.26, 0.16, 0.1` (float32).
Missing forests in fc contribute no feature blocks. `lbf_gather_oracle.py`
passes 16 byte-exact gathering comparisons (two models, four stages, both signs).

`0x1400dd410` sums selected int8 coefficient rows into int32 accumulators,
divides by per-output scales, adds biases, then applies tanh before each of
two dense matrices. Dense multiply/add order is preserved in Python without
BLAS reassociation. Original code uses an approximate SSE reciprocal with one
refinement step and its own tanh implementation; Python/NumPy is numerically
close, not bit-identical. All 16 matrix-stage comparisons passed at
`rtol=1e-5, atol=5e-6`; observed maximum absolute difference was
`8.940696716308594e-08`.

`0x1400acb80` adds paired output deltas times the stage scale to the current
coordinates; the second axis also applies the mirror sign. The complete native
four-stage routine was emulated with allocation/free/memset stubs only; tree,
activation, prediction and coordinate-update instructions executed unchanged.
Four comparisons (fa/fc, both signs) passed, with maximum coordinate difference
`1.52587890625e-05` on synthetic images/initial landmarks. Threshold tolerance
was `1e-4`; this is not real-image quality or 1:1 tracking validation.

Reproduction:

```sh
PYTHONPATH=.local/python:.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/lbf_predictor_oracle.py
PYTHONPATH=.local/python:.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/lbf_gather_oracle.py
PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/check_lbf_models.py
```

Retained reports: `evidence/private/lbf-predictor-oracle.json` and
`lbf-gather-oracle.json`. `models/tobii-et5/lbf-validation.json` contains twelve
native synthetic shape/finite-output checks with output hashes. Focused
`predictor-focused.c/.log` completed successfully. One Ghidra decompilation
misidentified the final free calls in `0x1400acb80` as an unreachable return;
raw instructions and successful complete emulation take precedence.

Remaining: recover actual landmark initialization/crop/scale and D1 inference
consumers, determine how LV results are used, reconstruct pose fitting, then
connect real USB images and the Wine-facing service. The refinement path is
usable as a component, not a complete native tracker.

## D1 neural output consumer found

The constructor chain is now explicit: `0x140048fc0` reads the dependency
container's `+0x38` D1 object, creates its inference session at `0x14004911a`,
and stores the wrapper at outer object `+0x28`. Virtual entry `0x140049450`
passes that wrapper through `0x1400557a0` to `0x14007dc50`.

`0x14007dc50` builds a 96×96 crop, invokes the session at `0x14007dfb9`,
copies the first `2*57` floats into a 57×2 matrix, multiplies each by 96 then
0.5, and adds the runtime-normalized initial shape at `0x14007e02f` (`vsAdd`).
Thus the 114 outputs are **XY offsets from the reference shape**, not absolute
normalized landmark locations. An inverse coordinate transform follows at
`0x14007e04d` before writing image landmarks. The remaining 57 outputs start
at result byte offset 456 and are passed to `0x14007d910`.

The writer at `0x14007d910` maps each pair using the loaded group/index tables,
stores `x/image_width`, `y/image_height`, z=-1, two validity flags set to 1,
and copies each trailing output into FDP record offset `+0x14`. These are
per-landmark quality scalars; sigmoid graph outputs and downstream record use
support a confidence interpretation, but thresholds/policy remain unverified.

`src/tobii_linux/d1.py` now decodes this output partition and offset arithmetic.
It intentionally requires the **normalized** reference shape as an argument;
passing the raw shape file is not a verified implementation. Crop resampling,
normalization of image intensities, the inverse transform and the reference
shape normalization are not yet implemented here.

Static evidence: `d1-wrapper-constructor.txt`, `d1-consumer-disassembly.txt`,
`d1-landmark-writer.txt` under `evidence/private/`. Focused decompilation completed successfully in
`d1-consumer-focused.c/.log`. Next helper targets are `0x14007d7b0`
(shape bounds), `0x1400dac40` (shape transform), `0x1400b45b0` (image warp),
`0x1400db010` (inverse point transform), and the session's virtual input handler
for actual intensity conversion.

## OpenVINO byte-input conversion

RTTI identifies the runner vtable at `0x140474518` as
`VNN::OpenVINORunner`. Its `+8` entry `0x1402a6e40` calls `0x1402a5740`,
which invokes exported byte-to-float helper `0x1402aa500` at `0x1402a57c9`.
The helper multiplies converted uint8 values by the float32 constant at
RVA `0x474aa0`: `0.003921568859368563` (rounded 1/255). It performs layout
conversion but no mean subtraction. D1 builds a one-channel 96×96 crop descriptor
with source layout value 1; this runner requests output layout value 0.
One-channel spatial ordering is unchanged in the checked path.

`prepare_crop_tensor` in `d1.py` implements that contiguous grayscale conversion.
`tools/d1_input_oracle.py` passed three byte-exact comparisons of all 9216 floats
(zero, all-byte-value ramp, deterministic pattern), running the original helper
without stubs. Report: `evidence/private/d1-input-oracle.json`. This identifies
and checks the OpenVINO backend path; it does not prove which backend was selected
in every live Windows session, or establish original OpenVINO inference parity.

The reference-shape bounds helper `0x14007d7b0` returns `(min_x, min_y,
max_x-min_x, max_y-min_y)`, retained in `d1-shape-bounds.txt`. The D1 consumer
scales the normalized reference shape by the average of requested bounding-box
width/reference-width and height/reference-height, then translates its bounding
center to the requested box center. It derives a 2×3 transform from that shape
and the reference, optionally composes roll compensation, warps the input image,
and later inverse-transforms predicted points. Resampling and transform-solver
parity remain open.

## D1 geometry continuation (2026-10-13)

`normalize_reference_shape` now implements runtime `0x1400ac560`: subtract
per-axis means, scale by `96 * (0.5 / max(x_extent, y_extent))`, then add 48.
Four original-code checks pass (installed reference, translated/scaled,
anisotropic and synthetic); maximum difference is `3.814697265625e-6`.
The installed reference case is exact. This confirms that loading the raw
initial shape without runtime normalization is not the full preparation path.

Reproduce:

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/d1_geometry_oracle.py
```

Report: `evidence/private/d1-geometry-oracle.json`. `tools/vsmat_oracle.py`
provides a reusable hash-bound matrix emulator. It stubs allocation/free only
and supplies a synthetic GS stack-limit field. The earlier blocker was the
Windows stack-limit lookup, not a need to replace matrix math. Original
reductions, extent selection and scaling execute unchanged. This is numerical
comparison under emulation, not a live Windows observation.

Focused geometry decompilation is retained in `d1-geometry-focused.c` and the
reusable Ghidra project `evidence/private/ghidra-focused/geometry.gpr`.
Use `-process platform_runtime_IS5LEYETRACKER5_service.exe -noanalysis` for
further focused functions; do not reimport/retry whole-program analysis.

In progress: shape fit `0x1400dac40`, inverse point transform `0x1400db010`,
and affine composition `0x1400db330` are implemented in `d1.py`. The provisional
`tools/d1_transform_oracle.py` passes identity, translation and ordinary
rotation/scale cases; a small-scale inverse case currently differs by
0.000244 pixels and is being investigated before accepting the check. The
failed run is not a completed parity result. Inverse helper `0x1400b2310` and
image warp core `0x1400b1890` are the next focused targets.


The transform mismatch above is resolved: the original float-matrix inverse
computes its determinant in float32 (both products and subtraction), then uses
double precision for the reciprocal and inverse coefficients. The float32
inverse matrix is subsequently used for point multiplication. After preserving
that boundary, all **12 transform checks pass** (four each for similarity fit,
inverse points and composition), max absolute error `2.9802322387695312e-8`.
Inverse and composition cases are exact in this set; this is not a general
bit-parity claim. `d1-transform-oracle.json/.log` retain the successful result.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/d1_transform_oracle.py
```

Image warp wrapper `0x1400b45b0` converts matrix headers then calls core
`0x1400b1890`; its fourth argument is unused. Core uses a double inverse and
builds fixed-point column contributions. Its worker vtable `0x141135468`
points to `0x1400b0be0`. The exploratory full-wrapper probe currently faults
at `0x1400b0094` writing null; this remains an initialization/dependency issue,
not passing warp evidence. Probe and log: `tools/d1_warp_probe.py` and
`evidence/private/d1-warp-probe.log`. Focused evidence in
`d1-warp-focused.c`, `d1-warp-wrapper.txt`.

## D1 image crop: original-code byte agreement

`warp_grayscale` now reproduces the checked one-channel path. Core
`0x1400b1890` inverts the image→crop transform in **double precision** (unlike
the float determinant used for the later point inverse), rounds separate
row/column contributions at 10 fractional bits, adds 16, then shifts by 5
for a 1/32-pixel interpolation grid. Worker `0x1400b0be0` samples bilinearly,
with constant-zero borders and rounded byte output.

The null table-storage pointer above is resolved by executing original static
initializer `0x1400011b0`, which aligns `0x14132562f` down to 16 bytes and stores
it at `0x1412fed98`. No interpolation values are fabricated or replaced.
All 12 full-wrapper comparisons pass byte-for-byte: ramp and seeded noise,
each with identity, integer/fractional translation, border overlap,
rotation/scale and small scale. Allocation/free are the only function stubs;
the synthetic GS stack limit remains. The oracle executes initialization and
original resampling instructions, but is not live camera evidence.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/d1_warp_oracle.py
```

Evidence: `d1-warp-oracle.json/.log`, `d1-warp-worker-focused.c`,
`d1-warp-worker.txt`. Unit suite after geometry additions: **46 passed**.

Next integration check: `crop_transform` builds a transform from the normalized
reference, an integer externally supplied face box, and optional roll. The
original caller gates compensation on enabled state and `abs(roll)>5`, uses
stored degree→radian constant `0.01745329424738884`, and composes a rotation
about `(48,48)` before the image→crop transform. `tools/d1_crop_oracle.py`
executes the original caller from `0x14007dc50`, capturing the exact input
buffer and transform immediately before inference at `0x14007dfb9`.
This boundary intentionally excludes neural inference and subsequent temporal
roll updates; no end-to-end tracking parity is implied.


All six complete-preprocessing cases passed: ordinary/full-frame/border boxes,
roll 0, exactly ±5 degrees, +20 and -32 degrees, and disabled compensation.
Crops match byte-for-byte; maximum transform difference is `9.5367431640625e-7`.
The captured descriptor is `(rows=96, columns=96, channels=1, layout=1)` in
every case. Report: `d1-crop-oracle.json`; original inference was not executed.

`D1LandmarkModel` now connects these pieces on Linux CPU: caller-supplied
contiguous grayscale frame and integer face box → crop → float32/255 → D1
OpenVINO inference → reference-relative offsets → inverse-transform → image
and normalized landmarks plus quality. It deliberately requires caller-supplied
roll and does not claim temporal tracking, detection or pose. Nine synthetic
integration cases pass (zero/ramp/noise at roll 0/+20/-32), all 57 points finite,
all 57 quality values within [0,1]. Report:
`models/tobii-et5/d1-pipeline-validation.json`.

```sh
PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/check_d1_pipeline.py
```

The original-code crop checks and Linux inference checks establish different
things: the former checks preprocessing, while the latter only establishes
that the reconstructed model/component executes. Original Windows inference
parity and real-image accuracy have not been measured.

## 3D fitting entry points

`tools/pose_rtti.py` resolves static RTTI for the actual installed runtime,
retaining `pose-rtti.json`. `VisionSDK::FaceModelNoVel` has vtable
`0x141135ad8` (locator `0x14113ec68`), with 17 methods; base `FaceModel`
has vtable `0x141135b68` and mostly pure-virtual entries. This identifies a
concrete fitting-model implementation; it does not yet prove every method's
semantics or live selection. Focused next methods are `0x1400e03d0`,
`0x1400e07a0`, `0x1400df850`, `0x1400dfbf0`.

Nix packaging check: a temporary source tree containing only `src/` and
`tests/`, with site packages disabled, runs all 46 tests successfully with
11 explicit skips. Optional NumPy/OpenVINO tests and tests requiring excluded
research assets skip; protocol/transport/parser checks still run. This fixes
asset-dependent tests failing in the minimal package, but Nix itself is still
unavailable and no Nix build is claimed. Retained result:
`evidence/private/package-isolation-tests.log`.

## Face-model projection recovered

`FaceModelNoVel` virtual method `+0x38` (`0x1400e03d0`) projects the deformed
mesh. The checked rotation is **Ry(ry) × Rx(rx) × Rz(rz)**, with angles in
radians. It scales the rotated model by object `+0x8c`, adds XYZ translation,
and writes `focal * X/Z`, `focal * Y/Z` using focal factor at `+0x88`.
There is no principal-point offset in this primitive. This is the internal
fitting convention; it has not been mapped to public Tobii head-pose axes or
units. Before projection, two original matrix products add deformation bases
to the neutral mesh. Their configuration/optimizer roles remain to be traced.

`src/tobii_linux/pose_projection.py` implements projection with observed
float32 operation order. Five original-code comparisons pass using the 57
installed mesh landmark vertices and synthetic neutral/pitch/yaw/roll/combined
states: max 2D error `5.960464477539063e-8`, max transformed-3D error
`4.76837158203125e-7`. Original state decode, trig and matrix operations run;
no mathematical stubs are used. This is forward projection, **not a solver**.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_projection_oracle.py
```

Reports: `pose-projection-oracle.json/.log`; static evidence:
`pose-methods-focused.c`, `pose-state-focused.c`, `pose-project-entry.txt`.
Initializer `0x1400df850` configures fit flags: bit1 adds six pose state values,
bit2 and bit4 add two deformation coefficient blocks. Encoder `0x1400dfbf0`
uses an inverse-hyperbolic-tangent form for bounded coefficients; decoder
`0x1400dff70` clamps latent values to [-3,3], applies a tanh-like helper and
maps via per-coefficient half-range/midpoint. These deformation semantics are
currently static observations pending their own oracle.

The model is constructed in fitting initializer `0x1400dbba0`; callers are
around `0x14007e91d` through `0x14007f00c`. Supplemental focused analysis of
`0x1400dbba0` and update candidate `0x1400dce50` is next. Full unit suite after
D1 integration still passes **46 tests**; projection tests have not been added
yet (the original-code oracle above is the primary differential validation).


Further pose work moved to [pose fitting](pose-fitting.md). Seven projection/
deformation cases, five rigid Jacobian cases and nine original fitting-step
comparisons pass. The weighted rigid update is implemented in `pose_fit.py`;
outer retry policy, observations, camera calibration and full deformation fitting
remain unfinished. `0x1400dce50` was cleanup, not the fitting update.


## Latest continuation: native supplied-face-box pose component

See [pose fitting](pose-fitting.md) for the subsequent completed checkpoints:
installed mesh normalization, 69-state fitting math, exact float32 inversion,
bounds/retry policy, confidence aliases/gates, numeric point selection, reduced
46/29-state modes and the original staged caller prefix. `NativeD1Pose` connects
frame/box → D1 → quality gate → staged pose fit, with low-confidence frames
skipping the fit. The raw grayscale runner is `tools/run_headpose_frame.py`.

Current verification: 65 passing unit tests; minimal package checks pass with
30 optional/private-asset skips. `evidence/private/landmark-pose-checkpoint.json`
indexes 22 pose/D1 oracle reports (173 cases/branches) with SHA-256 digests and
explicit limitations. This supersedes earlier counts/open items in this
chronological ledger; it does not establish live USB tracking or Wine support.
