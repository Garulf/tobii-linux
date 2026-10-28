# Tobii model assets

The head-pose pipeline runs Tobii's head-tracking models: four OpenVINO model
pairs (alignpre, fr, ff, d1) and the face assets the Eye Tracker 5 runtime uses
with them. They are Tobii's files. This project does not distribute them and
does not produce them.

To run the head-pose pipeline, supply the models yourself as a
`models/tobii-et5` layout:

```
tobii-et5/
  alignpre/   model.xml, model.bin, manifest.json
  fr/         model.xml, model.bin, manifest.json
  ff/         model.xml, model.bin, manifest.json
  d1/         model.xml, model.bin, manifest.json
  support/    configurations, face mesh, landmark mappings, shapes, LBF data
  catalog.json
```

The command line and the Python package look for it in this order:

1. the directory in `TOBII_LINUX_MODELS`;
2. `models/tobii-et5` in this checkout (ignored by Git), which the tests and
   tools also read;
3. `~/.local/share/tobii-linux/models` (`$XDG_DATA_HOME/tobii-linux/models`).

On NixOS, `hardware.tobii-eye-tracker-5.headPoseService.models` sets
`TOBII_LINUX_MODELS` for the head-pose service.

| Directory | Network | Input shape | Output shape |
| --- | --- | --- | --- |
| alignpre | AlignPre, IR 4 | 1 × 1 × 32 × 32 | 1 × 9 |
| fr | fr.vino, IR 6 | 1 × 3 × 128 × 128 | 1 × 256 |
| ff | model_256x192, IR 6 | 1 × 1 × 192 × 256 | 25380 |
| d1 | d1, IR 6 | 1 × 1 × 96 × 96 | 1 × 171 |

`catalog.json` lists the hashes of the model files and the 14 supporting
assets under `support/`. `crates/tobii-models/src/digests.rs` holds the SHA-256
of every file the pipeline was validated with; the tests compare a local
layout against it and skip what needs models when there are none.
`tools/check_face_assets.py` writes the expanded LBF coefficients to
`decoded-lbf/` for inspection. See [face asset findings](../docs/face-assets.md).
