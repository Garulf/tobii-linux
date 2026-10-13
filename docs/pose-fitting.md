# Native pose fitting reconstruction

Current result: forward projection/deformation, a rigid Jacobian and one
weighted fitting step are implemented and checked against the installed Windows
runtime. **The complete pose estimator is not yet implemented.** Camera calibration, iterative fitting, live mode selection and
public head-pose coordinates remain unresolved.

All addresses refer to runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`.
Evidence is supplemental static analysis and offline emulation, not fabricated
REA Evidence or live observations. No other ET5 projects were consulted.

## Verified projection

`src/tobii_linux/pose_projection.py` implements:

- Bounded coefficient decoding: clamp latent state to [-3,3], apply tanh,
  multiply by half-range, add midpoint.
- Two deformation matrix products added sequentially to neutral vertices.
- Rotation `Ry(ry) @ Rx(rx) @ Rz(rz)` in radians, followed by model scale and
  XYZ translation.
- Perspective output `(focal*X/Z, focal*Y/Z)`, with no principal-point offset
  in this primitive. This does not establish the camera/image coordinate map.

`tools/pose_projection_oracle.py` runs original `0x1400e03d0`, including its
state decoder, trigonometry, matrix operations and projection. Seven cases pass:
neutral/pitch/yaw/roll/combined poses, two synthetic deformation bases and
out-of-range latent clipping. Inputs use the 57 installed mesh landmark vertices.
Maximum error is `4.76837158203125e-7` for both the expanded 2D suite and
transformed 3D coordinates. Numerical agreement is not general bit parity.

```sh
PYTHONPATH=.local/python:.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/pose_projection_oracle.py
```

Reports: `evidence/private/pose-projection-oracle.json/.log`. Allocator/free
stubs are available in the shared emulator, but the current projection cases
record no calls to them. The executable is hash-bound; no service entry point
is executed. Synthetic GS stack-limit setup remains.

## Original class and state

`tools/pose_rtti.py` resolves `VisionSDK::FaceModelNoVel` vtable
`0x141135ad8` with locator `0x14113ec68`, versus mostly-pure-virtual base
`FaceModel` at `0x141135b68`. Report: `pose-rtti.json`.

| Virtual slot | Function | Observed role |
| --- | --- | --- |
| +0x38 | 0x1400e03d0 | Forward projection |
| +0x40 | 0x1400e07a0 | Projection derivative candidate; implementation pending |
| +0x58 | 0x1400df850 | Initialize model/state dimensions and matrices |
| +0x60 | 0x1400dfbf0 | Encode pose plus inverse-tanh coefficient states |
| +0x68 | 0x1400dfda0 | Configure state sensitivities |
| +0x70 | 0x1400dff70 | Decode pose, clamp/decode coefficient blocks |

Flags are masks `0x1` (six pose values), `0x2` (first latent block) and `0x4`
(second latent block). The state consists of rotation XYZ, translation XYZ,
then enabled coefficient blocks. Their exact association with installed
animation/shape-unit configuration still requires tracing; do not guess it.

| Object offset | Observed content |
| --- | --- |
| +0x28 | Neutral 3N×1 vertex matrix |
| +0x30 | N, projected point count |
| +0x60 / +0x68 | First deformation basis / decoded coefficients |
| +0x38 / +0x40 | Second deformation basis / decoded coefficients |
| +0x80 / +0x58 | First / second coefficient half-range and midpoint pairs |
| +0x88 | Focal factor |
| +0x8c | Model scale |
| +0x90 | Per-state sensitivity matrix |
| +0xa0 / +0xa8 | Intermediate deformed vertices |
| +0xb0 | Transformed 3D points |

The initializer uses symmetric [-1,1] bounds for the first block and caller
bounds for the second. Original encoder behavior at endpoints/non-finite values
is not implemented. Native functions reject invalid inputs explicitly.

## Iterative fitter: next work

`0x1400dbba0` creates the model, extracts vertex/deformation subsets from the
parent face model, configures sensitivity and constructs a 0x98-byte fitter
using `0x1400df430`. It encodes the starting state and initializes state-sized
matrices. Calls occur around `0x14007e91d`–`0x14007f00c`.

The fitting step is `0x1400dc3b0`, which calls `0x1400dc730`,
`0x1400df6b0`, and `0x1400dcc30`. Focused decompilation is retained in
`pose-update-focused.c/.log`; confirm these roles from its output and raw
instructions before implementing the optimizer.

Earlier candidate `0x1400dce50` was **not** an update: it follows container
cleanup/free paths. Its decompiler has control-flow warnings; do not pursue it
as fitting evidence. This corrects the provisional entry in the chronological
face-assets ledger.

Other retained files: `pose-methods-focused.c`, `pose-state-focused.c`,
`pose-fitting-focused.c`, `pose-project-entry.txt`, `pose-state-unpack.txt`
(the latter address is sensitivity configuration, despite the earlier filename).
The reusable Ghidra project is `evidence/private/ghidra-focused/geometry.gpr`.
No need to reimport or rerun broad analysis.

## Rigid Jacobian and fitting update implemented

The six rigid-state Jacobian columns are now implemented as `rigid_jacobian`
in `pose_projection.py`. Five original-method `0x1400e07a0` comparisons pass
(neutral and rotated synthetic points): max absolute difference `5.96e-8`.
Deformation Jacobian columns and excluded-observation branches remain unimplemented.
Report: `pose-jacobian-oracle.json/.log`.

`src/tobii_linux/pose_fit.py` implements one recovered rigid update:

```
P = diag(1 / state_sensitivity)
W = diag(1 / observation_sensitivity)
J = projection Jacobian
K = inverse(J.T @ W @ J + P) @ J.T @ W
state_next = state + K @ (observed - projected)
```

Intermediate original matrices are float32; accumulation and inversion can
round differently. This API currently accepts finite positive sensitivities,
with threshold/fallback and observation exclusion behavior outside its scope.
It is one weighted, regularized rigid update, not the whole fitting policy.

`tools/pose_fit_oracle.py` initializes the original fitter using `0x1400df430`
and executes `0x1400df6b0`, preserving the original Jacobian, matrix inversion
and matrix update. Nine comparisons (three iterations each of neutral, offset,
and unequal-weight inputs) pass with maximum state difference `7.45e-9`.
Allocator/free are the only available stubs, and the GS stack limit is synthetic.
No original mathematical helper is replaced. Report: `pose-fit-oracle.json/.log`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_jacobian_oracle.py
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_fit_oracle.py
```

