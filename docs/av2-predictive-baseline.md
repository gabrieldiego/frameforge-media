# AV2 Predictive Baseline

This checkpoint records the first AV2 lossless predictive plumbing baseline.
FrameForge is run as AV2 lossless with `--set predictive`. At this point the
predictive path starts a multi-picture AV2 stream and uses show-existing-frame
for exact repeated frames; non-identical pictures still use the existing
lossless key-frame path.

The comparison baseline is ffmpeg/libaom AV1 using the realtime screen-share
preset from `make compare-compression`. It is intentionally lossy and should be
treated as a speed floor/basement rather than a quality-equivalent reference.

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

## Commands

FrameForge command shape:

```sh
./ff encode <input.y4m> --frames 50 \
  --encode av2:<output.obu> \
  --set lossless --set predictive
```

ffmpeg/libaom command shape:

```sh
ffmpeg -y -hide_banner -loglevel error -i <input.y4m> -frames:v 50 \
  -c:v libaom-av1 -usage realtime -cpu-used 8 -threads 8 -row-mt 1 \
  -tiles 8x1 -lag-in-frames 0 -auto-alt-ref 0 \
  -b:v 4M -maxrate 4M -bufsize 4M -g 300 -aq-mode cyclic \
  -enable-cdef 1 -enable-restoration 0 -enable-global-motion 0 \
  -enable-obmc 0 -enable-palette 1 -enable-cfl-intra 0 \
  -enable-smooth-intra 0 -enable-angle-delta 0 -enable-filter-intra 0 \
  -use-intra-default-tx-only 1 -enable-ref-frame-mvs 0 \
  -enable-dual-filter 0 -enable-interintra-comp 0 -enable-masked-comp 0 \
  -enable-paeth-intra 0 -enable-rect-partitions 0 -enable-tx64 0 \
  -aom-params tune-content=screen <output.ivf>
```

## Results

| Vector | FrameForge Mbps | ffmpeg Mbps | Bitrate ratio | FrameForge fps | ffmpeg fps | FPS ratio |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 60.76 | 0.85 | 71.6x | 12.53 | 33.63 | 37.3% |
| Scene 422 8-bit | 68.55 | 0.98 | 70.0x | 11.64 | 31.40 | 37.1% |
| Scene 444 8-bit | 81.80 | 1.06 | 77.4x | 8.79 | 28.27 | 31.1% |
| Mission 420 10-bit | 560.65 | 6.55 | 85.5x | 8.84 | 16.90 | 52.3% |
| Mission 422 10-bit | 645.97 | 7.02 | 92.0x | 7.37 | 15.98 | 46.1% |
| Mission 444 10-bit | 800.55 | 7.47 | 107.2x | 5.81 | 10.69 | 54.3% |
| Total | n/a | n/a | 87.5x | 8.57 | 19.22 | 44.6% |

Raw totals across all six rows:

- Frames: 300.
- FrameForge bytes: 297,043,630.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge elapsed: 35.021 s.
- ffmpeg/libaom elapsed: 15.612 s.

## Later Checkpoints

### Mixed Motion Baseline

The first NEWMV plus mixed 8x8 inter-tile checkpoint reduced FrameForge bytes
to 188,690,766 for the same 300 frames, with total FrameForge encode speed at
3.67 fps. That is a 36.5% byte reduction from the initial predictive baseline,
with lower speed while motion estimation is still scalar and single-threaded.

### Region-Aware Motion Search

The region-aware motion-search checkpoint preclassifies exact zero-MV tiles and
only builds the 8x8 motion map for tiles that still need NEWMV or mixed inter
search. Bitstreams stayed byte-identical to the mixed-motion baseline.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 6.74 | 8.18 | +21.4% |
| Scene 422 8-bit | 0 | 6.09 | 7.29 | +19.7% |
| Scene 444 8-bit | 0 | 4.94 | 5.91 | +19.6% |
| Mission 420 10-bit | 0 | 3.16 | 3.27 | +3.5% |
| Mission 422 10-bit | 0 | 2.79 | 2.90 | +3.9% |
| Mission 444 10-bit | 0 | 2.24 | 2.31 | +3.1% |
| Total | 0 | 3.67 | 3.97 | +8.2% |

Raw totals for the region-aware checkpoint:

- Frames: 300.
- FrameForge bytes: 188,690,766.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 3.97 fps.

### Motion Candidate And Hash Reuse

