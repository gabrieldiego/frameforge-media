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

Each row encodes 50 frames at 1920x1080:

- SceneComposition_1 4:2:0 8-bit, original Y4M, 15 fps.
- SceneComposition_1 4:2:2 8-bit, chroma-upsampled local Y4M, 15 fps.
- SceneComposition_1 4:4:4 8-bit, chroma-upsampled local Y4M, 15 fps.
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
