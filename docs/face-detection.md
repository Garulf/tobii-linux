# Face detection

The tracker locates faces with the `ff` network before the D1
landmark crop. Addresses refer to runtime SHA-256
`0f13b1f1cfa4ca9bb6f02580e7c3780eb455057ab210fcac186ac61fe82dc469`; evidence is
focused decompilation (`evidence/private/facedet*-focused.c`) and emulation.

## Call site

`VisionSDK::VNNTrackerCore` process `0x140049450` calls the detector
`0x14007be00` when its state is 0 (searching), with sensitivity 0.75, minimum
face 0.15 and maximum face 1.0 of the shorter frame side (`.data` defaults at
`0x1412bcf6c..0x1412bcf74`) and an optional region rectangle. It uses only the
first returned box, as an integer `(x, y, side, side)` rectangle.

The installed `Face Detector.cfg` declares `face_detector_sensitivity 0.6`;
whether it overrides the 0.75 default at runtime is not established.

## Algorithm (`0x14007be00`)

1. Letterbox the 8-bit frame into a zeroed 256x192 image. If width/height is
   at most float32 4/3, scale = height/192 and the image is centred
   horizontally with width `round(192 / (height/width))`; otherwise scale =
   width/256 and it is centred vertically. Rounding is round-half-even.
2. Resize with `0x1400b4940` mode 1: bilinear with 11-bit fixed-point taps,
   pixel-centre alignment, edge clamping and `(sum + 2^21) >> 22`. The scale is
   `1 / (dst / src)` in double, the source position is rounded to float32
   before splitting into index and fraction, and the first tap is
   `rint((1 - fraction) * 2048)` in float32. Using `src / dst` or double
   fractions gives sparse off-by-one pixels.
3. Run `ff` (input 1x1x192x256, the runner's 1/255 scaling, output 25,380
   floats = 2,820 anchors x 9).
4. Anchors (`0x14007b260`): strides 8, 16, 32, 64 with sizes (10, 16, 24),
   (32, 48), (64, 96), (128, 192, 256); feature maps `ceil(192/s) x ceil(256/s)`;
   order level, row, column, size; centre `(j+0.5)*s/256`, `(i+0.5)*s/192`.
5. Decode (`0x14007bab0`): output blocks are offsets (4 per anchor), class
   pairs (2) and three extra values. Keep anchors whose second class value
   exceeds `max(0.02, min(1, 1 - sensitivity))` (0.25 by default). Centre
   variance 0.1, size variance 0.2, `exp` for sizes, boxes clipped to the image.
   The extra values become `v0*pi - pi/2`, `pi/2 - v1*pi`, `v2*pi - pi/2`; their
   meaning is not established (they look like coarse head angles).
6. Sort by score, greedy suppression (`0x14007af20`) at overlap 0.4 with
   inclusive `+1` pixel extents, then sort by width descending.
7. Map back to frame pixels: subtract the letterbox offset, take the rounded
   centre and a half side of `round(((h + w) * scale) / 4)`, producing a square
   box. Keep it when `0.15 * min(W, H) < side < 1.0 * min(W, H)` and, if a region
   is given, its rounded centre lies strictly inside the region.

## Implementation and checks

`src/tobii_linux/face_detect.py` implements all steps (`detect_faces`,
`FaceFinder` for the model). `tools/face_detect_oracle.py` runs the
original `0x14007be00` end to end with the network call hooked to return
synthetic outputs, and also captures the letterboxed network input. Four cases
(640x480, 320x180, 120x160 portrait, and a region filter; 30 detections in
total) are byte-exact for both the network input image and every output box.
Report: `evidence/private/face-detect-oracle.json`.

```sh
PYTHONPATH=.local/python:.local/inference:src python3 tools/face_detect_oracle.py
```

Limits: the network's own numerical parity with the Windows runtime
is not tested; the original uses unstable sorts, so exact score or width ties
may be ordered differently; the meaning of the three extra outputs and the
tracking-state transitions around the detector are documented separately as
they are recovered.
