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

## Validation

The predictive syntax checkpoint also passed the local required-reference
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
