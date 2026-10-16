# Rust core

The driver is moving from Python to Rust one layer at a time. During the move the
Rust crates are exposed to the existing Python package as the extension module
`tobii_linux._native`; Python modules that were ported keep their public API and
call it. The end state is one `tobii-linux` binary without Python. The research
tools in `tools/` stay Python and reach the Rust code through the same bindings,
so every emulation oracle checks the Rust implementation.

## Building

```sh
make native        # cargo build --release -p tobii-py, copied to src/tobii_linux/_native.abi3.so
make test          # make native, then the Python suite
make rust-test     # cargo test --workspace
make rust-lint     # cargo fmt --check, cargo clippy -D warnings
make oracles       # make native, then every oracle that touches the ported layers
```

`make native` must run once after a checkout and after every Rust change before
`PYTHONPATH=src` runs (tests, oracles, tools, `python3 -m tobii_linux...`). The
extension is built for the stable ABI from Python 3.10 (`abi3-py310`), so one
file serves every interpreter from 3.10 on. Cargo writes to `target/` in the
repository (gitignored); `/tmp` is too small for it.

`pip install .` builds the extension with maturin (`[tool.maturin]` in
`pyproject.toml`) and installs it with the Python package. The flake builds the
same way with `rustPlatform.cargoSetupHook` and `maturinBuildHook`, then runs
`cargo test` and the Python suite against the installed package:

```sh
~/.local/bin/nix-portable nix build -L --no-link \
  --override-input nixpkgs "path:$HOME/np-src/nixpkgs" '.#packages.x86_64-linux.default'
```

New files must be `git add`ed before a git flake build sees them.

## Crates

| Crate | Contents | Depends on |
| --- | --- | --- |
| `crates/tobii-protocol` | USB channel framing, TTP messages and items, typed values and the reply reader, request builders, stream ids, column sets, image frames, camera hardware info, gaze and presence stream decoding with the platmod records. Pure: no I/O, clocks or threads. | `hmac`, `md-5` |
| `crates/tobii-usb` | nusb backend: sysfs descriptor probe, `BulkTransport` on interface 0, `UsbChannel` (vendor open/close, framed writes, demultiplexed reads) with its continuous reader thread, the raw traffic log and its replay. | `tobii-protocol`, `nusb` |
| `crates/tobii-device` | `TrackerSession` (startup, realms, calls, hardware info, image frames), `StreamReader` and `ItemPump` (subscriptions, demultiplexing, silence and suspend checks), `DeviceClockWatch`, `ClockMapper`, `TrackerLink` with `Backoff`, reader threads (`newest`, `drained`), `usb_session`. Generic over the `Channel` trait. | `tobii-protocol`, `tobii-usb`, `rustix` |
| `crates/tobii-gaze` | Gaze configuration: calibration, persistent-file and display-area requests, setting files, stimulus and blob point records, the display-area model and moving-lines wizard, EDID hints, eye positioning, calibration profiles, monitor rules and parsers. Device procedures (backup, restore, display setup, profile switching, the calibration sequence) are `Operation`s, state machines whose caller sends each request and runs each UI event; `operation::run` drives them on a `TrackerSession`. | `tobii-protocol`, `tobii-device`, `serde_json`, `sha2` |
| `crates/tobii-service` | Game service: PRP envelopes, stream bodies and session codecs, the gaze PRP stream records, SESP framing and flatcc-shaped messages, the Wine counter clock (classifier, follower, `ServiceClock`, counter mapping), relay framing, property answers and presence cache of the PRP service, the SESP legacy position transform, the tet-tcp policy, answers and restamping. Pure apart from `ServiceClock` reading the host clocks; the asyncio servers stay in Python. | `tobii-protocol`, `tobii-device`, `rustix` |
| `crates/tobii-inference` | Tobii's networks on OpenVINO: the IR4/IR6 to IR v11 translation, the OpenVINO C API loaded at run time (`openvino`, the crate's only unsafe code), the D1 crop, tensor, decoding and landmark model, the D1 quality aggregation and gate, and the face finder (letterbox, priors, decoding, suppression). Bound as `_native.legacy_ir`, `d1`, `d1_quality` and `face_detect` from `tobii-py/src/inference/`. | `libloading`, `roxmltree`, `serde_json`, `sha2` |
| `crates/tobii-headpose` | Head-pose math rebuilt from the runtime's float32 code: the float32 inverse, face-model projection and Jacobians, mesh normalization, the installed fitting model (`load_pose_model`), the rigid, deformation and mode updates, the bounded retry policies, the temporal fitter (`TemporalPoseFitter`), LBF features, predictor and eye refinement, landmark smoothing (`LandmarkFilter`), Euler conversions, One Euro filters, the geometry stage and `HeadPoseOutput`. Plain types in and out; bound as `_native.pose_*`, `lbf_features`, `lbf_predictor`, `eye_refine`, `landmark_smoothing` and `head_pose_pipeline` from `tobii-py/src/headpose/`. | `tobii-models` |
| `crates/tobii-setup` | The guided setup as a native GTK 4 app, binary `tobii-linux-setup`: the setup flow (display setup with the moving lines and monitor moves, eye positioning, the gaze-chosen calibration, profiles, backups and restore on cancel) on a worker thread that owns the session, the `UiChannel` the window polls, the dry-run channel, the simulated tracker and user, and the display-free models of the alignment step, positioning and calibration animations and keys. The window (`ui`) is behind the `gtk` feature; nothing depends on the crate. | `tobii-gaze`, `tobii-device`, `tobii-usb`, `gtk4` (optional) |
| `crates/tobii-py` | PyO3 bindings, one submodule per Python module (`_native.ttp`, `fields`, `commands`, `frames`, `camera_info`, `gaze`, `usb`, `device`, `gaze_device`, `calibration`, `calibration_profiles`, `display_setup`, `eye_positioning`, `monitors`, `prp`, `prp_session`, `prp_service`, `qpc_clock`, `sesp`, `sesp_service`, `tet_tcp`, `tet_tcp_service`, `inference.*`, `headpose.*`; the model modules come from `src/models/`). Nothing else depends on it. | all of the above, `pyo3` |

