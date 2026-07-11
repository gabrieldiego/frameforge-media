#!/usr/bin/env python3
"""Generate deterministic raw YUV test vectors from CSV manifests."""

from __future__ import annotations

import argparse
import csv
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path

try:
    from PIL import Image
except ImportError:  # pragma: no cover - only needed for PNG-backed local manifests.
    Image = None


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SET_DIR = REPO_ROOT / "verification" / "test_vector_sets"
DEFAULT_OUT_DIR = REPO_ROOT / "verification" / "generated" / "test_vectors"
LOCAL_SET_DIR = "local"


@dataclass(frozen=True)
class TestVectorSource:
    id: str
    path: Path
    width: int
    height: int
    fmt: str


@dataclass(frozen=True)
class TestVector:
    name: str
    width: int
    height: int
    frames: int
    fmt: str
    pattern: str
    fps: str | None
    source_path: Path | None
    source: str | None
    crop_x: int | None
    crop_y: int | None
    lossless: bool
    codecs: frozenset[str] | None

    @property
    def filename(self) -> str:
        fps_part = f"_{filename_fps_label(self.fps)}" if self.fps is not None else ""
        return f"{self.name}_{self.width}x{self.height}{fps_part}_{self.frames}f_{self.fmt}.yuv"


@dataclass(frozen=True)
class Y4mMetadata:
    width: int
    height: int
    fmt: str
    fps: str | None


@dataclass(frozen=True)
class TestVectorSet:
    name: str
    manifest: Path
    description: str
    vectors: list[TestVector]
    sources: dict[str, TestVectorSource]


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

    vector_set = sets[set_name]
    out_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for vector in vector_set.vectors:
        path = out_dir / vector.filename
        path.write_bytes(generate_yuv(vector, vector_set.sources))
        paths.append(path)
    return paths


def vector_sets(set_dir: Path = DEFAULT_SET_DIR) -> dict[str, TestVectorSet]:
    sets: dict[str, TestVectorSet] = {}
    if not set_dir.exists():
        return sets
    paths = sorted(set_dir.glob("*.csv"))
    local_dir = set_dir / LOCAL_SET_DIR
    if local_dir.exists():
        paths.extend(sorted(local_dir.glob("*.csv")))
    for path in paths:
        loaded = load_vector_set(path)
        sets[loaded.name] = loaded
    return sets


def load_vector_set(path: Path) -> TestVectorSet:
    description = ""
    sources: dict[str, TestVectorSource] = {}
    rows: list[str] = []
    for raw_line in path.read_text().splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("#"):
            body = line[1:].strip()
            if body.startswith("description="):
                description = body.removeprefix("description=").strip()
            elif body.startswith("source="):
                source = parse_source(body.removeprefix("source="))
                sources[source.id] = source
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
        sources=sources,
    )


def parse_source(value: str) -> TestVectorSource:
    fields = parse_key_value_fields(value)
    return TestVectorSource(
        id=required_field(fields, "id", "source"),
        path=Path(required_field(fields, "path", "source")),
        width=parse_positive_int(required_field(fields, "width", "source"), "source width"),
        height=parse_positive_int(required_field(fields, "height", "source"), "source height"),
        fmt=required_field(fields, "format", "source"),
    )


