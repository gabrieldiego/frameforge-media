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
name,width,height,frames,format,pattern,fps
black_16x16,16,16,1,yuv420p8,black,30
```

`fps` may be an integer, decimal, or fraction such as `30000/1001`.
Local manifests may use `pattern=source_file` with a `path` column. Raw YUV
sources require explicit width, height, format, and frame count. Y4M source
rows may leave width, height, format, and fps empty; the generator reads those
from the Y4M header and strips the Y4M container markers when writing raw
generated fixtures. Source-file generation currently supports `yuv420p8`,
`yuv420p10le`, and `yuv444p8`.

Supported generated formats:

- `yuv420p8`
- `yuv444p8`

Supported patterns:

- `black`
- `checker`
- `gradient`
- `color_blocks`

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