Dependencies point down the table only. A later crate never reaches into the
bindings, and the bindings contain no protocol logic of their own: anything a
Python wrapper needs that is not a type conversion belongs in a core crate.

### Python module to Rust

| Python module | Rust | Wrapper |
| --- | --- | --- |
| `usb_framing.py` | `tobii_protocol::framing` | re-exports |
| `ttp.py` | `tobii_protocol::ttp` | re-exports (`Message`, `Field`, `Decoder` are native classes) |
| `ttp_fields.py` | `tobii_protocol::fields`, `numeric` | `Wire` enum and constants in Python |
| `ttp_commands.py` | `tobii_protocol::commands` | `Command` and `Stream` enums in Python |
| `frames.py`, `camera_info.py` | `tobii_protocol::frames`, `camera_info` | frozen dataclasses built from native tuples |
| `gaze.py` (columns, records, presence) | `tobii_protocol::gaze` | dataclasses built from native tuples |
| `usb.py`, `usb_channel.py` | `tobii_usb` | `USBError`, `ShortWriteError` and the descriptor dataclasses in Python |
| `traffic_log.py` | `tobii_usb::traffic` | re-exports |
| `driver.py` (session, clock mapper) | `tobii_device::session`, `clock` | `TrackerSession` keeps `pending_streams`, `usb_device`; head-pose glue stays |
| `prp.py`, `prp_session.py`, `sesp.py` | `tobii_service::prp`, `prp_session`, `sesp` | dataclasses built from native tuples; `sesp.Table`, `FrameDecoder`, `FlatccBuilder` are native classes |
| `gaze.py` (PRP stream records and bodies) | `tobii_service::prp_streams` | `PrpRecord` dataclass; records cross as their packed platmod bytes |
| `qpc_clock.py` | `tobii_service::qpc_clock` | `ServiceClock`, `QpcClockClassifier` subclass native classes; `QpcClockFollower` maps bridges to native keys |
| `prp_service.py`, `sesp_service.py` (messages, property answers, presence, position transform) | `tobii_service::prp_service`, `sesp_service` | asyncio servers, hubs, subscriptions and synthetic producers stay |
| `tet_tcp.py`, `tet_tcp_service.py` (policy, answers, restamp) | `tobii_service::tet_tcp` | recording proxy (threads, router, raw log) and asyncio endpoint stay |
| `streams.py` (reader, watches) | `tobii_device::streams` | `GazeAndImageStreams`, `serve_tracker` stay |
| `supervisor.py` | `tobii_device::supervisor` | `TrackerLink` holds the Python callables |
| `gaze_device.py` | `tobii_gaze::device` | dataclasses and enums in Python; `drive` sends the requests of native operations |
| `calibration.py` | `tobii_gaze::calibration`, `operation` | `Calibrator.run` drives the native sequence on Python sessions and UIs; `DryRunSession` and the reports stay |
| `calibration_profiles.py` | `tobii_gaze::profiles` | `ProfileStore` keeps the directory |
| `display_setup.py` | `tobii_gaze::display` | dataclasses built from native tuples |
| `eye_positioning.py` | `tobii_gaze::positioning` | classes around native objects, `Status` enum in Python |
| `monitors.py` | `tobii_gaze::monitors` | the xrandr and PowerShell queries stay |
| `legacy_ir.py` | `tobii_inference::legacy_ir` | `load_model` reads the translated IR with Python OpenVINO for the tools |
| `d1.py`, `d1_quality.py` | `tobii_inference::{d1, d1_quality}` | the result dataclasses; arrays cross as float32 bytes and come back as NumPy arrays |
| `face_detect.py` | `tobii_inference::face_detect` | `Letterbox`, `Detection`, the constants, and `detect_faces`, which calls the Python `run_network` between the native letterbox and decoding |
| `pose_linalg.py`, `pose_projection.py`, `pose_mesh.py`, `pose_geometry.py`, `pose_output.py` | `tobii_headpose::{linalg, projection, mesh, geometry, output}` | arrays cross as float32 bytes (`_arrays.py`); filters wrap native objects |
| `pose_fit.py`, `pose_model.py`, `pose_policy.py`, `pose_temporal.py` | `tobii_headpose::{fit, model, policy, temporal}` | `PreparedPoseModel` stays a dataclass, packed per call; `TemporalPoseFitter` wraps the native fitter and exposes its state |
| `lbf_features.py`, `lbf_predictor.py`, `eye_refine.py` | `tobii_headpose::{lbf, eye_refine}` | the `read_lbf` dicts are converted per call; `refine_eye_landmarks` keeps the caller's refine callable |
| `landmark_smoothing.py` | `tobii_headpose::smoothing` | `LandmarkRecord` stays a dataclass; least squares are solved in double-double, so coordinates may differ from the former NumPy/LAPACK path by a few float32 ulps |
| `head_pose_pipeline.py` | `tobii_headpose::pipeline` | `HeadPoseSample`, `DisplayArea` dataclasses in Python; `headpose.py` stays Python glue over D1 |

