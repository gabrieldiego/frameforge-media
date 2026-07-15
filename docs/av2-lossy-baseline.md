# AV2 Lossy Baseline

This report records the active AV2 lossy control point before further lossy
mode-selection work. Lossless numbers are included as the guardrail: lossy
features should not disturb `--set lossless --set predictive` bitrate, and
lossless speed should remain close unless a deliberate feature tradeoff is
validated.

Baseline commit:

```text
48ce42d Add AV2 QP residual path
```

## Test Set

Local set:

```sh
local-aomctc-b2-scc-1080p-lossless-50f
```

Each row encodes 50 frames. The AOM CTC rows are 1920x1080; the RGB
screen-capture row is 2560x1440 and uses raw packed `rgb24` input:

- SceneComposition_1 4:2:0 8-bit, original Y4M, 15 fps.
- SceneComposition_1 4:2:2 8-bit, chroma-upsampled local Y4M, 15 fps.
- screen_wayland_activity_rgb 8-bit RGB, raw Wayland screen capture, 30 fps.
- MissionControlClip1 4:2:0 10-bit, original Y4M, 60 fps.
- MissionControlClip1 4:2:2 10-bit, chroma-upsampled local Y4M, 60 fps.
- MissionControlClip1 4:4:4 10-bit, chroma-upsampled local Y4M, 60 fps.

Bitrate is computed from output bytes, source fps, and 50 encoded frames.

## Lossless Control

FrameForge command shape:

```sh
./ff encode <input.y4m> --frames 50 \
  --encode av2:<output.obu> \
  --set lossless --set predictive
```

| Vector | Format | Frames | FF setting | FF size | FF Mbps | FF fps | FF PSNR |
|---|---:|---:|---|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 50 | lossless+predictive | 4.08 MiB | 10.27 | 14.46 | inf |
| SceneComposition_1_422 | yuv422p8 | 50 | lossless+predictive | 4.59 MiB | 11.56 | 12.85 | inf |
| SceneComposition_1_444 | yuv444p8 | 50 | lossless+predictive | 5.50 MiB | 13.84 | 10.66 | inf |
| MissionControlClip1_420 | yuv420p10le | 50 | lossless+predictive | 18.60 MiB | 187.19 | 7.96 | inf |
| MissionControlClip1_422 | yuv422p10le | 50 | lossless+predictive | 21.64 MiB | 217.82 | 6.93 | inf |
| MissionControlClip1_444 | yuv444p10le | 50 | lossless+predictive | 27.27 MiB | 274.53 | 5.71 | inf |
| Total | mixed | 300 | lossless+predictive | 81.68 MiB | n/a | 8.75 | inf |

Raw total bytes: 85,648,119.

## Lossy Control

FrameForge command shape:

```sh
./ff encode <input.y4m> --frames 50 \
  --encode av2:<output.obu> \
  --qp 24
```

Reference command shape: ffmpeg/libaom AV1 with the
`realtime-screen` preset used by `make compare-compression`.

PSNR is all-plane source versus decoded/reconstructed output. FPS is encode
time only; decode time used for PSNR is excluded.

| Vector | Format | Frames | FF setting | FF size | FF Mbps | FF fps | FF PSNR | Ref setting | Ref size | Ref Mbps | Ref fps | Ref PSNR | Size ratio |
|---|---:|---:|---|---:|---:|---:|---:|---|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 50 | qp=24 | 4.56 MiB | 11.48 | 3.74 | 24.21 | libaom realtime-screen | 0.34 MiB | 0.85 | 33.31 | 45.05 | 13.52x |
| SceneComposition_1_422 | yuv422p8 | 50 | qp=24 | 4.91 MiB | 12.35 | 3.10 | 25.36 | libaom realtime-screen | 0.39 MiB | 0.98 | 31.09 | 46.02 | 12.61x |
| SceneComposition_1_444 | yuv444p8 | 50 | qp=24 | 5.35 MiB | 13.48 | 2.14 | 26.95 | libaom realtime-screen | 0.42 MiB | 1.06 | 28.13 | 47.24 | 12.75x |
| MissionControlClip1_420 | yuv420p10le | 50 | qp=24 | 11.98 MiB | 120.55 | 2.48 | 25.13 | libaom realtime-screen | 0.65 MiB | 6.55 | 17.21 | 33.80 | 18.39x |
| MissionControlClip1_422 | yuv422p10le | 50 | qp=24 | 12.51 MiB | 125.92 | 2.04 | 26.17 | libaom realtime-screen | 0.70 MiB | 7.02 | 14.94 | 34.98 | 17.94x |
| MissionControlClip1_444 | yuv444p10le | 50 | qp=24 | 13.12 MiB | 132.07 | 1.63 | 27.60 | libaom realtime-screen | 0.74 MiB | 7.47 | 14.21 | 36.74 | 17.69x |
| Total | mixed | 300 | qp=24 | 52.43 MiB | n/a | 2.34 | n/a | libaom realtime-screen | 3.24 MiB | n/a | 20.47 | n/a | 16.20x |

The next lossy feature work should improve the `qp=24` size/quality tradeoff
without regressing the lossless control. The ffmpeg/libaom rows are not an AV2
reference, but they are the current practical target for bitrate and encode
speed direction.

## Active Regression Comparison

This row-oriented chart tracks the current performance guardrails after the
`High-Bit Lossless Block Reuse` checkpoint. Each vector has a
`lossless+predictive` row and a `qp=24` row so future changes can compare
lossless and lossy behavior from the same place.

