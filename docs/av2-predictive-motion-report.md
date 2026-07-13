# AV2 Predictive Motion Report

This report tracks AV2 lossless predictive bitrate and encode speed while
motion-estimation support is added. Each checkpoint uses the same comparison
command unless noted:

```sh
make compare-compression CODEC=av2 \
  COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f \
  COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom \
  COMPRESSION_REFERENCE_PRESET=realtime-screen \
  COMPRESSION_DIRECT_SOURCE_FILES=1
```

The ffmpeg/libaom outputs are cached and used only as a lossy AV1 size
baseline. FrameForge fps is timed locally by the comparison script.

## Checkpoint 1: Exact Motion-Map Scaffolding

Changes:

- Added a shared planar YUV region/hash helper for AV2 4:2:0, 4:2:2, and
  4:4:4.
- Added an exact 8x8 lossless motion map with zero-MV, neighbor-MV, and
  bounded block-aligned candidate search.
- Reused the shared region comparison from the local IntraBC exact-match path.
- This checkpoint does not emit regular inter frames yet, so predictive
  non-identical frames still use the previous key-frame fallback.

Validation:

```sh
cargo test -p frameforge-codecs --all-features
```

Result: 161/161 tests passed.

Compression comparison result:

| Vector | FF bytes | FF Mbps | FF fps | Prev FF Mbps | Delta Mbps | Prev fps | Delta fps |
|---|---:|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 24,498,025 | 58.80 | 3.85 | 60.76 | -1.96 | 12.53 | -8.68 |
| Scene 422 8-bit | 27,890,525 | 66.94 | 3.36 | 68.55 | -1.61 | 11.64 | -8.28 |
| Scene 444 8-bit | 33,603,874 | 80.65 | 2.87 | 81.80 | -1.15 | 8.79 | -5.92 |
| Mission 420 10-bit | 70,741,219 | 679.12 | 2.28 | 560.65 | +118.47 | 8.84 | -6.56 |
| Mission 422 10-bit | 81,787,223 | 785.16 | 1.99 | 645.97 | +139.19 | 7.37 | -5.38 |
| Mission 444 10-bit | 101,589,699 | 975.26 | 1.65 | 800.55 | +174.71 | 5.81 | -4.16 |
| Total | 340,110,565 | n/a | 2.45 | n/a | n/a | 8.57 | -6.12 |

Total byte delta from `docs/av2-predictive-baseline.md`:

- Previous FrameForge bytes: 297,043,630.
- Current FrameForge bytes: 340,110,565.
- Delta: +43,066,935 bytes (+14.5%).

The fps drop is the immediate control point for the next patch. The regular
inter-frame syntax work should be compared against this checkpoint first, not
only against the older baseline report.

## Checkpoint 2: Zero-MV Regular-Inter Syntax Scaffold

Changes:

- Added AVM-derived CDF initializers for `intra_inter`, `single_ref`, and
  `inter_single_mode` symbols.
- Added a narrow zero-MV regular inter tile entropy helper for 8x8 lossless
  leaves. It emits `is_inter=1`, `skip_txfm=1`, and `GLOBALMV`, with neighbor
  contexts tracked on the MI grid.
- Added a regular tile-group header helper and OBU type coverage in tests.
- Kept the production repeated-frame path on SEF; this checkpoint does not
  change CLI-selected bitstreams or reconstruction.

Validation:

```sh
cargo test -p frameforge-codecs --all-features
make build
```

Result: 162/162 codec tests passed, and the release CLI build passed.

Compression comparison:

| Scope | FF bytes | FF fps | Delta bytes vs checkpoint 1 | Delta fps vs checkpoint 1 |
|---|---:|---:|---:|---:|
| CLI-selected AV2 lossless paths | unchanged | unchanged | 0 | 0 |

No full 1080p comparison was rerun for this scaffold because the new regular
inter payload is test-only until it is reference-decoder clean and can replace
part of the predictive key-frame fallback without regressing repeated-frame SEF
compression.