Outer `0x1400dc3b0` requires at least four points, prepares observations via
`0x1400dc730`, executes a caller-selected number of updates, decodes each state,
and calls `0x1400dcc30` to decide whether to reset/retry. At most two outer
attempts occur; a retry sets three iterations after `0x140052d70` initialization.
Exact criteria and observation coordinates are pending focused analysis in
`pose-observations-focused.c/.log`. This supersedes the earlier statement that
no iterative step is implemented; the complete estimator remains unfinished.

Full unit suite after the fitting-step additions: **52 passed**.

## Observation plane and exclusion verified

`prepare_observations` implements the fully populated D1 landmark path:

```
observed_x = (1 - 2*normalized_image_x) * max(width/height, 1)
observed_y = (1 - 2*normalized_image_y) * max(height/width, 1)
```

Both axes are flipped around the image center. `0x1400dc730` assigns ordinary
coordinates sensitivity float32 `0.002`, and IDs `(2,1)`, `(3,5)`, `(3,6)`
sensitivity `100001`. The D1 table includes `(3,5)` and `(3,6)`.
The original Jacobian writes zero for **both** coordinates when that point's
X sensitivity is >=100000; `rigid_step` now preserves this exclusion rule.
The fitting oracle adds three excluded-point iterations (including unequal X/Y
sensitivities): **12 update comparisons pass**, max state error `1.49e-8`.

