# Validation Notes

FrameForge should keep validation strict and reproducible.

Expected validation layers:

- unit tests for frame, packet, syntax, and reconstruction primitives;
- integration tests using deterministic generated vectors;
- reference-decoder checks for generated bitstreams when a reference is
  available;
- checksum comparison for lossless paths;
- PSNR and bitrate reporting for lossy paths;
- benchmark and throughput reporting for performance-sensitive stages.

Do not weaken pass criteria to hide incomplete codec support. Unsupported
syntax or geometry should fail visibly until implemented.
