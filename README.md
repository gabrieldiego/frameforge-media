# FrameForge

FrameForge is a safe Rust media pipeline toolkit. The project is built around
composable stages:

```text
input -> decode -> filter -> encode -> output
```

The initial focus is experimental video encoding and validation infrastructure,
with AV2 and VVC as the first planned codec families. The repository is a
software-only sibling of the FrameForge hardware project: FrameForge remains the
RTL, synthesis, and hardware-validation workspace, while this repository is free
to optimize for software APIs, usability, codec quality, and safe Rust
performance.

This repository is in bootstrap state. It currently provides project structure,
shared media primitives, a placeholder CLI, and development workflow targets.
It does not yet provide a functional AV2 or VVC encoder.

## Goals

- Provide safe Rust media pipeline components.
- Keep codec implementations modular and independently selectable at build time.
- Validate generated bitstreams and reconstructions with strict, reproducible
  tests.
- Support commercial and non-commercial use under a permissive license.
- Grow from codec and validation foundations into a broader media toolkit
  without forcing premature abstractions.

## Quick Start

Requirements:

- Rust toolchain with Cargo.
- `make`.

Check the local toolchain:

```sh
make check-tools
```

Build and test:

```sh
make build
make test
```

`make build` creates a release binary at:

```sh
./ff
```

For a debug build, use:

```sh
make debug
```

Run the placeholder CLI:

```sh
make run ARGS="--help"
```

The installed command name is intended to be short:

```sh
ff --help
```

Run the default local quality gate:

```sh
make release-check
```

## Build-Time Composition

Codec and filter availability should be selected at build time. The intended
shape is to use Cargo features or separate crates for optional media stages so a
binary can include only the codecs and filters it needs.

Example future shape:

```sh
make build CARGO_FEATURES="codec-av2 codec-vvc filter-scale"
```

No optional codec or filter features are implemented yet.

## Repository Layout

```text
crates/
  frameforge-core/  Shared frame, packet, error, and pipeline primitives.
  frameforge-cli/   Command-line entry point, installed as `ff`.
docs/                     Architecture and validation notes.
tests/                    Future integration tests and fixtures.
tools/                    Future development and validation helper scripts.
```

## Safety Posture

FrameForge should use safe Rust. Performance work should start with safe
Rust, better algorithms, optimizer-friendly data layout, and compiler-supported
optimizations. Optimized implementations that replace correctness-critical
kernels should be proven bit-exact against simple scalar implementations.

## Validation Direction

Validation should remain strict and reproducible:

- lossless paths must reconstruct exactly;
- lossy paths should report PSNR and bitrate;
- reference decoders should validate generated bitstreams when available;
- checksums and bitstream sizes should be recorded for regressions;
- generated test vectors should be deterministic.

## License

FrameForge is licensed under the Apache License, Version 2.0.

The project is open for commercial and non-commercial use. Companies and
individuals may build public or proprietary extensions, products, and services
on top of it under the terms of the Apache-2.0 license.

Codec patent rights are separate from source-code copyright licensing. Users are
responsible for evaluating any codec patent or deployment obligations that apply
to their use case and jurisdiction.
