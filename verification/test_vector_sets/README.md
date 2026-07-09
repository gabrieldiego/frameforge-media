# Test Vector Sets

This directory contains portable CSV manifests for deterministic raw-video
fixtures. They are generated on demand by `scripts/generate_test_vectors.py`;
generated `.yuv`, encoded streams, and logs live under
`verification/generated/` and are not committed.

Manifest format:

```text
# description=Short description.
name,width,height,frames,format,pattern,fps
black_16x16,16,16,1,yuv420p8,black,30
```

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

The same manifests can also drive input-free source-filter validation when the
pattern is supported by the CLI:

```sh
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SOURCE_FILTERS=1
```
