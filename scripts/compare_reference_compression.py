#!/usr/bin/env python3
"""Compare FrameForge bitstream sizes against a reference encoder."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path

import generate_test_vectors


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_VECTOR_DIR = REPO_ROOT / "verification" / "generated" / "test_vectors"
DEFAULT_OUT_DIR = REPO_ROOT / "verification" / "generated" / "compression_compare"
DEFAULT_LOG_DIR = REPO_ROOT / "verification" / "generated" / "compression_compare_logs"
REFERENCE_TOOLS = REPO_ROOT / "scripts" / "reference_tools.py"
VTM_CFG_DIR = REPO_ROOT / "verification" / "references" / "vvc" / "vtm" / "cfg"


@dataclass(frozen=True)
class ComparisonResult:
    vector_name: str
    frameforge_output: Path
    reference_output: Path
    frameforge_bytes: int
    reference_bytes: int
    ratio: float
    lossless: bool
    reference_cached: bool
    log: Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("set", nargs="?", default="smoke", help="test vector set name")
    parser.add_argument("--codec", required=True, help="codec name accepted by ff encode")
    parser.add_argument("--ff", type=Path, default=REPO_ROOT / "ff")
    parser.add_argument("--set-dir", type=Path, default=generate_test_vectors.DEFAULT_SET_DIR)
    parser.add_argument("--vector-dir", type=Path, default=DEFAULT_VECTOR_DIR)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--log-dir", type=Path, default=DEFAULT_LOG_DIR)
    parser.add_argument("--limit", type=int, default=0, help="run only the first N vectors")
    parser.add_argument(
        "--reference-args",
        default="",
        help="extra shell-style arguments appended to the reference encoder command",
    )
    parser.add_argument(
        "--reference-preset",
        choices=("default", "fast"),
        default="fast",
        help="reference encoder preset; 'default' keeps legacy AVM/VTM arguments",
    )
    parser.add_argument(
        "--reference-threads",
        default="auto",
        help="reference encoder threads for fast AVM runs; use auto or a positive integer",
    )
    parser.add_argument(
        "--avm-tile-columns",
        default="auto",
        help="AVM tile columns as log2 value for fast runs; use auto or a non-negative integer",
    )
    parser.add_argument(
        "--avm-tile-rows",
        default="0",
        help="AVM tile rows as log2 value for fast runs; use auto or a non-negative integer",
    )
    parser.add_argument(
        "--refresh-reference",
        action="store_true",
        help="rerun reference encoders instead of reusing matching cached outputs",
    )
    args = parser.parse_args()
    args.reference_threads = parse_auto_int(args.reference_threads, "reference threads", 1)
    args.avm_tile_columns = parse_auto_int(args.avm_tile_columns, "AVM tile columns", 0)
    args.avm_tile_rows = parse_auto_int(args.avm_tile_rows, "AVM tile rows", 0)

    if not args.ff.exists():
        print(f"error: missing CLI binary: {args.ff}; run 'make build' first", file=sys.stderr)
        return 2

    reference_encoder = resolve_reference_encoder(args.codec)
    vectors = generate_test_vectors.generate_vectors(args.set, args.vector_dir, args.set_dir)
    if args.limit:
        vectors = vectors[: args.limit]

    results: list[ComparisonResult] = []
    for index, vector_path in enumerate(vectors, start=1):
        vector = vector_for_path(args.set, args.set_dir, vector_path)
        print(f"[{index:03d}/{len(vectors):03d}] {vector_path.name}", flush=True)
        result = run_case(vector, vector_path, reference_encoder, args)
        results.append(result)
        print(
            "  mode={mode} reference={cache} FrameForge={ff} byte(s), reference={ref} byte(s), ratio={ratio:.3f}x".format(
                mode="lossless" if result.lossless else "default",
                cache="cached" if result.reference_cached else "fresh",
                ff=result.frameforge_bytes,
                ref=result.reference_bytes,
                ratio=result.ratio,
            ),
            flush=True,
        )

    print()
    print(f"FrameForge media compression comparison: {args.set} ({args.codec})")
    print("| # | vector | mode | reference | FrameForge bytes | reference bytes | FF/reference | delta | log |")
    print("|---:|---|---|---|---:|---:|---:|---:|---|")
    total_ff = 0
    total_ref = 0
    for index, result in enumerate(results, start=1):
        total_ff += result.frameforge_bytes
        total_ref += result.reference_bytes
        delta = result.frameforge_bytes - result.reference_bytes
        mode = "lossless" if result.lossless else "default"
        cache = "cached" if result.reference_cached else "fresh"
        print(
            f"| {index} | {result.vector_name} | {mode} | {cache} | {result.frameforge_bytes} | "
            f"{result.reference_bytes} | {result.ratio:.3f}x | {delta:+d} | "
            f"{relpath(result.log)} |"
        )
    if results:
        total_ratio = total_ff / total_ref if total_ref else float("inf")
        print(
            f"| total | {len(results)} vector(s) | mixed | mixed | {total_ff} | {total_ref} | "
            f"{total_ratio:.3f}x | {total_ff - total_ref:+d} | |"
        )
    print()
    print("NOTE: this is a size comparison only, not a validation pass/fail criterion.")
    return 0


def resolve_reference_encoder(codec: str) -> str:
    command = [
        sys.executable,
        str(REFERENCE_TOOLS),
        "encoder",
        "--codec",
        codec,
        "--no-build",
    ]
    process = subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    if process.returncode != 0:
        print(process.stdout, file=sys.stderr, end="")
        return_code = 2 if process.returncode == 2 else process.returncode
        raise SystemExit(return_code)
    lines = [line.strip() for line in process.stdout.splitlines() if line.strip()]
    if not lines:
        raise SystemExit(f"reference encoder command returned no path: {shlex.join(command)}")
    return lines[-1]


def vector_for_path(
    set_name: str, set_dir: Path, vector_path: Path
) -> generate_test_vectors.TestVector:
    vector_set = generate_test_vectors.vector_sets(set_dir)[set_name]
    by_filename = {vector.filename: vector for vector in vector_set.vectors}
    try:
        return by_filename[vector_path.name]
    except KeyError as err:
        raise SystemExit(f"generated vector is not present in manifest: {vector_path}") from err


def run_case(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    reference_encoder: str,
    args: argparse.Namespace,
) -> ComparisonResult:
    frameforge_output, reference_output, log = case_paths(vector_path.stem, args)
    if frameforge_output.exists():
        frameforge_output.unlink()

    frameforge_cmd = [
        str(args.ff),
        "encode",
        str(vector_path),
        "--video",
        f"{vector.width}x{vector.height}:{vector.fmt}",
        "--frames",
        str(vector.frames),
    ]
    if vector.fps is not None:
        frameforge_cmd.extend(["--fps", vector.fps])
    frameforge_cmd.extend(
        [
            "--encode",
            f"{args.codec}:{frameforge_output}",
        ]
    )
    if vector.lossless:
        frameforge_cmd.extend(["--set", "lossless"])
    reference_cmd = reference_encode_command(
        vector,
        vector_path,
        reference_output,
        reference_encoder,
        args,
    )
    metadata = reference_cache_metadata(vector, vector_path, reference_encoder, reference_cmd, args)
    metadata_path = reference_metadata_path(reference_output)

    frameforge_result = run_logged(frameforge_cmd)
    reference_cached = False
    reference_output_text = ""
    if not args.refresh_reference and reference_cache_valid(
        reference_output, metadata_path, metadata, args.codec
    ):
        reference_cached = True
        reference_output_text = (
            f"cached reference output: {relpath(reference_output)}\n"
            f"cache metadata: {relpath(metadata_path)}\n"
        )
        reference_returncode = 0
    else:
        remove_reference_outputs(reference_output, args.codec)
        reference_result = run_logged(reference_cmd)
        reference_output_text = reference_result.stdout
        reference_returncode = reference_result.returncode

    log.write_text(
        f"$ {shlex.join(frameforge_cmd)}\n\n{frameforge_result.stdout}\n\n"
        f"$ {shlex.join(reference_cmd)}\n\n{reference_output_text}"
    )

    if frameforge_result.returncode != 0:
        raise SystemExit(f"FrameForge encode failed for {vector_path.name}; see {relpath(log)}")
    if reference_returncode != 0:
        raise SystemExit(f"reference encode failed for {vector_path.name}; see {relpath(log)}")
    require_non_empty(frameforge_output, "FrameForge", vector_path, log)
    require_non_empty(reference_output, "reference", vector_path, log)
    require_reference_outputs(reference_output, args.codec, vector_path, log)
    if not reference_cached:
        metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n")

    frameforge_bytes = frameforge_output.stat().st_size
    reference_bytes = reference_output.stat().st_size
    ratio = frameforge_bytes / reference_bytes if reference_bytes else float("inf")
    return ComparisonResult(
        vector_name=vector_path.name,
        frameforge_output=frameforge_output,
        reference_output=reference_output,
        frameforge_bytes=frameforge_bytes,
        reference_bytes=reference_bytes,
        ratio=ratio,
        lossless=vector.lossless,
        reference_cached=reference_cached,
        log=log,
    )


def case_paths(stem: str, args: argparse.Namespace) -> tuple[Path, Path, Path]:
    output_dir = args.out_dir / args.codec / args.set
    log_dir = args.log_dir / args.codec
    output_dir.mkdir(parents=True, exist_ok=True)
    log_dir.mkdir(parents=True, exist_ok=True)
    extension = codec_extension(args.codec)
    frameforge_output = output_dir / f"{stem}_frameforge.{extension}"
    reference_output = output_dir / f"{stem}_reference.{extension}"
    log = log_dir / f"{args.set}_{stem}.log"
    return frameforge_output, reference_output, log


def reference_recon_path(output: Path) -> Path:
    return output.with_name(f"{output.stem}_recon.yuv")


def reference_metadata_path(output: Path) -> Path:
    return output.with_name(f"{output.stem}_metadata.json")


def reference_outputs(output: Path, codec: str) -> list[Path]:
    outputs = [output]
    if codec == "vvc":
        outputs.append(reference_recon_path(output))
    return outputs


def remove_reference_outputs(output: Path, codec: str) -> None:
    for path in [*reference_outputs(output, codec), reference_metadata_path(output)]:
        if path.exists():
            path.unlink()


def require_reference_outputs(
    output: Path, codec: str, vector_path: Path, log: Path
) -> None:
    for path in reference_outputs(output, codec):
        require_non_empty(path, "reference", vector_path, log)


def reference_cache_valid(
    output: Path,
    metadata_path: Path,
    expected_metadata: dict,
    codec: str,
) -> bool:
    if not metadata_path.exists():
        return False
    for path in reference_outputs(output, codec):
        if not path.exists() or path.stat().st_size == 0:
            return False
    try:
        existing_metadata = json.loads(metadata_path.read_text())
    except json.JSONDecodeError:
        return False
    return existing_metadata == expected_metadata


def reference_cache_metadata(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    reference_encoder: str,
    reference_cmd: list[str],
    args: argparse.Namespace,
) -> dict:
    encoder_path = Path(reference_encoder)
    encoder_stat = encoder_path.stat()
    return {
        "version": 1,
        "codec": args.codec,
        "set": args.set,
        "reference_args": args.reference_args,
        "reference_preset": args.reference_preset,
        "reference_threads": args.reference_threads,
        "avm_tile_columns": args.avm_tile_columns,
        "avm_tile_rows": args.avm_tile_rows,
        "reference_command": reference_cmd,
        "reference_encoder": {
            "path": str(encoder_path),
            "mtime_ns": encoder_stat.st_mtime_ns,
            "size": encoder_stat.st_size,
        },
        "input": {
            "path": str(vector_path),
            "sha256": sha256_file(vector_path),
            "size": vector_path.stat().st_size,
        },
        "vector": {
            "filename": vector.filename,
            "width": vector.width,
            "height": vector.height,
            "frames": vector.frames,
            "format": vector.fmt,
            "fps": vector.fps,
            "lossless": vector.lossless,
        },
    }


def reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    if args.codec == "av2":
        return av2_reference_encode_command(vector, vector_path, output, encoder, args)
    if args.codec == "vvc":
        return vvc_reference_encode_command(vector, vector_path, output, encoder, args)
    raise SystemExit(
        "reference compression comparison currently supports codecs av2 and vvc; "
        f"got {args.codec}"
    )


def av2_reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    command = [
        encoder,
        "--codec=av2",
        "--obu",
        f"--limit={vector.frames}",
        f"--width={vector.width}",
        f"--height={vector.height}",
        f"--fps={reference_fps_ratio(vector)}",
        "--input-bit-depth=8",
        "--bit-depth=8",
        "--cpu-used=9",
        "--psnr=0",
        "--quiet",
        "--disable-warning-prompt",
    ]
    command.extend(avm_reference_preset_args(vector, args))
    if vector.lossless:
        command.append("--lossless=1")
    if vector.fmt == "yuv420p8":
        command.append("--i420")
    elif vector.fmt == "yuv444p8":
        command.extend(["--i444", "--profile=1"])
    else:
        raise SystemExit(f"unsupported AV2 reference encode pixel format: {vector.fmt}")
    if args.reference_args:
        command.extend(shlex.split(args.reference_args))
    command.extend(["-o", str(output), str(vector_path)])
    return command


def avm_reference_preset_args(
    vector: generate_test_vectors.TestVector,
    args: argparse.Namespace,
) -> list[str]:
    if args.reference_preset == "default":
        return []

    threads = reference_thread_count(args.reference_threads)
    tile_columns = avm_tile_columns(vector, args.avm_tile_columns, threads)
    tile_rows = avm_tile_rows(vector, args.avm_tile_rows, threads)
    return [
        f"--threads={threads}",
        "--row-mt=1",
        f"--tile-columns={tile_columns}",
        f"--tile-rows={tile_rows}",
        "--lag-in-frames=0",
        "--auto-alt-ref=0",
        "--enable-keyframe-filtering=0",
        "--test-decode=off",
    ]


def reference_thread_count(value: int | None) -> int:
    if value is not None:
        return value
    return max(1, os.cpu_count() or 1)


def avm_tile_columns(
    vector: generate_test_vectors.TestVector,
    value: int | None,
    threads: int,
) -> int:
    if value is not None:
        return value
    if threads <= 1 or vector.width < 1280:
        return 0
    if vector.width >= 3840 and threads >= 4:
        return 2
    return 1


def avm_tile_rows(
    vector: generate_test_vectors.TestVector,
    value: int | None,
    threads: int,
) -> int:
    if value is not None:
        return value
    if threads >= 8 and vector.height >= 2160:
        return 1
    return 0


def vvc_reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    bit_depth = generate_test_vectors.yuv420_bit_depth(vector.fmt)
    if bit_depth is not None:
        chroma_format = "420"
        if bit_depth > 12:
            raise SystemExit(
                f"unsupported VVC reference encode pixel format for native FrameForge comparison: {vector.fmt}"
            )
    else:
        bit_depth = generate_test_vectors.yuv444_bit_depth(vector.fmt)
        if bit_depth is None:
            raise SystemExit(f"unsupported VVC reference encode pixel format: {vector.fmt}")
        chroma_format = "444"
        if bit_depth > 12:
            raise SystemExit(
                f"unsupported VVC reference encode pixel format for native FrameForge comparison: {vector.fmt}"
            )

    command = [
        encoder,
        "-c",
        str(VTM_CFG_DIR / "encoder_intra_vtm.cfg"),
    ]
    if vector.lossless:
        command.extend(["-c", str(VTM_CFG_DIR / "lossless" / "lossless.cfg")])
        if chroma_format == "444":
            command.extend(["-c", str(VTM_CFG_DIR / "lossless" / "lossless444.cfg")])

    command.extend(
        [
            "-i",
            str(vector_path),
            "-b",
            str(output),
            "-o",
            str(reference_recon_path(output)),
            "-wdt",
            str(vector.width),
            "-hgt",
            str(vector.height),
            "-fr",
            str(reference_integer_fps(vector)),
            "-f",
            str(vector.frames),
            f"--InputBitDepth={bit_depth}",
            f"--InternalBitDepth={bit_depth}",
            f"--InputChromaFormat={chroma_format}",
            f"--ChromaFormatIDC={chroma_format}",
            "--TemporalSubsampleRatio=1",
            "--Verbosity=0",
        ]
    )
    if args.reference_args:
        command.extend(shlex.split(args.reference_args))
    return command


def reference_fps_ratio(vector: generate_test_vectors.TestVector) -> str:
    fps = fps_fraction(vector)
    return f"{fps.numerator}/{fps.denominator}"


def reference_integer_fps(vector: generate_test_vectors.TestVector) -> int:
    fps = fps_fraction(vector)
    return (fps.numerator + fps.denominator // 2) // fps.denominator


def fps_fraction(vector: generate_test_vectors.TestVector) -> Fraction:
    return Fraction(vector.fps or "30")


def parse_auto_int(value: str, field: str, min_value: int) -> int | None:
    normalized = value.strip().lower()
    if normalized == "auto":
        return None
    try:
        parsed = int(normalized)
    except ValueError as err:
        raise SystemExit(
            f"{field} expects auto or an integer >= {min_value}, got '{value}'"
        ) from err
    if parsed < min_value:
        raise SystemExit(f"{field} expects auto or an integer >= {min_value}, got {parsed}")
    return parsed


def run_logged(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def require_non_empty(output: Path, label: str, vector_path: Path, log: Path) -> None:
    if not output.exists():
        raise SystemExit(f"{label} encode produced no output for {vector_path.name}; see {relpath(log)}")
    if output.stat().st_size == 0:
        raise SystemExit(f"{label} encode produced empty output for {vector_path.name}; see {relpath(log)}")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def codec_extension(codec: str) -> str:
    return {"av2": "obu", "vvc": "vvc"}.get(codec, codec)


def relpath(path: Path) -> Path:
    try:
        return path.resolve().relative_to(REPO_ROOT)
    except ValueError:
        return path


if __name__ == "__main__":
    raise SystemExit(main())
