# Compiler And Rust Optimization Notes

This document records practical ways to make FrameForge faster while preserving
the same public behavior, bitstream validity, and reconstruction rules. Treat
every item here as something to measure, not as a blanket rule. Codec changes
must still pass strict validation; an optimization that changes reconstruction,
syntax validity, or lossy quality guardrails is a codec change, not a compiler
cleanup.

The local toolchain observed while writing this note was:

```text
rustc 1.95.0 (59807616e 2026-04-14)
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

The repo already has important guardrails:

- `make build` builds the release CLI as `./ff`.
- `make release-check` runs the normal quality gate.
- `make validate-set` and `make compare-compression` provide codec-level
  validation and scoreboards.
- Analysis hooks such as `AV2_SB_BITS=1` and `AV2_LOSSY_STATS=1` are feature
  gated and should stay out of normal product builds.
- The workspace currently has `unsafe_code = "forbid"`, so unsafe SIMD and
  unchecked indexing are not available without an explicit policy change.

## Optimization Order

Use this order for most performance work:

1. Measure a representative workload.
2. Identify the hot function, loop, allocation, or memory path.
3. Add or update a focused benchmark or validation vector.
4. Refactor in safe Rust first.
5. Rebuild with controlled compiler flags.
6. Compare speed, bitstream size, PSNR where relevant, and reconstruction
   checksums.
7. Keep the change only if it improves the measured target without weakening
   validation.

For AV2 and VVC work, prefer workloads that match the current project goals:
small smoke vectors for correctness, high-depth vectors for bit-depth safety,
and local screen-content sets for realistic encoder pressure.

## Baseline Commands

Normal quality gate:

```sh
make release-check
```

Normal release build:

```sh
make build
```

Release build with all normal product features is already the Makefile default:

```sh
make build CARGO_FEATURES=all
```

Targeted validation:

```sh
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=auto
```

Use `VALIDATION_REFERENCE_MODE=required` when claiming reference compatibility.

## Cargo Profile Levers

Cargo profile settings live at the workspace root. Dependency profile settings
inside dependency manifests are ignored by Cargo, so workspace-level profile
experiments belong in the root `Cargo.toml`.

The default `release` profile already uses `opt-level = 3`,
`debug-assertions = false`, `overflow-checks = false`, `incremental = false`,
and `codegen-units = 16`. Important profile knobs:

- `opt-level = 3`: default release speed optimization.
- `lto = "thin"`: cross-crate optimization with lower cost than fat LTO.
- `lto = "fat"` or `lto = true`: stronger whole-program LTO, slower to link.
- `codegen-units = 1`: usually better final code, slower compilation.
- `panic = "abort"`: smaller binaries and simpler code paths for CLI products.
- `debug = "line-tables-only"` or `debug = 1`: useful for profiling symbols.
- `strip = "symbols"`: smaller distribution binary, not useful during profiling.
- `incremental = false`: recommended for release-style optimized builds.

A conservative experimental profile could look like this:

```toml
[profile.optimized]
inherits = "release"
lto = "thin"
codegen-units = 1
panic = "abort"
debug = "line-tables-only"
strip = "none"
```

Build it with:

```sh
cargo build --profile optimized -p frameforge-cli \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"
cp target/optimized/ff ./ff
```

Do not strip symbols until after profiling and debugging are done.

## Direct rustc Flags

For one-off experiments, prefer `RUSTFLAGS` or `cargo rustc` before editing
profiles.

Local-machine benchmark build:

```sh
RUSTFLAGS="-Ctarget-cpu=native" make build
```

`target-cpu=native` lets rustc tune for the current CPU. Use it for local
benchmarks and deployment to a known machine class. Do not use it for generic
release artifacts unless the deployment CPU is controlled.

Thin LTO and one codegen unit without editing `Cargo.toml`:

```sh
RUSTFLAGS="-Clto=thin -Ccodegen-units=1" make build
```

Profiling-friendly release build:

```sh
RUSTFLAGS="-Cdebuginfo=1 -Cforce-frame-pointers=yes -Csymbol-mangling-version=v0" \
  make build
