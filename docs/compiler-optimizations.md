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
