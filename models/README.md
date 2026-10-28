# Tobii model assets

The head-pose pipeline needs the four OpenVINO model pairs (alignpre, fr, ff,
d1) and the supporting face assets that Tobii's Eye Tracker 5 runtime embeds.
They are not part of this repository.

Put a copy in `models/tobii-et5` (ignored by git) for the tests and tools that
read them from the tree.

| Directory | Network | Input shape | Output shape |
| --- | --- | --- | --- |
| alignpre | AlignPre, IR 4 | 1 × 1 × 32 × 32 | 1 × 9 |
| fr | fr.vino, IR 6 | 1 × 3 × 128 × 128 | 1 × 256 |
| ff | model_256x192, IR 6 | 1 × 1 × 192 × 256 | 25380 |
| d1 | d1, IR 6 | 1 × 1 × 96 × 96 | 1 × 171 |

`catalog.json` lists source and output hashes and the 14 supporting assets
under `support/`: configurations, the face mesh and landmark mappings, initial
shapes and LBF data.