```

Frame pointers make native profilers easier to use. They can reduce peak
performance slightly, so do not measure final speed with frame pointers unless
the production build will also use them.

Useful discovery commands:

```sh
rustc -C help
rustc --print target-cpus
rustc --print target-features
```

## LTO Strategy

Try LTO only after a baseline profile exists.

Suggested sequence:

1. `make build`
2. `RUSTFLAGS="-Clto=thin -Ccodegen-units=1" make build`
3. `RUSTFLAGS="-Clto=fat -Ccodegen-units=1" make build`

Measure all three on the same workload. Thin LTO is often the best first
release setting because it exposes cross-crate optimization without the full
link-time cost of fat LTO. Fat LTO is worth testing for final release binaries,
but it is not automatically faster.

## Profile-Guided Optimization

PGO is the compiler equivalent of telling LLVM which paths are hot. The rustc
workflow is:

```text
instrumented build
-> run representative workloads
-> merge .profraw files into .profdata
-> rebuild using profile data
```

Install LLVM profiling tools:

```sh
rustup component add llvm-tools-preview
```

Find the host target:

```sh
rustc -vV
```

Example workflow:

```sh
PGO_DIR=/tmp/frameforge-pgo
HOST_TARGET=x86_64-unknown-linux-gnu
LLVM_PROFDATA="$HOME/.rustup/toolchains/$(rustup show active-toolchain | cut -d' ' -f1)/lib/rustlib/$HOST_TARGET/bin/llvm-profdata"

rm -rf "$PGO_DIR"
mkdir -p "$PGO_DIR"

RUSTFLAGS="-Cprofile-generate=$PGO_DIR" \
  cargo build --release --target "$HOST_TARGET" -p frameforge-cli \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"

./target/$HOST_TARGET/release/ff encode input_640x360_30_1f_yuv444p8.yuv \
  --encode av2:/tmp/frameforge-pgo-av2.obu --recon /tmp/frameforge-pgo-av2.yuv

"$LLVM_PROFDATA" merge -o "$PGO_DIR/merged.profdata" "$PGO_DIR"

RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata -Cllvm-args=-pgo-warn-missing-function" \
  cargo build --release --target "$HOST_TARGET" -p frameforge-cli \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"
```

Use a mix of representative inputs for the instrumented run. For FrameForge,
that should include:

- lossless AV2 4:2:0, 4:2:2, 4:4:4;
- AV2 lossy QP runs;
- VVC smoke runs;
- high-bit-depth paths;
- local screen-content clips when optimizing screen-share behavior.

PGO can make code worse if the profile does not match production usage. Keep
the profile set versioned or documented when using PGO for release builds.

## LLVM Optimization Remarks

There is no perfect "warn if slow" switch, but LLVM can explain many successful
and missed optimizations.

Start with rustc optimization remarks:

```sh
RUSTFLAGS="-Cremark=all" cargo build --release -p frameforge-cli \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"
```

This is noisy. For vectorization, inspect one crate at a time:

```sh
cargo rustc --release -p frameforge-codecs --features "av2 vvc" --lib -- \
  -C debuginfo=line-tables-only \
  -C "llvm-args=--pass-remarks=loop-vectorize --pass-remarks-missed=loop-vectorize --pass-remarks-analysis=loop-vectorize"
```

Useful pass filters:

- `loop-vectorize`: loop SIMD vectorizer.
- `slp-vectorizer`: straight-line scalar-to-vector packing.
- `inline`: inlining decisions.
- `unroll`: loop unrolling.

Read missed remarks as clues, not final truth. A loop may fail vectorization
because of bounds checks, unknown aliasing, calls, branches, type choices, or
because LLVM's cost model decided scalar code was better.

## Clippy And Lints

Clippy's `perf` group is the first warning layer to use:

```sh
cargo clippy --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale" \
  -- -W clippy::perf
```

For CI-style cleanup:

```sh
cargo clippy --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale" \
  -- -D warnings -W clippy::perf
```

Potentially useful cherry-picked lints:

- `clippy::perf`: low-risk performance suggestions.
- `clippy::large_enum_variant`: catches oversized enum variants that cause
  copies and cache pressure.
- `clippy::large_stack_arrays`: catches large stack allocations.
- `clippy::needless_collect`: catches temporary collections.
- `clippy::redundant_clone`: catches avoidable clones.
- `clippy::unnecessary_to_owned`: catches avoidable allocation.
- `clippy::unwrap_used` and `clippy::expect_used`: useful in non-test hot code,
  but noisy across existing tests.

Do not enable `clippy::restriction` as a group. It intentionally contains
policy lints that can contradict each other and should be selected case by
case.

Rust lints can also be configured in `[workspace.lints.rust]`. The existing
`unsafe_code = "forbid"` setting should stay unless a specific optimized kernel
is approved as an explicit exception.

## Profiling Runtime Hotspots

Use the existing gprof helper for the current AV2 lossless first-frame case:

```sh
make profile-av2-i-lossless
```

For Linux `perf`, build with line debug info and frame pointers:

```sh
RUSTFLAGS="-Cdebuginfo=1 -Cforce-frame-pointers=yes -Csymbol-mangling-version=v0" \
  make build

