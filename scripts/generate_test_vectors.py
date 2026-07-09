#!/usr/bin/env python3
"""Generate deterministic raw YUV test vectors from CSV manifests."""

from __future__ import annotations

import argparse
import csv
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SET_DIR = REPO_ROOT / "verification" / "test_vector_sets"
DEFAULT_OUT_DIR = REPO_ROOT / "verification" / "generated" / "test_vectors"


@dataclass(frozen=True)
class TestVector:
    name: str
    width: int
    height: int
    frames: int
    fmt: str
    pattern: str
    fps: int | None

    @property
    def filename(self) -> str:
        fps_part = f"_{self.fps}" if self.fps is not None else ""
        return f"{self.name}_{self.width}x{self.height}{fps_part}_{self.frames}f_{self.fmt}.yuv"


@dataclass(frozen=True)
class TestVectorSet:
    name: str
    manifest: Path
    description: str
    vectors: list[TestVector]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("set", nargs="?", default="smoke", help="test vector set name")
    parser.add_argument("--set-dir", type=Path, default=DEFAULT_SET_DIR)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--list-sets", action="store_true")
    args = parser.parse_args()

    if args.list_sets:
        for name, vector_set in sorted(vector_sets(args.set_dir).items()):
            print(f"{name}\t{len(vector_set.vectors)}\t{vector_set.manifest}")
        return 0

    paths = generate_vectors(args.set, args.out_dir, args.set_dir)
    print(f"Generated {len(paths)} test vector(s) in {args.out_dir}")
    for path in paths:
        print(path)
    return 0


def generate_vectors(set_name: str, out_dir: Path, set_dir: Path = DEFAULT_SET_DIR) -> list[Path]:
    sets = vector_sets(set_dir)
    if set_name not in sets:
        choices = ", ".join(sorted(sets)) or "<none>"
        raise ValueError(f"unknown test vector set '{set_name}'; choices: {choices}")

    out_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for vector in sets[set_name].vectors:
        path = out_dir / vector.filename
        path.write_bytes(generate_yuv(vector))
        paths.append(path)
    return paths


def vector_sets(set_dir: Path = DEFAULT_SET_DIR) -> dict[str, TestVectorSet]:
    sets: dict[str, TestVectorSet] = {}
    if not set_dir.exists():
        return sets
    for path in sorted(set_dir.glob("*.csv")):
        loaded = load_vector_set(path)
        sets[loaded.name] = loaded
    return sets


def load_vector_set(path: Path) -> TestVectorSet:
    description = ""
    rows: list[str] = []
    for raw_line in path.read_text().splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("#"):
            body = line[1:].strip()
            if body.startswith("description="):
                description = body.removeprefix("description=").strip()
            continue
        rows.append(raw_line)

    if not rows:
        raise ValueError(f"test vector manifest has no CSV rows: {path}")

    reader = csv.DictReader(rows)
    vectors = [parse_vector(row, path) for row in reader]
    if not vectors:
        raise ValueError(f"test vector manifest has no vectors: {path}")

    return TestVectorSet(
        name=path.stem,
        manifest=path,
        description=description,
        vectors=vectors,
    )


def parse_vector(row: dict[str, str], path: Path) -> TestVector:
    context = f"{path}:{row.get('name', '').strip() or '<unnamed>'}"
    return TestVector(
        name=required_field(row, "name", context),
        width=parse_positive_int(required_field(row, "width", context), "width"),
        height=parse_positive_int(required_field(row, "height", context), "height"),
        frames=parse_positive_int(required_field(row, "frames", context), "frames"),
        fmt=required_field(row, "format", context),
        pattern=required_field(row, "pattern", context),
        fps=parse_optional_int(row.get("fps", ""), "fps"),
    )


def required_field(row: dict[str, str], key: str, context: str) -> str:
    value = row.get(key, "").strip()
    if not value:
        raise ValueError(f"missing {key} in {context}")
    return value


def parse_positive_int(value: str, field: str) -> int:
    try:
        parsed = int(value)
    except ValueError as err:
        raise ValueError(f"{field} expects an integer, got '{value}'") from err
    if parsed <= 0:
        raise ValueError(f"{field} expects a positive integer, got {parsed}")
    return parsed


