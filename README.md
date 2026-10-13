# Tobii Eye Tracker 5 on NixOS — reconstruction in progress

The target is a native Linux driver with Star Citizen support through Wine.
**This is not yet a working tracking driver or a 1:1 replacement for Tobii.**

Work is based on Tobii's installed binaries and the connected tracker on the
Windows host `albedo`, plus Star Citizen's bundled Tobii DLL. No other ET5 driver
project is used as the implementation base.

Implemented:

- Native Linux libusb descriptor probe and bounded bulk transport.
- Partial inner TTP framing and protocol-upgrade request reconstruction.
- Direction-specific USB channel framing and explicit channel open/close helpers.
- Game-facing PRP head-pose/gaze codecs checked against original machine code.
- All four neural model pairs and native Linux graph inference checks.
- Decoded LBF assets and native landmark refinement checked against original code.
- D1 face-box crop/landmark component; original crop parity and Linux synthetic inference checks.
- Native supplied-face-box D1/quality/staged-pose component, with fitting, selection and retry checks against original code.
- Nix package and NixOS USB access module.
- Windows reference probes and retained REA analysis for further reconstruction.

The Windows reference probe successfully received 99 gaze and 92 head-pose
samples in three seconds. That verifies the Windows reference path, **not** the
Linux driver. USB initialization, live stream decoding, head-pose processing,
calibration and Wine service compatibility remain unfinished.

## Inspect the tracker on Linux

With Python 3.10+ and libusb installed:

```sh
PYTHONPATH=src python3 -m tobii_linux.cli probe
PYTHONPATH=src python3 -m unittest discover -s tests -v
```

The probe lists matching device descriptors. It does not start tracking or
claim the interface. An empty device list means no ET5 is visible to libusb.

## Offline head-pose component

With the project-local NumPy/OpenVINO dependencies and model assets:

```sh
PYTHONPATH=.local/inference:src OPENBLAS_NUM_THREADS=2 python3 tools/run_headpose_frame.py frame.gray --width 640 --height 480 --face-box 100 80 200 240
```

Input is packed 8-bit grayscale, with a supplied face box. Low-confidence frames
produce no fit. Accepted results use internal fitting coordinates; face detection,
temporal tracking, USB images and conversion to the public Tobii API remain
unfinished. See [the component evidence and limits](docs/pose-fitting.md).

## NixOS

The Nix expressions are prepared but have not been evaluated or built in this
environment, which has no Nix installation or attached Linux tracker.

```sh
nix build path:.#default
nix run path:.#default -- probe
```

Add this directory as a flake input, import
`inputs.tobii-linux.nixosModules.default`, and enable:

```nix
hardware.tobii-eye-tracker-5.enable = true;
```

The module installs the probe and a udev rule granting the active local seat
user access to `2104:0313`. It does not install a tracking daemon. Package sources
exclude private binaries, captures and research tools.

## Evidence and next steps

- [Project status and handoff — scope, findings, checks and remaining work](docs/project-status.md)
- [Native Linux model inference and next pipeline steps](docs/linux-inference.md)
- [Face assets and LBF coefficient decoding](docs/face-assets.md)
- [Pose projection and iterative fitting reconstruction](docs/pose-fitting.md)
- [Head-pose output path from fitted pose to public sample](docs/head-pose-output.md)
- [USB protocol findings](docs/usb-protocol.md)
- [USB transport framing and control requests](docs/usb-transport-static.md)
- [USB capture topology and limitations](docs/usb-capture-topology.md)
- [Managed API and Windows reference observations](docs/managed-api.md)
- [Star Citizen interface findings](docs/game-interface.md)

Local `evidence/private/` records include the proprietary analysis inputs and
user-specific tracking observations. They are excluded from Git and Nix package
sources. `tools/` contains repeatable research helpers; the Windows probes use
the original Windows runtime and are not Linux backends.

Full parity needs measured startup, streaming, calibration, tracking loss,
reconnection, head-pose behavior and Star Citizen/Wine tests. No NixOS gaming
machine is available yet, so end-to-end validation remains pending.
