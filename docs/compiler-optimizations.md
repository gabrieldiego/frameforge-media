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
PGO_DIR=verification/generated/profiling/pgo
HOST_TARGET=x86_64-unknown-linux-gnu
LLVM_PROFDATA="$HOME/.rustup/toolchains/$(rustup show active-toolchain | cut -d' ' -f1)/lib/rustlib/$HOST_TARGET/bin/llvm-profdata"

rm -rf "$PGO_DIR"
mkdir -p "$PGO_DIR"

RUSTFLAGS="-Cprofile-generate=$PGO_DIR" \
  cargo build --release --target "$HOST_TARGET" -p frameforge-cli \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"

./target/$HOST_TARGET/release/ff encode input_640x360_30_1f_yuv444p8.yuv \
  --encode av2:verification/generated/profiling/frameforge-pgo-av2.obu \
  --recon verification/generated/profiling/frameforge-pgo-av2.yuv

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
  --encode av2:verification/generated/profiling/frameforge-profile.obu \
  --recon verification/generated/profiling/frameforge-profile.yuv
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

## VVC Lean CABAC Events

Checkpoint: `vvc-cabac-lean-events`.

The VVC CABAC writer used to collect CABAC dump symbols, semantic symbols,
context events, and bin-engine events on every normal encode. Those vectors are
only needed for explicit CABAC dump and test paths, but release encodes paid for
per-bin pushes, repeated context model lookups, and debug trace environment
checks. The writer now records those vectors only when constructed through the
dump-enabled path; normal encode uses the same arithmetic state machine and
emits identical bits without the analysis bookkeeping. The two CABAC trace
environment flags are cached once with `OnceLock`.

This change also adds compile-gated VVC stage timing:

```sh
make build VVC_STATS=1
FRAMEFORGE_VVC_STATS=verification/generated/profiling/vvc_stage_scene420_lossless_1f.jsonl \
  ./ff encode /media/gabriel/storage/YUV/aomctc/b2_scc/SceneComposition_1.y4m \
  --frames 1 \
  --encode vvc:verification/generated/profiling/vvc_stage_scene420_lossless_1f.vvc \
  --recon verification/generated/profiling/vvc_stage_scene420_lossless_1f_recon.yuv \
  --set lossless
python3 scripts/summarize_encoder_instrumentation.py \
  --vvc-stats scene420_lossless/frameforge=verification/generated/profiling/vvc_stage_scene420_lossless_1f.jsonl
```

Normal builds do not compile this instrumentation. Generated traces and
profiling artifacts should stay under `verification/generated/`.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-cabac-lean-events \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-carried-reconstruction.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 0.77 | 1.65 | +114.3% | 0 | 0 |
| lossy | 0.76 | 1.00 | +31.6% | 0 | 0 |

All rows were byte-identical to `vvc-carried-reconstruction`; lossless rows
remained exact and lossy PSNR was unchanged.

First-frame VVC stage traces on `SceneComposition_1_420` after the CABAC event
cleanup showed:

| Case | Top stage | Time share | Notes |
|---|---|---:|---|
| lossless | `ctu_entropy_write` | 74.8% | residual extraction is now secondary at 20.0% |
| lossy | `ctu_quantize` | 71.0% | entropy write is secondary at 25.0% |

The next VVC parity work should split by path: entropy-symbol/CABAC
specialization for lossless and transform/quantization/reconstruction
specialization for lossy.

AV2 sanity command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=av2-after-vvc-cabac-lean-events \
  ENCODE_MATRIX_CODECS=av2 \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/av2-after-vvc-carried-reconstruction.json
```

AV2 sanity result:

| Mode | Bytes | FPS | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|
| lossless+predictive | 83,531,302 | 8.60 | 0 | 0 |
| qp=24+predictive | 41,098,794 | 3.67 | 0 | 0 |

Additional validation:

```sh
cargo test -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs --features "vvc vvc-stats"
make test
make validate-geometry-sweep
```

All checks passed after this checkpoint.

## VVC Direct Residual Symbol Emission

Checkpoint: `vvc-residual-callback-sink`.

Change retained:

- Normal VVC residual entropy coding now emits residual CABAC syntax directly
  while deriving it, instead of always building a `VvcResidualCabacSymbolStream`
  and then replaying it.
- The old symbol-stream constructors and replay path remain available to tests,
  so residual syntax expectations are still checked against recorded symbols.
- Direct residual emission uses typed sink callbacks for last-position,
  significance, level, remainder, and sign syntax, avoiding enum construction
  and dispatch in the normal encoder path.
- The regular CTU residual path and the 4:4:4 palette/IBC residual helpers now
  both use direct residual emission.

Rejected probe:

- A fixed-array pass-1 residual state removed per-TU state allocation, but the
  six-vector matrix showed mixed fps rows after tightening the arrays to the
  active context footprint. The gain was not clean enough to retain.

Profiling note:

- After `vvc-cabac-lean-events`, 40-run first-frame gprof on
  `SceneComposition_1_420` lossless still showed residual symbol construction
  and replay as a major entropy-side cost: `coefficients_with_tool_flags` plus
  `emit` accounted for about 15.5% self time before direct emission.
- After direct emission, the residual replay hotspot disappeared; the next
  durable hotspots are CABAC probability/context encode, DC prediction, and
  residual context derivation.

Matrix commands:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-direct-residual-symbols \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-cabac-lean-events.json

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-residual-callback-sink \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-direct-residual-symbols.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`, combined from the
previous committed checkpoint:

| Mode | `vvc-cabac-lean-events` FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 1.65 | 1.85 | +12.1% | 0 | 0 |
| lossy | 1.00 | 1.13 | +13.0% | 0 | 0 |

All rows were byte-identical across the retained runs; lossless rows remained
exact and lossy PSNR was unchanged. The full retained generated reports were
written to:

```text
verification/generated/encode_matrix/vvc-direct-residual-symbols.md
verification/generated/encode_matrix/vvc-residual-callback-sink.md
```

Additional validation:

```sh
cargo test -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs --features "vvc vvc-stats"
make test
make validate-geometry-sweep
```

All checks passed after this checkpoint.

## VVC Batched AC Projection

Checkpoint: `vvc-separable-chroma-ac`.

Changes retained:

- VVC luma lossy AC quantization now computes the 16 source cell sums once per
  transform unit and derives the first 4x4 Hadamard AC levels from those sums,
  instead of recomputing the same cell sums for each AC coefficient.
- VVC chroma lossy AC quantization now computes the active first-4x4 chroma
  coefficients with a separable projection: one vertical DCT accumulation per
  active coefficient row, then reused horizontal projections for each AC level.
- Luma and chroma DC searches now compute residual sum and SSE together in one
  pass before evaluating candidate DC levels.

Rejected probes:

- `vvc-coeff-scratch` added a reusable dense coefficient scratch buffer to the
  CTU CABAC generator. It was byte-identical, but the six-vector matrix
  regressed from 1.85 to 1.77 fps in lossless and from 1.13 to 1.12 fps in
  lossy, likely because the larger hot generator state hurt layout/cache
  behavior more than it saved allocation work.
- Reusing residual buffers inside the VVC quantizer improved one-frame
  lossless `ctu_quantize` timing, but made the lossy first-frame quantizer
  slower than the luma-AC-only checkpoint and immediately regressed the first
  two lossless matrix rows. The run was stopped and the change was reverted.

First-frame VVC stage trace on `SceneComposition_1_420` lossy:

| Checkpoint | `ctu_quantize` | Timed total | Bytes | PSNR |
|---|---:|---:|---:|---:|
| `vvc-residual-callback-sink` | 303.800 ms | 413.955 ms | 128,845 | 24.283 |
| `vvc-luma-ac-cell-sums` | 192.388 ms | 297.819 ms | 128,845 | 24.283 |

Matrix commands:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-ac-cell-sums \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-residual-callback-sink.json

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-separable-chroma-ac \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-ac-cell-sums.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`, compared with the
previous committed checkpoint:

| Mode | Baseline FPS | New FPS | FPS Delta | Byte Delta | PSNR Delta |
|---|---:|---:|---:|---:|---:|
| lossless | 1.85 | 1.82 | -1.6% | 0 | 0 |
| lossy | 1.13 | 1.35 | +19.8% | 0 | 0 |

The lossless code path is not supposed to consume the batched lossy AC
projection; its mixed row movement is treated as run-to-run/code-layout noise.
All rows were byte-identical to the comparison baselines; lossless rows
remained exact and lossy PSNR was unchanged. The retained generated reports
were written to:

```text
verification/generated/encode_matrix/vvc-luma-ac-cell-sums.md
verification/generated/encode_matrix/vvc-separable-chroma-ac.md
```

Additional validation:

```sh
cargo test -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs --features "vvc vvc-stats"
make test
make validate-geometry-sweep
```

All checks passed after this checkpoint.

## VVC Frame-Slice Lossless Residual

Checkpoint: `vvc-frame-slice-residual`.

Changes retained:

- VVC 4:2:0 and 4:2:2 lossless residual pictures now use one frame slice
  instead of one slice per CTU. This removes repeated slice headers and lets
  CABAC contexts carry across CTUs in the lossless residual path.
- The single-slice lossless quantizer predicts against the carried full-frame
  reconstruction and updates that reconstruction as CTUs are emitted.
- VVC 4:2:0 and 4:2:2 lossy residual pictures deliberately remain one slice
  per CTU for now. The CTU-slice path uses CTU-local prediction, which matches
  the decoder's slice-boundary prediction rules and keeps the previous lossy
  byte counts.
- Normal residual entropy emission uses compact first-4x4 coefficient
  accessors for the active coefficient subset, avoiding full coefficient-vector
  materialization in the common residual syntax path.
- `vvc-stats` frame records now include counters such as slice count,
  single-slice use, TU counts, nonzero counts, and CBF counts.
- VVC SPS signalling now raises the current luma MTT depth to 5, which keeps
  thin high-depth 4:2:0 lossless shapes within the coded partition tree.
- High-depth 4:4:4 palette BDPCM/transform-skip residual coding now emits the
  scaled transform-skip levels expected by VTM and rejects the shortcut when a
  coefficient is not exactly representable at that transform-skip scale.

Rejected probe:

- Using a single frame slice for all 4:2:0/4:2:2 residual modes was not
  retained. It kept the lossless size win, but moved lossy totals from
  311,683,720 bytes to 318,394,921 bytes and reduced matrix throughput to 1.27
  fps. The retained split keeps lossy subsampled rows byte-identical to
  `vvc-separable-chroma-ac`.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-frame-slice-residual \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-separable-chroma-ac.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`, compared with
`vvc-separable-chroma-ac`:

| Mode | Baseline bytes | New bytes | Byte delta | Baseline FPS | New FPS | Notes |
|---|---:|---:|---:|---:|---:|---|
| lossless | 562,246,601 | 547,557,841 | -14,688,760 | 1.82 | 1.78 | size win comes from 4:2:0/4:2:2 frame slices |
| lossy | 311,683,720 | 311,763,094 | +79,374 | 1.35 | 1.31 | subsampled lossy rows are byte-identical; 4:4:4 changed with the high-depth palette fix |

Lossless row deltas:

| Vector | Format | Bytes delta | FPS delta | PSNR |
|---|---|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | -3,013,712 | +0.23 | inf |
| SceneComposition_1_422 | yuv422p8 | -3,318,716 | -0.05 | inf |
| MissionControlClip1_420 | yuv420p10le | -4,053,076 | -0.08 | inf |
| MissionControlClip1_422 | yuv422p10le | -4,382,630 | -0.03 | inf |
| MissionControlClip1_444 | yuv444p10le | +79,374 | -0.06 | 65.612 |

The full generated report for this run was written to:

```text
verification/generated/encode_matrix/vvc-frame-slice-residual.md
```

Additional validation:

```sh
cargo test -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs --features "vvc vvc-stats"
make validate-geometry-sweep
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=screenshot-sweep-444-10bit VALIDATION_REFERENCE_MODE=required
```

Results: both VVC test builds passed with 122 tests, the full AV2/VVC geometry
sweep passed, VVC smoke passed 3/3 with the reference decoder required, and
the high-depth VVC 4:4:4 sweep passed 64/64 with the reference decoder
required.