The motion candidate/hash reuse checkpoint keeps one candidate buffer across
8x8 searches and computes reference block hashes lazily. Bitstreams stayed
byte-identical to the region-aware checkpoint.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 8.18 | 8.45 | +3.3% |
| Scene 422 8-bit | 0 | 7.29 | 7.52 | +3.2% |
| Scene 444 8-bit | 0 | 5.91 | 5.93 | +0.3% |
| Mission 420 10-bit | 0 | 3.27 | 3.37 | +3.1% |
| Mission 422 10-bit | 0 | 2.90 | 2.88 | -0.7% |
| Mission 444 10-bit | 0 | 2.31 | 2.37 | +2.6% |
| Total | 0 | 3.97 | 4.04 | +1.8% |

Raw totals for the motion candidate/hash reuse checkpoint:

- Frames: 300.
- FrameForge bytes: 188,690,766.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.04 fps.

### Chunked Planar Hashing

The chunked planar hashing checkpoint keeps motion-search candidate order and
exact reconstruction checks unchanged, but folds planar hash rows in 8-byte
chunks instead of byte-by-byte.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 8.45 | 8.33 | -1.4% |
| Scene 422 8-bit | 0 | 7.52 | 7.72 | +2.7% |
| Scene 444 8-bit | 0 | 5.93 | 6.11 | +3.0% |
| Mission 420 10-bit | 0 | 3.37 | 3.44 | +2.1% |
| Mission 422 10-bit | 0 | 2.88 | 2.96 | +2.8% |
| Mission 444 10-bit | 0 | 2.37 | 2.47 | +4.2% |
| Total | 0 | 4.04 | 4.15 | +2.7% |

Raw totals for the chunked planar hashing checkpoint:

- Frames: 300.
- FrameForge bytes: 188,690,766.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.15 fps.

### Mixed Inter/Intra Tiles

The mixed inter/intra tile checkpoint lets one fixed-8x8 regular-inter tile
combine exact inter-copy leaves with normal lossless intra leaves. This avoids
falling back to a full intra tile when only part of the tile has no exact
motion match.

| Vector | Previous Bytes | New Bytes | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 10,854,614 | 4,798,845 | -55.8% | 8.33 | 10.99 | +31.9% |
| Scene 422 8-bit | 12,407,036 | 5,377,435 | -56.7% | 7.72 | 10.52 | +36.3% |
| Scene 444 8-bit | 14,970,696 | 6,361,136 | -57.5% | 6.11 | 8.57 | +40.3% |
| Mission 420 10-bit | 42,229,798 | 22,458,420 | -46.8% | 3.44 | 5.18 | +50.6% |
| Mission 422 10-bit | 48,460,584 | 26,078,757 | -46.2% | 2.96 | 4.79 | +61.8% |
| Mission 444 10-bit | 59,768,038 | 33,007,478 | -44.8% | 2.47 | 3.87 | +56.7% |
| Total | 188,690,766 | 98,082,071 | -48.0% | 4.15 | 6.23 | +50.1% |

Raw totals for the mixed inter/intra tile checkpoint:

- Frames: 300.
- FrameForge bytes: 98,082,071.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 6.23 fps.

### Block-Unit Motion Search

The block-unit motion-search cleanup keeps candidate ordering and final pixel
motion vectors unchanged, but carries internal candidates in 8x8 block units to
avoid repeated per-candidate divisibility and division work. Bitstreams stayed
byte-identical to the mixed inter/intra tile checkpoint; measured fps changes
are within run-to-run noise on the six-vector pass.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 10.99 | 10.66 | -3.0% |
| Scene 422 8-bit | 0 | 10.52 | 10.50 | -0.2% |
| Scene 444 8-bit | 0 | 8.57 | 8.92 | +4.1% |
| Mission 420 10-bit | 0 | 5.18 | 5.17 | -0.2% |
| Mission 422 10-bit | 0 | 4.79 | 4.73 | -1.3% |
| Mission 444 10-bit | 0 | 3.87 | 3.88 | +0.3% |
| Total | 0 | 6.23 | 6.23 | 0.0% |

Raw totals for the block-unit motion-search checkpoint:

- Frames: 300.
- FrameForge bytes: 98,082,071.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 6.23 fps.

### Bounded Hash-Index Motion Fallback

The bounded hash-index fallback keeps the existing zero/neighbor/local motion
candidate order as the primary path, then uses a lazily-built reference 8x8
hash index for blocks that still have no exact match. The fallback only scans
hash buckets with at most eight reference blocks, which keeps it focused on
distinct screen-content blocks and avoids broad flat-region searches.