| Vector | Format | Mode | FF bytes | FF size | FF fps | Quality guardrail |
|---|---:|---|---:|---:|---:|---|
| SceneComposition_1_420 | yuv420p8 | lossless+predictive | 4,279,979 | 4.08 MiB | 15.88 | exact |
| SceneComposition_1_420 | yuv420p8 | qp=24 | 2,618,257 | 2.50 MiB | 5.65 | lossy smoke stable |
| SceneComposition_1_422 | yuv422p8 | lossless+predictive | 4,817,495 | 4.59 MiB | 13.32 | exact |
| SceneComposition_1_422 | yuv422p8 | qp=24 | 2,872,543 | 2.74 MiB | 4.54 | lossy |
| screen_wayland_activity_rgb | rgb24 | lossless+predictive | 3,604,512 | 3.44 MiB | 15.43 | exact |
| screen_wayland_activity_rgb | rgb24 | qp=24 | 8,179,011 | 7.80 MiB | 2.27 | lossy |
| MissionControlClip1_420 | yuv420p10le | lossless+predictive | 19,498,834 | 18.60 MiB | 8.10 | exact |
| MissionControlClip1_420 | yuv420p10le | qp=24 | 6,349,562 | 6.06 MiB | 3.74 | lossy |
| MissionControlClip1_422 | yuv422p10le | lossless+predictive | 22,689,992 | 21.64 MiB | 7.01 | exact |
| MissionControlClip1_422 | yuv422p10le | qp=24 | 6,599,158 | 6.29 MiB | 3.11 | lossy |
| MissionControlClip1_444 | yuv444p10le | lossless+predictive | 28,597,197 | 27.27 MiB | 5.85 | exact |
| MissionControlClip1_444 | yuv444p10le | qp=24 | 6,928,645 | 6.61 MiB | 2.35 | lossy |
| Total | mixed | lossless+predictive | 83,488,009 | 79.62 MiB | 9.38 | exact |
| Total | mixed | qp=24 | 33,547,176 | 31.99 MiB | 3.24 | lossy |

## Checkpoints

### Sparse Quantized Residual Candidate

This checkpoint adds a sparse quantized 4x4 residual candidate between the
lossy DC-delta path and the exact residual fallback. The candidate is only
searched when DC-only distortion is high enough to justify the extra work, and
only low-frequency sparse coefficient shapes are used for now.

It also fixes the UV EOB-one shortcut to use the same static EOB CDF state as
the normal chroma coefficient writer. Without that, repeated chroma DC-delta
TXBs could desynchronize AVM decoding on 10-bit 4:4:4 canary streams.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: checker_420_8, canary_422_10, canary_444_10 all pass
```

Lossless guardrail versus the baseline lossless control:

| Vector | Format | FF size | FF Mbps | FF fps | Bytes delta | FPS delta |
|---|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 4.08 MiB | 10.27 | 14.82 | +1,789 | +2.5% |
| SceneComposition_1_422 | yuv422p8 | 4.59 MiB | 11.56 | 12.56 | +4,526 | -2.2% |
| SceneComposition_1_444 | yuv444p8 | 5.50 MiB | 13.84 | 10.51 | -2,264 | -1.4% |
| MissionControlClip1_420 | yuv420p10le | 18.60 MiB | 187.19 | 7.52 | -9,075 | -5.5% |
| MissionControlClip1_422 | yuv422p10le | 21.64 MiB | 217.82 | 6.83 | +1,523 | -1.5% |
| MissionControlClip1_444 | yuv444p10le | 27.27 MiB | 274.53 | 5.77 | +3,501 | +1.0% |
| Total | mixed | 81.68 MiB | n/a | 8.63 | 0 | -1.4% |

Lossy `qp=24` versus the baseline lossy control and cached ffmpeg/libaom
realtime-screen target:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta | Ref size | Ref fps | Ref PSNR | Size ratio |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 4.54 MiB | 11.43 | 3.59 | 24.21 | -22,293 | -4.1% | -0.00 | 0.34 MiB | 33.31 | 45.05 | 13.43x |
| SceneComposition_1_422 | yuv422p8 | 4.89 MiB | 12.30 | 2.93 | 25.35 | -22,637 | -5.6% | -0.01 | 0.39 MiB | 31.09 | 46.02 | 12.56x |
| SceneComposition_1_444 | yuv444p8 | 5.33 MiB | 13.43 | 2.15 | 26.95 | -20,996 | +0.5% | -0.00 | 0.42 MiB | 28.13 | 47.24 | 12.70x |
| MissionControlClip1_420 | yuv420p10le | 11.90 MiB | 119.80 | 2.44 | 25.13 | -77,914 | -1.5% | +0.00 | 0.65 MiB | 17.21 | 33.80 | 18.28x |
| MissionControlClip1_422 | yuv422p10le | 12.44 MiB | 125.18 | 2.07 | 26.17 | -76,781 | +1.5% | +0.00 | 0.70 MiB | 14.94 | 34.98 | 17.71x |
| MissionControlClip1_444 | yuv444p10le | 13.05 MiB | 131.34 | 1.59 | 27.60 | -76,015 | -2.6% | -0.00 | 0.74 MiB | 14.21 | 36.74 | 17.46x |
| Total | mixed | 52.15 MiB | n/a | 2.30 | n/a | -296,636 | -1.7% | n/a | 3.25 MiB | n/a | n/a | 16.06x |

Current 50-frame three-way comparison with the RGB screen-capture slot:

The ffmpeg/libaom row is the realtime-screen predictive baseline. FrameForge
`qp=24` is currently all-intra, so the RGB screen-capture row intentionally
shows the current missing lossy predictive path rather than an all-intra-only
comparison. For the RGB row, ffmpeg is fed raw packed `rgb24`, converted to
full-range `gbrp` with GBR matrix metadata, and scored against decoded packed
RGB output.

| Vector | Format | FF lossless size | FF lossless Mbps | FF lossless fps | FF qp24 size | FF qp24 Mbps | FF qp24 fps | FF qp24 PSNR | ffmpeg size | ffmpeg Mbps | ffmpeg fps | ffmpeg PSNR |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 4.08 MiB | 10.27 | 15.88 | 2.50 MiB | 6.28 | 5.63 | 24.27 | 0.34 MiB | 0.85 | 33.31 | 45.05 |
| SceneComposition_1_422 | yuv422p8 | 4.59 MiB | 11.56 | 13.32 | 2.74 MiB | 6.89 | 4.72 | 25.37 | 0.39 MiB | 0.98 | 31.09 | 46.02 |
| screen_wayland_activity_rgb | rgb24 | 3.44 MiB | 17.30 | 15.43 | 7.80 MiB | 39.26 | 2.27 | 23.43 | 0.41 MiB | 2.09 | 22.42 | 47.78 |
| MissionControlClip1_420 | yuv420p10le | 18.60 MiB | 187.19 | 8.10 | 6.06 MiB | 60.96 | 3.71 | 25.14 | 0.65 MiB | 6.55 | 17.21 | 33.80 |
| MissionControlClip1_422 | yuv422p10le | 21.64 MiB | 217.82 | 7.01 | 6.29 MiB | 63.35 | 3.10 | 26.17 | 0.70 MiB | 7.02 | 14.94 | 34.98 |
| MissionControlClip1_444 | yuv444p10le | 27.27 MiB | 274.53 | 5.85 | 6.61 MiB | 66.51 | 2.34 | 27.68 | 0.74 MiB | 7.47 | 14.21 | 36.74 |
| Total | mixed | 79.62 MiB | n/a | 9.38 | 31.99 MiB | n/a | 3.24 | n/a | 3.23 MiB | n/a | 19.85 | n/a |

Current first-frame three-way comparison:

This one-frame view removes the inter-frame advantage and is the key-frame
quality/bitrate diagnostic. Single-frame FPS includes process startup and is
therefore much noisier than the 50-frame chart.

| Vector | Format | FF lossless size | FF lossless Mbps | FF lossless fps | FF qp24 size | FF qp24 Mbps | FF qp24 fps | FF qp24 PSNR | ffmpeg size | ffmpeg Mbps | ffmpeg fps | ffmpeg PSNR |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 0.38 MiB | 47.86 | 3.12 | 0.04 MiB | 5.24 | 5.40 | 24.60 | 0.16 MiB | 20.55 | 3.49 | 53.25 |
| SceneComposition_1_422 | yuv422p8 | 0.43 MiB | 54.55 | 3.03 | 0.05 MiB | 5.74 | 4.47 | 25.71 | 0.18 MiB | 22.78 | 3.24 | 53.49 |
| screen_wayland_activity_rgb | rgb24 | 0.89 MiB | 224.23 | 2.38 | 0.15 MiB | 38.96 | 2.12 | 23.50 | 0.29 MiB | 74.17 | 1.87 | 51.34 |
| MissionControlClip1_420 | yuv420p10le | 1.25 MiB | 630.22 | 1.91 | 0.18 MiB | 90.83 | 3.40 | 24.60 | 0.33 MiB | 167.71 | 2.34 | 49.51 |
| MissionControlClip1_422 | yuv422p10le | 1.47 MiB | 738.02 | 1.70 | 0.19 MiB | 93.26 | 3.00 | 25.67 | 0.36 MiB | 179.68 | 2.19 | 50.23 |
| MissionControlClip1_444 | yuv444p10le | 1.86 MiB | 935.13 | 1.26 | 0.19 MiB | 96.92 | 2.23 | 27.16 | 0.39 MiB | 195.78 | 2.03 | 51.20 |
| Total | mixed | 6.28 MiB | n/a | n/a | 0.80 MiB | n/a | n/a | n/a | 1.72 MiB | n/a | n/a | n/a |

### Reused Lossy TXB Analysis

This checkpoint reuses one 4x4 source/predictor analysis for AV2 lossy TXB
mode selection and reconstruction instead of rereading the same samples for
DC-delta, exact residual, DC SSE, quantized residual, and quantized SSE.
Bitstreams, bitrate, and PSNR are unchanged on the six-vector QP24 set; total
FrameForge QP24 encode speed improves from 2.30 fps to 3.29 fps.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: checker_420_8, canary_422_10, canary_444_10 all pass
```

