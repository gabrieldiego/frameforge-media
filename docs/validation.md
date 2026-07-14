# Validation Notes

FrameForge should keep validation strict and reproducible.

Expected validation layers:

- unit tests for frame, packet, syntax, and reconstruction primitives;
- integration tests using deterministic generated vectors;
- reference-decoder checks for generated bitstreams when a reference is
  available;
- checksum comparison for lossless paths;
- PSNR and bitrate reporting for lossy paths;
- benchmark and throughput reporting for performance-sensitive stages.

Do not weaken pass criteria to hide incomplete codec support. Unsupported
syntax or geometry should fail visibly until implemented. Manifest `codecs`
gates may keep future vectors generateable while excluding them from a codec's
validation run, but an enabled row is expected to pass.

## Batch Fixtures

Portable generated-vector manifests live under:

```text
verification/test_vector_sets/
```

Use these targets for software-only CLI regression batches:

```sh
make test-vector-sets
make test-vectors TEST_VECTOR_SET=smoke
make validate-set CODEC=av2 VALIDATION_SET=smoke
make validate-set CODEC=vvc VALIDATION_SET=smoke
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SOURCE_FILTERS=1
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_SETTINGS=lossless
make regression
```

`scripts/generate_test_vectors.py` writes deterministic raw YUV inputs under
`verification/generated/test_vectors/`. `scripts/run_validation_set.py` encodes
each generated vector through `./ff encode`, writes the encoder's internal
reconstruction through `--recon`, checks that non-empty bitstream and
reconstruction outputs were produced, and prints a markdown summary with output
size, SHA-256 checksums, reason, and log path.

Manifest `format` values use the same raw input names accepted by the CLI.
Planar YUV and gray bit depths from 8 through 16 are described in
[`raw-input-formats.md`](raw-input-formats.md), including exact higher-depth
codec support where available and the 8-bit fallback used by codec paths that
do not yet encode a higher depth natively.

Rows may set `lossless=true`. Validation passes that request to `ff encode`
with `--set lossless` and compares the encoder's internal reconstruction bytes
against the generated source bytes before optional reference-decoder checks.
When `VALIDATION_REFERENCE_MODE` is `auto` or `required` and a reference decoder
is used, the reference reconstruction must also match the internal
reconstruction. A lossless stream should only be enabled for a codec when both
checks are expected to pass.
For AV2 `rgb24` lossless vectors, FrameForge writes packed RGB reconstruction
bytes while AVM's raw decoder output is planar identity GBR. The validation
runner normalizes that reference output back to packed `rgb24` before comparing
checksums.

Additional encoder settings can be passed to validation with
`VALIDATION_SETTINGS="key key=value"`. This is intended for codec experiments
that are not part of a manifest row yet, such as AV2's experimental lossless
predictive mode:

```sh
make validate-set CODEC=av2 \
  VALIDATION_SET=local-aomctc-b2-scc-predictive-sweep-3f \
  VALIDATION_SETTINGS=predictive \
  VALIDATION_REFERENCE_MODE=required
```

Explicit lossy AV2 smoke checks can invoke `./ff encode ... --qp N` directly.
`--qp` is mutually exclusive with `--set lossless`; lossy checks should compare
bitstream size, reconstruction PSNR, and reference-decoder agreement with the
encoder reconstruction rather than source-byte equality.

`scripts/generate_predictive_sweep.py` creates that local ignored manifest and
384 local Y4M crops: six AOM CTC B2 screen-content variants, 64 geometries from
8x8 through 64x64, and three frames per crop. Each crop currently repeats one
randomly selected source frame three times so AV2 show-existing-frame and
reference-buffer syntax are exercised across bit depth and subsampling
variants. A future companion set should use consecutive frames once block-level
inter prediction is implemented.

The `high-depth-smoke` set uses deterministic lower-bit canary samples so
truncation of 10-bit or 12-bit input is visible as a validation failure. VVC
4:2:0, 4:2:2, and 4:4:4 canaries are expected to pass with reference decoding;
AV2 10-bit 4:2:0, 4:2:2, and 4:4:4 canaries are expected to pass with
reference decoding. AV2 12-bit canaries remain gated until a reference-valid
12-bit profile path is available.

Reference tools are declared by JSON manifests under:

```text
verification/reference_codecs/
```

List and build declared references with:

```sh
make reference-list
make reference-setup
make reference-setup REFERENCE_CODEC=vvc
```

