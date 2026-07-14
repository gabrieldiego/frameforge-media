# Raw Input Formats

FrameForge raw input metadata uses explicit pixel-format names. This document
covers both the `ff` CLI spelling and the Rust API shape used internally by the
codec pipeline.

## CLI Interface

The CLI accepts the compact `WxH:pixfmt` form:

```sh
ff encode input.yuv --video 1920x1080:yuv420p10le --encode av2:out.obu
```

Y4M inputs can be passed directly. The shared input reader parses the Y4M
stream header and `FRAME` markers, then presents contiguous planar frame bytes
to the selected codec:

```sh
ff encode input.y4m --encode av2:out.obu
```

Filename metadata may also provide the format:

```text
clip_1920x1080_30_1f_yuv444p12le.yuv
```

If a `.yuv` filename has dimensions but no pixel-format token, the CLI defaults
to `yuv420p8`.

Explicit `--video`, `--fps`, and `--frames` options override file metadata.
When no explicit `--video` is provided for a Y4M input, the Y4M header supplies
width, height, and pixel format; that header takes precedence over filename
metadata because it describes the container payload. When `--frames` is omitted,
raw file inputs infer frame count from file size, while Y4M inputs scan `FRAME`
markers and complete payloads.

Supported Y4M chroma tags currently map to planar YUV formats:

- `C420`, `C420jpeg`, `C420mpeg2`, and `C420paldv` map to `yuv420p8`.
- `C422` maps to `yuv422p8`.
- `C444` maps to `yuv444p8`.
- `C420pN`, `C422pN`, and `C444pN` map to little-endian planar YUV at numeric
  bit depths `N` from 8 through 16.

## Native Format Families

The current native raw format model is intentionally small and checked in Rust.
It is not an FFmpeg pixel-format mirror.

Supported planar input families:

- `yuv420p8` through `yuv420p16le`
- `yuv422p8` through `yuv422p16le`
- `yuv444p8` through `yuv444p16le`
- `gray8` through `gray16le`

For bit depths above 8, raw samples are currently little-endian 16-bit words
with the meaningful sample value stored in the low bits. Big-endian sample
spelling is rejected until a native reader is added for it.

Short aliases remain accepted:

- `yuv420p`, `yuv422p`, and `yuv444p` normalize to 8-bit.
- `i420`, `i422`, and `i444` normalize to 8-bit planar YUV.
- Hardware-style aliases such as `i010`, `i212`, and `i416` map to the matching
  planar YUV layout and numeric bit depth.

`rgb24` is accepted as packed 8-bit RGB for AV2 streams. The encoder re-packs
RGB into codec-internal planar GBR identity planes and signals sRGB identity
color metadata in the AV2 bitstream; it does not convert RGB to YUV. The
internal reconstruction written with `--recon` is packed back to `rgb24` so
validation compares the same byte layout as the source. VVC does not yet accept
RGB input.

## Rust API

`frameforge-core` represents bit depth as checked numeric data:

```rust
SampleBitDepth::new(10)
PixelFormat::yuv420(10)
PixelFormat::yuv444(12)
PixelFormat::gray(16)
```

These constructors return `Option` and reject depths outside 8 through 16:

```rust
let format = PixelFormat::yuv420(10).expect("valid bit depth");
let info = FrameInfo::new(1920, 1080, format)?;
let bytes_per_frame = info.expected_len();
```

Named 8-bit constants such as `PixelFormat::Yuv420p8` are compatibility shims
and are marked in code with a TODO to deprecate them in favor of numeric
constructors. Higher bit depths should use numeric constructors directly.

The shared helper `convert_planar_frame_bit_depth` changes only sample depth. It
does not change chroma sampling, color family, plane order, or packed layout.
Scaling maps the full source range to the full target range, so 10-bit max maps
to 8-bit max when current 8-bit codec paths are used.

```rust
let source = PixelFormat::yuv444(12).expect("valid bit depth");
let target = PixelFormat::yuv444(8).expect("valid bit depth");
let converted = convert_planar_frame_bit_depth(&frame, width, height, source, target)?;
```

## Codec Fallback

The CLI keeps the declared source format separate from the format passed to the
selected codec. If a codec does not yet accept the exact source bit depth but
does accept the same planar layout at 8-bit, the CLI streams frames through the
shared bit-depth converter before calling the codec.

Current behavior:

- AV2 accepts `yuv420p8`/`yuv420p10le`, `yuv422p8`/`yuv422p10le`, and
  `yuv444p8`/`yuv444p10le` natively. `--set lossless` uses the stream-exact
  paths. `--qp <1..255>` uses the experimental lossy planar residual path for
  4:2:0, 4:2:2, and 4:4:4, with per-transform-block decisions that can still
  emit exact residuals when that is cheaper. Higher AV2 depths are scaled to
  the matching 8-bit format before non-lossless encoding until a
  reference-valid 12-bit profile path is added.
- AV2 accepts `rgb24` by coding the three RGB components as 8-bit 4:4:4
  identity planes, signaling sRGB identity color metadata, and writing packed
  `rgb24` reconstruction bytes. `--set lossless` keeps the RGB byte stream
  exact; `--qp <1..255>` uses the same identity-plane interpretation with the
  experimental lossy residual path.
- VVC accepts `yuv420p8` through `yuv420p12le` natively for the current 4:2:0
  residual path. Higher 4:2:0 depths are scaled to `yuv420p8` before encoding.
- VVC accepts `yuv444p8` through `yuv444p12le` natively for the current 4:4:4
  palette path. Higher 4:4:4 depths are scaled to `yuv444p8` before encoding.
  Palette entries carry native samples; high-depth escape-coded samples follow
  VTM palette escape level scaling, which is exact for zero-padded 8-bit
  upconverts but can quantize arbitrary high-depth escape samples.
- VVC accepts `yuv422p8` through `yuv422p12le` natively for stream-exact
  lossless 4:2:2 encoding. The current non-lossless 4:2:2 compatibility path
  still accepts only 8-bit input; higher depths are scaled to `yuv422p8`
  before non-lossless encoding until that path gains native high-depth syntax
  and reconstruction.
- Unsupported chroma or color-family conversions still fail visibly. The
  fallback does not turn 4:2:2 into 4:2:0, RGB into YUV, or gray into YUV.

`--set lossless` is stricter than native input acceptance. A codec path may
accept a format for lossy encoding while still rejecting lossless mode until the
emitted stream reconstructs exactly through the reference decoder. Lossless
mode never uses the 8-bit fallback converter; unsupported exact source formats
fail before encoding. The current lossless stream paths are AV2 `yuv420p8` and
`yuv420p10le`, AV2 `yuv422p8` and `yuv422p10le`, AV2 `yuv444p8` and
`yuv444p10le`, AV2 `rgb24`, VVC `yuv420p8` through `yuv420p12le`, VVC
`yuv422p8` through `yuv422p12le`, and VVC `yuv444p8` through `yuv444p12le`.

When a codec grows true support for a higher bit depth, its accepted-format
check should be updated so the exact source format is passed through without
scaling.