def parse_optional_int(value: str | None, field: str) -> int | None:
    if value is None or not value.strip():
        return None
    try:
        parsed = int(value)
    except ValueError as err:
        raise ValueError(f"{field} expects an integer, got '{value}'") from err
    if parsed <= 0:
        raise ValueError(f"{field} expects a positive integer, got {parsed}")
    return parsed


def generate_yuv(vector: TestVector) -> bytes:
    validate_vector(vector)
    if vector.fmt == "yuv420p8":
        return generate_yuv420p8(vector)
    if vector.fmt == "yuv444p8":
        return generate_yuv444p8(vector)
    raise ValueError(f"unsupported generated pixel format: {vector.fmt}")


def validate_vector(vector: TestVector) -> None:
    if vector.width <= 0 or vector.height <= 0:
        raise ValueError(f"{vector.name} has invalid geometry {vector.width}x{vector.height}")
    if vector.fmt == "yuv420p8" and (vector.width % 2 != 0 or vector.height % 2 != 0):
        raise ValueError(f"{vector.name} yuv420p8 dimensions must be even")
    if vector.fmt == "yuv444p8" and (vector.width % 8 != 0 or vector.height % 8 != 0):
        raise ValueError(f"{vector.name} yuv444p8 fixtures use 8-pixel geometry for current codecs")


def generate_yuv420p8(vector: TestVector) -> bytes:
    out = bytearray()
    for frame in range(vector.frames):
        y_plane, u444, v444 = render_frame(vector, frame)
        u_plane = bytearray()
        v_plane = bytearray()
        for y in range(0, vector.height, 2):
            for x in range(0, vector.width, 2):
                indices = (
                    pixel_index(vector, x, y),
                    pixel_index(vector, x + 1, y),
                    pixel_index(vector, x, y + 1),
                    pixel_index(vector, x + 1, y + 1),
                )
                u_plane.append(sum(u444[idx] for idx in indices) // 4)
                v_plane.append(sum(v444[idx] for idx in indices) // 4)
        out.extend(y_plane)
        out.extend(u_plane)
        out.extend(v_plane)
    return bytes(out)


def generate_yuv444p8(vector: TestVector) -> bytes:
    out = bytearray()
    for frame in range(vector.frames):
        y_plane, u_plane, v_plane = render_frame(vector, frame)
        out.extend(y_plane)
        out.extend(u_plane)
        out.extend(v_plane)
    return bytes(out)


def render_frame(vector: TestVector, frame: int) -> tuple[bytearray, bytearray, bytearray]:
    y_plane = bytearray(vector.width * vector.height)
    u_plane = bytearray(vector.width * vector.height)
    v_plane = bytearray(vector.width * vector.height)
    for y in range(vector.height):
        for x in range(vector.width):
            yy, uu, vv = sample_yuv(vector, x, y, frame)
            idx = pixel_index(vector, x, y)
            y_plane[idx] = yy
            u_plane[idx] = uu
            v_plane[idx] = vv
    return y_plane, u_plane, v_plane


def sample_yuv(vector: TestVector, x: int, y: int, frame: int) -> tuple[int, int, int]:
    if vector.pattern == "black":
        return 0, 0, 0
    if vector.pattern == "checker":
        cell = ((x // 8) + (y // 8) + frame) & 1
        return (48, 96, 160) if cell else (208, 176, 80)
    if vector.pattern == "gradient":
        return (
            (x * 7 + y * 5 + frame * 17) & 0xFF,
            (64 + x * 3 + frame * 11) & 0xFF,
            (96 + y * 4 + frame * 13) & 0xFF,
        )
    if vector.pattern == "color_blocks":
        palette = (
            (32, 128, 128),
            (80, 96, 176),
            (144, 176, 96),
            (224, 112, 144),
        )
        return palette[((x // 8) + (y // 8) * 2 + frame) % len(palette)]
    raise ValueError(f"unsupported pattern: {vector.pattern}")


def pixel_index(vector: TestVector, x: int, y: int) -> int:
    return y * vector.width + x


if __name__ == "__main__":
    raise SystemExit(main())