| Vector | Previous Bytes | New Bytes | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 4,798,845 | 4,768,470 | -0.6% | 10.66 | 10.52 | -1.3% |
| Scene 422 8-bit | 5,377,435 | 5,338,169 | -0.7% | 10.50 | 9.64 | -8.2% |
| Scene 444 8-bit | 6,361,136 | 6,313,168 | -0.8% | 8.92 | 8.60 | -3.6% |
| Mission 420 10-bit | 22,458,420 | 22,443,304 | -0.1% | 5.17 | 4.96 | -4.1% |
| Mission 422 10-bit | 26,078,757 | 26,062,159 | -0.1% | 4.73 | 4.50 | -4.9% |
| Mission 444 10-bit | 33,007,478 | 32,989,639 | -0.1% | 3.88 | 3.76 | -3.1% |
| Total | 98,082,071 | 97,914,909 | -0.2% | 6.23 | 5.97 | -4.2% |

Raw totals for the bounded hash-index checkpoint:

- Frames: 300.
- FrameForge bytes: 97,914,909.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 5.97 fps.

### Sorted Reference Hash Index

The sorted reference hash index checkpoint replaces the fallback motion-search
`HashMap<u64, Vec<_>>` with one sorted vector of reference block hashes. Lookup
uses binary partitioning over the sorted table, keeping fallback bucket limits
and final motion-vector selection unchanged.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 10.52 | 10.53 | +0.1% |
| Scene 422 8-bit | 0 | 9.64 | 9.94 | +3.1% |
| Scene 444 8-bit | 0 | 8.60 | 8.12 | -5.6% |
| Mission 420 10-bit | 0 | 4.96 | 5.17 | +4.2% |
| Mission 422 10-bit | 0 | 4.50 | 4.59 | +2.0% |
| Mission 444 10-bit | 0 | 3.76 | 3.83 | +1.9% |
| Total | 0 | 5.97 | 6.06 | +1.5% |

Raw totals for the sorted reference hash index checkpoint:

- Frames: 300.
- FrameForge bytes: 97,914,909.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 6.06 fps.

### Residual Inter Tile Candidate

The residual inter tile candidate lets a mixed inter/intra tile compare the old
intra fallback against a zero-MV inter block with coded residual coefficients.
This is an intentionally conservative first step: the encoder keeps whichever
candidate has the smaller entropy payload for each tile, so the bitstream gains
compression before the residual mode-selection path is optimized for speed.

| Vector | Previous Bytes | New Bytes | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 4,768,470 | 4,280,592 | -10.2% | 10.53 | 7.52 | -28.6% |
| Scene 422 8-bit | 5,338,169 | 4,816,324 | -9.8% | 9.94 | 7.06 | -29.0% |
| Scene 444 8-bit | 6,313,168 | 5,762,473 | -8.7% | 8.12 | 6.02 | -25.9% |
| Mission 420 10-bit | 22,443,304 | 19,494,499 | -13.1% | 5.17 | 3.37 | -34.8% |
| Mission 422 10-bit | 26,062,159 | 22,685,653 | -13.0% | 4.59 | 2.93 | -36.2% |
| Mission 444 10-bit | 32,989,639 | 28,592,973 | -13.3% | 3.83 | 2.28 | -40.5% |
| Total | 97,914,909 | 85,632,514 | -12.5% | 6.06 | 3.96 | -34.7% |

Raw totals for the residual inter tile candidate checkpoint:

- Frames: 300.
- FrameForge bytes: 85,632,514.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 3.96 fps.

### Source-Backed Candidate Scratch

The source-backed candidate scratch checkpoint removes redundant reconstruction
copies from the mixed inter/intra candidate path. The fast lossless path already
uses source-backed reconstruction while choosing a payload, so this keeps the
selected bitstream identical and only reduces candidate-evaluation work.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 7.52 | 8.06 | +7.2% |
| Scene 422 8-bit | 0 | 7.06 | 7.60 | +7.6% |
| Scene 444 8-bit | 0 | 6.02 | 6.71 | +11.5% |
| Mission 420 10-bit | 0 | 3.37 | 3.53 | +4.7% |
| Mission 422 10-bit | 0 | 2.93 | 2.76 | -5.8% |
| Mission 444 10-bit | 0 | 2.28 | 2.51 | +10.1% |
| Total | 0 | 3.96 | 4.14 | +4.5% |

Raw totals for the source-backed candidate scratch checkpoint:

- Frames: 300.
- FrameForge bytes: 85,632,514.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.14 fps.

### Residual Payload Shortcut