## VVC Residual Metadata And Pass-1 State

Checkpoints: `vvc-tu-ac-presence-flags`, `vvc-fixed-pass1-state`.

Changes retained:

- VVC quantized TU metadata now carries `*_tu_has_ac` flags next to the AC
  coefficient arrays. CABAC CBF decisions use those flags instead of rescanning
  the 15-entry AC arrays for every luma/Cb/Cr TU.
- Lossless AC extraction computes the AC-present flag while copying the
  first-4x4 AC levels, so lossless does not pay an extra coefficient pass.
- The lossy luma and chroma quantizers return AC-present metadata with the
  selected quantized AC coefficients.
- `VvcResidualPass1State` now uses fixed first-4x4 coefficient state and a
  bounded subblock map instead of allocating three `Vec`s for every residual
  TU. Out-of-first-4x4 coefficient context lookups still return zero, matching
  the current emitted coefficient subset.

Rejected probe:

- Replacing `VvcChromaNeighbourState` with fixed CTU-sized arrays was not
  retained. It preserved bytes and PSNR, but total throughput dropped to 1.80
  fps for lossless and 1.29 fps for lossy against `vvc-borrow-ctu-params`.

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-fixed-pass1-state \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-tu-ac-presence-flags.json
```

VVC totals on `local-aomctc-b2-scc-1080p-lossless-50f`, compared with
`vvc-tu-ac-presence-flags`:

| Mode | Baseline bytes | New bytes | Byte delta | Baseline FPS | New FPS | Notes |
|---|---:|---:|---:|---:|---:|---|
| lossless | 547,557,841 | 547,557,841 | +0 | 1.91 | 2.00 | allocation-free residual pass-1 state |
| lossy | 311,763,094 | 311,763,094 | +0 | 1.35 | 1.41 | allocation-free residual pass-1 state |

The preceding AC-presence checkpoint was also byte-identical against
`vvc-borrow-ctu-params` and improved totals to 1.91 fps lossless and 1.35 fps
lossy.

Additional validation:

```sh
cargo test -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs --features "vvc vvc-stats"
```

Results: both VVC test builds passed with 122 tests.

## VVC Intra Feature Plumbing

Checkpoint: `vvc-intra-feature-default`.

Changes retained:

- VVC now accepts CLI `--qp` and maps it into the emitted slice QP. Chroma QP
  follows the existing VVC lossy chroma offset, preserving the old default when
  `--qp` is omitted.
- Packed `rgb24` source handling moved into the common frame conversion layer.
  AV2 and VVC now use the same reversible `rgb24` <-> planar `gbrp8`
  conversion at the CLI boundary, while codec internals continue to consume
  native planar frames.
- VVC compile-gated instrumentation now includes frame-level stage stats and a
  CTU bit JSONL sink through `FRAMEFORGE_VVC_STATS` and
  `FRAMEFORGE_VVC_CTU_BITS`.
- VVC luma intra mode selection now uses a shared candidate-cost path and can
  select horizontal and vertical prediction in addition to DC and planar.
- Generic VVC `Angular(index)` prediction and CABAC mode signalling are wired
  as infrastructure, but non-cardinal angular modes are not selected by
  default yet. The first probe produced mixed bitrate results, so the default
  selector remains on the smaller H/V candidate set until reference filtering
  and rate-aware selection are implemented.

First-frame VVC lossy deltas versus the previous default-DC/planar checkpoint:

| Vector | Format | Bytes delta | FPS delta | PSNR delta |
|---|---|---:|---:|---:|
| SceneComposition_1_420 | yuv420p8 | -12,639 | +0.02 | +0.088 |
| SceneComposition_1_422 | yuv422p8 | -12,639 | -0.02 | +0.066 |
| Wayland screen capture | rgb24 | -23,391 | +0.00 | +0.059 |
| MissionControlClip1_420 | yuv420p10le | +2,110 | +0.05 | +0.087 |
| MissionControlClip1_422 | yuv422p10le | +2,108 | +0.04 | +0.053 |
| MissionControlClip1_444 | yuv444p10le | +2,102 | -0.01 | +0.030 |

Current six-vector comparison, first frame only. Bytes are summed across the
six rows; FPS and PSNR are unweighted row averages, with full per-vector rows
kept in the generated report.

| Codec | Mode | Total bytes | Avg FPS | Avg PSNR |
|---|---|---:|---:|---:|
| AV2 | lossless | 6,586,445 | 3.04 | inf |
| AV2 | qp=24 | 2,400,148 | 1.33 | 49.418 |
| VVC | lossless | 10,659,047 | 1.96 | inf |
| VVC | qp=24 | 9,198,820 | 0.64 | 18.371 |

Current six-vector comparison, 50 frames:

| Codec | Mode | Total bytes | Avg FPS | Avg PSNR |
|---|---|---:|---:|---:|
| AV2 | lossless | 83,531,302 | 8.83 | inf |
| AV2 | qp=24 | 41,098,794 | 4.83 | 51.805 |
| VVC | lossless | 545,598,292 | 1.86 | inf |
| VVC | qp=24 | 463,160,046 | 0.65 | 18.394 |

Matrix commands:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-intra-feature-default-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-intra-feature-default-50f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy"
```

Generated reports:

```text
verification/generated/encode_matrix/vvc-intra-feature-default-1f.md
verification/generated/encode_matrix/vvc-intra-feature-default-50f.md
```

Instrumentation smoke command:

```sh
make build VVC_STATS=1
FRAMEFORGE_VVC_STATS=verification/generated/profiling/vvc_stats_probe.jsonl \
FRAMEFORGE_VVC_CTU_BITS=verification/generated/profiling/vvc_ctu_bits_probe.jsonl \
  ./ff encode \
  verification/generated/test_vectors/aomctc_b2_SceneComposition_1_420_1920x1080_15_1f_yuv420p8.yuv \
  --frames 1 \
  --encode vvc:verification/generated/profiling/vvc_stats_probe.obu \
  --recon verification/generated/profiling/vvc_stats_probe.recon \
  --qp 24
```

Validation:

```sh
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
cargo test -p frameforge-core --features ""
cargo test -p frameforge-cli encode_job \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"
cargo test -p frameforge-codecs vvc --features "vvc"
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-geometry-sweep GEOMETRY_SWEEP_REFERENCE_MODE=off
```

Results: all commands completed successfully. The geometry sweep covered AV2
and VVC, lossless and lossy, across the current screenshot sweep manifests.

## VVC Unified Lossless Intra Search

Checkpoint: `vvc-unified-lossless-intra-1f`.

Changes retained:

- VVC lossless luma now uses the same Planar/DC/H/V intra candidate machinery
  as lossy instead of forcing the reduced lossless-only path.
- VVC lossless chroma now evaluates Derived plus the existing explicit
  Planar/DC/H/V candidate list, using the same selector path as lossy.
- No mode-selection constants were tuned in this checkpoint. Non-cardinal
  angular modes and CCLM remain feature work rather than enabled defaults.

First-frame six-vector matrix versus `vvc-intra-feature-default-1f`:

| Codec | Mode | Total bytes | FPS | Notes |
|---|---|---:|---:|---|
| AV2 | lossless | 6,586,445 | 2.65 | unchanged reference context |
| AV2 | qp=24 | 2,400,148 | 1.16 | unchanged reference context |
| VVC | lossless | 6,780,255 | 1.03 | -3,878,792 bytes versus prior VVC checkpoint |
| VVC | qp=24 | 10,385,397 | 0.39 | current context only; this patch removes no lossy candidates |

The feature tradeoff is clear: allowing lossless to use the richer intra
candidate set cuts first-frame VVC lossless size by about 36% on the six-vector
screen-content matrix, at the cost of extra intra-search work. This is an
accepted feature checkpoint, not the final tuned path.

High-depth smoke lossless size spot-check after the change:

| Vector | Before | After | Delta |
|---|---:|---:|---:|
| canary_420_10 | 487 | 321 | -166 |
| canary_422_10 | 646 | 408 | -238 |
| canary_444_10 | 1,034 | 580 | -454 |
| canary_420_12 | 656 | 465 | -191 |
| canary_422_12 | 874 | 594 | -280 |
| canary_444_12 | 1,382 | 843 | -539 |

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-unified-lossless-intra-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-intra-feature-default-1f.json
```

Generated report:

```text
verification/generated/encode_matrix/vvc-unified-lossless-intra-1f.md
```

Validation:

```sh
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
cargo test -p frameforge-codecs vvc --features "vvc"
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
make validate-geometry-sweep GEOMETRY_SWEEP_REFERENCE_MODE=off
```

## VVC Base CCLM Chroma Mode

Checkpoint: `vvc-cclm-base-1f`.

Changes retained:

- VVC chroma intra mode selection can now choose the base CCLM/LM chroma mode
  where the current dual-tree CTU syntax allows `cclm_mode_flag`.
- The predictor derives LM parameters from reconstructed luma and neighboring
  chroma templates, and is shared by quantization and reconstruction so the
  internal encoder reconstruction stays aligned with reference decode.
- CCLM usage is counted by the compile-gated `vvc-stats` CTU and frame
  counters as `chroma_mode_cclm`.
- No mode-selection constants were tuned. The checkpoint wires a codec feature
  only: MDLM_L/MDLM_T and 4:2:2 CCLM remain TODO feature work.

First-frame six-vector matrix versus `vvc-unified-lossless-intra-1f`:

| Codec | Mode | Total bytes | FPS | Notes |
|---|---|---:|---:|---|
| AV2 | lossless | 6,586,445 | 2.64 | unchanged reference context |
| AV2 | qp=24 | 2,400,148 | 1.14 | unchanged reference context |
| VVC | lossless | 6,436,959 | 0.94 | -343,296 bytes versus prior VVC checkpoint |
| VVC | qp=24 | 8,828,183 | 0.37 | -1,557,214 bytes versus prior VVC checkpoint |

Most of the immediate win came from RGB and 4:4:4 chroma correlation. The
4:2:2 rows are byte-identical because this checkpoint keeps CCLM disabled for
that sampling mode until the compatible syntax/prediction path is completed.

High-depth smoke lossless size spot-check after the change:

| Vector | Previous | After | Delta |
|---|---:|---:|---:|
| canary_420_10 | 321 | 321 | 0 |
| canary_422_10 | 408 | 408 | 0 |
| canary_444_10 | 580 | 580 | 0 |
| canary_420_12 | 465 | 465 | 0 |
| canary_422_12 | 594 | 594 | 0 |
| canary_444_12 | 843 | 765 | -78 |

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-cclm-base-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-unified-lossless-intra-1f.json
```

Generated report:

```text
verification/generated/encode_matrix/vvc-cclm-base-1f.md
```

Validation:

```sh
cargo check -p frameforge-codecs --features "vvc"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
cargo test -p frameforge-codecs vvc --features "vvc"
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make build
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
make validate-geometry-sweep GEOMETRY_SWEEP_REFERENCE_MODE=off
```

## VVC MDLM Chroma Modes

Checkpoint: `vvc-mdlm-candidates-1f`.

Changes retained:

- VVC now models CCLM as three explicit chroma modes: base LM, MDLM_L, and
  MDLM_T.
- CABAC chroma mode syntax now writes the VTM-shaped `cclm_mode_idx` path:
  base LM uses symbol 0, while MDLM_L and MDLM_T use symbol 1/2 with the
  bypass follow-up bin. `cclm_mode_idx` also has a semantic instrumentation ID
  so CABAC vector dumps stay complete when MDLM is selected.
- The chroma predictor now derives MDLM parameters from extended below-left or
  top-right templates, then reuses the same linear chroma-from-luma fit used by
  base LM.
- The existing lossless/lossy chroma SAD selector evaluates all three LM-family
  candidates when CCLM is legal. No constants or thresholds were tuned.
- `vvc-stats` now records `chroma_mode_cclm_linear`,
  `chroma_mode_mdlm_left`, and `chroma_mode_mdlm_top` in addition to the
  aggregate `chroma_mode_cclm` counter.

First-frame six-vector matrix versus `vvc-cclm-base-1f`:

| Codec | Mode | Total bytes | FPS | Notes |
|---|---|---:|---:|---|
| AV2 | lossless | 6,586,445 | 2.63 | unchanged reference context |
| AV2 | qp=24 | 2,400,148 | 1.15 | unchanged reference context |
| VVC | lossless | 6,395,280 | 0.82 | -41,679 bytes versus prior VVC checkpoint |
| VVC | qp=24 | 6,683,289 | 0.39 | -2,144,894 bytes versus prior VVC checkpoint |

Affected VVC lossy rows improved in both size and PSNR because the new chroma
predictors remove residual energy instead of only moving syntax around. The
largest first-frame wins were the Wayland RGB row, from 2,090,954 bytes at
21.990 dB to 760,612 bytes at 24.373 dB, and the 10-bit 4:4:4 MissionControl
row, from 2,822,393 bytes at 13.830 dB to 2,254,980 bytes at 14.930 dB. The
4:2:2 rows remain byte-identical because CCLM is still disabled for 4:2:2 in
the current syntax gate.

Small reference-validation spot checks versus `vvc-cclm-base-1f`:

| Set | Vector | Previous | After | Delta |
|---|---|---:|---:|---:|
| smoke | checker_420 | 124 | 116 | -8 |
| smoke | blocks_444 | 328 | 251 | -77 |
| high-depth-smoke | canary_444_10 | 580 | 554 | -26 |
| high-depth-smoke | canary_444_12 | 765 | 754 | -11 |

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mdlm-candidates-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-cclm-base-1f.json
```

Generated report:

```text
verification/generated/encode_matrix/vvc-mdlm-candidates-1f.md
```

Validation:

```sh
cargo check -p frameforge-codecs --features "vvc"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
cargo test -p frameforge-codecs vvc --features "vvc"
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make build
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
make validate-geometry-sweep GEOMETRY_SWEEP_REFERENCE_MODE=off
```

## VVC Full Angular Intra Modes

Checkpoint: `vvc-full-angular-1f`.

Changes retained:

- VVC luma intra search now evaluates the full angular mode range 2..66
  instead of only the cardinal horizontal/vertical directional modes.
- Chroma explicit-mode validation now accepts the full VVC angular range,
  including the VDIA replacement candidate used when the co-located luma mode
  collides with the chroma candidate list.
- Angular prediction now uses VVC-style modified-wide-angle remapping for
  rectangular blocks.
- Luma angular prediction now has the VVC four-tap interpolation path,
  smoothing interpolation path, and filtered-reference path used by the
  non-planar angular predictors.
- The negative-angle reference extension now clamps against the physical side
  reference length instead of the scratch buffer length. This fixed the
  reference-decoder mismatch exposed by `blocks_444`.
- `vvc-stats` now emits per-angular-index counters such as
  `luma_mode_angular_21` and `chroma_mode_angular_66` so later search work can
  compare mode distribution directly.

This checkpoint intentionally does not tune thresholds or constants. It expands
the implemented VVC feature surface first; later work should make the expanded
mode set faster with rate-aware pruning or staged candidate generation.

First-frame six-vector matrix versus `vvc-mdlm-candidates-1f`:

| Codec | Mode | Total bytes | FPS | Notes |
|---|---|---:|---:|---|
| AV2 | lossless | 6,586,445 | 2.68 | unchanged reference context |
| AV2 | qp=24 | 2,400,148 | 1.17 | unchanged reference context |
| VVC | lossless | 6,009,752 | 0.18 | -385,528 bytes versus prior VVC checkpoint |
| VVC | qp=24 | 6,715,559 | 0.18 | +32,270 bytes versus prior VVC checkpoint |

The lossless path gets a broad bitrate win from the complete angular mode set.
The lossy path is mixed because exhaustive SAD selection now has more choices
but no rate-aware angular syntax cost yet: three rows shrink, two high-depth
rows grow, and total bytes rise slightly. FPS drops substantially in both VVC
modes because the current implementation evaluates all 65 luma angular
directions per candidate block.

Per-row VVC deltas versus `vvc-mdlm-candidates-1f`:

| Mode | Vector | Bytes | Delta bytes | FPS | PSNR mean |
|---|---|---:|---:|---:|---:|
| lossless | SceneComposition 420 8-bit | 357,191 | -28,049 | 0.22 | inf |
| lossless | SceneComposition 422 8-bit | 431,535 | -31,741 | 0.22 | inf |
| lossless | Wayland RGB 8-bit | 504,666 | -32,621 | 0.11 | inf |
| lossless | MissionControl 420 10-bit | 1,227,075 | -88,685 | 0.21 | inf |
| lossless | MissionControl 422 10-bit | 1,510,580 | -100,052 | 0.21 | inf |
| lossless | MissionControl 444 10-bit | 1,978,705 | -104,380 | 0.18 | inf |
| qp=24 | SceneComposition 420 8-bit | 192,454 | -8,805 | 0.27 | 24.650 |
| qp=24 | SceneComposition 422 8-bit | 987,467 | -10,348 | 0.22 | 20.057 |
| qp=24 | Wayland RGB 8-bit | 721,414 | -39,198 | 0.10 | 24.507 |
| qp=24 | MissionControl 420 10-bit | 883,257 | +19,115 | 0.25 | 15.721 |
| qp=24 | MissionControl 422 10-bit | 1,603,833 | -648 | 0.22 | 14.405 |
| qp=24 | MissionControl 444 10-bit | 2,327,134 | +72,154 | 0.15 | 14.773 |

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-full-angular-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-mdlm-candidates-1f.json
```

Generated report:

```text
verification/generated/encode_matrix/vvc-full-angular-1f.md
```

Validation:

```sh
cargo test -p frameforge-codecs vvc --features "vvc"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
make build
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
make validate-geometry-sweep GEOMETRY_SWEEP_REFERENCE_MODE=off
```

## VVC Staged Angular Search And 4:2:2 CCLM

Checkpoint: `vvc-staged-angular-cclm422-1f`.

Changes retained:

- VVC luma mode selection now keeps the full angular predictor/syntax feature
  surface but no longer evaluates all 65 angular directions for every luma TU.
- The angular search list is generated from VVC default directional families,
  already-coded left/above luma modes, and a source-block structure-tensor
  edge seed. Candidate generation deduplicates by VVC luma mode index.
- After the coarse directional pass, the encoder refines around the best
  angular family before final mode selection. This recovers most of the
  exhaustive-search bitrate while avoiding the global full sweep.
- The edge seed reads visible luma samples with the raw-frame stride, so thin
  coded geometries do not probe padded/coded-space samples.
- CCLM/MDLM chroma prediction is now enabled for 4:2:2. The predictor already
  had 4:2:2 luma downsampling; this checkpoint removes the remaining tool flag
  and residual candidate gates.

First-frame six-vector matrix versus `vvc-full-angular-1f`:

| Codec | Mode | Total bytes | FPS | Notes |
|---|---|---:|---:|---|
| AV2 | lossless | 6,586,445 | 2.48 | unchanged reference context |
| AV2 | qp=24 | 2,400,148 | 1.11 | unchanged reference context |
| VVC | lossless | 5,996,606 | 0.32 | -13,146 bytes, +0.14 fps versus full angular |
| VVC | qp=24 | 5,880,550 | 0.27 | -835,009 bytes, +0.09 fps versus full angular |

The staged search is a speed win without giving up the full predictor feature
surface. The 4:2:2 CCLM enablement more than pays for the small residual
regressions on 4:2:0/RGB/4:4:4 lossy rows: both 4:2:2 lossy rows are much
smaller than the exhaustive-angular baseline and their PSNR improves.

Per-row VVC deltas versus `vvc-full-angular-1f`:

| Mode | Vector | Bytes | Delta bytes | FPS | PSNR mean |
|---|---|---:|---:|---:|---:|
| lossless | SceneComposition 420 8-bit | 357,417 | +226 | 0.42 | inf |
| lossless | SceneComposition 422 8-bit | 424,892 | -6,643 | 0.39 | inf |
| lossless | Wayland RGB 8-bit | 505,362 | +696 | 0.23 | inf |
| lossless | MissionControl 420 10-bit | 1,227,907 | +832 | 0.36 | inf |
| lossless | MissionControl 422 10-bit | 1,500,890 | -9,690 | 0.33 | inf |
| lossless | MissionControl 444 10-bit | 1,980,138 | +1,433 | 0.27 | inf |
| qp=24 | SceneComposition 420 8-bit | 192,458 | +4 | 0.50 | 24.635 |
| qp=24 | SceneComposition 422 8-bit | 273,267 | -714,200 | 0.36 | 24.963 |
| qp=24 | Wayland RGB 8-bit | 723,433 | +2,019 | 0.14 | 24.496 |
| qp=24 | MissionControl 420 10-bit | 891,654 | +8,397 | 0.41 | 15.688 |
| qp=24 | MissionControl 422 10-bit | 1,424,164 | -179,669 | 0.31 | 15.029 |
| qp=24 | MissionControl 444 10-bit | 2,375,574 | +48,440 | 0.20 | 14.690 |

Matrix command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-staged-angular-cclm422-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-full-angular-1f.json
```

Generated report:

```text
verification/generated/encode_matrix/vvc-staged-angular-cclm422-1f.md
```

Validation:

```sh
cargo test -p frameforge-codecs vvc --features "vvc"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
make build
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
make validate-geometry-sweep GEOMETRY_SWEEP_REFERENCE_MODE=off
```

## VVC Residual Path Unification

Checkpoint: `vvc-residual-path-unified`.

This checkpoint keeps the `vvc-staged-angular-cclm422-1f` coding decisions but
removes another layer of lossy/lossless split from the VVC residual encoder.
The CTU luma/chroma mode-search loops now call common TU finalization helpers:
lossless and lossy still produce different coefficients and reconstructions,
but the selected prediction mode flows through one decision path.

The residual syntax configuration also now derives from one residual tool
profile keyed by `VvcResidualCodingMode`. Lossless still enables transform skip
globally because it is required by the current exact residual syntax, while
lossy keeps transform skip disabled until the block selector can actually pick
profitable transform-skip candidates without adding dead syntax flags.

Validation:

```sh
cargo check -p frameforge-codecs --features "vvc"
cargo test -p frameforge-codecs vvc --features "vvc"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Intra Search Instrumentation

Checkpoint: `vvc-intra-search-stats`.

This checkpoint keeps the `vvc-residual-path-unified` bitstreams unchanged in
normal builds while extending the compile-gated `vvc-stats` instrumentation:

- `VvcQuantizedColor` carries gated intra-search counters only when
  `frameforge-codecs/vvc-stats` is enabled.
- Frame stats and CTU bit JSONL records now include luma candidate counts
  split into DC, planar, directional coarse, and directional refinement.
- Chroma counters now split candidate work into derived, explicit, and CCLM
  candidates.
- `scripts/summarize_encoder_instrumentation.py --vvc-stats` now prints a
  compact counter table and caps per-angular-index counters with `--top`.
- The remaining final sampled-color branch now goes through
  `VvcResidualCodingMode`, removing another local lossy/lossless boolean from
  the CTU residual path.

The first-frame six-vector matrix against `vvc-staged-angular-cclm422-1f`
was byte-identical for AV2 and VVC, lossless and QP24 lossy:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| AV2 | lossless | 6,586,445 | 0 |
| AV2 | qp=24 | 2,400,148 | 0 |
| VVC | lossless | 5,996,606 | 0 |
| VVC | qp=24 | 5,880,550 | 0 |

Instrumentation probe on the first SceneComposition 4:2:0 frame, VVC QP24:

| Counter | Value |
|---|---:|
| `luma_tu_count` | 32,400 |
| `luma_candidate_count` | 665,495 |
| `luma_candidate_directional_coarse` | 501,085 |
| `luma_candidate_directional_refinement` | 99,610 |
| `chroma_tu_count` | 32,400 |
| `chroma_candidate_count` | 259,200 |
| `chroma_candidate_explicit` | 129,600 |
| `chroma_candidate_cclm` | 97,200 |

The probe also confirms `ctu_quantize` remains the dominant timed stage at
about 92% of the recorded encode time. That points the next VVC work toward
reducing candidate cost or improving residual/transform efficiency rather than
micro-optimizing file I/O or final reconstruction packing.

Commands:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-intra-stats-1f \
  ENCODE_MATRIX_CODECS="av2 vvc" \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-staged-angular-cclm422-1f.json

