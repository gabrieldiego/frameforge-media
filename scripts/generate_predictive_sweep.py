#!/usr/bin/env python3
"""Generate local 3-frame predictive crop sweeps from AOM CTC SCC streams."""

from __future__ import annotations

import argparse
import csv
import random
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT_DIR = (
    REPO_ROOT
    / "verification"
    / "generated"
    / "test_vectors"
    / "aomctc_b2_scc_predictive_sweep"
)
DEFAULT_MANIFEST = (
    REPO_ROOT
    / "verification"
    / "test_vector_sets"
    / "local"
    / "local-aomctc-b2-scc-predictive-sweep-3f.csv"
)
DEFAULT_SEED = 0xA02C_3F42


@dataclass(frozen=True)
class SourceVariant:
    name: str
    path: Path
    fmt: str


@dataclass(frozen=True)
class Y4mMetadata:
    width: int
    height: int
    fmt: str
    fps: str
    frame_len: int


@dataclass(frozen=True)
class CropCase:
    name: str
    variant: SourceVariant
    width: int
    height: int
    start_frame: int
    crop_x: int
    crop_y: int
    output: Path


SOURCE_VARIANTS = (
    SourceVariant(
        "scene_420_8",
        Path("/media/gabriel/storage/YUV/aomctc/b2_scc/SceneComposition_1.y4m"),
        "yuv420p8",
    ),
    SourceVariant(
        "scene_422_8",
        REPO_ROOT
        / "verification/generated/test_vectors/aomctc_b2_scc/SceneComposition_1_1920x1080_15_50f_yuv422p8.y4m",
        "yuv422p8",
    ),
    SourceVariant(
        "scene_444_8",
        REPO_ROOT
        / "verification/generated/test_vectors/aomctc_b2_scc/SceneComposition_1_1920x1080_15_50f_yuv444p8.y4m",
        "yuv444p8",
    ),
    SourceVariant(
        "mission_420_10",
        Path(
            "/media/gabriel/storage/YUV/aomctc/b2_scc/MissionControlClip1_1920x1080_60fps_10bit_420_0450_0579.y4m"
        ),
        "yuv420p10le",
    ),
    SourceVariant(
        "mission_422_10",
        REPO_ROOT
        / "verification/generated/test_vectors/aomctc_b2_scc/MissionControlClip1_1920x1080_60_50f_yuv422p10.y4m",
        "yuv422p10le",
    ),
    SourceVariant(
        "mission_444_10",
        REPO_ROOT
        / "verification/generated/test_vectors/aomctc_b2_scc/MissionControlClip1_1920x1080_60_50f_yuv444p10.y4m",
        "yuv444p10le",
    ),
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--seed", type=int, default=DEFAULT_SEED)
    parser.add_argument("--refresh", action="store_true", help="regenerate existing Y4M crops")
    parser.add_argument("--dry-run", action="store_true", help="print ffmpeg commands without running them")
    args = parser.parse_args()

    missing = [variant.path for variant in SOURCE_VARIANTS if not variant.path.exists()]
    if missing:
        print("error: missing source stream(s):", file=sys.stderr)
        for path in missing:
            print(f"  {path}", file=sys.stderr)
        return 2

    metadata = load_metadata(SOURCE_VARIANTS)
    cases = build_cases(SOURCE_VARIANTS, metadata, args.out_dir, args.seed)
    args.out_dir.mkdir(parents=True, exist_ok=True)
    args.manifest.parent.mkdir(parents=True, exist_ok=True)

    generated = 0
    for index, case in enumerate(cases, start=1):
        if case.output.exists() and not args.refresh:
            continue
        command = ffmpeg_crop_command(case)
        if args.dry_run:
            print(shlex.join(command))
            continue
        print(f"[{index:03d}/{len(cases):03d}] {case.output.name}", flush=True)
        subprocess.run(command, cwd=REPO_ROOT, check=True)
        generated += 1

    write_manifest(args.manifest, cases)
    print(
        f"Wrote {len(cases)} case(s) to {relative_or_absolute(args.manifest)}; "
        f"generated {generated} Y4M crop(s)"
    )
    return 0


def load_metadata(variants: tuple[SourceVariant, ...]) -> dict[str, Y4mMetadata]:
    out = {}
    for variant in variants:
        metadata = read_y4m_metadata(variant.path)
        if metadata.fmt != variant.fmt:
            raise ValueError(
                f"{variant.name} expected {variant.fmt}, but {variant.path} is {metadata.fmt}"
            )
        out[variant.name] = metadata
    return out


def read_y4m_metadata(path: Path) -> Y4mMetadata:
    with path.open("rb") as source:
        header = source.readline()
    if not header.startswith(b"YUV4MPEG2 "):
        raise ValueError(f"not a Y4M stream: {path}")
    tags = y4m_tags(header.decode("ascii", errors="replace").strip())
    width = int(tags["W"])
    height = int(tags["H"])
    fmt = y4m_pixel_format(tags.get("C"))
    fps = y4m_fps(tags.get("F"))
    return Y4mMetadata(
        width=width,
        height=height,
        fmt=fmt,
        fps=fps,
        frame_len=raw_frame_len(width, height, fmt),
    )


def y4m_tags(header: str) -> dict[str, str]:
    fields = header.split()
    if not fields or fields[0] != "YUV4MPEG2":
        raise ValueError(f"invalid Y4M header: {header}")
    return {field[0]: field[1:] for field in fields[1:] if len(field) >= 2}


def y4m_pixel_format(chroma_tag: str | None) -> str:
    normalized = (chroma_tag or "420").lower()
    if normalized in {"420", "420jpeg", "420mpeg2", "420paldv"}:
        return "yuv420p8"
    if normalized == "422":
        return "yuv422p8"
    if normalized == "444":
        return "yuv444p8"
    for prefix in ("420p", "422p", "444p"):
        if normalized.startswith(prefix):
            bits = int(normalized[len(prefix) :])
            family = {"420p": "yuv420p", "422p": "yuv422p", "444p": "yuv444p"}[prefix]
            return f"{family}{bits}le"
    raise ValueError(f"unsupported Y4M chroma format: {chroma_tag or '<default>'}")


def y4m_fps(value: str | None) -> str:
    if value is None:
        return ""
    num, den = value.split(":", 1)
    return num if den == "1" else f"{num}/{den}"


def raw_frame_len(width: int, height: int, fmt: str) -> int:
    samples = width * height
    if fmt.startswith("yuv420p"):
        samples = samples * 3 // 2
    elif fmt.startswith("yuv422p"):
        samples *= 2
    elif fmt.startswith("yuv444p"):
        samples *= 3
    else:
        raise ValueError(f"unsupported format: {fmt}")
    return samples * bytes_per_sample(fmt)


def bytes_per_sample(fmt: str) -> int:
    return 1 if fmt.endswith("p8") else 2


def build_cases(
    variants: tuple[SourceVariant, ...],
    metadata: dict[str, Y4mMetadata],
    out_dir: Path,
    seed: int,
) -> list[CropCase]:
    rng = random.Random(seed)
    frame_counts = {
        variant.name: count_y4m_frames(variant.path, metadata[variant.name].frame_len)
        for variant in variants
    }
    cases = []
    for variant in variants:
        meta = metadata[variant.name]
        total_frames = frame_counts[variant.name]
        if total_frames < 3:
            raise ValueError(f"{variant.path} has {total_frames} frame(s); expected at least 3")
        for width in range(8, 65, 8):
            for height in range(8, 65, 8):
                start_frame = rng.randrange(total_frames - 2)
                crop_x, crop_y = random_crop_origin(rng, meta, width, height)
                name = (
                    f"aomctc_pred_{variant.name}_{width}x{height}"
                    f"_f{start_frame:03d}_x{crop_x:04d}_y{crop_y:04d}"
                )
                cases.append(
                    CropCase(
                        name=name,
                        variant=variant,
                        width=width,
                        height=height,
                        start_frame=start_frame,
                        crop_x=crop_x,
                        crop_y=crop_y,
                        output=out_dir / f"{name}.y4m",
                    )
                )
    rng.shuffle(cases)
    return cases


def count_y4m_frames(path: Path, frame_len: int) -> int:
    frames = 0
    with path.open("rb") as source:
        header = source.readline()
        if not header.startswith(b"YUV4MPEG2 "):
            raise ValueError(f"not a Y4M stream: {path}")
        while True:
            marker = source.readline()
            if not marker:
                break
            if not marker.startswith(b"FRAME"):
                raise ValueError(f"{path} has invalid frame marker at frame {frames + 1}")
            payload = source.read(frame_len)
            if len(payload) != frame_len:
                break
            frames += 1
    return frames


def random_crop_origin(
    rng: random.Random, metadata: Y4mMetadata, width: int, height: int
) -> tuple[int, int]:
    if width > metadata.width or height > metadata.height:
        raise ValueError(f"crop {width}x{height} exceeds source {metadata.width}x{metadata.height}")
    align_x = 2 if metadata.fmt.startswith(("yuv420p", "yuv422p")) else 1
    align_y = 2 if metadata.fmt.startswith("yuv420p") else 1
    return (
        random_aligned(rng, metadata.width - width, align_x),
        random_aligned(rng, metadata.height - height, align_y),
    )


def random_aligned(rng: random.Random, maximum: int, alignment: int) -> int:
    if maximum <= 0:
        return 0
    slots = maximum // alignment
    return rng.randrange(slots + 1) * alignment


def ffmpeg_crop_command(case: CropCase) -> list[str]:
    end_frame = case.start_frame + 1
    filters = (
        f"trim=start_frame={case.start_frame}:end_frame={end_frame},"
        f"crop={case.width}:{case.height}:{case.crop_x}:{case.crop_y},"
        "loop=loop=2:size=1:start=0,"
        "setpts=N/FRAME_RATE/TB"
    )
    return [
        "ffmpeg",
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        str(case.variant.path),
        "-vf",
        filters,
        "-frames:v",
        "3",
        "-pix_fmt",
        ffmpeg_pixel_format(case.variant.fmt),
        "-strict",
        "-1",
        str(case.output),
    ]


def ffmpeg_pixel_format(fmt: str) -> str:
    return {
        "yuv420p8": "yuv420p",
        "yuv422p8": "yuv422p",
        "yuv444p8": "yuv444p",
    }.get(fmt, fmt)


def write_manifest(path: Path, cases: list[CropCase]) -> None:
    with path.open("w", newline="") as manifest:
        manifest.write("# generator=scripts/generate_predictive_sweep.py\n")
        manifest.write(
            "# description=Local 3-frame predictive crop sweep from AOM CTC B2 screen-content streams. "
            "Each vector repeats one randomly selected crop three times so AV2 SEF/reference-buffer "
            "syntax is exercised for every 8x8..64x64 geometry at 8/10-bit 4:2:0, 4:2:2, and 4:4:4.\n"
        )
        writer = csv.writer(manifest, lineterminator="\n")
        writer.writerow(
            ["name", "width", "height", "frames", "format", "pattern", "fps", "lossless", "codecs", "path"]
        )
        for case in cases:
            writer.writerow(
                [
                    case.name,
                    "",
                    "",
                    3,
                    "",
                    "source_file",
                    "",
                    "true",
                    "av2",
                    relative_or_absolute(case.output),
                ]
            )


def relative_or_absolute(path: Path) -> str:
    try:
        return str(path.resolve().relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


if __name__ == "__main__":
    raise SystemExit(main())
