# Architecture Notes

FrameForge is organized around a media pipeline:

```text
input -> decode -> filter -> encode -> output
```

The shared crate, `frameforge-core`, intentionally contains only stable
infrastructure:

- frame metadata and owned frame buffers;
- packet metadata and owned packet buffers;
- shared error types;
- source, decoder, filter, encoder, and sink traits.

Codec internals should remain independent until common APIs are proven by real
implementations. AV2 and VVC may share frame buffers, metrics, validation
adapters, and byte/bitstream helpers, but should not be forced into one entropy
or block-tree abstraction early.

Imported experimental AV2/VVC software models live in `frameforge-codecs`.
Those modules are allowed to keep codec-specific internal structures while they
are adapted from the hardware workspace model into a software-facing API.

Optional codecs and filters should be selected at build time using Cargo
features or separate crates. Runtime pipeline construction can still choose
which compiled stages to connect.

## CLI Contract

The `ff` CLI should remain easy to use for common work while staying explicit
enough for reproducible validation.

Initial command families:

- `ff codecs` lists known codec stages and the Cargo feature that compiles each
  one into the binary.
- `ff filters` lists known filter stages and the Cargo feature that compiles
  each one into the binary.
- `ff encode` is the path for one raw input, optional input metadata, zero or
  more filters, one encoder, and one output:
  `ff encode input.yuv --video 1920x1080:yuv444p --filter identity --encode av2:output.obu --set lossless`.
  The encode endpoint must name a codec, using `--encode codec:path`.
  Input-only options belong after the input path; output-only options belong
  after `--encode codec:path`.

Raw video metadata should use the compact `WxH:pixfmt` form, for example
`--video 1920x1080:yuv444p`, when it cannot be inferred from the raw input
filename or needs to be overridden. File names imply metadata with
`*_<WxH>[_<fps>][_<frames>f]_<pixfmt>.yuv`, for example
`clip_1920x1080_30_1f_yuv444p8.yuv`.

Prefer adding new stage-specific options behind repeated `--set key[=value]`
arguments until a setting is common enough to deserve a stable top-level flag.
Bare keys imply `true`, for example `--set lossless`. Shared settings such as
`lossless` are global and apply to any codec. Codec-specific setting catalogs
can be added later when a feature really needs codec-local control, while
unknown options should still fail early instead of silently becoming unused
metadata.
