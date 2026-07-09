# FrameForge Agent Guide

This file is for AI coding agents working in this repository. It captures the
initial project intent and boundaries agreed while splitting this work out of
the FrameForge hardware repository.

The scope of this file is the whole repository.

## Project Identity

FrameForge is the software-only media pipeline sibling of the original
FrameForge hardware project. The hardware repository remains the RTL research
and verification workspace.
This repository is intended to grow as a safe Rust media toolkit around the
pipeline model:

```text
input -> decode -> filter -> encode -> output
```

The initial implementation focus is experimental video encoding and validation,
starting from AV2 and VVC concepts already explored in FrameForge. The long-term
scope may include additional codecs, stream tools, filters, metrics, validation
adapters, and media-pipeline utilities.

This project should not be treated as the RTL hardware model. It is allowed to
optimize for software usability, safe Rust APIs, performance, and codec quality.

## Relationship To FrameForge

- `frameforge`: hardware/software co-design, RTL, synthesis, hardware-golden
  Rust models, and strict SW/RTL/reference validation.
- `frameforge-media`: software-only safe Rust media pipeline and codec toolkit.

This repository may reuse ideas and carefully imported code from FrameForge,
but it should not inherit hardware-specific constraints unless they are useful
for validation or interoperability.

Avoid trying to keep this repository as a full mirror of FrameForge. Treat
FrameForge as a source of tested codec syntax/reconstruction ideas and
validation practices, not as a repo to merge wholesale.

## Licensing Intent

The intended license model is permissive open source. The current preference is
Apache-2.0.

Project intent:

- commercial and non-commercial use should be allowed;
- companies and individuals may build public or proprietary extensions;
- paid support, custom development, integration, and optimization work should
  be allowed for the maintainer and for third parties;
- the license should not create copyleft obligations for downstream products.

Codec patent obligations are separate from source-code copyright licensing.
Documentation should make clear that users are responsible for evaluating codec
patent or deployment obligations for their use case and jurisdiction.

## Safety Posture

The implementation should use safe Rust.

Guidelines:

- Performance work should use safe Rust, algorithmic improvements,
  optimizer-friendly data layout, and compiler-supported optimizations.
- Prove optimized safe implementations are bit-exact against simple scalar
  implementations when replacing correctness-critical kernels.
- Prefer checked, saturating, or explicitly wrapping arithmetic where overflow
  behavior matters.
- Validate frame dimensions and buffer lengths before allocation or indexing.

The intended public claim is not that bugs are impossible; it is that the
default implementation avoids memory-unsafe Rust and uses validation to catch
codec correctness errors.

## Architecture Direction

Make the pipeline concept first-class. Useful abstractions may include:

- sources that produce packets or frames;
- decoders that convert packets into frames;
- filters that transform frames;
- encoders that convert frames into packets/bitstreams;
- sinks that write packets or streams;
- metrics stages for bitrate, PSNR, checksums, and validation.

Keep codec internals independent at first. Do not force AV2 and VVC into a
premature shared abstraction for entropy, block trees, prediction, transform, or
mode decisions. Share stable, boring infrastructure first:

- frame and plane buffers;
- pixel formats and color metadata;
- raw YUV/PNG I/O helpers;
- byte/bitstream output helpers;
- metrics and checksums;
- reference-decoder validation adapters;
- command-line plumbing and benchmarks.

## Profiles And Builds

If profile-specific behavior is needed, prefer build-time selection for
independent products rather than runtime mode flags. The user-facing software
encoder should not include hardware-model-only decision logic unless there is a
specific reason.

Codec and filter availability may also be selected at build time. Prefer
separate Cargo features or crates for optional codecs and filters so binaries
can be built with only the media stages they need.

Avoid scattering profile checks through codec syntax and reconstruction code.
If profiles are introduced, resolve them at construction or crate feature
boundaries and keep shared syntax/reconstruction code profile-neutral.

Potential future products:

- a user-facing safe software encoder build;
- optional experimental builds;
- optional hardware-compatibility import tools, kept separate from normal user
  binaries.

## Validation Principles

Validation should remain strict and reproducible:

- reference decoders validate generated bitstreams when available;
- lossless paths must reconstruct exactly;
- lossy paths should report PSNR and bitrate;
- bitstream sizes, checksums, and metrics should be recorded for regressions;
- test vectors should be deterministic and regenerable;
- do not weaken validation criteria to make incomplete work appear correct.

FrameForge hardware validation can inspire the workflow, but this repository is
free to add software-specific benchmarks and quality tests.

## Development Boundaries

Avoid early feature creep into a general-purpose media suite. The broad vision
is a media pipeline toolkit, but the early milestones should stay narrow:

1. establish a clean safe Rust project structure;
2. import or reimplement minimal frame/pixel/bitstream primitives;
3. bring up one codec path with reference-decoder validation;
4. add metrics and reproducible tests;
5. expand codecs and filters incrementally.

When adding features, keep APIs small and practical. Prefer code that can be
validated now over abstractions for codecs or container formats that do not
exist yet.

## Agent Workflow

- Read this file before making changes.
- Read the relevant instructions and notes under `docs/*.md` before changing
  code or project structure.
- Check `git status --short` before edits.
- Keep commits small and scoped.
- Do not copy large chunks from FrameForge without preserving attribution and
  checking license compatibility.
- Prefer `rg` for search.
- Use `cargo fmt`, `cargo test`, and targeted validation once a Rust crate
  exists.
- Keep generated artifacts out of version control unless they are intentionally
  committed fixtures.
