# Raw Input Formats

FrameForge raw input metadata uses explicit pixel-format names. This document
covers both the `ff` CLI spelling and the Rust API shape used internally by the
codec pipeline.

## CLI Interface

The CLI accepts the compact `WxH:pixfmt` form:

```sh
ff encode input.yuv --video 1920x1080:yuv420p10le --encode av2:out.obu
```

Filename metadata may also provide the format:

```text
clip_1920x1080_30_1f_yuv444p12le.yuv
```

If a `.yuv` filename has dimensions but no pixel-format token, the CLI defaults
to `yuv420p8`.

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

`rgb24` remains as an existing parsed format, but the current codec encode paths
do not convert RGB to YUV.

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

- AV2 accepts `yuv420p8` for the current 4:2:0 path. Higher-bit-depth 4:2:0
  inputs are scaled to `yuv420p8` before encoding.
- AV2 accepts `yuv444p8`, `yuv444p10le`, and `yuv444p12le` natively for the
  current 4:4:4 path. These exact formats are passed to the encoder without
  bit-depth conversion.
- VVC accepts 8-bit planar YUV layouts in the CLI path; higher-bit-depth planar
  YUV inputs are scaled to the same layout at 8-bit.
- Unsupported chroma or color-family conversions still fail visibly. The
  fallback does not turn 4:2:2 into 4:2:0, RGB into YUV, or gray into YUV.

When a codec grows true support for a higher bit depth, its accepted-format
check should be updated so the exact source format is passed through without
scaling.