## Setup app

`crates/tobii-setup` is the guided setup of `tobii-linux setup` rebuilt in GTK 4 with
gtk4-rs; the browser page stays as it is until the binary phase switches the CLI. The flow
runs on a worker thread that owns the tracker session and drives tobii-gaze's operations
(`operation::run` keeps reading the device while a calibration screen is drawn); the
window polls a `UiChannel` every 16 ms for the screen to draw and acknowledges each one
once drawn, so no device I/O runs on the GTK main loop.

Only the `ui` module and the binary need GTK, behind the `gtk` feature. `make rust-test`,
`make rust-lint` and the pre-commit hooks build and test the rest of the crate without
GTK. With GTK the Makefile has:

```sh
make setup-app     # release build of tobii-linux-setup (prints its path)
make setup-test    # tests and clippy -D warnings with the gtk feature
make setup-run SETUP_ARGS="--simulate unmapped --simulate-monitors"
```

They call cargo directly when `pkg-config` finds gtk4, else run in `nix/setup-shell.nix`
through `nix-shell`, or through `nix-portable` with the pre-extracted nixpkgs
(`NIXPKGS=$HOME/np-src/nixpkgs`). The shell uses nixpkgs' Rust toolchain and builds in
`target/gtk`: proc macros linked against the store's glibc cannot be loaded by a host
rustc. A binary built there runs inside the same shell. The flake has a `tobii-setup`
package (buildRustPackage with gtk4 and wrapGAppsHook4).

`tobii-linux-setup --help` lists the options of `tobii-linux setup` without the browser
ones (`--host`, `--port`, `--no-browser`), plus `--windowed` and `--simulate-monitors`.
`--simulate mapped|unmapped` runs against `simulated::SimulatedTracker` (the port of
`simulated_tracker.py`, same scripted device and user) with profiles and backups in a
fresh temporary directory; `--simulate-monitors` adds a portrait monitor left of the
27 inch primary one so the monitor switcher can be tried. On a real desktop the window
moves with `fullscreen_on_monitor` to the monitor whose GDK connector matches the
xrandr output.