### Wider Sparse Quantized Residual Candidate

This checkpoint allows the QP24 sparse quantized residual candidate to use
low-frequency shapes through EOB 8 instead of EOB 4. The focused reference
probe still passes, and the six-vector set improves from 52.15 MiB to 51.67
MiB while total QP24 encode speed rises from 3.29 fps to 4.15 fps.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: checker_420_8, canary_422_10, canary_444_10 all pass
```

### Full Sparse Quantized Residual Candidate Window

This checkpoint lets the QP24 quantized residual candidate use any 4x4 EOB
position after the distortion gate has selected the block for AC testing. The
tradeoff versus the previous EOB 8 checkpoint is quality-biased: total size
rises from 51.67 MiB to 51.78 MiB and total speed moves from 4.15 fps to 4.05
fps, while the 10-bit rows gain about 0.10 dB PSNR.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: checker_420_8, canary_422_10, canary_444_10 all pass
```

### Direct Lossy Planar Sample Access

This checkpoint keeps the same lossy decisions and bitstreams but removes the
generic checked planar sample helper from the AV2 lossy inner loop. The lossy
state still validates source and reconstruction buffer lengths at construction;
per-sample reads and writes then use direct safe slice indexing.

The six-vector QP24 set is byte-identical to the previous current chart. Total
QP24 encode speed improves from 4.05 fps to 4.59 fps, while rounded PSNR is
unchanged. Lossless byte totals are unchanged; measured lossless fps remains
within run-to-run noise.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: checker_420_8, canary_422_10, canary_444_10 all pass
```

### Adaptive Lossy Tile Layout

This checkpoint moves the AV2 QP path from one 64x64 tile per superblock to
the adaptive coarse software tile layout already used by the fast lossless
subsampled path. It also routes the lossy DC-delta TXB shortcut through the
generic coefficient writers instead of hand-emitting a one-coefficient syntax
subset. The DC writer change fixes reference decode for lossy Scene crops; the
coarser tile layout fixes high-depth 1080p QP streams that previously
desynchronized AVM while reducing tile overhead.

The six-vector QP24 set improves from 51.78 MiB to 42.41 MiB. Total speed moves
from 4.59 fps to 4.52 fps, and PSNR changes stay within 0.17 dB.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: 1-frame Scene/Mission 420/422/444 8/10-bit all pass
QP24 50-frame metrics: local-aomctc-b2-scc-1080p-lossless-50f, PSNR by ffmpeg psnr filter
```