make build VVC_STATS=1
FRAMEFORGE_VVC_STATS=verification/generated/profiling/vvc_intra_candidate_stats_probe.jsonl \
FRAMEFORGE_VVC_CTU_BITS=verification/generated/profiling/vvc_intra_candidate_ctu_probe.jsonl \
  ./ff encode \
  verification/generated/test_vectors/aomctc_b2_SceneComposition_1_420_1920x1080_15_1f_yuv420p8.yuv \
  --frames 1 \
  --encode vvc:verification/generated/profiling/vvc_intra_candidate_probe.vvc \
  --recon verification/generated/profiling/vvc_intra_candidate_probe_recon.yuv \
  --qp 24

python3 scripts/summarize_encoder_instrumentation.py \
  --vvc-stats scene420/frameforge=verification/generated/profiling/vvc_intra_candidate_stats_probe.jsonl \
  --top 12
```

## VVC Fast Chroma DC Search

Checkpoint: `vvc-chroma-dc-fast-search-1f`.

This checkpoint replaces the VVC chroma DC quantizer's generic exhaustive
`-255..255` level scan with an exact monotonic search. The fast path finds the
first level at or above the DC target, evaluates that reconstructed value and
the previous one, and keeps the existing strict-improvement tie behavior. When
the decoder-side residual mapping would wrap through `i16` at extreme QP and
bit-depth combinations, the encoder falls back to the old exhaustive selector
so bitstreams remain unchanged.

The new unit test compares the fast selector and the public chroma DC quantizer
against the old exhaustive search across 4/8/16/32-wide TUs, 8/10/12-bit input,
and representative QP values from 0 through 63.

First-frame six-vector matrix versus `vvc-intra-stats-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.34 | 0 |
| VVC | qp=24 | 5,880,550 | 0.40 | 0 |

Per-row lossy VVC FPS deltas in this run were positive by about +0.07 to
+0.17 fps, while lossless rows were unchanged apart from normal timing noise.

Commands:

```sh
cargo test -p frameforge-codecs vvc_chroma_dc_fast_search_matches_exhaustive_search --features vvc
cargo test -p frameforge-codecs vvc --features vvc
cargo check -p frameforge-codecs --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-chroma-dc-fast-search-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-intra-stats-1f.json
```

## VVC Lossy SSE Mode Scoring

Checkpoint: `vvc-lossy-sse-mode-score-1f`.

This checkpoint keeps VVC luma/chroma mode selection on the same shared
candidate path, but makes the candidate score depend on the residual coding
mode:

- lossless still ranks candidates by residual SAD, matching the exact-residual
  entropy proxy used by the current lossless path;
- lossy ranks candidates by residual SSE, matching the distortion term used by
  the QP path and PSNR measurements.

The selector API now stores neutral `score` values instead of SAD-specific
field names. The lossy behavior change is therefore gated at block mode
selection without reintroducing a separate lossy encode path.

First-frame six-vector matrix versus `vvc-chroma-dc-fast-search-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.35 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | -153,481 |

Per-row VVC QP24 deltas:

| Vector | Bytes delta | FPS delta | PSNR |
|---|---:|---:|---:|
| SceneComposition_1_420 | -6,323 | -0.01 | 24.846 |
| SceneComposition_1_422 | -9,138 | +0.00 | 25.205 |
| screen_wayland_activity_rgb | +18,220 | +0.00 | 24.657 |
| MissionControlClip1_420 | -25,060 | +0.01 | 15.870 |
| MissionControlClip1_422 | -51,137 | +0.01 | 15.243 |
| MissionControlClip1_444 | -80,043 | +0.00 | 14.890 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-lossy-sse-mode-score-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-chroma-dc-fast-search-1f.json
```

## VVC Luma Mode Map

Checkpoint: `vvc-luma-mode-map-1f`.

This checkpoint removes an O(prior-TU) scan from VVC luma directional candidate
generation. The quantizer now maintains a CTU-local luma mode map as leaves are
finalized, so left and above candidate seeds are direct lookups instead of
searches through previously visited transform nodes.

The candidate set is unchanged, so the first-frame matrix is byte-identical
against `vvc-lossy-sse-mode-score-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.36 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

Lossless rows improved by up to about +0.03 fps in this run. Lossy rows were
mixed within timing noise, but the cleanup keeps neighbour lookup cost bounded
as we add more VVC intra partition and search features.

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-mode-map-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-lossy-sse-mode-score-1f.json
```

## VVC Co-Located Luma Mode Map

Checkpoint: `vvc-colocated-mode-map-1f`.

This checkpoint reuses the CTU-local luma mode map for chroma's co-located luma
mode lookup. Chroma mode selection previously scanned the already-coded luma TU
list for every chroma TU. The new lookup reads the same center sample from the
mode map, so the candidate decisions and bitstreams stay unchanged while the
lookup cost remains bounded as partitioning work expands.

The first-frame matrix is byte-identical against `vvc-luma-mode-map-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.37 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-colocated-mode-map-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-mode-map-1f.json
```

## VVC Per-TU Transform-Skip Flags

Checkpoint: `vvc-tu-transform-skip-flags-1f`.

This checkpoint moves VVC transform-skip selection from a residual-writer
slice-level assumption into quantized TU metadata. The current decisions remain
unchanged: lossless luma/chroma TUs mark transform-skip, while lossy luma/chroma
TUs do not. The CABAC writer now consumes the per-TU flags, so later lossy
transform-skip trials can be selected at block mode decision time without
reintroducing a separate lossy residual writer.

The first-frame matrix is byte-identical against `vvc-colocated-mode-map-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.37 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-tu-transform-skip-flags-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-colocated-mode-map-1f.json
```

## VVC Per-TU MRL Index

Checkpoint: `vvc-tu-mrl-index-1f`.

This checkpoint moves the VVC multi-reference-line decision into luma TU
metadata. The current selector still emits only MRL index 0, so the CABAC
bitstream remains unchanged. Keeping the index in the quantized CTU lets future
intra prediction trials choose MRL per block without baking that assumption into
the syntax writer.

The first-frame matrix is byte-identical against `vvc-tu-transform-skip-flags-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.37 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-tu-mrl-index-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-tu-transform-skip-flags-1f.json
```

## VVC TU Residual Coding Selector

Checkpoint: `vvc-tu-residual-coding-selector-1f`.

This checkpoint moves the remaining VVC luma/chroma TU residual coding choice
out of finalization's lossy/lossless branch and into a shared block-mode
selector. The current selector still chooses transform-skip for lossless TUs and
transformed residual coding for lossy TUs, so the bitstream is unchanged. The
important cleanup is that future lossy transform-skip or per-block tool trials
can now be selected by the same per-TU decision path instead of adding another
standalone lossy path.

The first-frame matrix is byte-identical against `vvc-tu-mrl-index-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.37 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-tu-residual-coding-selector-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-tu-mrl-index-1f.json
```

## VVC TU Residual Coding Instrumentation

Checkpoint: `vvc-tu-residual-coding-stats`.

This checkpoint extends the compile-gated VVC stats path now that residual
coding is a per-TU decision. Frame stats and CTU-bit JSONL records report
transform-skip and transformed TU counts for luma, Cb, and Cr. Normal builds are
unchanged because the counters are behind `frameforge-codecs/vvc-stats`.

Probe on one 16x16 lossy VVC smoke frame:

| Counter | Total |
|---|---:|
| `luma_tu_count` | 4 |
| `luma_tu_transform_skip_count` | 0 |
| `luma_tu_transformed_count` | 4 |
| `chroma_tu_count` | 4 |
| `chroma_tu_transform_skip_count` | 0 |
| `chroma_tu_transformed_count` | 8 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make build VVC_STATS=1

FRAMEFORGE_VVC_STATS=verification/generated/profiling/vvc_residual_coding_stats_probe.jsonl \
FRAMEFORGE_VVC_CTU_BITS=verification/generated/profiling/vvc_residual_coding_ctu_probe.jsonl \
  ./ff encode \
  verification/generated/test_vectors/black_420_16x16_30_1f_yuv420p8.yuv \
  --frames 1 \
  --encode vvc:verification/generated/profiling/vvc_residual_coding_stats_probe.vvc \
  --recon verification/generated/profiling/vvc_residual_coding_stats_probe_recon.yuv \
  --qp 24

python3 scripts/summarize_encoder_instrumentation.py \
  --vvc-stats probe=verification/generated/profiling/vvc_residual_coding_stats_probe.jsonl \
  --top 8
```

## VVC Luma Partition Selector

Checkpoint: `vvc-luma-partition-selector-1f`.

This checkpoint moves the luma leaf-size decision into the shared
`VvcResidualModeDecisionContext` selector layer. The current policy is still
unchanged: lossy uses the current 8x8 luma leaf target, while lossless uses the
4x4 transform-skip target. The practical effect is that future partition
experiments can be made as mode-selection policy instead of as a separate
lossless/lossy encode path.

The first-frame matrix is byte-identical against
`vvc-tu-residual-coding-selector-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.36 | 0 |
| VVC | qp=24 | 5,727,069 | 0.41 | 0 |

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-partition-selector-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-tu-residual-coding-selector-1f.json
```

## VVC Per-TU MTS Index

Checkpoint: `vvc-tu-mts-index-1f`.

This checkpoint carries an explicit MTS index beside the luma TU residual
coding decision. The selector still chooses index 0 for every TU because
nonzero MTS transform/reconstruction is not wired yet. Keeping the value in
per-TU metadata removes another hardcoded lossy syntax assumption from the
CABAC emitter, while preserving byte-identical streams until mode selection can
legally choose another transform.

The first-frame matrix is byte-identical against
`vvc-luma-partition-selector-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.36 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

## VVC Luma Syntax Tool Selectors

Checkpoint: `vvc-luma-tool-selectors-1f`.

This checkpoint moves the current zero-valued MRL and MTS choices into explicit
luma TU selector functions. The selected values are still zero for every block,
but TU finalization no longer owns those syntax-tool defaults. Future MRL or
MTS experiments can therefore be gated alongside intra mode and residual coding
selection without creating a separate lossy or lossless encode path.

The first-frame matrix is byte-identical against `vvc-tu-mts-index-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,996,606 | 0.37 | 0 |
| VVC | qp=24 | 5,727,069 | 0.40 | 0 |

## VVC Luma MPM Tie-Breaking

Checkpoint: `vvc-luma-mpm-tiebreak-1f`.

This checkpoint makes VVC luma intra mode selection aware of the existing CABAC
MPM coding shape without tuning a rate-distortion constant. Candidate residual
energy remains the primary key; the exact luma mode syntax-bin count is packed
only into the low six bits, so it breaks residual ties in favor of cheaper MPM
signaling. The syntax-bin helper is shared with the CABAC MPM-list logic so the
mode selector and writer stay aligned.

First-frame six-vector matrix versus `vvc-luma-tool-selectors-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,885,070 | 0.36 | -111,536 |
| VVC | qp=24 | 5,714,171 | 0.41 | -12,898 |

Lossy PSNR moved only within small tie-breaker differences: three rows lost
0.014 to 0.038 dB, two rows gained 0.004 to 0.024 dB, and no reconstruction or
reference-validity rule changed.

## VVC Lossless Chroma Syntax Tie-Breaking

Checkpoint: `vvc-lossless-chroma-syntax-tiebreak-1f`.

This checkpoint adds the same residual-dominant syntax tie-breaker to the
shared chroma intra mode selector, but only when the residual mode is lossless.
The syntax helper mirrors the emitted CABAC shape for derived, explicit, and
CCLM chroma modes, so exact residual-score ties prefer the cheaper chroma mode
syntax. An unrestricted lossy probe increased the six-vector QP24 total by
6,875 bytes, so the selector leaves lossy chroma scoring byte-identical to the
previous checkpoint until a fuller rate-distortion cost is available.