## Conventions

### Errors

Each crate has one error type (`ProtocolError`, `FramingError`, `tobii_usb::Error`,
`DeviceError`) built with `thiserror`, carrying the messages the Python modules
used. The bindings map them to the Python exceptions callers catch:
`ProtocolError` (a `ValueError`), `ValueError` for framing and arguments,
`RuntimeError` for closed or failed objects, `TimeoutError` for an expired write
deadline, `USBError` and `ShortWriteError` (defined in Python, registered with
`_native.register_errors`), `DeviceError` and `NoTrackerError`. An error raised
by Python code (a fake channel, a callback) travels through Rust boxed and is
raised again unchanged.

A channel error is `fatal` when the transfer or the channel state failed; a
fatal error marks the session dead, as `OSError` and `RuntimeError` did in the
Python session. Framing and argument errors are not fatal.

### Float32 exactness

Values the runtime keeps in float32 must come out bit for bit as the recovered
machine code computes them. The rules:

- Do the arithmetic in f32 when the original does, in the same order and
  grouping. Rust never contracts `a * b + c` into a fused multiply-add on its own;
  `f32::mul_add` and `f64::mul_add` are denied by `clippy.toml`. Do not use
  `fast-math` style intrinsics, reassociation, `powi`/`hypot`/`ln_1p` style
  substitutions, or SIMD reductions whose summation order differs from the original.
- Round where the original rounds, with the helpers in
  `tobii_protocol::numeric`: `round_f32` (`cvtsd2ss`, nearest even),
  `fixed16`/`fixed16_f32`, `fixed42`/`fixed42_f32` (wire fixed point, exact power
  of two scaling), `cvttss2si` (truncation with the 0x80000000 indefinite value)
  and `truncate_in_range` (truncation with a range check).
- A float32 value may be stored in an `f64` when everything later done with it is
  exact in both precisions (copies, comparisons, clamping to 0 or 1, scaling by a
  power of two). `tobii_protocol::gaze` does this so Python floats pass through
  unchanged; say so in the type's documentation.
- Integer to float conversions (`as f32`, `as f64`) round to nearest even, as
  `cvtsi2ss` does. Float to integer `as` saturates and is never the runtime's
  conversion; use `cvttss2si` or `truncate_in_range`.

### Bindings

- Value types that Python code constructs, compares, `replace()`s or keeps in
  sets stay Python dataclasses; the native side returns tuples in field order and
  the wrapper builds the dataclass. Hot wire types (`Message`, `Field`,
  `IncomingFrame`) are native classes with dataclass-like equality, hashing and repr.
- Native mutexes are locked only with the interpreter detached
  (`crate::exit::detached`), so a thread waiting for a lock never holds the GIL.
- No Rust loop waits on Python code. With a native `USBChannel` the session's
  reading loops run in Rust. With a channel written in Python (tests, fakes,
  logging transports) the loops run in the Python wrapper, which calls the step
  functions of the Rust state machines (`route`, `finish_call`,
  `ImageFrames.pending_frame/received`, `ItemPump.pending_item/received`). Clocks
  are read in Python and passed in (`ClockOffsets`); stop requests use the native
  `StopEvent`. Before Python 3.14, a daemon thread running Python code underneath
  Rust frames when the interpreter finalizes aborts the process.
- After `atexit`, daemon threads leaving native code park instead of reattaching
  (`tobii-py/src/exit.rs`).
- `log` records of the core crates go to Python `logging` under
  `tobii_linux.<module>` (`tobii-py/src/logging.rs`).

### OpenVINO

Tobii's networks are researched IR4/IR6 graphs, which current OpenVINO cannot read, and
the C API cannot build graphs. `tobii_inference::legacy_ir` therefore translates each graph
into an IR v11 document and weights buffer with exactly the opset1 operations, attributes,
constants and topology the Python opset loader built; OpenVINO reads it from memory
(`ov_core_read_model_from_memory_buffer`) and compiles it on the CPU with
`INFERENCE_PRECISION_HINT=f32` and `INFERENCE_NUM_THREADS=2`
(`ov_core_compile_model_props`). Outputs are bit-identical to the Python-built graphs.

The C library is loaded at run time with libloading, from the first of:

1. `TOBII_OPENVINO_LIBRARY`: the `libopenvino_c` file, or a directory containing it;
2. directories the caller passes: the bindings pass the `libs` directory of an installed
   `openvino` wheel (found with `importlib.util.find_spec`, without importing it), so the
   process shares one OpenVINO with Python's; Rust tests pass `.local/inference/openvino/libs`;
3. `libopenvino_c.so` through the dynamic loader's search path.

A missing library is an ImportError in Python and `Error::Unavailable` in Rust; Rust tests
that need it print why and skip. All foreign calls are in
`crates/tobii-inference/src/openvino/ffi.rs`, the one module allowed `unsafe` (the crate
denies it instead of forbidding it so that module can opt in).

### Tests

- Port the vectors of the Python tests into the crate's unit tests, including the
  captured payloads in `tests/data` (`include_str!`) and the original-code vectors
  the oracles produced.
- Device behaviour is tested against `crates/tobii-device/tests/fake_device.rs`, a
  scripted tracker built from the recovered layouts: startup and realms, stream
  demultiplexing, the stall of a tracker whose endpoint is left unread with
  streams subscribed (it then stays silent for good), and reconnection after an
  unplugged tracker.
- The Python suite and the oracles run unchanged against the bindings
  (`make test`, `make oracles`). An oracle compares the original routine with the
  Python name it imports, which is now the Rust implementation.

## Migration checklist

Phase 1 (done): workspace, protocol, USB and device layers, bindings, Python
wrappers, Nix build, Rust hooks in pre-commit.

Game service (done, `tobii-service`): codecs and service decisions in Rust; the
asyncio servers and the tet-tcp recording proxy stay in Python until the binary
phase settles how the service runs its I/O.

The next phases can run in parallel. Each owns new crates and the Python modules
listed; shared files (`Cargo.toml` members, `crates/tobii-py/src/lib.rs`, the
Makefile `ORACLES` list, this document) take one line per phase.

| Phase | New crate | Python modules it replaces | Depends on | Parallel with |
| --- | --- | --- | --- | --- |
| Game service: PRP, SESP, tet-tcp (done) | `tobii-service` | `prp.py`, `prp_session.py`, `prp_service.py`, `sesp.py`, `sesp_service.py`, `tet_tcp.py`, `tet_tcp_service.py`, `qpc_clock.py`, PRP part of `gaze.py` | `tobii-protocol::gaze`, `tobii-device` | all |
| Head-pose math (done) | `tobii-headpose` | `lbf_features.py`, `lbf_predictor.py`, `eye_refine.py`, `landmark_smoothing.py`, `pose_*.py`, `head_pose_pipeline.py` (`headpose.py` stays glue) | `tobii-models` | all |
| Face tracking and HeadPoseProcessor | (in `tobii-headpose` or the binary) | `face_tracking.py`, `headpose.py`, `HeadPoseProcessor` in `driver.py` | `tobii-inference`, `tobii-headpose`, `tobii-device::session` | after both |
| Gaze device and calibration, display setup (done) | `tobii-gaze` | `gaze_device.py`, `calibration.py`, `calibration_profiles.py`, `display_setup.py`, `eye_positioning.py`, `monitors.py` | `tobii-device` | all |
| Setup app (gtk4-rs, done; the CLI still starts the Python page until the binary phase) | `tobii-setup` | `setup/`, `calibration_page.py`, `display_setup_page.py`, `simulated_tracker.py` | `tobii-gaze` API | after `tobii-gaze` has its API |
| Inference (done) | `tobii-inference` | `legacy_ir.py`, `face_detect.py`, `d1.py`, `d1_quality.py` | none | all |
| Binary | `tobii-linux` (bin) | `cli.py`, `streams.GazeAndImageStreams`, `serve_tracker`, `demo/` | everything | last |

For each module moved:

1. Write the Rust in its crate, following the float32 rules; port the Python
   test vectors into Rust tests.
2. Add bindings in a new `crates/tobii-py/src/<module>.rs`, registered in `lib.rs`.
3. Turn the Python module into a wrapper with the same public names (grep `src`,
   `tools` and `tests` for every name it exports).
4. `make test`, `make oracles` (add the module's oracles to `ORACLES`),
   `make rust-lint`, `make rust-test`, then the Nix build.
5. Remove the wrapper once nothing in `src` imports it; tools keep using the
   bindings.
