# Native Linux model inference checkpoint

All four neural-network graphs now compile and execute using native
OpenVINO CPU operations on this Linux workspace. This does not invoke Wine,
Tobii's Windows runtime or device hardware. It is a graph-execution milestone,
not a head-pose implementation or original-runtime parity test.

## Later pipeline checkpoint

The graph results below are the earlier inference milestone. Subsequent work
has reconstructed native LBF refinement, D1 crop/landmark preparation, confidence
aggregation and rejection, numeric landmark selection, and the single-parent
staged pose fit. `NativeD1Pose` and `tools/run_headpose_frame.py` connect a supplied
grayscale frame and face box through those components. Original fitting-caller
prefix comparisons pass through the output-conversion boundary; original Windows
neural inference parity and live tracking remain unverified.

Current tests: **65 passed**; minimal package tests pass with 30 optional/private
asset skips. [Pose fitting](pose-fitting.md) and [project status](project-status.md) are
the current continuation references. Remaining gaps include face detection,
temporal modes/roll, optional model branches, output postprocessing, device
calibration, USB frames and Wine integration.

## Files and reproduction

- `src/tobii_linux/legacy_ir.py`: explicit translation of the observed IR4/IR6
  layer types to OpenVINO operations; checks manifest hashes, blob extents,
  connections and every declared tensor shape.
- `tools/check_models.py`: zero/ramp/seeded-noise inputs for each model, plus
  independent NumPy evaluation of the complete AlignPre graph.
- `models/tobii-et5/inference-validation.json`: actual results and runtime
  version. Export manifests separately record structural validation only.
- `tests/test_legacy_ir.py`: corrupted hashes, missing connections and truncated
  constant blobs must fail. Tests skip if optional dependencies are absent.

Scoped dependencies installed successfully: OpenVINO 2026.4.1 and NumPy 2.4.6,
under `.local/inference/`. They are not added to the USB probe's dependencies.

```sh
PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/check_models.py
PYTHONPATH=.local/inference:src python3 -m unittest discover -s tests -v
```

Compilation explicitly requests CPU FP32 and two inference threads. All
**12 synthetic cases passed**, with finite outputs and expected shapes.
At this earlier milestone the complete unit suite passed **30 tests**.
`evidence/private/tests-latest.log` now records the later 65-test suite.
AlignPre's three cases agreed with the independent NumPy evaluator within
`atol=2e-6, rtol=2e-4`; maximum observed absolute difference was approximately
`9.84e-7`. The other three graphs passed shape and finite-output checks, but
do not yet have independent numerical references.

The reference evaluates convolution, bias, ReLU, max pooling, reshape, dense
weights and softmax independently of the OpenVINO translation. This checks the
AlignPre graph's weight orientation and arithmetic. Neither implementation has
yet been compared with Tobii's original Windows inference output.

## Why explicit graph translation

The installed models use IR versions 4 and 6. We reconstruct only the operations
observed in those files, preserving their shapes, attributes and weight bytes.
Unknown operations fail. This is not a general legacy-IR converter.

OpenVINO's [IR documentation](https://docs.openvino.ai/2026/documentation/openvino-ir-format.html)
describes the XML graph/binary weights representation;
[Intel's compatibility guidance](https://www.intel.com/content/www/us/en/support/articles/000096498/software.html)
does not guarantee old IR compatibility with newer runtimes. The current loader
uses the runtime operation API directly instead of depending on legacy parsing.

## Earlier pipeline work and static leads (partly superseded)

Inputs are synthetic tensors. Their range is a test choice, **not recovered
camera normalization**. Crop selection, image resize, channel ordering,
normalization, landmark interpretation, pose fitting, calibration, smoothing and
output coordinates still need reconstruction.

Additional installed assets are in `models/tobii-et5/support/`. The mesh and
FDP/configuration files are text. Initial-shape and landmark files have numeric
headers. All three LBF file layouts and compressed coefficients are now decoded;
tree evaluation and predictor integration remain incomplete. See [face assets](face-assets.md).

Supplemental static leads in the hash-bound platform executable:

- `fa.lbf` string use at `0x140057061`/`0x14005707e` leads to constructor
  `0x1400ac740` through call `0x1400570d0`.
- That constructor calls loader `0x1400ac7d0` at `0x1400ac7b3`.
- The loader opens through `0x1400aa2b0`, then reads a one-byte header and
  subsequent four-byte fields through `0x1403e5adc`. The later investigation
  confirmed plaintext/codebook-compressed data.
- `init_shape.bin` path construction occurs around `0x1400abf66`; its file-open
  call is at `0x1400abfd3`. `landmarks.bin` path references occur around
  `0x1400abd03` and `0x1400abd26`.

These addresses come from retained objdump output, not new REA Evidence. The
full earlier evidence and hardware limitations remain in
[the project handoff](project-status.md).