perf record -F 99 --call-graph fp -- ./ff encode input_640x360_30_1f_yuv444p8.yuv \
  --encode av2:/tmp/frameforge-profile.obu --recon /tmp/frameforge-profile.yuv
perf report
```

Use `--call-graph dwarf` instead of `fp` when frame pointers are unavailable,
but expect more overhead.

Good profiling targets:

- wall time per encoded frame;
- cycles in intra prediction, residual, transform, quantization, entropy, and
  tile payload writing;
- allocation counts and bytes;
- branch misses in mode decisions;
- cache misses in frame/plane access;
- time spent formatting traces or errors in hot paths.

## Benchmarking

Add focused benchmarks before doing risky refactors. `cargo bench` uses the
`bench` profile and supports custom benchmark harnesses. Stable Rust projects
commonly use Criterion for statistically stronger microbenchmarks.

Suggested benchmark groups:

- planar sample read/write and bit-depth conversion;
- AV2 4x4 transform, quantization, reconstruction;
- SAD/SATD or prediction-error kernels;
- palette color counting and palette selection;
- IntraBC or motion-search candidate scoring;
- entropy token emission and tile payload assembly;
- frame copy, plane split, and RGB/GBR conversion.

Benchmark rules:

- Use fixed deterministic inputs.
- Report throughput in samples, pixels, blocks, or frames per second.
- Keep benchmark data small enough for microbenchmarks, then validate with full
  encode runs.
- Never optimize solely for synthetic data if it hurts real validation sets.

## Safe Rust Refactoring Patterns

Most worthwhile Rust speedups come from making invariants visible to LLVM.

### Validate Once, Iterate Simply

Move dimension and buffer length checks to construction or setup code. Hot
loops should operate on already validated slices and fixed spans.

Prefer:

```rust
let row_start = y * stride;
let row = &src[row_start..row_start + width];
let out = &mut dst[row_start..row_start + width];
for (d, s) in out.iter_mut().zip(row) {
    *d = *s;
}
```

over repeatedly checking computed indexes inside the inner loop.

### Use Row Slices And chunks_exact

For image kernels, row slices and `chunks_exact` often remove bounds checks and
make vectorization easier:

```rust
for (src_row, dst_row) in src
    .chunks_exact(src_stride)
    .zip(dst.chunks_exact_mut(dst_stride))
    .take(height)
{
    let src_row = &src_row[..width];
    let dst_row = &mut dst_row[..width];
    for (d, &s) in dst_row.iter_mut().zip(src_row) {
        *d = s;
    }
}
```

This also exposes a predictable contiguous memory pattern to LLVM.

### Prove Non-Aliasing With Split Slices

When two mutable regions come from one buffer, use `split_at_mut` or helper
layout methods to prove they do not overlap:

```rust
let (y_plane, chroma) = frame.split_at_mut(y_len);
let (u_plane, v_plane) = chroma.split_at_mut(chroma_len);
```

This is better than carrying indexes into one large mutable buffer.

### Prefer Fixed Arrays For Small Blocks

Codec kernels often operate on 4x4, 8x8, or 16x16 blocks. Prefer arrays for
small fixed-size working sets:

```rust
let mut coeffs = [0i32; 16];
```

Arrays let LLVM see the exact size, unroll small loops, keep data in registers,
and avoid heap allocation.

### Reuse Scratch Buffers

Avoid allocating per block, per transform, or per symbol group. Put scratch
storage in tile/frame state and clear it between uses:

```rust
scratch.clear();
scratch.extend_from_slice(block_samples);
```

Use `Vec::with_capacity` when the final size is known or tightly bounded.

### Avoid Temporary collect In Hot Paths

Iterator chains are often fine, but `collect::<Vec<_>>()` inside hot loops is a
red flag unless the allocation is essential. Prefer direct iteration, stack
arrays, or reusable scratch.

### Keep Error Formatting Cold

Error strings and `format!` are fine in CLI parsing and setup. In hot codec
paths, validate upfront and keep the inner loop free of formatting. When an
error helper is truly cold, consider:

```rust
#[cold]
fn invalid_geometry_message(width: usize, height: usize) -> String {
    format!("invalid geometry {width}x{height}")
}
```

### Use debug_assert For Proven Invariants

If setup code validates an invariant, use `debug_assert!` in hot code when the
check is only for developer mistakes. `assert!` remains in release builds and
can cost branches or panic paths.

Do not replace validation with `debug_assert!` at public boundaries. Public
input checks must still run in release builds.

### Be Explicit About Overflow Semantics

Release Rust disables overflow checks by default, while debug Rust enables
them. Codec code should not depend on that difference. Use:

- `checked_*` for size and allocation math;
- `saturating_*` for pixel clamp semantics;
- `wrapping_*` only where codec syntax or modular arithmetic requires it;
- wider intermediates for transforms and error scores.

This keeps debug and release behavior aligned.

### Specialize Carefully

When format choice is known after validation, consider separate internal paths
for important cases:

- 8-bit 4:2:0;
- 10-bit 4:2:0;
- 8-bit 4:4:4 RGB/GBR screen content;
- lossless versus lossy residual paths.

Specialization can remove runtime branches and make loops simpler. Avoid
exploding the public API or duplicating whole codecs. Specialize small kernels
and dispatch at construction or frame/tile setup boundaries.

### Avoid Dynamic Dispatch In Inner Loops

Pipeline traits are appropriate at stage boundaries. Inside codec kernels,
prefer enums, generics, direct function calls, or function selection before the
loop. Trait objects and function pointers in per-pixel or per-block loops can
block inlining and vectorization.

### Keep Branches Predictable

Mode decision code naturally has branches. In pixel kernels, prefer moving rare
cases out of the inner loop. For example, handle edges, padding, and partial
blocks outside the full-block fast path when that keeps the main loop straight.

### Use Tables For Repeated Codec Constants

Scan orders, quant tables, CDF defaults, block layouts, and fixed syntax maps
should be static arrays or compact structs. Rebuilding them per block or per
frame wastes time and cache.

## SIMD Strategy

There are three levels of SIMD in Rust:

1. Auto-vectorization from ordinary optimized Rust.
2. Portable SIMD through `std::simd`, currently nightly-only experimental.
3. Architecture intrinsics through `core::arch`.

For this repository, the current default should be:

```text
safe scalar kernel
-> benchmark
-> make loop/vectorization-friendly
-> inspect LLVM remarks or assembly
-> consider SIMD only for proven hotspots
```

Auto-vectorization works best when loops have:

- contiguous slices;
- simple arithmetic;
- no calls inside the inner loop;
- no complicated branches;
- clear non-aliasing;
- fixed or easily analyzable trip counts.

Architecture intrinsics usually require `unsafe` and CPU feature dispatch. That
conflicts with the current workspace `unsafe_code = "forbid"` policy. If SIMD
intrinsics become necessary, isolate them in tiny modules with scalar reference
tests, runtime feature detection, and a clear safety rationale before changing
the lint policy.

For portable binaries, do not compile the whole program with `-Ctarget-feature`
such as `+avx2` unless every deployment CPU supports it. Prefer runtime
dispatch for CPU-specific kernels.

## Post-Link Optimization

LLVM BOLT can optimize an already linked ELF binary using a sampled execution
profile. It is an advanced release step, not a normal development loop.

Use it only after:

- normal profiling has identified stable hot paths;
- LTO and PGO have been tested;
- the release workload is representative;
- the binary is built with enough symbols/relocations for BOLT.

This is probably later-stage work for FrameForge. It may matter once the CLI
has large hot code and stable production workloads.

## Allocator And Memory Behavior

FrameForge currently has no external allocator dependency. Before changing the
global allocator, reduce allocations in hot code:

- preallocate output buffers with known capacities;
- reuse per-tile scratch;
- avoid per-block `Vec`;
- avoid cloning frame planes unless ownership truly requires it;
- stream frame input/output instead of materializing larger-than-needed data;
- keep trace JSON and instrumentation behind feature/runtime gates.

If allocation still dominates after refactoring, compare allocators only with a
representative encode workload. An allocator swap can improve one workload and
hurt another.

## Parallelism

Compiler flags will not create codec-level parallelism. Add parallelism where
the codec structure supports deterministic independent work:

- tiles;
- rows of independent prediction/error scoring;
- frame-level lookahead when future encoder design supports it;
- independent validation/compression jobs.

Rules for parallel codec work:

- preserve deterministic bitstream ordering;
- avoid sharing mutable state in inner loops;
- aggregate per-thread outputs in a fixed order;
- keep small clips single-threaded if thread overhead dominates;
- measure both speed and bitstream impact.

## FrameForge-Specific Hotspot Candidates

Based on the current repository layout, likely optimization targets are:

- `crates/frameforge-codecs/src/av2/lossy420.rs`: prediction, transform,
  quantization, residual scoring, and TXB selection.
- `crates/frameforge-codecs/src/av2/palette_prediction.rs`: palette color
  counting, sorting, dynamic-programming palette choice.
- `crates/frameforge-codecs/src/av2/palette_444.rs`: screen-content palette
  path and block traversal.
- `crates/frameforge-codecs/src/av2/tile.rs` and `tile_payload.rs`: tile
  assembly and entropy payload handling.
- `crates/frameforge-codecs/src/av2/motion.rs` and `ibc.rs`: future motion and
  IntraBC search kernels.
- `crates/frameforge-codecs/src/vvc/residual/`: transform, quantization,
  prediction, and reconstruction.
- `crates/frameforge-core/src/frame.rs` and
  `crates/frameforge-codecs/src/picture.rs`: frame length, bit-depth
  conversion, and planar sample access.

Use profiling before changing any of these. Some files are large because they
contain tests or syntax scaffolding rather than runtime hotspots.

## Validation Requirements For Optimized Kernels

Every optimized correctness-critical kernel should have:

- a simple scalar reference implementation;
- tests over edge values, bit depths, and odd/even dimensions as applicable;
- deterministic random or generated vectors when useful;
- exact reconstruction comparison for lossless;
- PSNR and bitrate comparison for lossy;
- reference decoder checks when a reference decoder is available.

Suggested validation after a codec kernel change:

```sh
make test
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=auto
make validate-set CODEC=av2 VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=auto
```

For release claims:

```sh
make validate-set CODEC=av2 VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
```

## Things To Avoid

- Do not use `target-cpu=native` as a portable release setting.
- Do not rely on release overflow wrapping unless the operation uses
  `wrapping_*` explicitly.
- Do not turn public validation checks into `debug_assert!`.
- Do not enable whole Clippy groups like `restriction`.
- Do not add `#[inline(always)]` everywhere; poor inlining increases code size
  and can reduce instruction-cache locality.