The residual payload shortcut emits the zero-MV residual candidate first for
mixed predictive tiles. If that payload is already below a conservative
tile-source-size budget, the encoder keeps it without running the competing
intra fallback search. Larger residual candidates still use the exact
residual-vs-intra payload comparison, so the measured bitstream sizes stay
unchanged on the 50-frame baseline.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 8.06 | 8.16 | +1.2% |
| Scene 422 8-bit | 0 | 7.60 | 7.54 | -0.8% |
| Scene 444 8-bit | 0 | 6.71 | 6.85 | +2.1% |
| Mission 420 10-bit | 0 | 3.53 | 3.54 | +0.3% |
| Mission 422 10-bit | 0 | 2.76 | 3.05 | +10.5% |
| Mission 444 10-bit | 0 | 2.51 | 2.50 | -0.4% |
| Total | 0 | 4.14 | 4.25 | +2.7% |

Raw totals for the residual payload shortcut checkpoint:

- Frames: 300.
- FrameForge bytes: 85,632,514.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.25 fps.

### Residual Shortcut Threshold Tuning

The residual shortcut threshold tuning makes the tile-source-size budget
explicit and relaxes it from 1/96 to 1/64 of the source bytes covered by the
tile. The 50-frame baseline keeps identical bitstream sizes while avoiding more
intra fallback searches.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 8.16 | 8.50 | +4.2% |
| Scene 422 8-bit | 0 | 7.54 | 7.94 | +5.3% |
| Scene 444 8-bit | 0 | 6.85 | 7.00 | +2.2% |
| Mission 420 10-bit | 0 | 3.54 | 3.65 | +3.1% |
| Mission 422 10-bit | 0 | 3.05 | 3.16 | +3.6% |
| Mission 444 10-bit | 0 | 2.50 | 2.59 | +3.6% |
| Total | 0 | 4.25 | 4.40 | +3.5% |

Raw totals for the residual shortcut threshold tuning checkpoint:

- Frames: 300.
- FrameForge bytes: 85,632,514.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.40 fps.

### Residual Shortcut Threshold Tradeoff

The residual shortcut threshold tradeoff relaxes the tile-source-size budget
again, from 1/64 to 1/32 of the source bytes covered by the tile. This skips
more intra fallback searches at a negligible bitrate cost on the 50-frame
baseline.

| Vector | Previous Bytes | New Bytes | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 4,280,592 | 4,282,069 | +0.03% | 8.50 | 8.55 | +0.6% |
| Scene 422 8-bit | 4,816,324 | 4,819,315 | +0.06% | 7.94 | 8.19 | +3.1% |
| Scene 444 8-bit | 5,762,473 | 5,766,361 | +0.07% | 7.00 | 6.94 | -0.9% |
| Mission 420 10-bit | 19,494,499 | 19,494,499 | 0.00% | 3.65 | 3.66 | +0.3% |
| Mission 422 10-bit | 22,685,653 | 22,685,653 | 0.00% | 3.16 | 3.20 | +1.3% |
| Mission 444 10-bit | 28,592,973 | 28,592,973 | 0.00% | 2.59 | 2.65 | +2.3% |
| Total | 85,632,514 | 85,640,870 | +0.01% | 4.40 | 4.45 | +1.1% |

Raw totals for the residual shortcut threshold tradeoff checkpoint:

- Frames: 300.
- FrameForge bytes: 85,640,870.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.45 fps.

### Predictor-First Motion Search

The predictor-first motion-search checkpoint splits zero/neighbor predictor
checks from local-search expansion. Once the reference hash index exists,
candidate selection tries exact hash-index matches before expanding the local
search ring; if the hash index is not built yet, local search still runs first
and then builds the index only on a miss. This keeps the bitstream effectively
unchanged while avoiding many local candidate probes after the first indexed
miss in a frame.

| Vector | Previous Bytes | New Bytes | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 4,282,069 | 4,282,079 | +10 | 8.55 | 9.80 | +14.6% |
| Scene 422 8-bit | 4,819,315 | 4,819,333 | +18 | 8.19 | 8.92 | +8.9% |
| Scene 444 8-bit | 5,766,361 | 5,766,372 | +11 | 6.94 | 7.51 | +8.2% |
| Mission 420 10-bit | 19,494,499 | 19,494,498 | -1 | 3.66 | 3.88 | +6.0% |
| Mission 422 10-bit | 22,685,653 | 22,685,657 | +4 | 3.20 | 3.28 | +2.5% |
| Mission 444 10-bit | 28,592,973 | 28,592,971 | -2 | 2.65 | 2.73 | +3.0% |
| Total | 85,640,870 | 85,640,910 | +40 | 4.45 | 4.70 | +5.6% |

