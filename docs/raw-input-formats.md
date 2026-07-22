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
- `gbrp8` for 8-bit planar RGB-family input in green, blue, red plane order

For bit depths above 8, raw samples are currently little-endian 16-bit words
with the meaningful sample value stored in the low bits. Big-endian sample
spelling is rejected until a native reader is added for it.

Short aliases remain accepted:

- `yuv420p`, `yuv422p`, and `yuv444p` normalize to 8-bit.
- `i420`, `i422`, and `i444` normalize to 8-bit planar YUV.
- Hardware-style aliases such as `i010`, `i212`, and `i416` map to the matching
  planar YUV layout and numeric bit depth.

`gbrp8` is accepted as 8-bit planar RGB-family input for AV2 and VVC plumbing:
the byte layout is one full-resolution green plane, then blue, then red. AV2
signals sRGB identity color metadata for `gbrp8` and keeps the internal
reconstruction in the same planar layout. VVC accepts the same planar bytes
through its 4:4:4 component interface and signals sRGB-compatible VUI metadata.

`rgb24` is accepted as packed 8-bit RGB for AV2 and VVC streams through the
shared driver conversion path. The driver repacks RGB into codec-native planar
GBR identity planes before encoding and packs reconstruction bytes back to
`rgb24` for `--recon`; it does not convert RGB to YUV.

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

The shared helper `convert_frame_format` is the general raw-frame bridge used
by CLI plumbing. It supports exact packed `rgb24` to planar `gbrp8` repacking,
the reverse reconstruction repack, and planar bit-depth conversion. Bit-depth
conversion still changes only sample depth: it does not change chroma sampling
or convert RGB to YUV. Scaling maps the full source range to the full target
range, so 10-bit max maps to 8-bit max when current 8-bit codec paths are used.

```rust
let source = PixelFormat::yuv444(12).expect("valid bit depth");
let target = PixelFormat::yuv444(8).expect("valid bit depth");
let converted = convert_frame_format(&frame, width, height, source, target)?;
```

## Codec Fallback

The CLI keeps the declared source format separate from the format passed to the
selected codec. If a codec does not yet accept the exact source bit depth but
does accept the same planar layout at 8-bit, the CLI streams frames through the
shared frame-format converter before calling the codec. The same converter also
handles reversible packed `rgb24` to planar `gbrp8` repacking for codecs whose
native RGB path is planar.

Current behavior:

- AV2 accepts `yuv420p8`/`yuv420p10le`, `yuv422p8`/`yuv422p10le`, and
  `yuv444p8`/`yuv444p10le` natively. `--set lossless` uses the stream-exact
  paths. `--qp <1..255>` uses the experimental lossy planar residual path for
  4:2:0, 4:2:2, and 4:4:4, with per-transform-block decisions that can still
  emit exact residuals when that is cheaper. Higher AV2 depths are scaled to
  the matching 8-bit format before non-lossless encoding until a
  reference-valid 12-bit profile path is added.
- AV2 accepts `gbrp8` by coding the three RGB components as 8-bit 4:4:4
  identity planes, signaling sRGB identity color metadata, and writing planar
  `gbrp8` reconstruction bytes. AV2 also accepts legacy packed `rgb24` by
  re-packing it to the same identity planes before encoding and re-packing the
  reconstruction back to `rgb24`. `--set lossless` keeps the RGB byte stream
  exact; `--qp <1..255>` uses the same identity-plane interpretation with the
  experimental lossy residual path.
- VVC accepts `yuv420p8` through `yuv420p12le` natively for the current 4:2:0
  residual path. Higher 4:2:0 depths are scaled to `yuv420p8` before encoding.
- VVC accepts `yuv444p8` through `yuv444p12le` natively for the current 4:4:4
  palette path. Higher 4:4:4 depths are scaled to `yuv444p8` before encoding.
  Palette entries carry native samples; high-depth escape-coded samples follow
  VTM palette escape level scaling, which is exact for zero-padded 8-bit
  upconverts but can quantize arbitrary high-depth escape samples.
- VVC accepts `yuv422p8` through `yuv422p12le` natively for both stream-exact
  lossless 4:2:2 encoding and the current non-lossless residual path.
- VVC accepts `gbrp8` through the same 8-bit 4:4:4 component pipeline used by
  planar 4:4:4 input and signals sRGB-compatible VUI metadata. VVC also
  accepts legacy packed `rgb24` through the shared lossless repack to `gbrp8`.
- Unsupported chroma or color-family conversions still fail visibly. The
  fallback does not turn 4:2:2 into 4:2:0, RGB into YUV, or gray into YUV.

`--set lossless` is stricter than native input acceptance. A codec path may
accept a format for lossy encoding while still rejecting lossless mode until the
emitted stream reconstructs exactly through the reference decoder. Lossless
mode never uses the 8-bit fallback converter; unsupported exact source formats
fail before encoding. The current lossless stream paths are AV2 `yuv420p8` and
`yuv420p10le`, AV2 `yuv422p8` and `yuv422p10le`, AV2 `yuv444p8` and
`yuv444p10le`, AV2 `gbrp8` and `rgb24`, VVC `yuv420p8` through
`yuv420p12le`, VVC `yuv422p8` through `yuv422p12le`, VVC `yuv444p8` through
`yuv444p12le`, and VVC `gbrp8` and `rgb24`.

When a codec grows true support for a higher bit depth, its accepted-format
check should be updated so the exact source format is passed through without
scaling.