- Do not keep analysis counters, JSON formatting, or environment checks in
  normal hot paths.
- Do not evaluate speed using instrumentation builds such as `AV2_SB_BITS=1` as
  the final runtime baseline.
- Do not accept a faster lossy path without checking quality and reference
  reconstruction behavior.

## Suggested Next Steps

1. Add a `clippy-perf` Makefile target that runs `clippy::perf` with the normal
   product feature set.
2. Add Criterion benchmarks for the AV2 residual/transform path and palette
   selection path.
3. Add a documented optimized profile experiment using ThinLTO and
   `codegen-units = 1`.
4. Create a PGO script once representative AV2/VVC training clips are stable.
5. Use LLVM vectorization remarks on the hottest codec crate functions.
6. Refactor hot loops toward row slices, fixed arrays, scratch reuse, and
   branch-light inner loops.
7. Revisit SIMD only after safe scalar code and compiler-assisted
   vectorization have plateaued.

## Measured Checkpoints

### Source Buffer Reuse And Planar Pack/Unpack

Checkpoint: `post-pack-reuse`.

Changes retained:

- AV2 reuses the source frame buffer across frames instead of allocating it per
  frame.
- AV2 `rgb24` <-> planar GBR conversion uses exact pixel chunks instead of
  manually computed byte offsets.