Reference source and build trees are local artifacts under
`verification/references/` and are not committed. `make validate-set` uses
`VALIDATION_REFERENCE_MODE=auto` by default. In `auto` mode, a built or
environment-configured decoder is used to decode the FrameForge bitstream and
the decoded output must match the internal reconstruction checksum. Missing
reference tools are reported as a skip. Use `VALIDATION_REFERENCE_MODE=required`
to make missing reference tools a failure, or `VALIDATION_REFERENCE_MODE=off`
for encode-only validation.

Reference encoder compression comparisons are intentionally separate from
decode-side validation. `make compare-compression` uses AVM/VTM encoders by
default to produce codec-native size baselines. These default reference outputs
are cached under `verification/generated/compression_compare/<codec>/<set>/`
and reused when the input, encoder path, preset, thread settings, and extra
reference arguments match. Set `COMPRESSION_REFRESH_REFERENCE=1` only when a
cached reference result should be regenerated.

AV2 does not currently have a fast dav1d-like production encoder separate from
AVM. For faster lossy iteration, `COMPRESSION_REFERENCE_BACKEND=rav1e` uses
the rav1e AV1 encoder as an explicit AV1 size/time baseline. rav1e does not
currently implement lossless encoding, so lossless manifests should keep the
default `COMPRESSION_REFERENCE_BACKEND=reference` path when a lossless AV1
baseline is required. The rav1e result is not an AV2 reference result, and it
is written under a backend-specific subdirectory such as
`verification/generated/compression_compare/av2/<set>/rav1e/` so it does not
clobber cached AVM results. Build it with:

```sh
make reference-setup REFERENCE_CODEC=rav1e
make compare-compression CODEC=av2 COMPRESSION_SET=smoke COMPRESSION_REFERENCE_BACKEND=rav1e
```

For realtime AV1 production-ceiling checks, `COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom`
uses the system `ffmpeg` binary with `libaom-av1`. The
`COMPRESSION_REFERENCE_PRESET=realtime-screen` preset enables realtime,
low-latency, screen-content-oriented libaom settings. This baseline may be
lossy even when the FrameForge row has `lossless=true`; use it to compare
speed and size against a realistic AV1 screen-share profile, not as a
stream-exact reference. Set `COMPRESSION_REFERENCE_PRESET=lossless` when an
AV1 lossless libaom baseline is needed instead. ffmpeg/libaom outputs are
cached under a backend-specific directory such as
`verification/generated/compression_compare/av2/<set>/ffmpeg-libaom/`.

For local source-file manifests backed by large Y4M inputs, set
`COMPRESSION_DIRECT_SOURCE_FILES=1` to feed the source path directly and use
the manifest `frames` value as the total-frame limiter instead of materializing
a frame-limited raw copy first:

```sh
make compare-compression CODEC=av2 \
  COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f \
  COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom \
  COMPRESSION_REFERENCE_PRESET=realtime-screen \
  COMPRESSION_SETTINGS=predictive \
  COMPRESSION_DIRECT_SOURCE_FILES=1
```

For AV2 lossy QP comparisons, set `COMPRESSION_QP=<1..255>`. This forwards
the dedicated `./ff encode --qp` option and treats manifest `lossless=true`
rows as lossy FrameForge rows for that comparison run:

```sh
make compare-compression CODEC=av2 \
  COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f \
  COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom \
  COMPRESSION_REFERENCE_PRESET=realtime-screen \
  COMPRESSION_QP=24 \
  COMPRESSION_DIRECT_SOURCE_FILES=1
```

For AV2 native reference comparisons, the Makefile defaults to
`COMPRESSION_REFERENCE_PRESET=fast`, which keeps `--cpu-used=9` and adds AVM
threading and low-latency speed options. Use
`COMPRESSION_REFERENCE_PRESET=default` to keep the legacy AVM argument set.
The fast preset can be tuned with:

```sh
make compare-compression CODEC=av2 COMPRESSION_REFERENCE_THREADS=8
make compare-compression CODEC=av2 COMPRESSION_AVM_TILE_COLUMNS=1
make compare-compression CODEC=av2 COMPRESSION_REFERENCE_PRESET=default
```

For vectors whose manifest pattern can be generated by the CLI, set
`VALIDATION_SOURCE_FILTERS=1`. The runner will skip input-file generation and
invoke the source filter directly, for example:

```sh
./ff encode --filter pattern=checker --video 16x24:yuv420p8 \
  --frames 1 --fps 30 --encode av2:out.obu
```