Raw totals for the predictor-first motion-search checkpoint:

- Frames: 300.
- FrameForge bytes: 85,640,910.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.70 fps.

### Fast Planar Palette Decode

The fast planar palette decode checkpoint keeps palette decisions and decoded
sample values unchanged, but validates the contiguous source-plane range once
and then decodes 8-bit samples directly from the byte slice and high-depth
samples from little-endian chunks. Bitstreams stayed byte-identical to the
predictor-first checkpoint.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 9.80 | 10.60 | +8.2% |
| Scene 422 8-bit | 0 | 8.92 | 9.69 | +8.6% |
| Scene 444 8-bit | 0 | 7.51 | 8.13 | +8.3% |
| Mission 420 10-bit | 0 | 3.88 | 3.98 | +2.6% |
| Mission 422 10-bit | 0 | 3.28 | 3.40 | +3.7% |
| Mission 444 10-bit | 0 | 2.73 | 2.77 | +1.5% |
| Total | 0 | 4.70 | 4.89 | +4.0% |

Raw totals for the fast planar palette decode checkpoint:

- Frames: 300.
- FrameForge bytes: 85,640,910.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 4.89 fps.

### Homogeneous Inter Partitions

The homogeneous inter partitions checkpoint lets mixed predictive tiles keep
larger leaves when all covered 8x8 motion blocks share the same inter mode. The
partition chooser still splits regions that mix zero-MV, NEWMV, residual, and
intra leaves, so the feature is available to 4:2:0, 4:2:2, and 4:4:4 through
one shared block-mode map. This trades a small byte increase for substantially
less partition and leaf entropy work on the 50-frame baseline.

| Vector | Previous Bytes | New Bytes | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|---:|---:|
| Scene 420 8-bit | 4,282,079 | 4,306,732 | +0.58% | 10.60 | 13.02 | +22.8% |
| Scene 422 8-bit | 4,819,333 | 4,847,524 | +0.58% | 9.69 | 11.69 | +20.6% |
| Scene 444 8-bit | 5,766,372 | 5,788,443 | +0.38% | 8.13 | 9.69 | +19.2% |
| Mission 420 10-bit | 19,494,498 | 19,739,272 | +1.26% | 3.98 | 5.05 | +26.9% |
| Mission 422 10-bit | 22,685,657 | 22,954,706 | +1.19% | 3.40 | 4.09 | +20.3% |
| Mission 444 10-bit | 28,592,971 | 28,905,400 | +1.09% | 2.77 | 3.20 | +15.5% |
| Total | 85,640,910 | 86,542,077 | +1.05% | 4.89 | 5.88 | +20.2% |

Raw totals for the homogeneous inter partitions checkpoint:

- Frames: 300.
- FrameForge bytes: 86,542,077.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 5.88 fps.

### Static CDF Keys For Tile Syntax

The static CDF key checkpoint keeps tile syntax and output bytes unchanged, but
resolves hot inter, IntraBC, partition split, FSC, and intra mode-index CDFs by
fixed numeric keys instead of the generic string/key adaptive-CDF path.

| Vector | Bytes Delta | Previous FPS | New FPS | FPS Delta |
|---|---:|---:|---:|---:|
| Scene 420 8-bit | 0 | 13.02 | 13.71 | +5.3% |
| Scene 422 8-bit | 0 | 11.69 | 12.12 | +3.7% |
| Scene 444 8-bit | 0 | 9.69 | 9.76 | +0.7% |
| Mission 420 10-bit | 0 | 5.05 | 4.90 | -3.0% |
| Mission 422 10-bit | 0 | 4.09 | 4.12 | +0.7% |
| Mission 444 10-bit | 0 | 3.20 | 3.25 | +1.6% |
| Total | 0 | 5.88 | 5.93 | +0.9% |

Raw totals for the static CDF key checkpoint:

- Frames: 300.
- FrameForge bytes: 86,542,077.
- ffmpeg/libaom bytes: 3,394,010.
- FrameForge aggregate speed: 5.93 fps.

## Validation

The latest predictive checkpoint also passed the local required-reference
geometry sweep:

```sh
make validate-set CODEC=av2 \
  VALIDATION_SET=local-aomctc-b2-scc-predictive-sweep-3f \
  VALIDATION_SETTINGS=predictive \
  VALIDATION_REFERENCE_MODE=required \
  VALIDATION_STOP_ON_FAIL=1
```

Result: 384/384 cases passed with lossless reconstruction and AVM reference
reconstruction matching the internal reconstruction.