- VVC input sample unpacking and reconstruction packing use bit-depth-specific
  slice loops after public geometry and length validation has already completed.
- Validation runner gained explicit lossy overrides for geometry sweeps.
- `make benchmark-encode-matrix` records bytes, fps, PSNR where available, and
  output/reconstruction hashes for AV2/VVC lossy/lossless matrices.
- `make validate-geometry-sweep` runs small AV2/VVC geometry sweeps in both
  lossless and lossy modes.

One-off compiler flag probe:

```sh
RUSTFLAGS="-Clto=thin -Ccodegen-units=1 -Cembed-bitcode=yes" \
  make benchmark-encode-matrix \
    ENCODE_MATRIX_RUN=probe-thinlto-1 \
    ENCODE_MATRIX_LIMIT=2 \
    ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/baseline-compiler-opt.json
```

Result: not retained. The first AV2 row was only +0.14 fps and the second was
-0.25 fps versus baseline, while release build time increased substantially.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=post-pack-reuse \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/baseline-compiler-opt.json
```

Matrix totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Codec | Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta |
|---|---|---:|---:|---:|---:|
| AV2 | lossless+predictive | 6.91 | 9.03 | +30.7% | 0 |
| AV2 | qp=24+predictive | 3.16 | 3.77 | +19.3% | 0 |
| VVC | lossless | 0.66 | 0.68 | +3.0% | 0 |
| VVC | lossy | 0.95 | 1.02 | +7.4% | 0 |

The full generated reports for this run were written to:

```text
verification/generated/encode_matrix/baseline-compiler-opt.md
verification/generated/encode_matrix/post-pack-reuse.md
```

Geometry sweep command:

```sh
make validate-geometry-sweep
```

Result: passed. This ran `screenshot-sweep-444`,
`screenshot-sweep-444-10bit`, and `screenshot-sweep-420-10bit-canary` for AV2
and VVC in both lossless and lossy modes. Lossless rows used exact
reconstruction checks; lossy rows required encoded output and internal
reconstruction to be produced.

### VVC Native 4:2:2 Residual And Shared Pixel Metrics

Checkpoint: `vvc-parity-native-422-dc-search`.

Changes retained:

- Core `ChromaSampling` now exposes shared subsampling factors, and core
  planar byte-slice SSE is used by the CLI PSNR path.
- VVC non-lossless residual syntax and reconstruction now keep native 4:2:2
  input instead of routing through the old decoder-compatibility frame.
- VVC residual quantization borrows CTU frames instead of cloning them.
- VVC luma DC residual search uses the actual bit depth and inverse-transform
  response before choosing the DC level.
- The validation runner cleanup path tolerates already-removed generated files.

Rejected probe:

- A luma DCT AC estimator increased the first lossy 4:2:0 row from 7.15 MB to
  8.70 MB, slowed encode from 1.11 fps to 0.89 fps, and only improved PSNR by
  about 0.10 dB, so it was not retained.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-parity-native-422-dc-search \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/post-pack-reuse.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | Notes |
|---|---:|---:|---:|---:|---|
| lossless | 0.68 | 0.71 | +4.4% | 0 | 4:2:0/4:2:2 rows remain exact; 4:4:4 palette bytes unchanged |
| lossy | 1.02 | 0.63 | -38.2% | +31,508,423 | Native 4:2:2 replaces prior compatibility behavior |

Key lossy row deltas:

| Vector | Format | Bytes Delta | FPS Delta | New PSNR | Notes |
|---|---:|---:|---:|---:|---|
| SceneComposition_1_420 | yuv420p8 | -14,344 | +0.02 | 23.700 | DC search gives a small size win |
| SceneComposition_1_422 | yuv422p8 | +5,005,613 | -0.67 | 24.715 | Native 4:2:2 now measures the real path |
| MissionControlClip1_420 | yuv420p10le | -2,186,574 | -0.12 | 19.005 | Bit-depth-aware DC search fixes a poor high-depth response |
| MissionControlClip1_422 | yuv422p10le | +28,703,728 | -1.10 | 18.364 | Native high-depth 4:2:2 needs better mode/residual decisions |
| MissionControlClip1_444 | yuv444p10le | 0 | -0.03 | 65.611 | Existing palette path unchanged |

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-parity-native-422-dc-search.md
```