`tools/pose_observations_oracle.py` executes the entire original preparation
method on valid, mapped D1 records with a prebuilt lookup tree. Six cases
(center/spread coordinates × landscape/portrait/square dimensions) match
coordinates and sensitivities exactly. Original iteration, lookups and math
execute unchanged; no mathematical stub is used. Report:
`pose-observations-oracle.json/.log`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_observations_oracle.py
```

Missing/invalid landmark retention and view-dependent weights for groups
10/13 remain unresolved; the native helper explicitly rejects those groups.
These groups do not occur in the installed 57-entry D1 ID table.

The observer's facing angle is `(ry - atan2(tx,tz) - pi)` converted to degrees.
Its side-selection hysteresis uses 15° to enter and 10° to remain active.
The reset caller passes **pi** as neutral yaw; the installed Head Tracker.cfg
rotation-y limits `[1.05,5.24]` support this convention. This is internal pose,
not the public API's neutral angle. The config declares `camera_focus 2.6`;
actual loading/override, mesh normalization and conversion into device/world
units still need confirmation.

Bounds checker `0x1400dcc30` checks pose against model rotation/translation
limits, wraps the third rotation with helper `0x140434d38`, and rejects/zeros
non-finite deformation coefficients. Reset `0x140052d70` derives scale and
translation from selected group-3 landmark pairs and their mapped mesh points.
These two routines are retained in `pose-observations-focused.c`, but their
complete numerical behavior is not implemented or oracle-validated yet.

## Reset initialization checked

`initial_rigid_state` now reconstructs the valid four-correspondence branch of
`0x140052d70`. Correspondences are ordered FDP IDs `(3,7), (3,11), (3,8),
(3,12)`. It compares the distance between their two XY model midpoints with
the aspect-normalized image midpoint distance; that ratio and the focal factor
set initial depth. Image midpoint positions set XY translation. Initial
rotation is `(0, pi, 0)` in the observed caller.

`tools/pose_initialization_oracle.py` executes the original reset routine using
explicit valid mapped pairs. Nine cases (reference, translated and scaled
image points across landscape/portrait/square) match exactly. Report:
`pose-initialization-oracle.json/.log`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_initialization_oracle.py
```

The supplied mesh/reference coordinates in this oracle are numerical fixtures,
not a claim that runtime mesh preparation has been reconstructed. Correct
camera focus selection and mesh scale remain necessary before linking D1 to
physical head-pose translation. `initial_rigid_state` therefore accepts already
prepared model points explicitly. Null-input reset, invalid/missing pairs and
outer bounds/retry behavior are not implemented.

Current full unit result: **54 passed**. Minimal source-only package simulation
passes with **19 explicit skips** for missing optional inference dependencies
and excluded research assets. Nix itself still has not been built/evaluated.

## Mesh/camera preparation trace in progress

Model loader `0x140019550` allocates a 0x168-byte face model, constructs it at
`0x140050540`, loads/configures it through `0x140050690`, and applies helper
`0x140052bc0` with tracker field `+0x124`. A reuse path calls `0x140051f90`.
These are the next focused targets for runtime mesh scale/focal setup; retained
loader evidence is `pose-loader-focused.c/.log`, with the ongoing setup trace
in `pose-model-setup-focused.c/.log`. Configuration binding at `0x14000581e`
connects string `camera_focus` to configuration object `+0x1b0`; this is still
upstream of the concrete face-model focal value.

Model setup now statically confirms that configuration `camera_focus` (`+0x1b0`)
is copied to face-model focal field `+0x50`; fitter initialization then copies
that field to the projection model's `+0x88`. Face-model scale `+0x54` is set
to 1 after loading. This connects the configured focal value through the
internal pipeline, though live configuration overrides remain unobserved.

`0x140052bc0` is a real mesh-rescaling operation: it computes the supplied
tracker `+0x124` value divided by face-model `+0x10c`, scales both deformation
bases and the neutral vertex matrix, copies the scaled vertices to the working
matrix, and stores the new measurement in `+0x10c`. Thus raw file vertices
are not automatically the runtime inputs. Reference-metric computation at
`0x140053f50` is being traced in `pose-mesh-normalization-focused.c/.log`.
Do not assign units or a biometric interpretation to `+0x124`/`+0x10c` yet.