First-frame six-vector matrix versus `vvc-luma-mpm-tiebreak-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.37 | -346 |
| VVC | qp=24 | 5,714,171 | 0.41 | 0 |

## VVC Score Policy Selectors

Checkpoint: `vvc-score-policy-selectors-1f`.

This checkpoint moves the remaining VVC residual-score metric choice into an
explicit selector. Lossless still uses SAD, lossy still uses SSE, and the
lossless-only chroma syntax tie-breaker is now selected through the same mode
decision policy layer. The quantizer no longer directly matches on
`VvcResidualCodingMode` while scoring candidates, which keeps lossy/lossless
differences at block mode selection boundaries instead of as a hidden scoring
branch.

The first-frame matrix is byte-identical against
`vvc-lossless-chroma-syntax-tiebreak-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,714,171 | 0.40 | 0 |

## VVC CTU Bit Categories

Checkpoint: `vvc-ctu-category-stats-1f`.

This checkpoint extends the compile-gated VVC CTU JSONL sink with category
counters for partition, luma mode, chroma mode, residual, intra-block-copy,
inter, palette, and other syntax. The counters are syntax-bin costs derived
from the CABAC semantic dump, while `total_symbol_bits` remains the final
arithmetic-coded CTU bit length. The summarizer now normalizes category
percentages against category totals when those domains differ, so VVC
syntax-bin categories do not report impossible shares above 100%.

The instrumented first-frame six-vector matrix was byte-identical against
`vvc-score-policy-selectors-1f` for VVC lossless and QP24 lossy. The current
VVC residual path remains CTU-quantization bound and residual-syntax dominated:

| Measurement | Value |
|---|---:|
| CTU quantize stage share | 89.0% |
| Frame entropy write stage share | 10.2% |
| Residual syntax-bin share | 93.5% |
| Luma-mode syntax-bin share | 2.5% |
| Partition syntax-bin share | 2.1% |

## VVC Transform-Skip Reconstruction Source

Checkpoint: `vvc-ts-recon-from-coeffs-1f`.

This checkpoint removes a hidden assumption from the unified VVC residual TU
finalizer. Transform-skipped luma and chroma TUs now rebuild their residual
samples from the encoded DC plus first-4x4 AC coefficient payload before
updating the encoder reconstruction, rather than copying the full original
residual buffer. Current lossless residual leaves are still 4x4, so the
reconstructed samples and bitstreams are unchanged. For future lossy
transform-skip trials on larger leaves, the finalizer now models the same
coefficient subset the entropy path can actually signal.

The first-frame six-vector matrix was byte-identical against
`vvc-ctu-category-stats-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.39 | 0 |
| VVC | qp=24 | 5,714,171 | 0.46 | 0 |

## VVC TU Coding Decision Unification

Checkpoint: `vvc-tu-decision-unified-1f`.

This checkpoint groups the remaining per-TU luma and chroma tool selections
into explicit coding-decision structs. The CTU quantizer now asks block mode
selection for one luma decision carrying residual coding, MRL index, and MTS
index, and one chroma decision carrying residual coding. The current policy is
unchanged: lossless TUs still choose transform skip, lossy TUs still choose
transformed residuals, and MRL/MTS stay at index 0 until their predictors and
transforms are wired. The important cleanup is that future lossy-only tool
trials can be gated at block mode selection without forking the residual path.

The first-frame six-vector matrix was byte-identical against
`vvc-ts-recon-from-coeffs-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,714,171 | 0.40 | 0 |

## VVC Residual Tail Energy Instrumentation

Checkpoint: `vvc-residual-tail-stats`.

This checkpoint adds compile-gated residual-energy counters to the VVC stats
path. Normal builds and bitstreams are unchanged; with
`frameforge-codecs/vvc-stats`, each quantized CTU now reports total residual
SSE, the portion covered by the currently coded first-4x4 coefficient subset,
and the uncoded tail outside that subset for luma and chroma.

The first-frame matrix was byte-identical against
`vvc-tu-decision-unified-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.42 | 0 |
| VVC | qp=24 | 5,714,171 | 0.45 | 0 |

Probe on the first SceneComposition 4:2:0 frame, VVC QP24:

| Component | Total SSE | First4x4 SSE | Tail SSE | Tail share |
|---|---:|---:|---:|---:|
| luma | 712,894,918 | 169,371,320 | 543,523,598 | 76.2% |
| chroma | 37,585,004 | 37,585,004 | 0 | 0.0% |

The same probe still shows residual syntax as the dominant CTU category:
1,701,400 residual syntax-bin bits, or 88.3% of categorized syntax-bin cost.
The largest CTUs spend about 97% of categorized syntax bins on residuals. This
confirms that the next VVC intra feature work should target wider or staged
coefficient coding for luma before more mode-search constants.

Commands:

```sh
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make build VVC_STATS=1
FRAMEFORGE_VVC_STATS=verification/generated/profiling/vvc_residual_tail_stats_probe.jsonl \
FRAMEFORGE_VVC_CTU_BITS=verification/generated/profiling/vvc_residual_tail_ctu_probe.jsonl \
  ./ff encode \
  verification/generated/test_vectors/aomctc_b2_SceneComposition_1_420_1920x1080_15_1f_yuv420p8.yuv \
  --frames 1 \
  --encode vvc:verification/generated/profiling/vvc_residual_tail_probe.vvc \
  --recon verification/generated/profiling/vvc_residual_tail_probe_recon.yuv \
  --qp 24

python3 scripts/summarize_encoder_instrumentation.py \
  --vvc-stats scene420/frameforge=verification/generated/profiling/vvc_residual_tail_stats_probe.jsonl \
  --top 12

python3 scripts/summarize_encoder_instrumentation.py \
  --sb-bits scene420/frameforge=verification/generated/profiling/vvc_residual_tail_ctu_probe.jsonl \
  --top 5

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-residual-tail-stats-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-tu-decision-unified-1f.json
```

## VVC 8x8 Residual Context State

Checkpoint: `vvc-pass1-8x8-context-1f`.

This checkpoint removes another first-4x4 assumption from VVC residual context
derivation. `VvcResidualPass1State` can now track pass-1 coefficient presence
and template magnitudes across the current production 8x8 luma TU footprint,
while the emitted coefficient scan still remains the existing first-4x4 subset.
That means the normal bitstreams are unchanged, but the context model is ready
for a later grouped-subblock scan to set neighbour state outside the first
subblock.

The first-frame six-vector matrix was byte-identical against
`vvc-residual-tail-stats-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.42 | 0 |
| VVC | qp=24 | 5,714,171 | 0.46 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_residual_pass1_state_tracks_8x8_neighbour_coefficients --features vvc
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-pass1-8x8-context-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-residual-tail-stats-1f.json

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Grouped 8x8 Residual Syntax

Checkpoint: `vvc-grouped-8x8-syntax-1f`.

This checkpoint wires the generic VVC luma coefficient path for grouped 8x8
residual syntax. It adds last-significant coefficient suffix bins, 4x4 subblock
scan grouping inside 8x8 TUs, reverse subblock traversal, and `sb_coded_flag`
emission for intermediate coded subblocks. The production quantized TU payloads
still feed the existing first-4x4 coefficient subset, so normal bitstreams are
unchanged. This is a syntax prerequisite for later coding wider luma residual
coefficients from the unified TU mode decision.

The first-frame six-vector matrix was byte-identical against
`vvc-pass1-8x8-context-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.35 | 0 |
| VVC | qp=24 | 5,714,171 | 0.40 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_residual_symbol_stream_supports_grouped_8x8_luma_scan --features vvc
cargo test -p frameforge-codecs vvc_residual_ac_symbol_stream_uses_spec_context_derivations --features vvc
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-grouped-8x8-syntax-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-pass1-8x8-context-1f.json

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Luma Coefficient Storage

Checkpoint: `vvc-luma-coeff-storage-1f`.

This checkpoint widens VVC luma TU coefficient storage from the first 4x4 AC
subset to a compact 8x8-capable payload while keeping chroma at its 4x4 AC
shape. The CTU body now calls generalized luma residual emission helpers, and
the inverse transform / transform-skip reconstruction derive luma coefficient
positions from the coded coefficient extent instead of a hard-coded 4x4 shape.

The default luma quantizer still selects the legacy first-subblock projection.
A direct DCT 8x8 candidate is wired as an implementation building block, but it
is not selected by default because the initial matrix increased bitrate
substantially and lowered high-depth PSNR. That keeps this checkpoint as a
non-regressive plumbing step for a future rate/distortion selector rather than
a quality-mode fork.

The first-frame six-vector matrix was byte-identical against
`vvc-grouped-8x8-syntax-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.35 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-coeff-storage-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-grouped-8x8-syntax-1f.json

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Gated Luma DCT Candidate

Checkpoint: `vvc-luma-dct-selector-gated-1f`.

This checkpoint adds the implementation pieces for a per-8x8 luma AC candidate
selector: a direct DCT-II coefficient path, reconstructed-residual SSE scoring,
and a QP/bit-depth scaled coefficient-cost estimate. The production selector is
compile-time disabled by `VVC_ENABLE_EXPERIMENTAL_LUMA_DCT_COEFF_SELECTION`
because enabling it exposed a residual syntax mismatch against VTM.

The enabled trial was useful but not committable as production behavior:
`smoke/checker_420` failed VTM decode with `Expecting a terminating bit`, and
the first local SceneComposition vector decoded with a reconstruction checksum
mismatch. The one-frame matrix from that enabled trial improved lossy PSNR by
about 0.4 to 1.5 dB, but increased total lossy bytes by about 282 KiB and
dropped FPS modestly. The next residual feature step should therefore fix
multi-subblock residual syntax/reference compatibility before the selector is
allowed to pick the DCT payload.

With the selector gated off, the first-frame six-vector matrix was
byte-identical against `vvc-luma-coeff-storage-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.35 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale"

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-dct-selector-gated-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-coeff-storage-1f.json

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Residual Coding Policy

Checkpoint: `vvc-residual-policy-unified-1f`.

This checkpoint makes the unified VVC residual path explicit by introducing a
single `VvcResidualCodingPolicy` for CTU quantization. The policy carries the
residual-mode context, luma leaf-size selection, residual score metric, chroma
syntax tie-breaker, intra-mode selection, and per-TU coding decisions. Lossless
and lossy still select different tools where needed, but those differences now
live at block-mode selection boundaries instead of being pulled piecemeal by
the quantizer.

The test-only residual reconstruction helper was also updated to consume the
per-TU transform-skip flags. It now reconstructs planar 4:2:0, 4:2:2, and
4:4:4 residual frames through the same transformed or transform-skip metadata
used by the encoder path.

The first-frame six-vector matrix was byte-identical against
`vvc-luma-dct-selector-gated-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.35 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-residual-policy-unified-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-dct-selector-gated-1f.json

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Progressive Residual Contexts

Checkpoint: `vvc-progressive-residual-contexts-1f`.

This checkpoint changes production VVC coefficient emission to derive residual
CABAC contexts from a progressively populated pass-1 coefficient state, matching
decoder-order residual traversal. The symbolic residual stream remains
test-only, while production now uses a compact delayed-bypass symbol queue for
second-pass remainders and bypass-coded levels.

The active default path is byte-identical against
`vvc-luma-dct-selector-gated-1f`, so this is a compatibility cleanup for larger
transformed intra-block experiments rather than a tuned coding change:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.35 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-progressive-residual-contexts-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-dct-selector-gated-1f.json

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

## VVC Stored Residual Emitter Unification

Checkpoint: `vvc-stored-residual-unified-1f`.

This checkpoint removes the last production CTU-body branch that chose separate
stored-coefficient emitter wrappers for transformed and transform-skipped VVC
TUs. The luma and chroma CTU emitters now pass the selected TU residual coding
mode as data into one stored-coefficient entry point per component family. This
keeps lossy/lossless behavior gated at block-mode selection while sharing the
same residual syntax implementation.

The change is intentionally byte-neutral against
`vvc-luma-dct-selector-gated-1f`:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-stored-residual-unified-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-dct-selector-gated-1f.json
```

## VVC CU-Level MTS Hook

Checkpoint: `vvc-cu-mts-hook-1f`.

This checkpoint moves the default `mts_idx` syntax hook out of residual
coefficient emission and into the luma CU body after residual coding, matching
the VTM `cu_residual()` order. Nonzero MTS remains asserted off until matching
forward/inverse transform support is implemented, but the syntax gate now has
the right owner: explicit intra MTS, transformed luma TU, non-DC residual, and
MTS-sized CU.

The default product configuration does not enable explicit MTS, so the change
is byte-neutral against `vvc-stored-residual-unified-1f`:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-cu-mts-hook-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-stored-residual-unified-1f.json
```

