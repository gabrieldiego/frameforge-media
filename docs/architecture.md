# Architecture Notes

FrameForge is organized around a media pipeline:

```text
input -> decode -> filter -> encode -> output
```

The first shared crate, `frameforge-core`, intentionally contains only
stable infrastructure:

- frame metadata and owned frame buffers;
- packet metadata and owned packet buffers;
- shared error types;
- source, decoder, filter, encoder, and sink traits.

Codec internals should remain independent until common APIs are proven by real
implementations. AV2 and VVC may share frame buffers, metrics, validation
adapters, and byte/bitstream helpers, but should not be forced into one entropy
or block-tree abstraction early.

Optional codecs and filters should be selected at build time using Cargo
features or separate crates. Runtime pipeline construction can still choose
which compiled stages to connect.