Lossy `qp=24` versus the direct-sample checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 3.34 MiB | 8.40 | 7.02 | 24.17 | -1,267,791 | +0.9% | -0.07 |
| SceneComposition_1_422 | yuv422p8 | 3.64 MiB | 9.16 | 5.81 | 25.34 | -1,314,077 | -3.3% | -0.04 |
| SceneComposition_1_444 | yuv444p8 | 4.06 MiB | 10.22 | 4.27 | 26.80 | -1,340,713 | -2.6% | -0.17 |
| MissionControlClip1_420 | yuv420p10le | 9.93 MiB | 99.98 | 4.87 | 25.22 | -1,929,844 | +1.3% | -0.02 |
| MissionControlClip1_422 | yuv422p10le | 10.45 MiB | 105.14 | 4.04 | 26.25 | -1,951,850 | -1.5% | -0.03 |
| MissionControlClip1_444 | yuv444p10le | 11.00 MiB | 110.69 | 3.07 | 27.59 | -2,015,517 | -2.8% | -0.10 |
| Total | mixed | 42.41 MiB | n/a | 4.52 | n/a | -9,819,792 | -1.6% | n/a |

### Adaptive Lossy Partition Leaves

This checkpoint lets the AV2 QP path use the shared adaptive screen-content
partition policy instead of fixed 8x8 coding leaves. Simple 64x64 luma regions
can stay merged, while detailed regions fall back toward 16x16 or 8x8 leaves.
The residual decisions remain 4x4 TXB based, so this mainly removes avoidable
partition and intra-mode syntax and reduces per-leaf writer work.

The helper and policy names were made lossless/lossy neutral; the lossless
thresholds and behavior are otherwise unchanged. The 50-frame lossless
predictive guardrail stayed byte-identical at 85,648,119 total bytes.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: 1-frame Scene/Mission 420/422/444 8/10-bit all pass
QP24 50-frame metrics: local-aomctc-b2-scc-1080p-lossless-50f, PSNR by ffmpeg psnr filter
Lossless predictive 50-frame guardrail: local-aomctc-b2-scc-1080p-lossless-50f
```

Lossy `qp=24` versus the adaptive lossy tile-layout checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 3.32 MiB | 8.37 | 8.12 | 24.22 | -13,164 | +15.7% | +0.06 |
| SceneComposition_1_422 | yuv422p8 | 3.63 MiB | 9.13 | 6.58 | 25.34 | -12,544 | +13.2% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 4.05 MiB | 10.20 | 4.63 | 26.80 | -11,800 | +8.5% | +0.00 |
| MissionControlClip1_420 | yuv420p10le | 9.92 MiB | 99.88 | 5.20 | 25.22 | -10,975 | +6.8% | +0.00 |
| MissionControlClip1_422 | yuv422p10le | 10.44 MiB | 105.05 | 4.41 | 26.25 | -10,054 | +9.2% | +0.00 |
| MissionControlClip1_444 | yuv444p10le | 10.99 MiB | 110.58 | 3.36 | 27.59 | -11,578 | +9.6% | +0.00 |
| Total | mixed | 42.35 MiB | n/a | 4.97 | n/a | -70,115 | +9.9% | n/a |

### Sampled DC/H/V Lossy Intra Mode Search

This checkpoint adds block-level DC, horizontal, and vertical intra-mode
selection to the AV2 QP path for luma and chroma. The scorer folds the three
candidate predictors into one TXB scan and samples large leaves on a fixed
grid plus the bottom and right edges. Residual coding still uses the shared
4x4 TXB path, so the feature applies to 4:2:0, 4:2:2, and 4:4:4 inputs and to
all supported bit depths.

A full per-sample DC/H/V scorer was also measured. It reduced the six-vector
QP24 set to 28.47 MiB at 3.47 fps. The sampled scorer keeps nearly all of that
bitrate gain at 28.58 MiB and improves the measured speed to 3.63 fps.

The 50-frame lossless predictive guardrail stayed byte-identical at
85,648,119 total bytes.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
QP24 reference probe: 1-frame Scene/Mission 420/422/444 8/10-bit all pass
QP24 50-frame metrics: local-aomctc-b2-scc-1080p-lossless-50f, PSNR by ffmpeg psnr filter
Lossless predictive 50-frame guardrail: local-aomctc-b2-scc-1080p-lossless-50f
```

Lossy `qp=24` versus the adaptive lossy partition-leaves checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.56 MiB | 6.44 | 5.90 | 24.28 | -803,665 | -27.3% | +0.06 |
| SceneComposition_1_422 | yuv422p8 | 2.81 MiB | 7.06 | 4.77 | 25.39 | -862,076 | -27.5% | +0.05 |
| SceneComposition_1_444 | yuv444p8 | 3.20 MiB | 8.05 | 3.17 | 26.87 | -894,288 | -31.5% | +0.07 |
| MissionControlClip1_420 | yuv420p10le | 6.40 MiB | 64.47 | 4.32 | 25.26 | -3,688,551 | -17.0% | +0.04 |
| MissionControlClip1_422 | yuv422p10le | 6.66 MiB | 67.00 | 3.40 | 26.28 | -3,963,707 | -22.9% | +0.03 |
| MissionControlClip1_444 | yuv444p10le | 6.96 MiB | 70.05 | 2.30 | 27.79 | -4,221,441 | -31.6% | +0.19 |
| Total | mixed | 28.58 MiB | n/a | 3.63 | n/a | -14,433,728 | -27.0% | n/a |

### Precomputed Lossy TXB Predictors

This checkpoint keeps the sampled DC/H/V mode decisions and resulting
bitstreams unchanged, but removes repeated selected-predictor dispatch inside
`analyze_txb()`. Each lossy 4x4 TXB now precomputes the selected DC,
horizontal, or vertical predictor once and reuses it while building the
source, predictor, and residual arrays.

The comparison target also gained `COMPRESSION_QP`, which forwards the
dedicated `./ff encode --qp` option and lets the 50-frame lossless manifest be
reused for explicit lossy QP runs without also passing `--set lossless`.

Validation:

```text
python3 -m py_compile scripts/compare_reference_compression.py
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

Lossy `qp=24` versus the sampled DC/H/V checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.56 MiB | 6.44 | 6.79 | 24.28 | 0 | +15.1% | +0.00 |
| SceneComposition_1_422 | yuv422p8 | 2.81 MiB | 7.06 | 5.55 | 25.39 | 0 | +16.4% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 3.20 MiB | 8.05 | 3.72 | 26.87 | 0 | +17.3% | +0.00 |
| MissionControlClip1_420 | yuv420p10le | 6.40 MiB | 64.47 | 4.56 | 25.26 | 0 | +5.6% | +0.00 |
| MissionControlClip1_422 | yuv422p10le | 6.66 MiB | 67.00 | 3.45 | 26.28 | 0 | +1.5% | +0.00 |
| MissionControlClip1_444 | yuv444p10le | 6.96 MiB | 70.05 | 2.46 | 27.79 | 0 | +7.0% | +0.00 |
| Total | mixed | 28.58 MiB | n/a | 3.97 | n/a | 0 | +9.4% | n/a |

### Luma Paeth Lossy Intra Candidate

This checkpoint adds Paeth as a luma intra candidate for AV2 QP mode
selection. The predictor is shared by all input chroma formats because the
selected luma mode feeds the existing common residual path. A full luma+chroma
Paeth probe was also measured, but it saved less total size than the luma-only
variant and cost more PSNR on the Mission rows, so this checkpoint keeps
chroma mode selection on DC/H/V while the chroma RD scorer is improved.

The luma-only Paeth candidate reduces the six-vector QP24 set from 29,969,076
bytes to 29,284,107 bytes. Total measured encode speed moves from 3.97 fps to
3.87 fps. Scene PSNR is effectively unchanged; Mission PSNR drops by about
0.08 to 0.09 dB while bitrate drops by about 2.3%.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

PSNR for 4:2:2 and 10-bit rows was measured with a scratch `--recon` encode
and ffmpeg's `psnr` filter, using matching raw reconstruction framerates.

Lossy `qp=24` versus the precomputed-predictor checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.51 MiB | 6.31 | 6.31 | 24.28 | -54,616 | -7.1% | +0.00 |
| SceneComposition_1_422 | yuv422p8 | 2.75 MiB | 6.93 | 5.16 | 25.39 | -54,251 | -7.0% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 3.15 MiB | 7.92 | 3.63 | 26.87 | -53,685 | -2.4% | -0.00 |
| MissionControlClip1_420 | yuv420p10le | 6.24 MiB | 62.82 | 4.26 | 25.17 | -171,518 | -6.6% | -0.09 |
| MissionControlClip1_422 | yuv422p10le | 6.48 MiB | 65.27 | 3.45 | 26.20 | -180,291 | +0.0% | -0.08 |
| MissionControlClip1_444 | yuv444p10le | 6.80 MiB | 68.42 | 2.50 | 27.70 | -170,608 | +1.6% | -0.09 |
| Total | mixed | 27.93 MiB | n/a | 3.87 | n/a | -684,969 | -2.5% | n/a |

### Adaptive Smooth Lossy Intra Candidate

This checkpoint adds AV2 smooth, smooth-vertical, and smooth-horizontal as
luma-only lossy intra candidates. Smooth is probed in a second stage: the
encoder first scores DC/H/V/Paeth, then only runs the smooth scorer on leaves
where the cheap scores look gradient-like. The smooth scorer is split from the
generic DC/H/V/Paeth scorer so selected leaves do not repeat unused work.

The six-vector QP24 set improves from 29,284,107 bytes to 28,683,216 bytes.
Total measured encode speed moves from 3.87 fps to 3.39 fps. PSNR drops by
about 0.01 to 0.03 dB, while bitrate drops by about 2.1%.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

PSNR for 4:2:2 and 10-bit rows was measured with scratch `--recon` encodes
and ffmpeg's `psnr` filter, using matching raw reconstruction framerates.

Lossy `qp=24` versus the luma Paeth checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.50 MiB | 6.30 | 5.44 | 24.27 | -4,536 | -13.8% | -0.01 |
| SceneComposition_1_422 | yuv422p8 | 2.75 MiB | 6.92 | 4.47 | 25.37 | -4,749 | -13.4% | -0.02 |
| SceneComposition_1_444 | yuv444p8 | 3.14 MiB | 7.91 | 3.19 | 26.86 | -4,401 | -12.1% | -0.01 |
| MissionControlClip1_420 | yuv420p10le | 6.05 MiB | 60.95 | 3.65 | 25.14 | -195,564 | -14.3% | -0.03 |
| MissionControlClip1_422 | yuv422p10le | 6.30 MiB | 63.38 | 2.99 | 26.17 | -196,194 | -13.3% | -0.03 |
| MissionControlClip1_444 | yuv444p10le | 6.61 MiB | 66.54 | 2.26 | 27.68 | -195,447 | -9.6% | -0.02 |
| Total | mixed | 27.35 MiB | n/a | 3.39 | n/a | -600,891 | -12.4% | n/a |

### Lossy Scoring SSE Cleanup

This checkpoint keeps AV2 QP mode decisions, bitstreams, bitrate, and PSNR
unchanged. The lossy intra scoring loops now compute each reconstruction
difference once before squaring it for SSE, instead of recomputing absolute
differences twice per predictor. The measured effect is small and within some
run-to-run noise on the Scene rows, but the 10-bit rows improve and the total
six-vector QP24 encode speed moves from 3.39 fps to 3.42 fps.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

Lossy `qp=24` versus the adaptive smooth checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.50 MiB | 6.30 | 5.38 | 24.27 | 0 | -1.1% | +0.00 |
| SceneComposition_1_422 | yuv422p8 | 2.75 MiB | 6.92 | 4.42 | 25.37 | 0 | -1.1% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 3.14 MiB | 7.91 | 3.19 | 26.86 | 0 | +0.0% | +0.00 |
| MissionControlClip1_420 | yuv420p10le | 6.05 MiB | 60.95 | 3.73 | 25.14 | 0 | +2.2% | +0.00 |
| MissionControlClip1_422 | yuv422p10le | 6.30 MiB | 63.38 | 3.03 | 26.17 | 0 | +1.3% | +0.00 |
| MissionControlClip1_444 | yuv444p10le | 6.61 MiB | 66.54 | 2.33 | 27.68 | 0 | +3.1% | +0.00 |
| Total | mixed | 27.35 MiB | n/a | 3.42 | n/a | 0 | +0.9% | n/a |

### Direct DC-Delta Proxy Score

This checkpoint keeps AV2 QP mode decisions, bitstreams, bitrate, and PSNR
unchanged. The lossy TXB chooser now scores the DC-delta candidate directly
from the quantized DC coefficient level instead of constructing a 16-entry
coefficient array and passing it through the generic coefficient proxy scorer.
The direct path uses the same position-0 low-frequency high-range rule, so the
score is equivalent for luma and chroma transform TXBs.

Total measured six-vector QP24 encode speed moves from 3.42 fps to 3.51 fps.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

Lossy `qp=24` versus the lossy scoring SSE cleanup checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.50 MiB | 6.30 | 5.57 | 24.27 | 0 | +3.5% | +0.00 |
| SceneComposition_1_422 | yuv422p8 | 2.75 MiB | 6.92 | 4.53 | 25.37 | 0 | +2.5% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 3.14 MiB | 7.91 | 3.31 | 26.86 | 0 | +3.8% | +0.00 |
| MissionControlClip1_420 | yuv420p10le | 6.05 MiB | 60.95 | 3.81 | 25.14 | 0 | +2.1% | +0.00 |
| MissionControlClip1_422 | yuv422p10le | 6.30 MiB | 63.38 | 3.11 | 26.17 | 0 | +2.6% | +0.00 |
| MissionControlClip1_444 | yuv444p10le | 6.61 MiB | 66.54 | 2.37 | 27.68 | 0 | +1.7% | +0.00 |
| Total | mixed | 27.35 MiB | n/a | 3.51 | n/a | 0 | +2.6% | n/a |

### Syntax-Aware Chroma Mode Gate

This checkpoint tightens the second-stage luma smooth probe so very low
residual leaves skip the smooth-family scorer, and adds a small syntax-cost
estimate to the lossy chroma DC/H/V mode decision. The chroma estimate uses
the actual AV2 UV mode index for the selected luma mode, so H/V only pay the
extra proxy cost when the current luma context would make that chroma mode
less compact to signal.

The six-vector QP24 set improves from 28,683,216 bytes to 28,654,482 bytes.
Total measured encode speed moves from 3.51 fps to 3.53 fps. Rounded PSNR is
unchanged at report precision.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

Lossy `qp=24` versus the direct DC-delta proxy score checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.50 MiB | 6.30 | 5.69 | 24.27 | +341 | +2.2% | +0.00 |
| SceneComposition_1_422 | yuv422p8 | 2.74 MiB | 6.91 | 4.59 | 25.37 | -4,360 | +1.3% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 3.13 MiB | 7.87 | 3.39 | 26.86 | -16,970 | +2.4% | +0.00 |
| MissionControlClip1_420 | yuv420p10le | 6.05 MiB | 60.94 | 3.78 | 25.14 | -77 | -0.8% | +0.00 |
| MissionControlClip1_422 | yuv422p10le | 6.29 MiB | 63.34 | 3.14 | 26.17 | -4,159 | +1.0% | +0.00 |
| MissionControlClip1_444 | yuv444p10le | 6.61 MiB | 66.51 | 2.33 | 27.68 | -3,509 | -1.7% | +0.00 |
| Total | mixed | 27.33 MiB | n/a | 3.53 | n/a | -28,734 | +0.6% | n/a |

### Syntax-Aware Luma H/V Penalty

This checkpoint threads the current AV2 luma mode syntax ordering into the
lossy luma mode estimator. Horizontal and vertical keep their normal small
proxy cost when they occupy the early mode slots, but pay an additional
late-index surcharge when neighbor-derived directional modes push them deeper
into the syntax list. The cached lossy mode lookup keeps the syntax computation
on the miss path for chroma and residual events, so repeated syntax stages for
the same leaf do not rescore the mode.

The six-vector QP24 set improves from 28,654,482 bytes to 28,641,539 bytes.
Total measured encode speed moves from 3.53 fps to 3.52 fps. Rounded PSNR is
unchanged at report precision.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless VALIDATION_REFERENCE_MODE=auto
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1
```