def parse_key_value_fields(value: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for field in next(csv.reader([value], skipinitialspace=True)):
        key, sep, item = field.partition("=")
        if not sep:
            raise ValueError(f"expected key=value source field, got '{field}'")
        out[key.strip()] = item.strip()
    return out


def parse_vector(row: dict[str, str], path: Path) -> TestVector:
    context = f"{path}:{row.get('name', '').strip() or '<unnamed>'}"
    name = required_field(row, "name", context)
    pattern = required_field(row, "pattern", context)
    source_path = parse_optional_path(row.get("path", ""))
    y4m_metadata = None
    if (
        pattern == "source_file"
        and source_path is not None
        and source_path.suffix.lower() == ".y4m"
    ):
        y4m_metadata = read_y4m_metadata(source_path, name)

    width = parse_optional_positive_int(row.get("width", ""), "width")
    height = parse_optional_positive_int(row.get("height", ""), "height")
    fmt = optional_field(row.get("format", ""))
    fps = parse_optional_fps(row.get("fps", ""), "fps")

    if y4m_metadata is not None:
        if width is not None and width != y4m_metadata.width:
            raise ValueError(
                f"{context} declares width {width}, but Y4M source is {y4m_metadata.width}"
            )
        if height is not None and height != y4m_metadata.height:
            raise ValueError(
                f"{context} declares height {height}, but Y4M source is {y4m_metadata.height}"
            )
        if fmt is not None and fmt != y4m_metadata.fmt:
            raise ValueError(f"{context} declares {fmt}, but Y4M source is {y4m_metadata.fmt}")
        if (
            fps is not None
            and y4m_metadata.fps is not None
            and not fps_matches(fps, y4m_metadata.fps)
        ):
            raise ValueError(
                f"{context} declares {fps} fps, but Y4M source is {y4m_metadata.fps}"
            )
        width = width or y4m_metadata.width
        height = height or y4m_metadata.height
        fmt = fmt or y4m_metadata.fmt
        fps = fps or y4m_metadata.fps

    if width is None:
        raise ValueError(f"missing width in {context}")
    if height is None:
        raise ValueError(f"missing height in {context}")
    if fmt is None:
        raise ValueError(f"missing format in {context}")

    return TestVector(
        name=name,
        width=width,
        height=height,
        frames=parse_positive_int(required_field(row, "frames", context), "frames"),
        fmt=fmt,
        pattern=pattern,
        fps=fps,
        source_path=source_path,
        source=optional_field(row.get("source", "")),
        crop_x=parse_optional_non_negative_int(row.get("crop_x", ""), "crop_x"),
        crop_y=parse_optional_non_negative_int(row.get("crop_y", ""), "crop_y"),
        lossless=parse_optional_bool(row.get("lossless", ""), "lossless"),
        codecs=parse_optional_codecs(row.get("codecs", "")),
    )


def required_field(row: dict[str, str], key: str, context: str) -> str:
    value = row.get(key, "").strip()
    if not value:
        raise ValueError(f"missing {key} in {context}")
    return value


def optional_field(value: str | None) -> str | None:
    if value is None:
        return None
    stripped = value.strip()
    return stripped or None


def parse_positive_int(value: str, field: str) -> int:
    try:
        parsed = int(value)
    except ValueError as err:
        raise ValueError(f"{field} expects an integer, got '{value}'") from err
    if parsed <= 0:
        raise ValueError(f"{field} expects a positive integer, got {parsed}")
    return parsed


def parse_optional_positive_int(value: str | None, field: str) -> int | None:
    stripped = optional_field(value)
    if stripped is None:
        return None
    return parse_positive_int(stripped, field)


def parse_optional_non_negative_int(value: str | None, field: str) -> int | None:
    stripped = optional_field(value)
    if stripped is None:
        return None
    try:
        parsed = int(stripped)
    except ValueError as err:
        raise ValueError(f"{field} expects an integer, got '{stripped}'") from err
    if parsed < 0:
        raise ValueError(f"{field} expects a non-negative integer, got {parsed}")
    return parsed


def parse_optional_bool(value: str | None, field: str) -> bool:
    stripped = optional_field(value)
    if stripped is None:
        return False
    normalized = stripped.lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise ValueError(f"{field} expects a boolean, got '{stripped}'")


def parse_optional_codecs(value: str | None) -> frozenset[str] | None:
    stripped = optional_field(value)
    if stripped is None:
        return None
    normalized = stripped.lower()
    if normalized in {"none", "-"}:
        return frozenset()
    codecs = [
        item.strip()
        for item in normalized.replace(";", "|").replace(" ", "|").split("|")
        if item.strip()
    ]
    if not codecs:
        return None
    return frozenset(codecs)


def parse_optional_path(value: str | None) -> Path | None:
    stripped = optional_field(value)
    if stripped is None:
        return None
    return Path(stripped)


def parse_optional_fps(value: str | None, field: str) -> str | None:
    stripped = optional_field(value)
    if stripped is None:
        return None
    parse_fps_fraction(stripped, field)
    return stripped


def parse_fps_fraction(value: str, field: str) -> Fraction:
    try:
        parsed = Fraction(value)
    except ValueError as err:
        raise ValueError(f"{field} expects a positive frame rate, got '{value}'") from err
    if parsed <= 0:
        raise ValueError(f"{field} expects a positive frame rate, got '{value}'")
    return parsed


def filename_fps_label(value: str | None) -> str:
    if value is None:
        return ""
    return value.replace("/", "over")


def fps_matches(declared: str, actual: str) -> bool:
    declared_fraction = parse_fps_fraction(declared, "fps")
    actual_fraction = parse_fps_fraction(actual, "Y4M fps")
    if "/" not in declared and "." not in declared:
        rounded = (
            actual_fraction.numerator + actual_fraction.denominator // 2
        ) // actual_fraction.denominator
        return int(declared) == rounded
    if "/" in declared:
        return declared_fraction == actual_fraction
    return abs(declared_fraction - actual_fraction) <= Fraction(1, 100)


def read_y4m_metadata(path: Path, context: str) -> Y4mMetadata:
    if not path.exists():
        raise ValueError(f"{context} source file does not exist: {path}")
    with path.open("rb") as source:
        header = source.readline()
    if not header.startswith(b"YUV4MPEG2 "):
        raise ValueError(f"{context} source is not a Y4M stream: {path}")
    return parse_y4m_metadata(header.decode("ascii", errors="replace").strip(), context)


def generate_yuv(vector: TestVector, sources: dict[str, TestVectorSource]) -> bytes:
    validate_vector(vector)
    if vector.pattern == "source_crop":
        return generate_source_crop(vector, sources)
    if vector.pattern == "source_file":
        return generate_source_file_clip(vector)
    bit_depth = yuv420_bit_depth(vector.fmt)
    if bit_depth is not None:
        return generate_yuv420p(vector, bit_depth)
    bit_depth = yuv422_bit_depth(vector.fmt)
    if bit_depth is not None:
        return generate_yuv422p(vector, bit_depth)
    bit_depth = yuv444_bit_depth(vector.fmt)
    if bit_depth is not None:
        return generate_yuv444p(vector, bit_depth)
    raise ValueError(f"unsupported generated pixel format: {vector.fmt}")


def validate_vector(vector: TestVector) -> None:
    if vector.width <= 0 or vector.height <= 0:
        raise ValueError(f"{vector.name} has invalid geometry {vector.width}x{vector.height}")
    if yuv420_bit_depth(vector.fmt) is not None and (
        vector.width % 2 != 0 or vector.height % 2 != 0
    ):
        raise ValueError(f"{vector.name} {vector.fmt} dimensions must be even")
    if yuv422_bit_depth(vector.fmt) is not None and vector.width % 2 != 0:
        raise ValueError(f"{vector.name} {vector.fmt} width must be even")
    if yuv444_bit_depth(vector.fmt) is not None and (
        vector.width % 8 != 0 or vector.height % 8 != 0
    ):
        raise ValueError(
            f"{vector.name} {vector.fmt} fixtures use 8-pixel geometry for current codecs"
        )


def generate_source_file_clip(vector: TestVector) -> bytes:
    if vector.source_path is None:
        raise ValueError(f"{vector.name} uses source_file but has no path")
    if not vector.source_path.exists():
        raise ValueError(f"{vector.name} source file does not exist: {vector.source_path}")
    if vector.source_path.suffix.lower() == ".y4m":
        return generate_y4m_source_file_clip(vector)
    frame_len = raw_frame_len(vector)
    byte_len = frame_len * vector.frames
    with vector.source_path.open("rb") as source:
        data = source.read(byte_len)
    if len(data) != byte_len:
        raise ValueError(
            f"{vector.name} source is too short: expected {byte_len} byte(s), got {len(data)}"
        )
    return data


def generate_y4m_source_file_clip(vector: TestVector) -> bytes:
    frame_len = raw_frame_len(vector)
    out = bytearray()
    with vector.source_path.open("rb") as source:
        header = source.readline()
        if not header.startswith(b"YUV4MPEG2 "):
            raise ValueError(f"{vector.name} source is not a Y4M stream: {vector.source_path}")
        metadata = parse_y4m_metadata(
            header.decode("ascii", errors="replace").strip(),
            vector.name,
        )
        validate_y4m_header(vector, metadata)
        for frame_index in range(vector.frames):
            frame_header = source.readline()
            if not frame_header:
                raise ValueError(
                    f"{vector.name} Y4M source is too short: missing frame {frame_index + 1}"
                )
            if not frame_header.startswith(b"FRAME"):
                raise ValueError(
                    f"{vector.name} Y4M source has invalid frame marker at frame {frame_index + 1}"
                )
            frame = source.read(frame_len)
            if len(frame) != frame_len:
                raise ValueError(
                    f"{vector.name} Y4M source is too short: expected {frame_len} byte(s) "
                    f"for frame {frame_index + 1}, got {len(frame)}"
                )
            out.extend(frame)
    return bytes(out)


def parse_y4m_metadata(header: str, context: str) -> Y4mMetadata:
    tags = y4m_header_tags(header)
    return Y4mMetadata(
        width=parse_y4m_positive_int(tags.get("W"), "width", context),
        height=parse_y4m_positive_int(tags.get("H"), "height", context),
        fmt=y4m_pixel_format(tags.get("C")),
        fps=y4m_fps(tags.get("F"), context),
    )


def validate_y4m_header(vector: TestVector, metadata: Y4mMetadata) -> None:
    if metadata.width != vector.width or metadata.height != vector.height:
        raise ValueError(
            f"{vector.name} declares {vector.width}x{vector.height}, "
            f"but Y4M source is {metadata.width}x{metadata.height}"
        )
    if metadata.fmt != vector.fmt:
        raise ValueError(f"{vector.name} declares {vector.fmt}, but Y4M source is {metadata.fmt}")
    if (
        vector.fps is not None
        and metadata.fps is not None
        and not fps_matches(vector.fps, metadata.fps)
    ):
        raise ValueError(
            f"{vector.name} declares {vector.fps} fps, but Y4M source is {metadata.fps}"
        )


def y4m_header_tags(header: str) -> dict[str, str]:
    fields = header.split()
    if not fields or fields[0] != "YUV4MPEG2":
        raise ValueError(f"invalid Y4M header: {header}")
    tags: dict[str, str] = {}
    for field in fields[1:]:
        if len(field) >= 2:
            tags[field[0]] = field[1:]
    return tags


def parse_y4m_positive_int(value: str | None, field: str, context: str) -> int:
    if value is None:
        raise ValueError(f"{context} Y4M header is missing {field}")
    try:
        parsed = int(value)
    except ValueError as err:
        raise ValueError(f"{context} Y4M {field} expects an integer, got '{value}'") from err
    if parsed <= 0:
        raise ValueError(f"{context} Y4M {field} expects a positive integer, got {parsed}")
    return parsed


def y4m_pixel_format(chroma_tag: str | None) -> str:
    normalized = (chroma_tag or "420").lower()
    if normalized in {"420", "420jpeg", "420mpeg2", "420paldv"}:
        return "yuv420p8"
    if normalized.startswith("420p"):
        bit_depth = numeric_y4m_bit_depth(normalized, "420p")
        if bit_depth is not None:
            return f"yuv420p{bit_depth}le"
    if normalized == "422":
        return "yuv422p8"
    if normalized.startswith("422p"):
        bit_depth = numeric_y4m_bit_depth(normalized, "422p")
        if bit_depth is not None:
            return f"yuv422p{bit_depth}le"
    if normalized == "444":
        return "yuv444p8"
    if normalized.startswith("444p"):
        bit_depth = numeric_y4m_bit_depth(normalized, "444p")
        if bit_depth is not None:
            return f"yuv444p{bit_depth}le"
    raise ValueError(f"unsupported Y4M chroma format: {chroma_tag or '<default>'}")


def numeric_y4m_bit_depth(normalized: str, prefix: str) -> int | None:
    try:
        bit_depth = int(normalized[len(prefix) :])
    except ValueError:
        return None
    if 8 <= bit_depth <= 16:
        return bit_depth
    return None


def y4m_fps(value: str | None, context: str) -> str | None:
    if value is None:
        return None
    try:
        num_text, den_text = value.split(":", 1)
        num = int(num_text)
        den = int(den_text)
    except ValueError as err:
        raise ValueError(f"{context} Y4M fps expects N:D, got '{value}'") from err
    if num <= 0 or den <= 0:
        raise ValueError(f"{context} Y4M fps expects positive N:D, got '{value}'")
    if den == 1:
        return str(num)
    return f"{num}/{den}"


def generate_source_crop(vector: TestVector, sources: dict[str, TestVectorSource]) -> bytes:
    if vector.source is None:
        raise ValueError(f"{vector.filename} uses source_crop but has no source id")
    if vector.source not in sources:
        raise ValueError(f"{vector.filename} references unknown source '{vector.source}'")
    if vector.crop_x is None or vector.crop_y is None:
        raise ValueError(f"{vector.filename} uses source_crop but has no crop_x/crop_y")
    if vector.frames != 1:
        raise ValueError("source_crop vectors currently use the first frame only")

    source = sources[vector.source]
    if not source.path.exists():
        raise ValueError(f"source file is missing for '{source.id}': {source.path}")
    if vector.crop_x + vector.width > source.width or vector.crop_y + vector.height > source.height:
        raise ValueError(f"{vector.filename} crop exceeds source dimensions")

    if source.fmt in {"png_rgb8", "png_rgba8"} and yuv444_bit_depth(vector.fmt) is not None:
        return generate_png_yuv444_crop(vector, source)
    if source.fmt != "yuv420p8" or vector.fmt != "yuv420p8":
        raise ValueError(
            "source_crop supports yuv420p8->yuv420p8 and PNG RGB/RGBA->planar YUV 4:4:4"
        )

    frame_size = source.width * source.height * 3 // 2
    source_frame = source.path.read_bytes()[:frame_size]
    if len(source_frame) != frame_size:
        raise ValueError(f"{source.id} source is smaller than one {source.width}x{source.height} frame")

    y_size = source.width * source.height
    uv_size = y_size // 4
    source_y = source_frame[:y_size]
    source_u = source_frame[y_size : y_size + uv_size]
    source_v = source_frame[y_size + uv_size : y_size + (uv_size * 2)]

    out = bytearray()
    for row in range(vector.height):
        start = (vector.crop_y + row) * source.width + vector.crop_x
        out.extend(source_y[start : start + vector.width])

    chroma_width = vector.width // 2
    chroma_height = vector.height // 2
    source_chroma_width = source.width // 2
    crop_chroma_x = vector.crop_x // 2
    crop_chroma_y = vector.crop_y // 2
    for plane in (source_u, source_v):
        for row in range(chroma_height):
            start = (crop_chroma_y + row) * source_chroma_width + crop_chroma_x
            out.extend(plane[start : start + chroma_width])
    return bytes(out)


def generate_png_yuv444_crop(vector: TestVector, source: TestVectorSource) -> bytes:
    if Image is None:
        raise ValueError("PNG-backed source_crop vectors require Pillow")

    with Image.open(source.path) as image:
        if image.size != (source.width, source.height):
            raise ValueError(
                f"{source.id} declares {source.width}x{source.height}, "
                f"but PNG is {image.size[0]}x{image.size[1]}"
            )
        crop = image.convert("RGB").crop(
            (
                vector.crop_x,
                vector.crop_y,
                vector.crop_x + vector.width,
                vector.crop_y + vector.height,
            )
        )
        red_plane, green_plane, blue_plane = crop.split()
        out = green_plane.tobytes() + blue_plane.tobytes() + red_plane.tobytes()
        bit_depth = yuv444_bit_depth(vector.fmt)
        if bit_depth is None:
            raise ValueError(f"unsupported PNG-backed output format: {vector.fmt}")
        if bit_depth > 8:
            return zero_pad_planar8_to_le(out, bit_depth)
        return out


def zero_pad_planar8_to_le(data: bytes, bit_depth: int) -> bytes:
    shift = bit_depth - 8
    out = bytearray(len(data) * 2)
    for index, sample in enumerate(data):
        value = sample << shift
        out[index * 2] = value & 0xFF
        out[index * 2 + 1] = value >> 8
    return bytes(out)


def raw_frame_len(vector: TestVector) -> int:
    luma = vector.width * vector.height
    bit_depth = yuv420_bit_depth(vector.fmt)
    if bit_depth is not None:
        return luma * 3 // 2 * bytes_per_sample(bit_depth)
    bit_depth = yuv422_bit_depth(vector.fmt)
    if bit_depth is not None:
        return luma * 2 * bytes_per_sample(bit_depth)
    bit_depth = yuv444_bit_depth(vector.fmt)
    if bit_depth is not None:
        return luma * 3 * bytes_per_sample(bit_depth)
    raise ValueError(f"unsupported source_file format: {vector.fmt}")


def yuv444_bit_depth(fmt: str) -> int | None:
    return planar_yuv_bit_depth(fmt, "yuv444p")


def yuv420_bit_depth(fmt: str) -> int | None:
    return planar_yuv_bit_depth(fmt, "yuv420p")


def yuv422_bit_depth(fmt: str) -> int | None:
    return planar_yuv_bit_depth(fmt, "yuv422p")


def planar_yuv_bit_depth(fmt: str, prefix: str) -> int | None:
    if fmt == f"{prefix}8":
        return 8
    suffix = "le"
    if not fmt.startswith(prefix) or not fmt.endswith(suffix):
        return None
    depth_text = fmt[len(prefix) : -len(suffix)]
    try:
        bit_depth = int(depth_text)
    except ValueError:
        return None
    if 8 <= bit_depth <= 16:
        return bit_depth
    return None


def bytes_per_sample(bit_depth: int) -> int:
    return 1 if bit_depth <= 8 else 2


def generate_yuv420p(vector: TestVector, bit_depth: int) -> bytes:
    if vector.pattern == "bitdepth_canary":
        return generate_yuv420p_bitdepth_canary(vector, bit_depth)

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
    data = bytes(out)
    if bit_depth > 8:
        return zero_pad_planar8_to_le(data, bit_depth)
    return data


def generate_yuv422p(vector: TestVector, bit_depth: int) -> bytes:
    if vector.pattern == "bitdepth_canary":
        return generate_yuv422p_bitdepth_canary(vector, bit_depth)

    out = bytearray()
    for frame in range(vector.frames):
        y_plane, u444, v444 = render_frame(vector, frame)
        u_plane = bytearray()
        v_plane = bytearray()
        for y in range(vector.height):
            for x in range(0, vector.width, 2):
                indices = (
                    pixel_index(vector, x, y),
                    pixel_index(vector, x + 1, y),
                )
                u_plane.append(sum(u444[idx] for idx in indices) // 2)
                v_plane.append(sum(v444[idx] for idx in indices) // 2)
        out.extend(y_plane)
        out.extend(u_plane)
        out.extend(v_plane)
    data = bytes(out)
    if bit_depth > 8:
        return zero_pad_planar8_to_le(data, bit_depth)
    return data


def generate_yuv444p(vector: TestVector, bit_depth: int) -> bytes:
    if vector.pattern == "bitdepth_canary":
        return generate_yuv444p_bitdepth_canary(vector, bit_depth)

    out = bytearray()
    for frame in range(vector.frames):
        y_plane, u_plane, v_plane = render_frame(vector, frame)
        out.extend(y_plane)
        out.extend(u_plane)
        out.extend(v_plane)
    data = bytes(out)
    if bit_depth > 8:
        return zero_pad_planar8_to_le(data, bit_depth)
    return data


def generate_yuv420p_bitdepth_canary(vector: TestVector, bit_depth: int) -> bytes:
    if bit_depth <= 8:
        raise ValueError("bitdepth_canary is intended for high-depth generated vectors")
    out = bytearray()
    for frame in range(vector.frames):
        append_canary_plane(out, vector.width, vector.height, bit_depth, frame, plane=0)
        append_canary_plane(out, vector.width // 2, vector.height // 2, bit_depth, frame, plane=1)
        append_canary_plane(out, vector.width // 2, vector.height // 2, bit_depth, frame, plane=2)
    return bytes(out)


def generate_yuv422p_bitdepth_canary(vector: TestVector, bit_depth: int) -> bytes:
    if bit_depth <= 8:
        raise ValueError("bitdepth_canary is intended for high-depth generated vectors")
    out = bytearray()
    for frame in range(vector.frames):
        append_canary_plane(out, vector.width, vector.height, bit_depth, frame, plane=0)
        append_canary_plane(out, vector.width // 2, vector.height, bit_depth, frame, plane=1)
        append_canary_plane(out, vector.width // 2, vector.height, bit_depth, frame, plane=2)
    return bytes(out)


def generate_yuv444p_bitdepth_canary(vector: TestVector, bit_depth: int) -> bytes:
    if bit_depth <= 8:
        raise ValueError("bitdepth_canary is intended for high-depth generated vectors")
    out = bytearray()
    for frame in range(vector.frames):
        for plane in range(3):
            append_canary_plane(out, vector.width, vector.height, bit_depth, frame, plane)
    return bytes(out)


def append_canary_plane(
    out: bytearray,
    width: int,
    height: int,
    bit_depth: int,
    frame: int,
    plane: int,
) -> None:
    for y in range(height):
        for x in range(width):
            out.extend(bitdepth_canary_sample(x, y, frame, plane, bit_depth).to_bytes(2, "little"))


def bitdepth_canary_sample(x: int, y: int, frame: int, plane: int, bit_depth: int) -> int:
    shift = bit_depth - 8
    low_mask = (1 << shift) - 1
    block_index = ((x // 8) + (y // 8) * 2 + frame) % 4
    base = (
        (32, 96, 160, 224),
        (80, 144, 208, 48),
        (112, 176, 64, 240),
    )[plane][block_index]
    lower = ((x & 3) | ((y & 3) << 2) | (plane << 1) | frame) & low_mask
    if lower == 0:
        lower = low_mask
    return (base << shift) | lower


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
    if vector.pattern == "bitdepth_canary":
        raise ValueError("bitdepth_canary is only supported for high-depth generated vectors")
    raise ValueError(f"unsupported pattern: {vector.pattern}")


def pixel_index(vector: TestVector, x: int, y: int) -> int:
    return y * vector.width + x


if __name__ == "__main__":
    raise SystemExit(main())
