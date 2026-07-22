#!/usr/bin/env python3
"""Convert packed RGB24 rawvideo frames to planar GBR 8-bit rawvideo."""

from __future__ import annotations

import argparse
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="Input packed rgb24 rawvideo")
    parser.add_argument("output", type=Path, help="Output planar gbrp8 rawvideo")
    parser.add_argument("--width", type=parse_positive_int, required=True)
    parser.add_argument("--height", type=parse_positive_int, required=True)
    parser.add_argument("--frames", type=parse_positive_int)
    args = parser.parse_args()

    frame_len = args.width * args.height * 3
    pixels = args.width * args.height
    input_size = args.input.stat().st_size
    if input_size % frame_len != 0:
        raise SystemExit(
            f"{args.input} size {input_size} is not a whole number of "
            f"{args.width}x{args.height} rgb24 frame(s)"
        )
    available_frames = input_size // frame_len
    frames = args.frames if args.frames is not None else available_frames
    if frames > available_frames:
        raise SystemExit(
            f"requested {frames} frame(s), but {args.input} contains only {available_frames}"
        )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.input.open("rb") as source, args.output.open("wb") as output:
        for frame_index in range(frames):
            frame = source.read(frame_len)
            if len(frame) != frame_len:
                raise SystemExit(f"{args.input} ended during frame {frame_index + 1}")
            planar = bytearray(frame_len)
            planar[:pixels] = frame[1::3]
            planar[pixels : pixels * 2] = frame[2::3]
            planar[pixels * 2 :] = frame[0::3]
            output.write(planar)

    print(
        f"wrote {args.output} as gbrp8 {args.width}x{args.height} "
        f"{frames}f ({frames * frame_len} bytes)"
    )
    return 0


def parse_positive_int(value: str) -> int:
    try:
        parsed = int(value, 10)
    except ValueError as err:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got '{value}'") from err
    if parsed <= 0:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got {parsed}")
    return parsed


if __name__ == "__main__":
    raise SystemExit(main())