## VVC Integer Directional Seed

Checkpoint: `vvc-integer-directional-seed-1f`.

This checkpoint changes the VVC luma directional source-seed gradient scan from
per-sample floating-point accumulation to integer accumulation followed by a
single floating-point angle calculation. The selected orientation is unchanged;
the update removes unnecessary FP work from the hot intra candidate-generation
path and keeps the later candidate evaluation identical.

The first-frame six-vector matrix is byte-neutral against `vvc-cu-mts-hook-1f`:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-integer-directional-seed-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-cu-mts-hook-1f.json
```

## VVC Luma Mode Cell Map

Checkpoint: `vvc-luma-mode-cell-map-1f`.

This checkpoint replaces per-sample luma intra-mode neighbour maps with 4x4
cell maps in both the quantization search state and CABAC MPM state. Current
VVC luma leaves are aligned to at least 4x4, so left/above and chroma
co-located mode queries see the same selected modes while mark operations write
up to 16x fewer entries.

The first-frame six-vector matrix is byte-neutral against
`vvc-integer-directional-seed-1f` and shows small fps improvements on several
rows:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-mode-cell-map-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-integer-directional-seed-1f.json
```

## VVC Split Neighbour Cell Maps

Checkpoint: `vvc-neighbour-cell-map-1f`.

This checkpoint extends the cell-map approach to the remaining split-context
neighbour state. Luma split metadata now uses 4x4 cells, and chroma split
metadata uses 2x2 chroma-sample cells so 4:2:0 boundary leaves still keep
distinct context information. This removes per-sample neighbour writes from
both lossy and lossless VVC coding-tree walks without changing syntax or
reconstruction.

The first-frame six-vector matrix is byte-neutral against
`vvc-luma-mode-cell-map-1f`:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-neighbour-cell-map-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-mode-cell-map-1f.json
```

## VVC Finalized Residual Blocks

Checkpoint: `vvc-finalized-residual-blocks-1f`.

This checkpoint removes the remaining duplicated luma/chroma TU finalizer
branches that directly looked like lossy-vs-lossless paths. The finalizers now
consume the per-block `VvcTuResidualCodingMode` selected by block mode policy,
build a common finalized residual block, reconstruct it through the matching
transform-skip or transformed helper, and then fill the visible reconstruction.
This keeps lossy-specific and lossless-specific behavior at TU mode selection
boundaries instead of as independent finalization paths.

The reference-incompatible experimental 8x8 luma DCT coefficient selector
remains compile-time disabled. The associated residual syntax mismatch still
needs to be fixed before that candidate can be selected by production mode
decision.

The first-frame six-vector matrix is byte-neutral against
`vvc-neighbour-cell-map-1f`:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-finalized-residual-blocks-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-neighbour-cell-map-1f.json
```

## VVC Progressive Rice State

Checkpoint: `vvc-progressive-rice-remcap-1f`.

This checkpoint tightens the shared VVC residual syntax path without changing
mode decisions. Go-Rice parameter derivation now uses the progressively decoded
residual state for both second-pass remainders and bypass-coded coefficients,
matching the state visible to the decoder instead of consulting final
coefficients. The CABAC EP absolute-remainder helper also now applies VVC's
`maxLog2TrDynamicRange` prefix cap.

An attempted lossless luma leaf-size unification to 8x8 exposed a remaining
reference incompatibility in 8x8 transform-skip coefficient syntax:
VTM rejected the stream at slice termination. Lossless luma therefore remains
gated to 4x4 leaves at block mode selection while lossy luma keeps the 8x8
leaf path. This keeps the unified finalizer/syntax machinery validated without
weakening reference-decoder checks.

The first-frame six-vector matrix is byte-neutral against
`vvc-finalized-residual-blocks-1f`:

| Codec | Mode | Total bytes | Byte delta |
|---|---|---:|---:|
| VVC | lossless | 5,884,724 | 0 |
| VVC | qp=24 | 5,714,171 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-progressive-rice-remcap-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-finalized-residual-blocks-1f.json
```

## VVC Last-Significant Suffix Order

Checkpoint: `vvc-last-sig-suffix-order-1f`.

This checkpoint fixes the VVC residual emitter order for last-significant
coefficient positions that require suffix bins. The direct CABAC path and the
test-only symbolic stream now emit X and Y prefixes first, then X and Y suffixes,
matching VTM's `last_sig_coeff()` ordering. The current default product path is
byte-neutral because lossless luma still selects 4x4 leaves and lossy luma does
not change mode decisions, but the fix makes 8x8 residual syntax
reference-compatible.

After this fix, a trial that changed lossless luma leaves from 4x4 to 8x8 passed
both VVC smoke and high-depth reference validation. It was not kept as the
default because the first-frame six-vector lossless total rose from 5,884,724
bytes to 6,231,140 bytes, a +346,416 byte regression, while lossy stayed
byte-identical. A future 4x4/8x8 selector should therefore be rate-aware rather
than a global leaf-size switch.

The active first-frame six-vector matrix is byte-neutral against
`vvc-progressive-rice-remcap-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.35 | 0 |
| VVC | qp=24 | 5,714,171 | 0.40 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-last-sig-suffix-order-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-progressive-rice-remcap-1f.json
```

## VVC MRL Syntax Capability

Checkpoint: `vvc-mrl-syntax-1f`.

This checkpoint wires the CABAC emission shape for nonzero VVC luma
multi-reference-line indices. The production selector still returns MRL index
0 because luma prediction has not yet been shifted to the additional reference
lines, but the CTU body can now encode VTM's supported `MULTI_REF_LINE_IDX`
values `[0, 1, 2]` instead of asserting on nonzero values. This keeps MRL as a
future block-mode-selection tool without a separate lossy/lossless path.

The new unit coverage sets a below-top-line luma TU to indices 0, 1, and 2 and
checks that the CABAC bitstreams differ. Normal encoding is byte-neutral against
`vvc-last-sig-suffix-order-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_luma_mrl_syntax_supports_nonzero_reference_lines --features vvc
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mrl-syntax-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-last-sig-suffix-order-1f.json
```

## VVC MTS Syntax Capability

Checkpoint: `vvc-mts-syntax-1f`.

This checkpoint wires the CABAC emission shape for VVC explicit intra MTS
indices. The CTU body now accepts VTM's non-transform-skip MTS types:
`DCT2_DCT2=0`, `DST7_DST7=2`, `DCT8_DST7=3`, `DST7_DCT8=4`, and
`DCT8_DCT8=5`. `SKIP=1` remains represented by the existing transform-skip
flag instead of the post-residual `mts_idx` syntax.

The production selector still returns `DCT2_DCT2` until matching forward and
inverse non-DCT transforms are available, so normal encoding should remain
byte-neutral against `vvc-mrl-syntax-1f`. The added unit coverage forces each
non-default MTS index through a 16x16 luma TU with AC coefficients and checks
that the CABAC bitstreams differ.

The first-frame six-vector matrix is byte-neutral against `vvc-mrl-syntax-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_luma_mts_syntax_supports_non_default_transform_indices --features vvc
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mts-syntax-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-mrl-syntax-1f.json
```

## VVC Chroma Sample Decision Source

Checkpoint: `vvc-chroma-sample-from-tu-decision-1f`.

This checkpoint removes the last direct residual-mode branch from VVC residual
CTU output finalization. The legacy sampled chroma fields now derive their
lossless-versus-quantized value from the finalized chroma TU transform-skip
metadata, which is selected by `VvcChromaTuCodingDecision`. That keeps even the
compatibility fields behind the unified per-block decision path instead of
checking the global lossy/lossless mode.

The first-frame six-vector matrix is byte-neutral against `vvc-mts-syntax-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,714,171 | 0.39 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-chroma-sample-from-tu-decision-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-mts-syntax-1f.json
```

## VVC Luma DCT Selector Enabled

Checkpoint: `vvc-luma-dct-selector-enabled-1f`.

This checkpoint promotes the luma 8x8 DCT coefficient candidate from a disabled
implementation hook to the default lossy selector candidate. Earlier trials
failed VTM validation, but the later residual suffix-order and grouped-syntax
fixes made the wider luma coefficient payload reference-compatible. The
selector remains a compile-time constant so it can be bisected quickly, but the
normal build now evaluates the legacy first-subblock payload against the DCT
payload and chooses the better reconstructed-residual score.

Lossless remains byte-neutral because the block-mode selector still chooses
transform skip there. Lossy first-frame output trades more bytes and a small
speed hit for PSNR gains on every row:

| Vector | Format | Bytes delta | FPS delta | PSNR delta |
|---|---|---:|---:|---:|
| Scene 420 | yuv420p8 | +52,942 | -0.04 | +1.263 |
| Scene 422 | yuv422p8 | +53,012 | -0.02 | +0.936 |
| Wayland RGB | gbrp8 | +89,544 | -0.00 | +0.700 |
| Mission 420 | yuv420p10le | +55,876 | -0.08 | +0.469 |
| Mission 422 | yuv422p10le | +18,301 | -0.03 | +0.418 |
| Mission 444 | yuv444p10le | +13,202 | -0.02 | +0.249 |

First-frame six-vector totals against
`vvc-chroma-sample-from-tu-decision-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,997,048 | 0.36 | +282,877 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-dct-selector-enabled-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-chroma-sample-from-tu-decision-1f.json
```

## VVC MTS Transform Plumbing

Checkpoint: `vvc-mts-transform-plumbing-1f`.

This checkpoint passes the selected luma `mts_index` through transformed luma
quantization and inverse reconstruction. The selector still returns
`DCT2_DCT2=0`; this checkpoint made transform choice reach the transform
boundary from the unified block-mode decision instead of staying as syntax-only
metadata.

The first-frame six-vector matrix is byte-neutral against
`vvc-luma-dct-selector-enabled-1f`:

| Codec | Mode | Total bytes | FPS | Byte delta |
|---|---|---:|---:|---:|
| VVC | lossless | 5,884,724 | 0.36 | 0 |
| VVC | qp=24 | 5,997,048 | 0.36 | 0 |

Commands:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"

make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required

make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mts-transform-plumbing-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-luma-dct-selector-enabled-1f.json
```

## VVC RGB 4:4:4 Signaling And MRL Plumbing

Checkpoint: `vvc-rgb444-mrl-plumbing`.

This cleanup fixes the VVC 4:2:2/4:4:4 profile signaling and makes the RGB
path explicit. Residual 4:2:2/4:4:4 streams now select VVC 4:4:4-capable
profiles (`general_profile_idc` 33 through 10-bit, 34 above 10-bit), including
planar `gbrp8` input. `gbrp8` still flows as full-resolution green, blue, red
planes; VVC/VTM disallow identity matrix coefficient `0` when
`sps_chroma_format_idc=3`, so the VUI signals full-range sRGB-compatible
primaries/transfer with matrix coefficient `2` left unspecified.

The validation/test plumbing stays byte-oriented:

- Native `gbrp8` compares source, internal reconstruction, and reference
  reconstruction directly as planar GBR.
- Legacy packed `rgb24` remains a shared driver conversion boundary and is
  only normalized in validation when a reference decoder emits planar GBR.
- The VVC reference-comparison script repacks legacy packed `rgb24` to planar
  GBR before invoking VTM; native `gbrp8` is passed through unchanged.

Two reference-compatibility bugs were fixed while validating the planar RGB
path:

- Positive VVC angular prediction now replicates the formal main-reference
  extension instead of reading real pixels beyond the VTM reference span.
- Luma MPM remaining-mode coding now uses the correct circular angular
  threshold at the 2/63 boundary.
- Nonzero-MRL luma mode syntax now follows VTM's rule: MRL modes must be
  MPM-coded, skip `IntraLumaMpmFlag`, and cannot use planar prediction.

MRL is now syntax-enabled and selected conservatively. The luma prediction,
final reconstruction, and CABAC syntax plumbing accept an explicit MRL index.
The predictor now builds shifted reference lines for angular/H/V nonzero-MRL
trials, and the quantizer can score DC and angular/H/V nonzero-MRL candidates
when they are eligible for MPM-coded MRL syntax. The block-mode search now keeps
frame-wide luma mode neighbours, matching the CABAC neighbour shape across CTU
boundaries. MRL remains gated on the CTU top line, where higher reference lines
are unavailable.