Lossy `qp=24` versus the syntax-aware chroma mode gate checkpoint:

| Vector | Format | FF size | FF Mbps | FF fps | FF PSNR | Bytes delta | FPS delta | PSNR delta |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 2.50 MiB | 6.28 | 5.63 | 24.27 | -5,304 | -1.1% | +0.00 |
| SceneComposition_1_422 | yuv422p8 | 2.74 MiB | 6.89 | 4.72 | 25.37 | -5,605 | +2.8% | +0.00 |
| SceneComposition_1_444 | yuv444p8 | 3.12 MiB | 7.86 | 3.39 | 26.86 | -5,296 | +0.0% | +0.00 |
| MissionControlClip1_420 | yuv420p10le | 6.06 MiB | 60.96 | 3.71 | 25.14 | +1,177 | -1.9% | +0.00 |
| MissionControlClip1_422 | yuv422p10le | 6.29 MiB | 63.35 | 3.10 | 26.17 | +1,051 | -1.3% | +0.00 |
| MissionControlClip1_444 | yuv444p10le | 6.61 MiB | 66.51 | 2.34 | 27.68 | +1,034 | +0.4% | +0.00 |
| Total | mixed | 27.31 MiB | n/a | 3.52 | n/a | -12,943 | -0.3% | n/a |

### Fast Lossless Reconstruction Copy

This checkpoint fixes an internal reconstruction bookkeeping bug in the fast
subsampled lossless path. Fast mode scoring treats reconstructed neighbors as
source-backed so it can avoid per-TXB reconstruction writes while choosing
modes. When a caller still provides a reconstruction buffer, the final region
copy must write the source bytes into that buffer. Without that copy, large
single-region 4:2:0 lossless key frames could leave zero-filled internal recon
data even though the bitstream payload itself was unchanged.

