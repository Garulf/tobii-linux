# Tobii model assets

Each directory contains `model.xml`, `model.bin` and a provenance/hash manifest.

| Directory | Network | Input shape | Output shape |
| --- | --- | --- | --- |
| [alignpre](tobii-et5/alignpre/manifest.json) | AlignPre, IR 4 | 1 × 1 × 32 × 32 | 1 × 9 |
| [fr](tobii-et5/fr/manifest.json) | fr.vino, IR 6 | 1 × 3 × 128 × 128 | 1 × 256 |
| [ff](tobii-et5/ff/manifest.json) | model_256x192, IR 6 | 1 × 1 × 192 × 256 | 25380 |
| [d1](tobii-et5/d1/manifest.json) | d1, IR 6 | 1 × 1 × 96 × 96 | 1 × 171 |

All XML files parse, and their declared weight extents exactly match their unpadded binary lengths. Network names and shapes do not alone establish their roles in head tracking.

The [complete catalog](tobii-et5/catalog.json) includes source/output hashes and 14 supporting assets under `tobii-et5/support/`: configurations, the text face mesh and landmark mappings, numeric initial shapes, and LBF data. Supporting assets were copied unchanged. `fa.lbf` and `fc.lbf` now parse completely as plaintext metadata, tree payloads and codebook-compressed coefficients. Expanded float32 coefficients and manifests are in `tobii-et5/decoded-lbf/{fa,fc}/`; reproduce with `PYTHONPATH=src python3 tools/check_face_assets.py`. The separate `LBF/lv` layout is decoded too, with coefficients in `decoded-lbf/lv/`. First-matrix int8 weights and scales for fa/fc are also exported. Tree semantics and complete prediction remain unresolved. See [face asset findings](../docs/face-assets.md).

Source: the Tobii installation on `albedo`, `C:/ProgramData/Tobii/Tobii Platform Runtime/IS5LEYETRACKER5/dependencies`.

Full preprocessing, pose fitting and end-to-end tracking are still under reconstruction.