## Mesh preparation byte agreement

`pose_mesh.py` implements sparse-basis expansion, normalization and subsequent
rescaling. `0x140053f50` centers XY on the midpoint between the two reference
midpoints, centers Z on the four reference vertices' mean, and scales both
vertices and deformation bases so the **XY** midpoint distance equals the
face model's `+0x10c` value. Both model constructor `0x140050540` and tracker
constructor initialize the distance to float32 **0.065**. The tracker setter
`0x1400173e0` can change it and rescale loaded models. Public/device units are
still not established solely by this numeric value.

Four `tools/pose_mesh_oracle.py` comparisons pass byte-for-byte for every
coordinate/coefficient: installed arrays, translated vertices, alternative
0.072 distance, and normalization followed by 0.065→0.072 rescaling.
All 357 vertices and both sparse-expanded bases (23+40 units) are included.
Reports: `pose-mesh-oracle.json/.log`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_mesh_oracle.py
```

Text reader `0x140053220` now statically resolves the basis ordering: the
`ANIMATION UNITS LIST` is stored at parent `+0x18`, and `SHAPE UNITS LIST`
at parent `+0x10`; sparse indices select raw vertex triples directly. Vertices
are read as sequential floats and copied to the neutral matrix without an
axis transform in that loop. Evidence: `pose-mesh-reader-focused.c`.
The fitting model's first block (`+0x60`, flag 0x2) is therefore **shape units**,
and its second block (`+0x38`, flag 0x4) is **animation units**. This resolves
the earlier unknown block labels. Disabled configuration columns are zeroed
in setup before fitting; all stored basis columns retain their positions.

## Full deformation update preserved

A significant original-code quirk is now verified. In deformation columns,
`0x1400e07a0` transforms **each basis component b** using
`(1 - tanh(b)^2) * tanh(b) * half_range`, then rotates/projects that vector.
It does not use the conventional derivative evaluated at the current latent
coefficient. Both raw instructions around `0x1400e11ae` and four original-code
comparisons confirm this (zero/nonzero/clamped latents, enabled/disabled masks).
`model_jacobian` deliberately preserves it; calling it an update matrix avoids
claiming it is the mathematical derivative of the projection function.
Maximum difference across those comparisons: `5.96e-8`.

`deformation_step` extends the recovered weighted update to shape and animation
states. It clamps the copied starting latents to [-3,3] before prediction, and
preserves the original output's possible excursion beyond the bound until a
subsequent decode. Six complete original update comparisons pass with 11 states
(six pose + three synthetic shape + two synthetic animation): three iterations
each from neutral and out-of-range latents. Original nonlinear decode,
update matrix, inversion and multiplication all execute unchanged.
Reports: `pose-deformation-jacobian-oracle.json/.log` and
`pose-deformation-fit-oracle.json/.log`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_deformation_jacobian_oracle.py
PYTHONPATH=.local/python:.local/inference:src python3 tools/pose_deformation_fit_oracle.py
```

These checks use synthetic bases, not the complete configured 40+23-unit
installed fit. Next: construct the normalized, masked installed model with its
configured sensitivities/ranges and verify the larger update. Outer bounds,
mode selection, temporal policy and actual hardware/game integration still
remain unfinished.

### Configured 69-state checkpoint (in progress)

`pose_model.py` now prepares the installed normalized/masked model, joins all
57 D1 landmarks, and loads the 40 shape + 23 animation bounds and sensitivities.
Physical zero animation coefficients require nonzero latents because the bounds
are asymmetric. The original `0x1400dfbf0` encoder agrees **byte-for-byte** on
all 69 initial state values (`tools/pose_configured_oracle.py`).