Validation:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
python3 -m py_compile \
  scripts/compare_reference_compression.py \
  scripts/run_validation_set.py \
  scripts/benchmark_encode_matrix.py \
  scripts/generate_test_vectors.py \
  scripts/convert_rgb24_to_gbrp8.py \
  scripts/capture_wayland_portal_rgb.py
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
./ff encode verification/generated/rgb_signalling_check/wayland_crop64_gbrp8.rgb \
  --video 64x64:gbrp8 --frames 1 --fps 30 \
  --encode vvc:verification/generated/rgb_signalling_check/wayland_crop64_gbrp8_check.vvc \
  --recon verification/generated/rgb_signalling_check/wayland_crop64_gbrp8_check.recon.rgb \
  --set lossless
python3 scripts/reference_tools.py decode --codec vvc \
  --bitstream verification/generated/rgb_signalling_check/wayland_crop64_gbrp8_check.vvc \
  --output verification/generated/rgb_signalling_check/wayland_crop64_gbrp8_check.reference.rgb \
  --no-build
cmp -s \
  verification/generated/rgb_signalling_check/wayland_crop64_gbrp8_check.recon.rgb \
  verification/generated/rgb_signalling_check/wayland_crop64_gbrp8_check.reference.rgb
```

## VVC Angular MRL, Frame-Wide Modes, And Guarded MTS

Checkpoint: `vvc-mrl-rd-score-1f`.

This checkpoint finishes the first validated slice of the remaining VVC intra
tool gaps:

- Angular/H/V MRL prediction uses shifted reference-line sampling instead of
  reusing the base reference line.
- MRL selection is active below the CTU top line and is gated by the same MPM
  eligibility required by VTM syntax.
- Luma mode neighbours are tracked in a frame-wide 4x4-cell map and reused
  across CTUs, so the selector can make the same left/top MPM decision as the
  CABAC writer.
- Lossy MRL selection scores the quantized candidate it will actually emit:
  reconstructed residual SSE plus coefficient and MRL syntax cost. Lossless
  keeps the cheaper raw residual score.
- Explicit intra MTS signaling is enabled for lossy residual streams, and
  luma MTS indices `2..=5` are carried through quantization, inverse transform,
  reconstruction, and syntax tests.
- Non-DCT2 MTS production selection remains disabled. A checker-smoke probe
  with active non-DCT2 MTS produced a VTM reconstruction checksum mismatch even
  though VTM accepted the bitstream. Keep the gate closed until the transform
  and coefficient constraints are proven VTM-exact.

Validation:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_luma_mrl --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Strict VTM validation passed for all smoke rows and all high-depth smoke rows.
The high-depth lossless byte counts were unchanged from the previous validated
checkpoint: 322, 409, 555, 475, 594, and 768 bytes.

First-frame six-vector matrix versus `vvc-mts-transform-plumbing-1f`:

| Mode | Previous bytes | Current bytes | Byte delta | Previous FPS | Current FPS |
|---|---:|---:|---:|---:|---:|
| lossless | 5,884,724 | 5,856,819 | -27,905 | 0.36 | 0.33 |
| qp=24 | 5,997,048 | 5,541,589 | -455,459 | 0.36 | 0.29 |

The RD-aware MRL scoring step by itself was byte-neutral for lossless and
reduced every lossy row versus `vvc-frame-luma-mode-state-1f`:

| Mode | Vector | Bytes | FPS | PSNR | Byte delta |
|---|---|---:|---:|---:|---:|
| qp=24 | SceneComposition_1_420 | 233,302 | 0.49 | 26.131 | -570 |
| qp=24 | SceneComposition_1_422 | 311,833 | 0.41 | 26.170 | -2,434 |
| qp=24 | screen_wayland_activity_rgb | 809,785 | 0.20 | 25.393 | -11,496 |
| qp=24 | MissionControlClip1_420 | 823,645 | 0.32 | 16.850 | -9,961 |
| qp=24 | MissionControlClip1_422 | 1,271,391 | 0.28 | 16.074 | -22,444 |
| qp=24 | MissionControlClip1_444 | 2,091,633 | 0.23 | 15.619 | -35,493 |

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mrl-rd-score-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-frame-luma-mode-state-1f.json
```

## VVC Intra Parity Checklist

Checkpoint in progress: `vvc-intra-parity`.

The AV2 encoder is still ahead of the VVC intra encoder in both implemented
mode surface and mode-selection efficiency. The VVC residual path is now mostly
unified across lossy/lossless, bit depth, and chroma sampling, so new intra work
should land as selectable tools inside that path rather than as separate
format-specific encoders.

Current gaps to close against AV2-style intra behavior:

1. Reuse finalized RD decisions. VVC lossy MRL scoring quantizes candidate
   residuals; TU finalization must reuse the selected quantized block instead
   of recomputing it.
2. Rank more predictor choices by emitted/reconstructed cost. AV2 uses sampled
   RD-style scores for lossy intra decisions, while VVC still selects most
   luma/chroma predictors from raw residual energy.
3. Integrate screen-content mode choice into the residual path. VVC has
   palette, IBC, transform-skip residual, and BDPCM helpers, but they remain a
   separate 4:4:4 path instead of competing against residual intra decisions.
4. Finish safe non-DCT2 MTS selection. Syntax, transform plumbing, a
   one-candidate selector, and stats counters exist. Production selection stays
   disabled because the validated selector probes were not yet rate/FPS
   positive on the six-vector first-frame matrix.
5. Continue refining lossy transform-skip/BDPCM selection. Lossy transform-skip
   and regular luma/chroma BDPCM now compete inside the residual path, but the
   selector should still learn better rate pruning from the stats traces.
6. Add VVC-only intra tools that are not AV2 analogues but are needed for
   parity in practice. MIP/ISP/LFNST CABAC contexts and SPS flags are now
   plumbed; active MIP, active ISP, and LFNST transform selection still need
   predictor/transform ownership before they can compete in production.
7. Improve partition/transform-size decisions. Current VVC residual coding is
   dominated by 8x8 lossy leaves and 4x4 lossless leaves; AV2 has more
   effective rate-aware leaf and block choices.
8. Keep instrumentation compile-gated. Per-CTU bit categories, stage timing,
   residual energy, and candidate counters are useful for this work but must
   stay out of normal product builds.

### VVC MRL RD Residual Reuse

This checkpoint implements item 1. The MRL selector now returns the selected
reference-line index plus the finalized quantized luma residual block when the
candidate was scored through lossy RD. TU finalization consumes that cached
block, preserving the selected coefficients and reconstruction while avoiding a
second quantization pass for MRL-eligible transformed TUs. Lossless MRL scoring
continues to use the cheaper raw residual score and does not cache a residual.

Validation:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_luma_mrl --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Strict VTM validation passed for all smoke and high-depth smoke rows. The
six-vector first-frame matrix was byte-identical against
`vvc-mrl-rd-score-1f`:

| Mode | Bytes | FPS | Byte delta |
|---|---:|---:|---:|
| lossless | 5,856,819 | 0.33 | 0 |
| qp=24 | 5,541,589 | 0.30 | 0 |

Per-row VVC QP24 deltas versus `vvc-mrl-rd-score-1f`:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 233,302 | 0.51 | 26.131 | 0 |
| SceneComposition_1_422 | 311,833 | 0.44 | 26.170 | 0 |
| screen_wayland_activity_rgb | 809,785 | 0.21 | 25.393 | 0 |
| MissionControlClip1_420 | 823,645 | 0.34 | 16.850 | 0 |
| MissionControlClip1_422 | 1,271,391 | 0.28 | 16.074 | 0 |
| MissionControlClip1_444 | 2,091,633 | 0.24 | 15.619 | 0 |

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mrl-rd-cache-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-mrl-rd-score-1f.json
```

### VVC Frame-Aligned MPM And CTU-Bit Stats

This checkpoint fixes a nonzero-MRL robustness issue found while collecting a
fresh first-frame stats trace. The residual quantizer and CABAC writer must use
the same luma-neighbor availability when deciding whether a luma mode is MPM
coded. The quantizer now mirrors the CABAC CTU-top rule for above-neighbor mode
availability, so nonzero MRL is not selected from a neighbor context that the
writer will later suppress.

The compile-gated CTU-bit sink also now keeps frame-level CABAC context and
neighbour state across CTUs. This fixes the stats path for MRL-enabled streams:
per-CTU bit-category rows can be emitted without encoding each CTU as an
isolated picture. Normal builds are unaffected because this state exists only
under `vvc-stats`.

Validation:

```sh
cargo fmt
cargo check -p frameforge-codecs --features "vvc vvc-stats"
cargo test -p frameforge-codecs vvc_luma_mrl --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
cargo check --workspace \
  --features "codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale frameforge-codecs/vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

The stats probe that previously panicked now completes:

```sh
FRAMEFORGE_VVC_STATS=verification/generated/profiling/vvc_mpm_aligned_scene420_lossy_1f.jsonl \
FRAMEFORGE_VVC_CTU_BITS=verification/generated/profiling/vvc_mpm_aligned_scene420_lossy_1f_ctu.jsonl \
./ff encode \
  verification/generated/test_vectors/aomctc_b2_SceneComposition_1_420_1920x1080_15_1f_yuv420p8.yuv \
  --video 1920x1080:yuv420p8 --frames 1 --fps 15 \
  --encode vvc:verification/generated/profiling/vvc_mpm_aligned_scene420_lossy_1f.vvc \
  --recon verification/generated/profiling/vvc_mpm_aligned_scene420_lossy_1f_recon.yuv \
  --qp 24
```

Probe result: 233,233 encoded bytes, PSNR 26.126 dB, and VVC stats reported
`ctu_quantize` at about 2.06 s versus `frame_entropy_write` at about 0.12 s on
the first SceneComposition 4:2:0 frame. Candidate/residual work remains the
dominant intra bottleneck.

First-frame six-vector matrix versus `vvc-mrl-rd-cache-1f`:

| Mode | Previous bytes | Current bytes | Byte delta | Previous FPS | Current FPS |
|---|---:|---:|---:|---:|---:|
| lossless | 5,856,819 | 5,856,894 | +75 | 0.33 | 0.34 |
| qp=24 | 5,541,589 | 5,530,447 | -11,142 | 0.30 | 0.30 |

Per-row VVC QP24 deltas:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 233,291 | 0.51 | 26.126 | -11 |
| SceneComposition_1_422 | 313,075 | 0.40 | 26.159 | +1,242 |
| screen_wayland_activity_rgb | 813,021 | 0.21 | 25.371 | +3,236 |
| MissionControlClip1_420 | 821,385 | 0.34 | 16.851 | -2,260 |
| MissionControlClip1_422 | 1,269,904 | 0.30 | 16.080 | -1,487 |
| MissionControlClip1_444 | 2,079,771 | 0.23 | 15.649 | -11,862 |

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-intra-parity-mpm-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-mrl-rd-cache-1f.json
```

### VVC Luma RD Shortlist

Checkpoint: `vvc-luma-rd-pareto-1f`.

This checkpoint starts item 2 by adding an output-aware lossy luma mode
refinement pass. The regular predictor search still supplies the candidate
ordering, but the best raw mode is now compared against the next shortlisted
mode after quantization and inverse reconstruction. The selector is conservative
and only switches when the candidate improves estimated residual rate without
raising reconstructed-residual distortion, or improves distortion without
raising estimated residual rate. The selected quantized residual is cached and
reused by TU finalization.

This is useful plumbing but not yet a strong production tradeoff by itself. On
the first-frame six-vector matrix, lossy total size improved by only 1,004 bytes
versus `vvc-intra-parity-mpm-1f`, while several rows lost noticeable FPS.

| Mode | Current bytes | FPS |
|---|---:|---:|
| lossless | 5,857,190 | 0.32 |
| qp=24 | 5,529,443 | 0.25 |

Per-row VVC QP24 deltas versus `vvc-intra-parity-mpm-1f`:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 231,229 | 0.41 | 26.183 | -2,062 |
| SceneComposition_1_422 | 311,019 | 0.36 | 26.194 | -2,056 |
| screen_wayland_activity_rgb | 812,752 | 0.18 | 25.391 | -269 |
| MissionControlClip1_420 | 825,674 | 0.26 | 16.834 | +4,289 |
| MissionControlClip1_422 | 1,272,654 | 0.23 | 16.070 | +2,750 |
| MissionControlClip1_444 | 2,076,115 | 0.19 | 15.665 | -3,656 |

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-luma-rd-pareto-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-intra-parity-mpm-1f.json
```

