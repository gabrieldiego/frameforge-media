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
shared media primitives, a CLI, and imported experimental AV2/VVC software
models from the FrameForge hardware workspace.

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

Run the CLI:

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

Codec and filter availability is selected at build time. By default,
`make build` enables the imported AV2 and VVC software models so the copied
`./ff` binary can encode with either codec.

Override `CARGO_FEATURES` to build a smaller or more specialized binary:

```sh
make build CARGO_FEATURES="codec-av2 filter-scale"
make build CARGO_FEATURES=
```

The `codec-av2` and `codec-vvc` features enable the imported experimental
software models. Filter features are discovery placeholders for now; parsed
filters are not executed yet.

## CLI Shape

The CLI entry point is `ff`. The initial interface is centered on stage
discovery and a single encode action:

```sh
ff codecs
ff filters
ff encode input.yuv --video 640x360:yuv444p \
  --encode av2:output.obu --set lossless
ff encode input_640x360_30_1f_yuv444p8.yuv \
  --filter identity --encode av2:output.obu
```

The commands validate command-line structure and report stage availability.
When built with `codec-av2` or `codec-vvc`, `ff encode` can encode raw YUV
inputs through the imported software model for that codec. Filters are still
parsed for the future pipeline shape but are not executed yet.

Input options, such as `--video`, `--fps`, and `--frames`, belong after the
input path. Filter options come next. Output/encoder options, such as
`--set lossless`, `--preset`, and repeated `--set key[=value]`, belong after
`--encode codec:output`. Bare `--set` keys imply `true`. Global accepted
settings are listed by `ff codecs`; codec-specific settings can be added later
when a feature really needs them.

Raw video metadata uses a compact `WxH:pixfmt` spelling when it cannot be
inferred from the input filename or needs to be overridden. File names imply
metadata with `*_<WxH>[_<fps>][_<frames>f][_<pixfmt>].yuv`, for example
`input_640x360_30_1f_yuv444p8.yuv`. Short 8-bit aliases such as `yuv444p` and
`yuv420p` are accepted and normalized to `yuv444p8` and `yuv420p8` internally.
If a `.yuv` filename has dimensions but no pixel-format token, the CLI assumes
`yuv420p8`. Encode endpoints must name the codec and output path together, such
as `--encode av2:output.obu`.

## Repository Layout

```text
crates/
  frameforge-core/  Shared frame, packet, error, and pipeline primitives.
  frameforge-codecs/  Imported experimental AV2/VVC software models.
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