The first full configured update initially failed the 1e-5 comparison threshold
(maximum error 5.41e-5). This is retained as a real unresolved result, not a pass.
Tracing intermediate matrices showed two sources: reassociated Jacobian float
operations and inversion. Replacing analytic double arithmetic with the original
float32 derivative order makes the first configured prediction, Jacobian,
weighted Jacobian and normal matrix **byte-exact**. The remaining update error
is 5.21e-5; inverse maximum error 1.049e-5. The normal matrix condition number is
approximately 6.96e6. Native `numpy.linalg.inv` is not numerically identical to
the runtime inversion routine. Do not loosen the threshold silently.

Next: recover `0x140085510` → `0x14007fba0` inversion arithmetic. Retained matrices
are in `evidence/private/pose-configured-matrices.npz`, the earlier failure magnitudes are recorded above;
`pose-configured-oracle.log` now contains the subsequent successful rerun. The larger emulated matrix update exceeds the
original 5-million-instruction harness budget; this one oracle explicitly uses
30 million, with the same ten-second timeout. Default oracle limits are unchanged.
After fixing inversion, rerun configured and previous Jacobian/update oracles
and the unit suite. Bounds/retry policy and live tracking remain unresolved.

### Configured inversion resolved

The configured failure above is resolved. `pose_linalg.inverse_float32`
reconstructs the original partial-pivot elimination and sequential back
substitution, with float32 rounding after every scalar operation. Eight direct
original-wrapper checks are **byte-exact**, including random orders 4/6/11/69,
pivot swaps, singular output zeroing, and pivots below/at float epsilon.
Source chain: `0x140085510` → `0x14007fba0` (mode 0) → `0x140085b70`.
Evidence: `pose-{inverse,inverse-core,lu}-focused.c`, `pose-lu-tail.txt`,
`pose-inverse-oracle.json`. This is supplied-binary analysis, not external code.

The installed model's three consecutive 69-state updates now pass the original
1e-5 threshold: first **byte-exact**, then maximum errors 1.67e-6 and 1.10e-7.
Initial physical-zero coefficient encoding is byte-exact. Report:
`evidence/private/pose-configured-oracle.json`. Synthetic landmarks still do not
establish live tracking, original neural inference parity or complete policy.
Previous Jacobian/update oracles passed after the derivative change; rerun them
after the inversion change before the final checkpoint. The unit suite before
the new inversion/bounds additions passed **57 tests**.

### Bounds and retry policy continuation

`pose_policy.check_pose_bounds` passes **29 original-code cases** at
`0x1400dcc30`: inclusive rotation/translation edges, outside values, large
finite rolls, selected NaN axes, and nonfinite coefficient cleanup. Roll is
checked modulo **3.141592502593994**, not 2*pi, and the stored roll is unchanged.
The configured roll bounds [-3.2,3.2] encompass that remainder interval.
Invalid decoded shape/animation coefficients become zero and mark the pose bad.

Important decompiler correction: its ordinary `<` expression concealed x64
`COMISS` + `JB`, which also rejects unordered comparisons. A NaN yaw is rejected,
even though only pitch has explicit finite/NaN helper calls. The first native
NaN-yaw test caught this; `pose-bounds-entry.txt` verifies the actual branches.
Do not infer NaN behavior from this pseudocode alone. Nonfinite roll CRT error
handling is not exercised by the oracle.

`fit_landmarks` now implements a bounded complete-D1-record branch: supplied
initial state (or explicit four-eye-point initialization), requested updates,
coefficient decoding/clipping after every update, validation, and at most one
retry with three updates. Retry preserves decoded deformations, resets the
rigid pose, then re-encodes the latents. The no-external-pose-hint branch is being
checked through original outer entry `0x1400dc3b0` using
`tools/pose_policy_oracle.py`; don't claim it validated until its report exists.
`0x140052710` finalizes deformed/projected mesh and eye offsets without changing
the fitted rigid state (focused static evidence in `pose-finalize-focused.c`).


### Outer loop and native component integration verified