The fix does not gate off predictive coding. The 50-frame predictive lossless
validation still emits regular inter/show-existing-frame-sized 6-byte frames
on repeated MissionControl pictures, and all six rows reconstruct exactly.

Validation:

```text
cargo test -p frameforge-codecs --all-features
make test
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_REFERENCE_MODE=off VALIDATION_SETTINGS=predictive
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_SETTINGS=predictive COMPRESSION_DIRECT_SOURCE_FILES=1
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1 COMPRESSION_LIMIT=1
```

Lossless predictive guardrail versus the baseline lossless control:

| Vector | Format | FF bytes | FF size | FF fps | Bytes delta | FPS delta | Recon |
|---|---:|---:|---:|---:|---:|---:|---|
| SceneComposition_1_420 | yuv420p8 | 4,279,979 | 4.08 MiB | 14.91 | 0 | +3.1% | exact |
| SceneComposition_1_422 | yuv422p8 | 4,817,495 | 4.59 MiB | 12.93 | 0 | +0.6% | exact |
| SceneComposition_1_444 | yuv444p8 | 5,764,622 | 5.50 MiB | 10.67 | 0 | +0.1% | exact |
| MissionControlClip1_420 | yuv420p10le | 19,498,834 | 18.60 MiB | 7.89 | 0 | -0.9% | exact |
| MissionControlClip1_422 | yuv422p10le | 22,689,992 | 21.64 MiB | 6.87 | 0 | -0.9% | exact |
| MissionControlClip1_444 | yuv444p10le | 28,597,197 | 27.27 MiB | 5.64 | 0 | -1.2% | exact |
| Total | mixed | 85,648,119 | 81.68 MiB | 8.72 | 0 | -0.3% | exact |

The one-vector QP24 lossy smoke check on `SceneComposition_1_420` produced
2,618,257 bytes, 6.28 Mbps, and 5.63 fps against the ffmpeg/libaom
realtime-screen reference. This matches the current rounded QP24 checkpoint
for that row and does not show a lossy-path regression.

### Lossless IBC Search Work Reduction

This checkpoint reduces first-frame intra lossless work without changing the
selected coding decisions. Local IBC direct above/left candidates still win
before explicit-DV candidates, but explicit-DV search is now lazy and only runs
when no direct candidate was selected. The explicit search also uses a
tile-local sorted hash index, so it visits same-hash already-coded 8x8 blocks
instead of scanning the whole 64x64 local tile. Exact byte comparison remains
the final accept check for every candidate.

On the 200-run first-frame gprof sampling profile for
`SceneComposition_1_420`, sampled time moved from 42.34 s to 29.47 s
(-30.4%). The local IBC/equality cluster moved from about 16.77 s to 3.78 s:
`build_local_ibc_subsampled` dropped from 7.68 s to 2.78 s, and the
`regions_equal_between`/`plane_regions_equal_between` exact-compare pair
dropped from 9.09 s to 1.00 s. The first-frame lossless payload stayed
byte-identical at 387,721 bytes.

Validation:

```text
cargo test -p frameforge-codecs av2_local_ibc --all-features
cargo test -p frameforge-codecs --all-features
make profile-av2-i-lossless GPROF_SAMPLE_RUNS=200
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_LIMIT=1 VALIDATION_REFERENCE_MODE=off
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_LIMIT=1 COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_DIRECT_SOURCE_FILES=1
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_REFERENCE_MODE=off VALIDATION_SETTINGS=predictive
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_SETTINGS=predictive COMPRESSION_DIRECT_SOURCE_FILES=1
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1 COMPRESSION_LIMIT=1
```

Lossless predictive guardrail versus the previous reconstruction-copy
checkpoint:

| Vector | Format | FF bytes | FF size | FF fps | Bytes delta | FPS delta | Recon |
|---|---:|---:|---:|---:|---:|---:|---|
| SceneComposition_1_420 | yuv420p8 | 4,279,979 | 4.08 MiB | 14.87 | 0 | -0.3% | exact |
| SceneComposition_1_422 | yuv422p8 | 4,817,495 | 4.59 MiB | 13.59 | 0 | +5.1% | exact |
| SceneComposition_1_444 | yuv444p8 | 5,764,622 | 5.50 MiB | 10.57 | 0 | -0.9% | exact |
| MissionControlClip1_420 | yuv420p10le | 19,498,834 | 18.60 MiB | 7.80 | 0 | -1.1% | exact |
| MissionControlClip1_422 | yuv422p10le | 22,689,992 | 21.64 MiB | 6.90 | 0 | +0.4% | exact |
| MissionControlClip1_444 | yuv444p10le | 28,597,197 | 27.27 MiB | 5.63 | 0 | -0.2% | exact |
| Total | mixed | 85,648,119 | 81.68 MiB | 8.75 | 0 | +0.3% | exact |

The non-predictive first-vector intra lossless guardrail produced 24,498,025
bytes at 5.00 fps with exact reconstruction. The one-vector QP24 lossy smoke
check on `SceneComposition_1_420` produced 2,618,257 bytes and 5.61 fps,
matching the previous byte count and showing no lossy-path regression beyond
normal timing noise.

### Fast Lossless Score Edge Reuse

This checkpoint reduces the sampled intra lossless scoring cost without
changing mode decisions. The DC/H/V/BDPCM scorer now loads the 4x4 source
block once and reuses one left-edge and one above-edge vector for the five
candidate score accumulators. The 8-bit source block path uses direct safe row
slices after construction-time buffer validation; higher bit depths keep the
existing validated sample helper.

On the 200-run first-frame gprof sampling profile for
`SceneComposition_1_420`, sampled time moved from 29.47 s to 28.70 s (-2.6%).
The first-frame lossless payload stayed byte-identical at 387,721 bytes. The
non-predictive first-vector intra lossless guardrail improved from 5.00 fps to
5.78 fps with the same 24,498,025 bytes and exact reconstruction.