AV2 sanity matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-shared-pixel-metrics-check \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/post-pack-reuse.json
```

Result: all 12 AV2 rows were byte-identical to `post-pack-reuse`. Totals were
83,531,302 bytes at 9.01 fps for `lossless+predictive` and 41,098,794 bytes at
3.74 fps for `qp=24+predictive`.

The AV2 generated report for this run was written to:

```text
verification/generated/encode_matrix/av2-shared-pixel-metrics-check.md
```

This checkpoint is correctness-positive but exposes the real VVC lossy parity
gap. Next VVC work should focus on mode decisions and residual coding for
4:2:0 and 4:2:2 rather than treating the old non-native 4:2:2 byte counts as a
valid target.

### VVC CTU Traversal Cleanup

Checkpoint: `vvc-direct-luma-nodes`.

Changes retained:

- VVC residual quantization now uses a luma transform-node walker instead of
  constructing a full CABAC-op vector and filtering luma leaves.
- Quantization constructs `VvcCtuPartitionShape` directly when only traversal
  shape is needed, avoiding large zeroed partition-parameter arrays.
- The streaming encoder reuses one scratch CTU frame per input frame and
  removes the full-frame clone on the residual path.
- Obsolete lossy transform observation helpers are test-only or removed, so the
  release path does not compute discarded transforms.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-direct-luma-nodes \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-parity-native-422-dc-search.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.70 | 0.71 | +1.4% | 0 | 0 |
| lossy | 0.63 | 0.65 | +3.2% | 0 | 0 |

All rows were byte-identical to `vvc-parity-native-422-dc-search`; lossless
rows remained exact and lossy PSNR was unchanged. The largest positive row was
the high-depth 4:2:0 lossy case, which moved from 0.53 fps to 0.57 fps in this
matrix run.

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-direct-luma-nodes.md
```