### VVC Chroma RD Shortlist

Checkpoint: `vvc-chroma-rd-pareto-1f`.

This checkpoint extends item 2 to chroma predictor selection. The chroma search
now keeps its normal derived/explicit/CCLM candidate list, then compares the
selected raw mode against the next shortlisted candidate after quantization and
inverse reconstruction of both Cb and Cr. The selector uses the same Pareto rule
as luma and caches the selected chroma residual blocks for finalization.

The change validated against VTM on smoke and high-depth smoke. Unlike the luma
shortlist, it produced consistent first-frame gains across all six lossy rows:
total QP24 size dropped 208,614 bytes versus `vvc-intra-parity-mpm-1f`, with
PSNR increasing on every row. The cost is extra candidate reconstruction work,
so total QP24 FPS moved from about 0.30 at `vvc-intra-parity-mpm-1f` to 0.23.
The `vvc-stats` instrumentation now also records
`luma_rd_refinement_attempts`, `luma_rd_refinement_switches`,
`chroma_rd_refinement_attempts`, and `chroma_rd_refinement_switches` per CTU and
in aggregate stats.

| Mode | Current bytes | FPS |
|---|---:|---:|
| lossless | 5,857,190 | 0.33 |
| qp=24 | 5,321,833 | 0.23 |

Per-row VVC QP24 deltas versus `vvc-intra-parity-mpm-1f`:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 221,496 | 0.40 | 26.517 | -11,795 |
| SceneComposition_1_422 | 295,710 | 0.33 | 26.638 | -17,365 |
| screen_wayland_activity_rgb | 769,779 | 0.16 | 25.778 | -43,242 |
| MissionControlClip1_420 | 812,856 | 0.25 | 17.051 | -8,529 |
| MissionControlClip1_422 | 1,241,090 | 0.21 | 16.333 | -28,814 |
| MissionControlClip1_444 | 1,980,902 | 0.17 | 16.061 | -98,869 |

Validation:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-chroma-rd-pareto-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-intra-parity-mpm-1f.json
```

### VVC CTU Luma Leaf Selector

Checkpoint: `vvc-ctu-leaf-sse-selector-1f`.

This checkpoint addresses item 7 for the current luma partition surface. VVC
already has legal 8x8 and 4x4 luma residual leaves; the encoder now chooses the
per-CTU lossy luma leaf size from a rate-aware split proxy instead of using a
single picture-wide 8x8 choice. The proxy compares each 8x8 luma block's SSE
to its local mean with the sum of its four 4x4 local-mean SSEs, then selects
4x4 CTU leaves only when the estimated distortion reduction clears both a
QP/bit-depth-scaled rate penalty and a meaningful fraction of the CTU's luma
variance. Lossless keeps the existing 4x4 path unchanged.

The first-frame six-vector matrix is reference-compatible and lossless
byte-neutral versus `vvc-chroma-rd-pareto-1f`. QP24 total size drops by 72,342
bytes, and PSNR improves on every row. Two 10-bit 4:2:x rows spend extra luma
bits to buy that quality, while screen/RGB and 4:4:4 rows carry most of the net
bitrate win. A tighter split-gain probe (`vvc-ctu-leaf-sse-selector-tight-1f`)
reduced those row regressions but lost the useful overall size reduction, so
the looser net-positive selector remains active.

| Mode | Current bytes | FPS | Byte delta |
|---|---:|---:|---:|
| lossless | 5,857,190 | 0.33 | 0 |
| qp=24 | 5,249,491 | 0.22 | -72,342 |

Per-row VVC QP24 deltas versus `vvc-chroma-rd-pareto-1f`:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 213,467 | 0.37 | 28.292 | -8,029 |
| SceneComposition_1_422 | 277,512 | 0.32 | 28.201 | -18,198 |
| screen_wayland_activity_rgb | 735,952 | 0.15 | 26.236 | -33,827 |
| MissionControlClip1_420 | 879,658 | 0.24 | 18.378 | +66,802 |
| MissionControlClip1_422 | 1,251,166 | 0.21 | 17.479 | +10,076 |
| MissionControlClip1_444 | 1,891,736 | 0.17 | 17.070 | -89,166 |

Validation:

```sh
cargo fmt
cargo test -p frameforge-codecs vvc_ctu_luma_leaf_size_selector_uses_local_split_gain --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-ctu-leaf-sse-selector-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-chroma-rd-pareto-1f.json
```

### VVC MTS Selector Probe

Checkpoint probe: `vvc-mts-enabled-1f`.

The non-DCT2 luma MTS transform and syntax path is now reference-valid on the
smoke and high-depth smoke validation sets when the selector is enabled. The
current selector remains production-disabled, however, because its first-frame
matrix tradeoff is poor: total QP24 size increased from 5,321,833 bytes to
5,357,473 bytes versus `vvc-chroma-rd-pareto-1f`, and FPS fell from 0.23 to
0.07. This should be revisited with a cheaper mode-directed shortlist and a
rate-safer selection rule instead of trying every non-DCT2 transform on every
eligible 8x8 luma TU.

| Mode | Current bytes | FPS | Byte delta |
|---|---:|---:|---:|
| lossless | 5,857,190 | 0.34 | 0 |
| qp=24 | 5,357,473 | 0.07 | +35,640 |

Validation while temporarily enabling `VVC_ENABLE_LUMA_MTS_SELECTION`:

```sh
cargo test -p frameforge-codecs vvc_luma_mts --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mts-enabled-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-chroma-rd-pareto-1f.json
```

### VVC Residual BDPCM Selection

Checkpoint: `vvc-bdpcm-residual-1f`.

This checkpoint completes the first usable form of item 5. Residual slices now
signal BDPCM capability in the SPS, and both lossy and lossless luma/chroma TUs
can select regular horizontal or vertical BDPCM inside the unified residual
path. The selector compares BDPCM candidates against the already-selected
regular predictor using reconstructed residual distortion and estimated syntax
rate, then only switches on Pareto wins so a BDPCM candidate does not buy rate
by making the reconstructed residual worse.

The coefficient path applies forward residual DPCM before transform-skip
quantization and inverse residual DPCM before transform-skip dequantization,
matching the VTM ordering. Dedicated BDPCM predictors bypass angular filtering
and PDPC while still using the same left/top availability model as the regular
intra predictor. The compile-gated `vvc-stats` path now records aggregate
luma/chroma horizontal/vertical BDPCM counts and includes `bdpcm_mode` in the
per-TU trace.

First-frame six-vector matrix versus `vvc-ctu-leaf-sse-selector-1f`:

| Mode | Previous bytes | Current bytes | Byte delta | Previous FPS | Current FPS |
|---|---:|---:|---:|---:|---:|
| lossless | 5,857,190 | 5,508,189 | -349,001 | 0.33 | 0.25 |
| qp=24 | 5,249,491 | 1,502,019 | -3,747,472 | 0.22 | 0.19 |

Per-row VVC QP24 deltas versus `vvc-ctu-leaf-sse-selector-1f`:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 130,885 | 0.32 | 34.731 | -82,582 |
| SceneComposition_1_422 | 141,690 | 0.27 | 35.896 | -135,822 |
| screen_wayland_activity_rgb | 309,755 | 0.13 | 36.574 | -426,197 |
| MissionControlClip1_420 | 280,461 | 0.22 | 31.702 | -599,197 |
| MissionControlClip1_422 | 303,102 | 0.18 | 32.841 | -948,064 |
| MissionControlClip1_444 | 336,126 | 0.15 | 34.396 | -1,555,610 |

Validation:

```sh
cargo fmt
cargo check -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-bdpcm-residual-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-ctu-leaf-sse-selector-1f.json
```

### VVC MIP/ISP/LFNST Context Plumbing

This checkpoint wires the currently inactive VVC-only intra tool syntax sites
far enough that future predictors can be added without reworking SPS or CABAC
context ownership:

- MIP, ISP, and LFNST now have explicit CABAC context entries, VTM-derived
  I-slice init/log2 tables, RTL trace ids, and stats bit-category mapping.
- Residual SPS tool flags now carry `isp_enabled` and `mip_enabled` from the
  active slice configuration instead of hard-coding both to false.
- The luma CU syntax path contains inactive MIP, ISP, and LFNST emitters in
  the VTM-shaped order. Normal residual configs keep the flags false, so
  production streams remain byte-neutral. Enabling the flags currently emits
  the no-tool branch; active MIP still needs matrix predictor tables, active
  ISP needs split transform-tree ownership, and active LFNST needs transform
  candidate ownership plus coefficient-group constraints.

Validation:

```sh
cargo fmt
cargo check -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

### VVC MTS Candidate Accounting

Checkpoint: `vvc-mts-cost-accounting-1f`.

This checkpoint keeps non-DCT2 MTS transform and syntax support available, but
does not enable production selection. Two active selector probes were
reference-valid against VTM, yet neither was a useful default:

- `vvc-mts-directed-pareto-1f` tried one residual-gradient-directed MTS
  candidate per eligible 8x8 luma TU. It finished at 1,502,177 lossy bytes
  versus 1,502,019 for `vvc-bdpcm-residual-1f`, and reduced total lossy FPS
  from 0.19 to 0.16.
- `vvc-mts-directed-pareto-tsfirst-1f` avoided MTS trials when transform skip
  already won against DCT2, but still finished at 1,503,572 lossy bytes and
  did not recover enough FPS to justify enabling the selector.

The retained default-path change is narrower: the shared luma quantized
residual scorer now includes the explicit MTS flag cost for transformed blocks,
and the gated `vvc-stats` counters report nonzero luma MTS index counts by
transform pair. With `VVC_ENABLE_LUMA_MTS_SELECTION=false`, the stats probe on
the first SceneComposition 4:2:0 frame reported all luma MTS counts as zero.

First-frame six-vector matrix versus `vvc-bdpcm-residual-1f`:

| Mode | Previous bytes | Current bytes | Byte delta | Previous FPS | Current FPS |
|---|---:|---:|---:|---:|---:|
| lossless | 5,508,189 | 5,508,189 | 0 | 0.25 | 0.25 |
| qp=24 | 1,502,019 | 1,503,572 | +1,553 | 0.19 | 0.19 |

Per-row VVC QP24 deltas:

| Vector | Bytes | FPS | PSNR | Byte delta |
|---|---:|---:|---:|---:|
| SceneComposition_1_420 | 130,858 | 0.30 | 34.731 | -27 |
| SceneComposition_1_422 | 141,676 | 0.26 | 35.896 | -14 |
| screen_wayland_activity_rgb | 309,776 | 0.13 | 36.575 | +21 |
| MissionControlClip1_420 | 281,027 | 0.21 | 31.696 | +566 |
| MissionControlClip1_422 | 303,523 | 0.18 | 32.834 | +421 |
| MissionControlClip1_444 | 336,712 | 0.15 | 34.395 | +586 |

Validation:

```sh
cargo fmt
cargo check -p frameforge-codecs --features vvc
cargo test -p frameforge-codecs vvc --features "vvc vvc-stats"
make validate-set CODEC=vvc VALIDATION_SET=smoke VALIDATION_REFERENCE_MODE=required
make validate-set CODEC=vvc VALIDATION_SET=high-depth-smoke VALIDATION_REFERENCE_MODE=required
```

Command:

```sh
make benchmark-encode-matrix \
  ENCODE_MATRIX_RUN=vvc-mts-cost-accounting-1f \
  ENCODE_MATRIX_CODECS=vvc \
  ENCODE_MATRIX_MODES="lossless lossy" \
  ENCODE_MATRIX_FRAMES=1 \
  ENCODE_MATRIX_BASELINE=verification/generated/encode_matrix/vvc-bdpcm-residual-1f.json
```

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