`pose-policy-oracle.json` now contains **four passing original outer-loop cases**:
normal fit, retry, retry with nonzero deformations, and a retry that remains
outside deliberately impossible bounds. State maximum errors are respectively
0, 1.79e-7, 5.37e-7 and 1.79e-7. Original update/reset/finalization call counts
also match. These calls execute the original bounds, observation conversion,
state codec, fitting and finalization functions; no math or branch stubs are
used. Only the shared allocator/free environment stubs remain. The harness
supplies already-initialized objects, complete records and no external pose hint.
Its outer-loop budget is explicitly 100 million instructions / 30 seconds;
the shared defaults remain five million / ten seconds.

`tools/check_landmark_pose_pipeline.py` connects native D1 preprocessing,
OpenVINO inference, inverse crop mapping, mesh preparation and fitting on three
synthetic images. Report: `models/tobii-et5/landmark-pose-validation.json`.
All three non-face images yield finite, bounds-valid states, with low neural
quality scores. This is a component execution check, **not face detection or
valid head tracking**. `tracking_valid` is deliberately unknown in the report.
Next work must trace quality/landmark selection and caller modes before exposing
these states to any game or treating them as device observations.

### Confidence boundary and caller-mode correction

The D1 writer `0x14007d910` unconditionally sets both FDP flags to one and copies
quality into record +0x14. Three emulated cases (171 records total) confirm this,
including zero and out-of-range quality: `d1-records-oracle.json`. Those flags
alone cannot mean reliable tracking.

The wrapper `0x1400557a0` retains the 57 D1 IDs then calls `0x140055a20`, which
fills aliases. In the complete-D1 case four records are added: 2.1 and 13.17
copy 15.17, 14.21 copies 9.3, and 9.12 copies 14.23. `0x14007a880` averages
present, nonnegative qualities in ascending FDP group/index order with float32
accumulation. Thus the 61-record score is not the mean of 57 neural outputs.
`d1_quality.py` and `d1-quality-oracle.json` validate five complete aggregation
cases byte-exactly, including the all-negative → NaN case.

The caller at `0x1400495d4` accepts modes 0/1 only when score > 0.5, and mode 2
only when score > float32(0.35). Twenty short original-branch comparisons cover
strict boundaries, the adjacent float, unsupported mode and NaN. This executes
only the branch range, not the whole caller; the caller mode meanings and
selection remain unresolved. The integration report now evaluates this gate
separately. All synthetic non-face frames are rejected in modes 0/1/2; forced
fitting remains only a component exercise.

**Mode-chain correction:** focused `0x14007e840` shows a fitting caller that
initializes zero physical coefficients, runs flag **3** (pose + shape) for two
iterations, then flag **5** (pose + animation) for one. The previously validated
69-state flag-7 simultaneous fit is a supported primitive, **not proof that this
caller uses simultaneous fitting**. Optional additional model passes, external
pose hints and the alternate `0x14007e440` branch remain to reconstruct. Continue
by validating the 46-state and 29-state substeps and the caller's point mapping.
Evidence: `d1-selection-focused.c`; no other project or external solver used.

### Reduced mode updates verified

`pose_fit.mode_step` supports the original flag-3 (46-state pose/shape) and
flag-5 (29-state pose/animation) updates, retaining the inactive block as physical
coefficients. Re-decoding an inactive zero coefficient from its asymmetric
latent introduces rounding the original mode does not perform, so the fixed
block is supplied explicitly. Consecutive same-mode calls can retain the active
latent state rather than re-encode physical coefficients on every iteration.

Six original-update/decode comparisons pass (`pose-modes-oracle.json`), using
zero and nonzero deformations and the 3→3→5 sequence; max latent/state error
4.21e-6. Inactive physical coefficients are byte-exact in every case. The
zero-coefficient sequence has byte-exact active states for all three updates.
The harness orchestrates these substeps; it does not execute the entire
high-level `0x14007e840` caller or prove the caller selects the same 57 points.

