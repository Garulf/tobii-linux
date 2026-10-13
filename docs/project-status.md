# Project status and handoff

Recorded 2026-10-13. This document preserves the investigation context and the
next steps; detailed evidence is linked below.

## Latest continuation checkpoint

### Driver checkpoint: live USB, full pipeline, Wine route

- **Live hardware (WSL2 on the reference host, usbipd-win):** channel 0
  carries TTP; protocol upgrade, unlock, realm `0x2712` challenge, camera
  hardware info, image subscription and unsubscribe all succeed on the real
  tracker. Image frames are 280x280 at about 33 Hz. See
  [USB streams, live observations](usb-streams.md#live-observations-wsl2-on-the-windows-reference).
- **Live head pose:** 150 frames recorded on Linux with a person in front of
  the tracker give a valid sample on every frame; mean position
  (-42.8, -9.8, 690.8) mm against (-27.9, -8.3, 694.5) mm from the Windows
  runtime recorded just before. `tools/replay_frames.py` replays saved
  frames offline.
- **Pipeline, all native and mostly checked against the original by
  emulation:** face finder ([face detection](face-detection.md)), tracking
  loop and working view ([face tracking](face-tracking.md)), D1 and gate,
  temporal fitter ([pose temporal](pose-temporal.md)), output conversion,
  geometry stage ([pose geometry](pose-geometry.md)) and One Euro filters
  ([head-pose output](head-pose-output.md)), USB session and CLI
  (`driver.py`, `cli.py headpose`).
- **Game side, end to end:** with the tracker attached to WSL2, `cli serve`
  feeds tracker samples to the PRP service; a bridge inside a Wine 11.0
  prefix carrying the pipe-listing patch relays the Tobii pipes, and both
  Star Citizen's `tobii_gameintegration_x64.dll` and `tobii_stream_engine.dll`
  discover `tobii-prp://ET5-LINUX` and receive 361 samples in 10 s with a
  flat 11 ms timestamp lag. No face was in view, so the samples were invalid;
  Star Citizen itself has not been run
  ([PRP bridge](prp-bridge.md), [Wine pipe listing](wine-pipe-enumeration.md)).
- **Game integration layer:** the API Star Citizen calls (`GetApi`, tracking
  and head-pose reads) needs the display area, display id, model name and a
  presence subscription answered; the service now does so in the real
  service's encoding, and the bridge reports the prefix's primary monitor id.
  Under patched Wine 11.0 the layer receives every synthetic sample
  ([game integration layer](game-integration-layer.md)).
- **Fit and smoothing:** the shipped configuration fits in the per-object
  driver (`0x14004a2c0`) on full-frame records after quality-weighted landmark
  smoothing (`0x14007f5a0`), now reproduced within 5 ulps
  ([landmark smoothing](landmark-smoothing.md)). It reduces jitter but leaves
  the 15 mm / 3 degree offset against Windows unexplained; a simultaneous
  comparison needs frames captured from the Windows runtime, and USBPcap's
  device listing returned nothing on the reference host.
- **Clock:** the service clock must match the game Wine's
  `QueryPerformanceCounter` source (`CLOCK_BOOTTIME` from Wine 10.17,
  `CLOCK_MONOTONIC_RAW` before); `--wine-clock` selects it.
- **Speed:** on the reference host's CPU, 9.6 ms per frame with landmark
  smoothing (3.8 ms before it), after vectorizing the float32 inverse
  (results unchanged bit for bit).
- **Nix:** the flake evaluates against nixpkgs unstable
  (`7fd4004d0511991fff3a0524856a9be09e28c5f0`), and the NixOS module, including
  the optional `headPoseService` systemd user service, evaluates in a minimal
  system; `checks.nixos-module` repeats that evaluation. The package builds
  with Python 3.14 from that nixpkgs and its check phase passes (214 tests,
  88 skipped for missing private inputs). Checked with nix-portable in a
  container, not on NixOS hardware.
- **Fitting coordinates:** the live comparison only matches when the fit uses
  full-frame landmarks, so the tracker now does that; the static path that
  maps working-view records to the frame is still unidentified, and lateral
  and angular differences of up to 15 mm and 3 degrees remain
  ([face tracking](face-tracking.md#fitting-coordinates-live-comparison)).

### Head-pose output path traced

The fitted pose reaches the public sample through `platmod_algo`: a face
adapter scales translation to millimetres and builds an order-3 rotation
matrix from (rx, ry - pi, rz); a producer converts it back to angles, applies a
configured geometry stage (camera/display matrices plus a 180 degree Y
rotation), then One Euro filters for each angle and for position. The public
48-byte record takes those values unchanged in x, y, z order. Static evidence
only; matrices, filter parameters and an emulated check remain. See
[head-pose output](head-pose-output.md).

### Native supplied-face-box pose component checkpoint

The installed 57-landmark model now fits all 69 state values natively: six rigid,
40 shape and 23 animation. Original-code checks include:

- Initial coefficient encoding and first configured update: byte-exact.
- Three configured updates: maximum difference 1.67e-6.
- Float32 inversion: eight byte-exact cases, including singular and pivot cases.
- Pose bounds and coefficient cleanup: 29 matching cases.
- Complete outer fitting loop: four matching normal/retry/failure cases,
  maximum state difference 5.37e-7.
- Native synthetic grayscale frame → D1 → landmarks → pose component chain:
  three cases execute. Bounds-valid poses on these non-face inputs demonstrate
  why quality/face selection remains necessary; these are **not valid tracking**.

The original single-parent staged caller prefix also passes two comparisons
through its output-conversion boundary (maximum coefficient error 7.16e-7).
Numeric point selection passes three constructor comparisons. `NativeD1Pose`
connects D1, confidence rejection and the staged fit; `tools/run_headpose_frame.py`
runs a local raw grayscale frame. **65 unit tests pass**. Minimal package checks
pass with optional/private assets skipped; Nix and live tracking remain untested.

See [pose fitting](pose-fitting.md), `pose_{configured,inverse,bounds,policy}_oracle.py`
and `models/tobii-et5/landmark-pose-validation.json`. The prior smaller mesh,
Jacobian and fitting checks remain documented there. D1 quality now has five byte-exact aggregation and twenty caller-gate checks;
all synthetic non-face inputs fail this gate. Six reduced-mode update checks
(46/29 active states) agree within 4.21e-6. Next: output conversion/postprocessing, alternate and additional model branches,
temporal state, camera geometry, USB frames
and public head-pose conversion. Original Windows neural inference parity and
NixOS/Wine end-to-end operation remain unverified.

### Latest D1 crop and native landmark component

- Added reference normalization, similarity fit, inverse point transform,
  affine composition, bilinear grayscale warp and face-box/roll crop preparation.
- Original-code comparisons: four normalization cases (max error 3.82e-6),
  12 transforms (max error 2.98e-8), 12 byte-exact warps, and six complete
  preprocessing paths with byte-exact crops (transform max error 9.54e-7).
- `D1LandmarkModel` runs supplied grayscale frame + face box → crop → OpenVINO
  inference → 57 image landmarks and quality values on Linux CPU. Nine synthetic
  integration cases pass. Original Windows neural inference remains untested;
  face detection, temporal roll estimation, 3D pose and USB frames remain open.
- Reports: `evidence/private/d1-{geometry,transform,warp,crop}-oracle.json` and
  `models/tobii-et5/d1-pipeline-validation.json`. Reproduce with corresponding
  `tools/d1_*_oracle.py` scripts and `tools/check_d1_pipeline.py`.
- Full unit suite: **46 passed** before the latest crop integration additions.
  Optional inference tests and private-asset tests skip when absent from the
  minimal package. This does not establish a successful Nix build.
- Detailed progression and evidence are in [face assets](face-assets.md).

### Latest D1 consumer checkpoint

- Traced the D1 session wrapper from dependency object `+0x38` through
  `0x140048fc0`, `0x140049450`, `0x1400557a0`, and `0x14007dc50`.
- Confirmed first 114 outputs are 57 XY **offset pairs**: multiply by 96×0.5,
  add normalized reference shape, then inverse-transform into image coordinates.
  Last 57 values are copied to per-landmark quality fields by `0x14007d910`.
- Added `d1.py` output interpretation and contiguous crop tensor preparation.
  Caller must supply normalized reference shape; raw `init_shape.bin` is not
  interchangeable. Crop/roll warp and pose remain incomplete.
- Identified OpenVINORunner via RTTI/vtable. Its uint8 input helper multiplies by
  float32 1/255 with no mean subtraction. Three 96×96 input conversion cases
  match original instructions byte-for-byte, no stubs.
- Full unit suite: **42 tests passed**. Independent oracle:
  `PYTHONPATH=.local/python:.local/inference:src python3 tools/d1_input_oracle.py`.
- D1 focused Ghidra run completed. Exact addresses, evidence and next transform
  helper targets are recorded in [face assets](face-assets.md).

### Latest complete LBF component checkpoint

- Added `lbf_predictor.py`: stage feature gathering, sparse int8 projection,
  tanh/dense matrices and all four landmark-update stages. Native Linux runs
  with explicitly supplied image buffers, initial landmarks and face scale.
- Gathering: 16 original-code cases match byte-for-byte. Matrix prediction:
  16 cases pass numerical comparison; maximum absolute error `8.94e-8`.
- Complete four-stage refinement: four original-code comparisons pass, maximum
  coordinate error `1.53e-5`. This is numerical agreement on synthetic data,
  not bit parity, real-image accuracy, pose reconstruction or live tracking.
- Reproduce with `tools/lbf_gather_oracle.py` and `lbf_predictor_oracle.py` using
  `.local/python:.local/inference:src` on PYTHONPATH. Original matrix/loop math
  runs unchanged; only allocation/free/memset are stubbed for those checks.
- `tools/check_lbf_models.py` produces twelve native synthetic checks and
  `models/tobii-et5/lbf-validation.json`; use `.local/inference:src` on PYTHONPATH.
- Full unit suite: **39 tests passed**. Independent oracle reports above cover
  the subsequent predictor implementation; they are supplemental emulation
  evidence, not REA Evidence IDs. Focused Ghidra predictor run is complete.
- Next: actual image initialization, landmark crop/scale, D1 consumer mapping,
  LV result use, camera model and pose solver. No NixOS test machine exists.

### Latest LV, quantization and tree continuation

- `LBF/lv` now parses exactly through EOF: 13 records, 650 trees, 13 compressed
  6400×1 matrices. Loader `0x1400aadc0`; exports in `decoded-lbf/lv/`.
- All three supporting LBF layouts use ordinary reads and codebook expansion.
- Implemented/exported first-matrix int8 conversion and float scales for fa/fc.
  Expanded coefficient oracle passes **25** byte-exact comparisons, covering
  all 13 LV matrices and selected float/int8 fa/fc and synthetic cases.
- Implemented zero-threshold pixel-pair tree traversal in `lbf_features.py`.
  **18 original-code comparisons pass**, without import stubs. Image-buffer
  axes are explicit; no camera/crop mapping is assumed.
- `fitting-focused.c/.log` completed. Following feature gathering at
  `0x1400debc0`, matrix prediction at `0x1400dd410`, and shape reduction helper
  `0x1400d2af0` in `predictor-focused.c/.log` (in progress).
- Reproduction, exact evidence paths, remaining parity limits and the superseded
  LV-unknown checkpoint are in [face assets](face-assets.md).

### Face assets and LBF continuation

- Added validated native readers in `face_assets.py`: mesh, landmark mapping,
  identifiers and shape matrices. All 57 D1 IDs resolve to the 357-vertex mesh.
  Coordinate units and fitting behavior remain unknown.
- D1 constructor `0x1400abb00` confirms plain reads of the ID/initial-shape
  files and a normalization call at `0x1400ac560` before model creation.
  Do not use raw initial-shape coordinates as normalized runtime coordinates.
- Added `lbf.py`: complete version-4 `fa.lbf`/`fc.lbf` parsing through exact EOF.
  Their coefficients use float codebooks with packed four-bit indices, followed
  by ordinary float matrices.
- Expanded coefficients are saved in `models/tobii-et5/decoded-lbf/{fa,fc}/`
  with hashes and shapes. The original forest payloads are retained in support
  files; their evaluator is not implemented. `LBF/lv` is explicitly unsupported.
- `PYTHONPATH=src python3 tools/check_face_assets.py` reproduces the exports and
  `models/tobii-et5/face-assets-validation.json`.
- `PYTHONPATH=.local/python:src python3 tools/lbf_coeff_oracle.py` passed **six
  byte-exact original-code comparisons** for the float-output unpacking branch.
  This uses explicit libc/math stubs and does not establish int8/predictor parity.
  Evidence: `evidence/private/lbf-coeff-oracle.json`.
- Full suite now **35 tests passed**, including malformed asset rejection and
  installed-model layout checks. Log: `evidence/private/tests-latest.log`.
- Focused supplemental Ghidra run completed: `pipeline-focused.c/.log` under
  `evidence/private/`; retained disassembly covers tree, stage and coefficient
  readers. See [face assets](face-assets.md) for exact addresses and limitations.
- Next independent steps: trace `LBF/lv`'s own loader; recover tree evaluation
  and first-matrix int8 quantization; trace D1 normalization/run/output consumers
  and the pose solver. No complete Linux tracking or Wine service exists yet.

### Neural model inference checkpoint

- All **four** discovered model pairs are in
  `models/tobii-et5/{alignpre,fr,ff,d1}/`. Each has XML, weights and a manifest.
  All XML parses and every maximum weight extent equals its unpadded binary
  length. `models/tobii-et5/catalog.json` records all input/output hashes,
  tensor shapes, graph operations and supporting files.
- All 14 additional installed dependency files are copied under
  `models/tobii-et5/support/`. Text mesh/configurations and numeric shape files
  are readable. LBF source data is copied unchanged; subsequent `fa`/`fc` decoding is
  described above. `lv` and tree semantics remain unresolved.
- Original remote directory listing is retained as
  `evidence/private/model-files.txt`.
- Remote PowerShell inventory and `certutil -hashfile` returned access denied
  during this pass. Legacy SCP succeeded. New model hashes are computed from
  local copies; do not claim an independent remote hash match for those files.
- Current new implementation: `src/tobii_linux/legacy_ir.py`, an explicit
  IR4/IR6-to-OpenVINO graph loader. **All four graphs compile and execute on
  Linux CPU; 12 synthetic cases passed.** It verifies model hashes, blob bounds
  and tensor shapes; it does not implement camera normalization or head pose.
- Optional inference dependencies were installed successfully locally with
  `python3 -m pip install --target .local/inference openvino==2026.4.1`.
  Versions: OpenVINO 2026.4.1, NumPy 2.4.6. Installation log:
  `evidence/private/inference-install.log`.
- Repeatable check:
  `PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/check_models.py`.
  Results: `models/tobii-et5/inference-validation.json`. AlignPre matches an
  independent NumPy implementation on three inputs (maximum absolute error
  about `9.84e-7`). Other graphs have shape/finite-output checks only. This does
  not prove Tobii numerical parity or real tracking.
- At the earlier neural-model checkpoint, **30 tests passed**. Latest
  log is `evidence/private/tests-latest.log`; documentation links also resolve.
- Next concrete work: recover image preprocessing and map graph outputs into
  landmarks and pose fitting. Follow the exact LBF/shape-loader addresses in
  [the Linux inference checkpoint](linux-inference.md), or the earlier device
  image callback boundary. Keep new evidence type and parity limits explicit.

The sections below retain the earlier baseline; this checkpoint supersedes
their one-model-only scope and will be updated as new checks finish.

## Objective and user constraints

- Build a native Linux replacement for Tobii Eye Tracker 5 with eventual 1:1
  behavior, targeting NixOS and Star Citizen under Wine.
- Use REA and Tobii's shipped artifacts. The user explicitly requested **not
  looking into other projects**; do not use other ET5 implementations as research
  or implementation sources.
- Windows reference host: SSH hostname `albedo`. The tracker is connected there.
- Star Citizen installation: `F:\StarCitizen`; analyzed game DLL:
  `F:\StarCitizen\LIVE\Bin64\tobii_gameintegration_x64.dll`.
- No NixOS gaming machine currently exists. There is no Linux game installation
  or Wine prefix to test against yet.

## Current result

This is a partial reconstruction, **not a working tracking driver or a verified
1:1 replacement**. The Windows reference path works. Linux USB plumbing and
protocol codecs exist, but startup, live tracking and Wine integration remain
unfinished.

| Area | Established | Still missing |
| --- | --- | --- |
| Game head pose | PRP stream 2 carries timestamp, validity and plain float fields; original encoder/decoder checked offline | Full PRP service, discovery/session behavior and Wine/game validation |
| Host image processing | Inspected callbacks pass pixel buffers unchanged into image deployment | Earlier device decoding and whether any upstream payload is encrypted |
| USB | ET5 `2104:0313`; interface 0 uses WinUSB on Windows; framing and channel control reconstructed | Complete startup/subscription sequence and live Linux device validation |
| Gaze | Windows probe received valid samples; game-side codec reconstructed | Native device decoding, calibration, tracking-loss behavior |
| NixOS | Flake/package and USB access module prepared | Nix evaluation/build and real NixOS hardware testing |

## Implementation and checks already completed

| Files | Purpose |
| --- | --- |
| `src/tobii_linux/usb.py` | libusb descriptor probe, bounded bulk/control transfers and resource cleanup |
| `src/tobii_linux/usb_framing.py`, `usb_channel.py` | Direction-specific framing, incremental receive parsing, explicit channel open/close |
| `src/tobii_linux/ttp.py` | Inner TTP framing, typed uint32 arrays and protocol-upgrade request |
| `src/tobii_linux/prp.py` | Game-facing envelope, gaze/head records and non-compound stream packets |
| `flake.nix`, `nix/module.nix` | Nix packaging and NixOS USB access; no tracking daemon |
| `tools/windows_stream_probe.py` | Bounded Windows reference sample probe |
| `tools/*oracle.py` | Offline checks against selected original machine-code routines |

Last verification:

- **27 unit tests passed** with `PYTHONPATH=src python3 -m unittest discover -s tests -v`.
- Python wheel built successfully using `pip wheel --no-deps --no-build-isolation`.
- Local libusb probe ran successfully and found no attached ET5.
- Original TTP encoder matched three synthetic cases byte for byte.
- Original USB framing functions matched ten synthetic cases. I/O and mutex
  stubs return chosen values; these are not real USB success observations.
- Original game serializer matched six synthetic samples; original decoder
  accepted all six, including two head-pose samples.
- Windows Stream Engine probe received **99 gaze and 92 head-pose samples in
  three seconds**, then unsubscribed and destroyed handles successfully.
- Nix, live Linux tracking and Star Citizen under Wine have **not** been tested.

## Evidence index

- [Inner TTP and device identity](usb-protocol.md)
- [Outer USB framing, controls and image boundary](usb-transport-static.md)
- [USB capture topology and access limitations](usb-capture-topology.md)
- [Managed API, ABI and Windows sample observations](managed-api.md)
- [Star Citizen PRP format and REA evidence ledger](game-interface.md)

The detailed reports retain source hashes, addresses and actual REA Evidence IDs
where available. Static analysis, offline emulation and observed Windows runtime
behavior are separate evidence types. Full platform REA analysis timed out
twice; focused Ghidra `-noanalysis` decompilation succeeded and is labeled
supplemental. The game REA evidence bundle was saved and native REA sessions
were closed.

Private evidence lives in `evidence/private/`. Scoped research dependencies are
under `.local/`. Initial SFTP downloads truncated large binaries at 204800 bytes;
replacement copies used legacy `scp -O -T` and verified hashes. Do not reuse
truncated copies or apply address offsets to an unverified binary version.

## Resume here

1. Trace the earlier device-decoder-to-image-callback path to determine whether
   any actual USB input is encrypted. Do not infer wire encryption from service licensing errors or unrelated crypto imports.
2. Establish which models run for ET5 head pose and reconstruct their
   preprocessing, inference inputs/outputs, pose fitting and coordinate units.
3. Complete and validate normal USB startup, TTP negotiation, stream selection
   and timestamps. Keep the firmware-download path separate from tracking.
4. Compare Linux gaze/head output with bounded Windows reference observations,
   including calibration, tracking loss and reconnect behavior.
5. Implement the game-facing PRP service and Wine connection path, then test on
   a real NixOS machine with Star Citizen when one becomes available.

The existing Windows descriptor probe failed at `CreateFileW` with access denied;
USBPcap enumeration produced no useful capture. No Windows Tobii service was
stopped, no USB device was rebound, and no firmware write was performed. There
is currently no verified live USB capture to use as a wire-protocol oracle.
