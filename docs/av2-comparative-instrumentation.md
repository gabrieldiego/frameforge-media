# AV2 Comparative Instrumentation

This note records the workflow for improving FrameForge AV2 bitrate while
holding the current PSNR guardrail. The intent is to compare FrameForge against
production AV1 (`ffmpeg`/libaom and direct `aomenc`) and AV2 research behavior
(AVM), then use the differences to choose targeted encoder changes.

## Guardrails

- Keep the six-vector local screen-content set as the main scoreboard:
  `local-aomctc-b2-scc-1080p-lossless-50f`.
- Track both first-frame intra behavior and 50-frame predictive behavior.
- Treat PSNR as the quality floor and bitrate as the main optimization target.
- Keep fps from regressing substantially; large mode-search probes need
  measured bitrate wins to justify the cost.
- Validate changed AV2 bitstreams with AVM decode and compare AVM raw output
  against FrameForge internal reconstruction.
- Keep lossless validation strict; lossy instrumentation must not gate out
  lossless P-frame support.
- Keep generated traces and bitstreams under `verification/generated/`, not
  `/tmp`.

## Compile Gating

Normal `make build` uses normal product features only. Analysis code is compiled
only when the corresponding Makefile flag is set:

```sh
make build AV2_SB_BITS=1
make build AV2_LOSSY_STATS=1
make build AV2_SB_BITS=1 AV2_LOSSY_STATS=1
```

`AV2_SB_BITS=1` enables `frameforge-codecs/av2-sb-bit-profile` and writes
per-superblock JSONL only when `FRAMEFORGE_AV2_SB_BITS` is set at runtime.
`AV2_LOSSY_STATS=1` enables `frameforge-codecs/av2-lossy-stats` and prints
mode/TXB summaries only when `FRAMEFORGE_AV2_LOSSY_STATS` is set at runtime.

Reference instrumentation is built into separate reference build directories:

```sh
make reference-setup REFERENCE_CODEC=libaom LIBAOM_SB_BITS=1
make reference-setup REFERENCE_CODEC=av2 AVM_SB_BITS=1
```

The patched reference encoders write JSONL only when their runtime environment
variables are set:

```sh
FRAMEFORGE_LIBAOM_SB_BITS=verification/generated/instrumentation/libaom.jsonl
FRAMEFORGE_AVM_SB_BITS=verification/generated/instrumentation/avm.jsonl
```

## Superblock Bit Maps

FrameForge:

```sh
make build AV2_SB_BITS=1
FRAMEFORGE_AV2_SB_BITS=verification/generated/instrumentation/scene_ff_sb.jsonl \
  ./ff encode /media/gabriel/storage/YUV/aomctc/b2_scc/SceneComposition_1.y4m \
  --frames 1 --encode av2:verification/generated/instrumentation/scene_ff.obu \
  --set predictive --qp 24
```

Direct libaom:

```sh
make reference-setup REFERENCE_CODEC=libaom LIBAOM_SB_BITS=1
FRAMEFORGE_LIBAOM_SB_BITS=verification/generated/instrumentation/scene_libaom_sb.jsonl \
  make compare-compression CODEC=av2 \
  COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f \
  COMPRESSION_LIMIT=1 \
  COMPRESSION_REFERENCE_BACKEND=libaom \
  COMPRESSION_REFERENCE_PRESET=realtime-screen \
  COMPRESSION_QP=24 \
  COMPRESSION_DIRECT_SOURCE_FILES=1 \
  COMPRESSION_REFRESH_REFERENCE=1 \
  LIBAOM_SB_BITS=1
```

AVM:

```sh
make reference-setup REFERENCE_CODEC=av2 AVM_SB_BITS=1
FRAMEFORGE_AVM_SB_BITS=verification/generated/instrumentation/scene_avm_sb.jsonl \
  make compare-compression CODEC=av2 \
  COMPRESSION_SET=local-aomctc-b2-scc-1080p-lossless-50f \
  COMPRESSION_LIMIT=1 \
  COMPRESSION_REFERENCE_BACKEND=reference \
  COMPRESSION_REFERENCE_PRESET=fast \
  COMPRESSION_QP=24 \
  COMPRESSION_DIRECT_SOURCE_FILES=1 \
  COMPRESSION_REFRESH_REFERENCE=1 \
  AVM_SB_BITS=1
```

Summarize and compare the traces:

```sh
scripts/summarize_encoder_instrumentation.py \
  --sb-bits scene/frameforge=verification/generated/instrumentation/scene_ff_sb.jsonl \
  --sb-bits scene/libaom=verification/generated/instrumentation/scene_libaom_sb.jsonl \
  --sb-bits scene/avm=verification/generated/instrumentation/scene_avm_sb.jsonl
```

Use the summary to answer:

- Is FrameForge overspending uniformly, or only in specific superblocks?
- Is the excess mostly residual, mode syntax, partition syntax, palette/IBC
  absence, or inter syntax?
- Do hot superblocks correspond to text edges, flat UI regions, repeated screen
  elements, chroma, or high-bit-depth content?
- Does AVM spend bits in the same places as libaom, or does it use AV2-specific
  tools differently?

## Mode And TXB Summaries

Compile the gated stats hook and capture stderr:

```sh
make build AV2_LOSSY_STATS=1
FRAMEFORGE_AV2_LOSSY_STATS=1 \
  ./ff encode /media/gabriel/storage/YUV/aomctc/b2_scc/SceneComposition_1.y4m \
  --frames 1 --encode av2:verification/generated/instrumentation/scene_stats.obu \
  --set predictive --qp 24 \
  2> verification/generated/instrumentation/scene_stats.log
```

Summarize:

```sh
scripts/summarize_encoder_instrumentation.py \
  --lossy-stats-log scene/frameforge=verification/generated/instrumentation/scene_stats.log
```

Use these summaries to check whether a heuristic actually changes the selected
mode population. A proposal is suspicious if the mode/TXB statistics barely
move while bitrate or PSNR moves sharply.

## Entropy Field Traces

FrameForge has JSONL entropy field helpers for AV2 syntax traces. When a local
harness emits those traces, summarize the fields with:

```sh
scripts/summarize_encoder_instrumentation.py \
  --field-trace scene/frameforge=verification/generated/instrumentation/scene_fields.jsonl
```

Use field traces to find syntax categories that are expensive despite low
reconstruction value, such as excessive 4x4 TXB signaling, repeated mode
symbols, or transform flags that a larger partition would avoid.

## Source-Code Audit Pointers

Use libaom for practical AV1 screen-share behavior:

- `verification/references/libaom/libaom/av1/encoder/nonrd_pickmode.c`
- `verification/references/libaom/libaom/av1/encoder/nonrd_opt.c`
- `verification/references/libaom/libaom/av1/encoder/partition_search.c`
- `verification/references/libaom/libaom/av1/encoder/rdopt.c`
- `verification/references/libaom/libaom/av1/encoder/txb_rdopt.c`
- `verification/references/libaom/libaom/av1/encoder/encodetxb.c`
- `verification/references/libaom/libaom/av1/encoder/bitstream.c`

Use AVM for AV2-native tool guidance:

- `verification/references/av2/avm/av2/encoder/partition_search.c`
- `verification/references/av2/avm/av2/encoder/partition_strategy.c`
- `verification/references/av2/avm/av2/encoder/rdopt.c`
- `verification/references/av2/avm/av2/encoder/encodetxb.c`
- `verification/references/av2/avm/av2/encoder/encodeframe.c`
- `verification/references/av2/avm/av2/encoder/bitstream.c`
- `verification/references/av2/avm/doc/dev_guide/av2_encoder.dox`

The most relevant tools to study next are palette coding, IntraBC, larger
coding partitions, larger transform partitions, transform skip/identity choices,
adaptive QP/delta-q, and non-RD screen-content mode selection.

## Iteration Checklist

1. Generate first-frame and 50-frame scoreboards for FrameForge and the AV1
   reference at the current PSNR floor.
2. Generate SB maps for one or two representative bad rows, especially the
   Wayland RGB row and one high-bit-depth YUV row.
3. Compare FrameForge against libaom and AVM at the same frame count.
4. Inspect hot superblocks visually or by source coordinates.
5. Check mode/TXB stats to confirm which tools dominate.
6. Make one targeted algorithm change.
7. Re-run the same first-frame and 50-frame tables.
8. Keep the change only if the bitrate reduction is meaningful at the same PSNR
   floor and fps remains acceptable.

Current likely next wins:

- RGB screen content needs palette or IntraBC before more 4x4 intra refinement.
- The first-frame gap needs larger partitions and transform blocks to reduce
  repeated 4x4 syntax.
- The 50-frame gap needs real lossy inter mode decision and rate control beyond
  exact zero-MV residual tiles.
- Delta-q should be used only after the per-SB maps show stable regions where
  quality can be relaxed without dropping below the target PSNR floor.