Further focused observations (`pose-caller-focused.c`): `0x14007e330` creates
fitting objects through `0x1400db680` using current landmark records and up to
three configured parent models. `0x14007ac80` flips normalized Y on a saved
copy of present landmarks; the main fitting call still receives its original
argument. `0x140055470` outputs (rx, ry - float32(pi), rz), copies tx/ty/tz, and
optionally returns transformed mapped point FDP 7.1. This is an intermediate
output boundary, **not yet the final public Tobii/Star Citizen coordinate API**.
Next: inspect `pose-point-selection-focused.c` from `0x1400db680`, then connect
confidence/mode/selection in a single original-caller comparison before calling
an image-to-pose implementation equivalent to the Windows tracking pipeline.

### Actual selection order verified

`0x1400db680` selects present records with valid model FDP mappings, ignoring
quality and the second record flag, then scans in numeric FDP group/index order.
It copies corresponding parent working vertices and constructs its own lookup
map. `pose-selection-oracle.json` verifies full input, full input plus aliases,
and a 29-point subset byte-for-byte, including selected IDs and vertex indices.
The four confidence aliases are absent from `jk_300_rigid.fdp`, so the complete
D1 fit still has **57** points. Its ordering differs from the neural landmark
file's order; `select_pose_landmarks` explicitly reorders the model and provides
the point permutation.

`fit_staged_landmarks` now connects numeric selection, reset, two flag-3 updates,
one flag-5 update and bounds/retry behavior for the no-hint/single-parent branch.
`tools/pose_caller_oracle.py` is checking original `0x14007e840` from entry through
its output boundary at `0x140055470`, with original constructor, selection and
mode switching included. Do not claim this higher-level comparison until its
JSON result exists. It deliberately stops before output/auxiliary postprocessing.

### Original staged caller prefix verified; native component connected

`pose-caller-oracle.json` now has **two passing comparisons** from original
`0x14007e840` entry to immediately before `0x140055470` output conversion. The
original constructor, point selection, model/fitter initialization, mode
switching, three updates and mesh finalization all execute. Both execute update
flags [3,3,5]. Neutral pose and shape coefficients are byte-exact; animation
max error 1.20e-7. Offset-case maximum errors: pose 5.96e-8, shape 7.85e-8,
animation 7.16e-7. No neural inference, optional extra parent models or output
postprocessing are part of this comparison.

The first caller attempt exposed an initialization mistake in the harness,
not a solver success: zero-initialized caller +0x3c yielded yaw 0 and five flag-3
updates after a retry. Raw instructions showed `0x14007e270` stores **integer 3**
in +0x3c; `0x14007e840` converts that integer to float for initial yaw. Calling
the original reset before the prefix test fixes the harness. Native
`fit_staged_landmarks` preserves initial yaw **3.0**, with **float32(pi)** used
for the outer retry. Do not replace either with an assumed universal neutral
constant. Inspect `pose-caller-first-update.npz` for the latest captured first
model/state arrays; the mismatch and its correction are recorded here.

`headpose.NativeD1Pose` connects a supplied grayscale frame + face box to D1,
the original-style confidence gate, numeric point selection and the staged
single-parent fitting branch. It skips fitting when confidence fails. The
caller must still supply the face box, quality mode and crop roll; no face
detector, temporal selection or device calibration is implied. Pose coordinates
remain internal fitting values. `check_landmark_pose_pipeline.py` verifies all
three synthetic non-face inputs are rejected by the component; it separately
forces fitting only to exercise the downstream stages.

Run a local packed 8-bit grayscale frame (NumPy and OpenVINO required):

```sh
PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/run_headpose_frame.py frame.gray --width 640 --height 480 --face-box 100 80 200 240
```

The output reports confidence and either no fit or internal pose/deformation
values. This tool does not send samples to Wine or claim live tracker support.
Final verification for this checkpoint: **65 unit tests pass**; isolated minimal
package tests pass with optional/private-asset tests skipped (see retained log).
The previous rigid/deformation Jacobian and update oracles also pass after the
float32 arithmetic/inversion changes. Nix has not been built in this workspace.

### Output conversion

The path after `0x140055470` to the public head-pose sample is documented in
[head-pose output](head-pose-output.md).