AV2 sanity command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-after-vvc-direct-luma-nodes \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/av2-shared-pixel-metrics-check.json
```

Result: all 12 AV2 rows were byte-identical to
`av2-shared-pixel-metrics-check`. Totals were 83,531,302 bytes at 8.99 fps for
`lossless+predictive` and 41,098,794 bytes at 3.82 fps for
`qp=24+predictive`.

### VVC Direct Residual Extraction

Checkpoint: `vvc-direct-residual-extract`.

Change retained:

- VVC residual quantization now builds luma/chroma residual vectors directly
  from source samples and predictors, instead of first allocating copied sample
  blocks and then allocating residual blocks from those samples. Off-visible
  padding behavior is unchanged: luma padding remains zero-derived and chroma
  padding remains neutral-sample-derived.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-direct-residual-extract \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-direct-luma-nodes.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.71 | 0.73 | +2.8% | 0 | 0 |
| lossy | 0.65 | 0.65 | 0.0% | 0 | 0 |

All rows were byte-identical to `vvc-direct-luma-nodes`; lossless rows remained
exact and lossy PSNR was unchanged. The high-depth 4:2:2 lossless row improved
from 0.46 fps to 0.48 fps in this run.

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-direct-residual-extract.md
```

### VVC Prediction Scratch

Checkpoint: `vvc-prediction-stack-scratch`.

Change retained:

- VVC residual quantization and reconstruction reuse the predicted luma/Cb/Cr
  buffers across transform units within a frame.
- DC intra prediction now keeps top and left reference samples in fixed arrays
  sized to the encoder CTU edge, avoiding per-TU reference-vector allocation.
- Residual reconstruction also uses the direct luma transform-node traversal
  instead of constructing CABAC partition ops only to filter luma leaves.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-prediction-stack-scratch \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-direct-residual-extract.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.73 | 0.74 | +1.4% | 0 | 0 |
| lossy | 0.65 | 0.67 | +3.1% | 0 | 0 |

All rows were byte-identical to `vvc-direct-residual-extract`; lossless rows
remained exact and lossy PSNR was unchanged. The 8-bit 4:2:0 and 4:2:2 lossy
rows each gained about 0.04 fps in this run.

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-prediction-stack-scratch.md
```

AV2 sanity command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-after-vvc-prediction-stack-scratch \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/av2-shared-pixel-metrics-check.json
```

Result: all 12 AV2 rows were byte-identical to
`av2-shared-pixel-metrics-check` and lossy PSNR was unchanged. Totals were
83,531,302 bytes at 9.09 fps for `lossless+predictive` and 41,098,794 bytes at
3.83 fps for `qp=24+predictive`.

### VVC Sparse Active Transform

Checkpoint: `vvc-sparse-active-transform`.

Change retained:

- VVC lossy residual quantizers now fill the stored DC/first-4x4 AC subset
  directly instead of constructing full coefficient vectors and copying the
  subset back out.
- VVC inverse transform now has sparse quantized-block entry points that reuse
  dequantized/vertical scratch buffers and only traverse active coefficient
  rows/columns for the stored first-4x4 subset.
- The general full-coefficient inverse transform remains available to tests,
  but the production residual path no longer allocates coefficient,
  dequantized, and vertical vectors per TU.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-sparse-active-transform \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-prediction-stack-scratch.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.74 | 0.74 | 0.0% | 0 | 0 |
| lossy | 0.67 | 0.70 | +4.5% | 0 | 0 |

All rows were byte-identical to `vvc-prediction-stack-scratch`; lossless rows
remained exact and lossy PSNR was unchanged. The largest row gain was the
8-bit 4:2:0 lossy residual path, which moved from 1.24 fps to 1.39 fps in this
run.

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-sparse-active-transform.md
```

AV2 sanity command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-after-vvc-sparse-active-transform \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/av2-shared-pixel-metrics-check.json
```

AV2 sanity result:

| Mode | Bytes | FPS | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|
| lossless+predictive | 83,531,302 | 9.61 | 0 | 0 |
| qp=24+predictive | 41,098,794 | 3.66 | 0 | 0 |

All AV2 rows remained byte-identical and PSNR-identical to the baseline. The
cross-codec report was written to:

```text
verification/generated/encode_matrix/av2-after-vvc-sparse-active-transform.md
```

Additional validation:

```sh
make test
make validate-geometry-sweep
```

Both checks passed after this checkpoint.

### VVC Fixed Active Residual Scan

Checkpoint: `vvc-fixed-active-scan`.

Change retained:

- VVC residual symbol construction now uses a fixed 16-position diagonal scan
  for the active first 4x4 coefficient group.
- This removes the per-TU grouped full-transform scan allocation. The current
  encoder only populates the first 4x4 residual subset, so scanning beyond that
  group could not change the last significant coefficient or emitted syntax.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-fixed-active-scan \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-sparse-active-transform.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.74 | 0.76 | +2.7% | 0 | 0 |
| lossy | 0.70 | 0.71 | +1.4% | 0 | 0 |

All rows were byte-identical to `vvc-sparse-active-transform`; lossless rows
remained exact and lossy PSNR was unchanged. Residual-backed rows improved
consistently, while the 4:4:4 palette rows were effectively unchanged.

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-fixed-active-scan.md
```

AV2 sanity command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-after-vvc-fixed-active-scan \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/av2-shared-pixel-metrics-check.json
```

AV2 sanity result:

| Mode | Bytes | FPS | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|
| lossless+predictive | 83,531,302 | 8.97 | 0 | 0 |
| qp=24+predictive | 41,098,794 | 3.82 | 0 | 0 |

All AV2 rows remained byte-identical and PSNR-identical to the baseline. The
cross-codec report was written to:

```text
verification/generated/encode_matrix/av2-after-vvc-fixed-active-scan.md
```

Additional validation:

```sh
make test
make validate-geometry-sweep
```

Both checks passed after this checkpoint.

### VVC Carried Residual Reconstruction

Checkpoint: `vvc-carried-reconstruction`.

Change retained:

- VVC lossy residual quantization now returns the reconstructed CTU samples it
  already produced for closed-loop prediction.
- The streaming encoder consumes that carried reconstruction instead of running
  a second prediction and inverse-transform pass from the same coefficients.
- The explicit reconstruction helper remains test-only, with a regression test
  proving the carried reconstruction matches the old explicit path.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-carried-reconstruction \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-fixed-active-scan.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.76 | 0.77 | +1.3% | 0 | 0 |
| lossy | 0.71 | 0.76 | +7.0% | 0 | 0 |

All rows were byte-identical to `vvc-fixed-active-scan`; lossless rows
remained exact and lossy PSNR was unchanged. The gain is concentrated in the
subsampled lossy residual rows because 4:4:4 currently routes through the
palette path.

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-carried-reconstruction.md
```

AV2 sanity command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-after-vvc-carried-reconstruction \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/av2-after-vvc-fixed-active-scan.json
```

AV2 sanity result:

| Mode | Bytes | FPS | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|
| lossless+predictive | 83,531,302 | 9.03 | 0 | 0 |
| qp=24+predictive | 41,098,794 | 3.82 | 0 | 0 |

All AV2 rows remained byte-identical and PSNR-identical to the baseline. The
cross-codec report was written to:

```text
verification/generated/encode_matrix/av2-after-vvc-carried-reconstruction.md
```

Additional validation:

```sh
make test
make validate-geometry-sweep
```

Both checks passed after this checkpoint.

## References

- Cargo profile settings:
  <https://doc.rust-lang.org/cargo/reference/profiles.html>
- rustc codegen options:
  <https://doc.rust-lang.org/stable/rustc/codegen-options/index.html>
- rustc profile-guided optimization:
  <https://doc.rust-lang.org/nightly/rustc/profile-guided-optimization.html>
- rustc lints:
  <https://doc.rust-lang.org/rustc/lints/index.html>
- Clippy lint groups and performance lints:
  <https://doc.rust-lang.org/clippy/index.html>
  <https://doc.rust-lang.org/clippy/lints.html>
- Rust code generation attributes:
  <https://doc.rust-lang.org/reference/attributes/codegen.html>
- Rust architecture intrinsics:
  <https://doc.rust-lang.org/stable/core/arch/>
- Rust portable SIMD:
  <https://doc.rust-lang.org/std/simd/index.html>
- LLVM optimization remarks:
  <https://llvm.org/docs/Remarks.html>
- LLVM vectorizers:
  <https://llvm.org/docs/Vectorizers.html>
- LLVM BOLT:
  <https://github.com/llvm/llvm-project/blob/main/bolt/README.md>
- Cargo benchmarks:
  <https://doc.rust-lang.org/cargo/commands/cargo-bench.html>
- Criterion:
  <https://docs.rs/criterion/latest/criterion/>