Validation:

```text
cargo test -p frameforge-codecs av2_local_ibc --all-features
cargo test -p frameforge-codecs --all-features
make profile-av2-i-lossless GPROF_SAMPLE_RUNS=200
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_REFERENCE_MODE=off VALIDATION_SETTINGS=predictive
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_SETTINGS=predictive COMPRESSION_DIRECT_SOURCE_FILES=1
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_LIMIT=1 VALIDATION_REFERENCE_MODE=off
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_LIMIT=1 COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_DIRECT_SOURCE_FILES=1
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1 COMPRESSION_LIMIT=1
make test
```

Lossless predictive guardrail versus the lossless IBC search work reduction
checkpoint:

| Vector | Format | FF bytes | FF size | FF fps | Bytes delta | FPS delta | Recon |
|---|---:|---:|---:|---:|---:|---:|---|
| SceneComposition_1_420 | yuv420p8 | 4,279,979 | 4.08 MiB | 14.37 | 0 | -3.4% | exact |
| SceneComposition_1_422 | yuv422p8 | 4,817,495 | 4.59 MiB | 13.13 | 0 | -3.4% | exact |
| SceneComposition_1_444 | yuv444p8 | 5,764,622 | 5.50 MiB | 10.73 | 0 | +1.5% | exact |
| MissionControlClip1_420 | yuv420p10le | 19,498,834 | 18.60 MiB | 8.14 | 0 | +4.4% | exact |
| MissionControlClip1_422 | yuv422p10le | 22,689,992 | 21.64 MiB | 6.98 | 0 | +1.2% | exact |
| MissionControlClip1_444 | yuv444p10le | 28,597,197 | 27.27 MiB | 5.86 | 0 | +4.1% | exact |
| Total | mixed | 85,648,119 | 81.68 MiB | 8.88 | 0 | +1.5% | exact |

The one-vector QP24 lossy smoke check on `SceneComposition_1_420` produced
2,618,257 bytes and 5.51 fps, matching the previous byte count and showing no
lossy-path regression beyond normal timing noise.

### High-Bit Lossless Block Reuse

This checkpoint generalizes the source-block reuse from the sampled scoring
path into the source-backed 4x4 residual helpers. The 4x4 source block loader
now uses direct safe row slices for both 8-bit and high-bit-depth planar input,
and the source-backed DC/H/V intra residual, DPCM residual, and luma-palette
residual paths reuse that loaded block instead of rereading the same samples
inside each residual loop. Mode decisions and syntax are unchanged.

The six first-frame I-only gprof sweep uses 100 sampled runs per row from the
`local-aomctc-b2-scc-1080p-lossless-50f` manifest. All first-frame payload
sizes stayed byte-identical.

| Vector | Format | First-frame bytes | Before sampled s | After sampled s | Delta |
|---|---:|---:|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | 387,721 | 14.47 | 14.29 | -1.2% |
| SceneComposition_1_422 | yuv422p8 | 443,864 | 16.99 | 16.47 | -3.1% |
| SceneComposition_1_444 | yuv444p8 | 535,455 | 18.51 | 18.26 | -1.4% |
| MissionControlClip1_420 | yuv420p10le | 1,305,766 | 36.32 | 34.33 | -5.5% |
| MissionControlClip1_422 | yuv422p10le | 1,530,687 | 39.92 | 39.01 | -2.3% |
| MissionControlClip1_444 | yuv444p10le | 1,940,894 | 48.99 | 46.72 | -4.6% |

The biggest localized win is the source-backed DPCM residual bucket on 10-bit
content: `MissionControlClip1_420` moved from 1.15 s to 0.20 s, and
`MissionControlClip1_422` moved from 1.96 s to 0.20 s in the 100-run profiles.
The dense 4:4:4 rows remain primarily entropy-bound, especially in chroma
coefficient and chroma BDPCM writing.

Validation:

```text
cargo test -p frameforge-codecs --all-features
six first-frame gprof profiles with GPROF_SAMPLE_RUNS=100
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_REFERENCE_MODE=off VALIDATION_SETTINGS=predictive
make validate-set CODEC=av2 VALIDATION_SET=local-aomctc-b2-scc-1080p-lossless-50f VALIDATION_LIMIT=1 VALIDATION_REFERENCE_MODE=off
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_SETTINGS=predictive COMPRESSION_DIRECT_SOURCE_FILES=1
make compare-compression CODEC=av2 COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom COMPRESSION_REFERENCE_PRESET=realtime-screen COMPRESSION_QP=24 COMPRESSION_DIRECT_SOURCE_FILES=1 COMPRESSION_LIMIT=1
make test
```

Lossless predictive guardrail versus the fast lossless score edge reuse
checkpoint:

| Vector | Format | FF bytes | FF size | FF fps | Bytes delta | FPS delta | Recon |
|---|---:|---:|---:|---:|---:|---:|---|
| SceneComposition_1_420 | yuv420p8 | 4,279,979 | 4.08 MiB | 15.88 | 0 | +10.5% | exact |
| SceneComposition_1_422 | yuv422p8 | 4,817,495 | 4.59 MiB | 13.32 | 0 | +1.4% | exact |
| SceneComposition_1_444 | yuv444p8 | 5,764,622 | 5.50 MiB | 10.82 | 0 | +0.8% | exact |
| MissionControlClip1_420 | yuv420p10le | 19,498,834 | 18.60 MiB | 8.10 | 0 | -0.5% | exact |
| MissionControlClip1_422 | yuv422p10le | 22,689,992 | 21.64 MiB | 7.01 | 0 | +0.4% | exact |
| MissionControlClip1_444 | yuv444p10le | 28,597,197 | 27.27 MiB | 5.85 | 0 | -0.2% | exact |
| Total | mixed | 85,648,119 | 81.68 MiB | 8.99 | 0 | +1.2% | exact |

The one-vector QP24 lossy smoke check on `SceneComposition_1_420` produced
2,618,257 bytes and 5.33 fps, matching the previous byte count and showing no
lossy-path regression beyond normal timing noise.
