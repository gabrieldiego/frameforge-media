# Test Vector Sets

This directory contains portable CSV manifests for deterministic raw-video
fixtures. They are generated on demand by `scripts/generate_test_vectors.py`;
generated `.yuv`, encoded streams, and logs live under
`verification/generated/` and are not committed.
External reference source/build trees live under `verification/references/`
when `make reference-setup` is used; those are local artifacts too.

Manifest format:

```text
# description=Short description.
name,width,height,frames,format,pattern,fps,lossless
black_16x16,16,16,1,yuv420p8,black,30,false
```

`fps` may be an integer, decimal, or fraction such as `30000/1001`.
`lossless` is optional and defaults to false. When true, validation passes
`--set lossless` to the encoder and compares the internal reconstruction
against the generated source bytes.
Local manifests may use `pattern=source_file` with a `path` column. Raw YUV
sources require explicit width, height, format, and frame count. Y4M source
rows may leave width, height, format, and fps empty; the generator reads those
from the Y4M header and strips the Y4M container markers when writing raw
generated fixtures. Source-file generation currently supports `yuv420p8`,
`yuv420p10le`, and `yuv444p8`.

Manifest `format` values follow the CLI raw input contract. Planar YUV and gray
formats use checked numeric bit depths from 8 through 16, such as
`yuv420p9le`, `yuv444p12le`, and `gray16le`; see
`docs/raw-input-formats.md` for the CLI and Rust API details.

Supported generated formats:

- `yuv420p8` through `yuv420p16le`
- `yuv444p8` through `yuv444p16le`

Supported patterns:

- `black`
- `checker`
- `gradient`
- `color_blocks`
- `bitdepth_canary`

`bitdepth_canary` is a high-depth smoke pattern that writes deterministic
non-zero lower bits into generated 10-bit and 12-bit samples. It is intended to
catch internal truncation, not to act as a compression-efficiency benchmark.

Generated filenames include metadata in the CLI-supported form:

```text
name_<WxH>[_<fps>]_<frames>f_<pixfmt>.yuv
```

Fractional FPS values use a slash-safe filename label such as
`30000over1001`; validation and comparison scripts pass the exact FPS from the
manifest to `ff`.

The same manifests can also drive input-free source-filter validation when the
pattern is supported by the CLI:

```sh
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SOURCE_FILTERS=1
```
