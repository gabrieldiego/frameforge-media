#!/usr/bin/env python3
"""Compare FrameForge bitstream sizes against a reference encoder."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
import shutil
import shlex
import subprocess
import sys
import time
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
VVC_MIN_BIT_DEPTH = 8
VVC_MAX_BIT_DEPTH = 12
REFERENCE_BACKEND_NATIVE = "reference"
REFERENCE_BACKEND_RAV1E = "rav1e"
REFERENCE_BACKEND_FFMPEG_LIBAOM = "ffmpeg-libaom"


@dataclass(frozen=True)
class ComparisonResult:
    vector_name: str
    frameforge_output: Path
    reference_output: Path
    frameforge_bytes: int
    reference_bytes: int
    ratio: float
    frameforge_seconds: float
    frameforge_fps: float
    frame_count: int
    lossless: bool
    qp: int | None
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
        "--setting",
        action="append",
        default=[],
        help="extra FrameForge --set key[=value] setting; repeat for multiple settings",
    )
    parser.add_argument(
        "--qp",
        type=parse_qp,
        default=None,
        help=(
            "FrameForge AV2 lossy QP; when present, it overrides manifest "
            "lossless=true rows for the FrameForge encode"
        ),
    )
    parser.add_argument(
        "--reference-backend",
        default=REFERENCE_BACKEND_NATIVE,
        help=(
            "compression baseline backend: reference for AVM/VTM, rav1e "
            "for an AV1 rav1e baseline, or ffmpeg-libaom for an AV1 libaom baseline"
        ),
    )
    parser.add_argument(
        "--reference-preset",
        choices=("default", "fast", "realtime-screen", "lossless"),
        default="fast",
        help=(
            "reference encoder preset; 'default' keeps legacy AVM/VTM arguments, "
            "'realtime-screen' selects ffmpeg/libaom realtime screen-share settings, "
            "and 'lossless' selects ffmpeg/libaom AV1 lossless"
        ),
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
    parser.add_argument(
        "--direct-source-files",
        action="store_true",
        help=(
            "for source_file manifest rows, feed the source path directly and use the "
            "manifest frame count as the limiter instead of materializing a raw clip"
        ),
    )
    args = parser.parse_args()
    args.reference_backend = normalize_reference_backend(args.reference_backend)
    args.reference_threads = parse_auto_int(args.reference_threads, "reference threads", 1)
    args.avm_tile_columns = parse_auto_int(args.avm_tile_columns, "AVM tile columns", 0)
    args.avm_tile_rows = parse_auto_int(args.avm_tile_rows, "AVM tile rows", 0)

    if not args.ff.exists():
        print(f"error: missing CLI binary: {args.ff}; run 'make build' first", file=sys.stderr)
        return 2
    args.ff = args.ff.resolve()

    reference_encoder = resolve_reference_encoder(args.codec, args.reference_backend)
    cases = comparison_cases(args)
    enabled_vectors = []
    skipped = 0
    for vector, vector_path in cases:
        if vector.codecs is not None and args.codec.lower() not in vector.codecs:
            skipped += 1
            continue
        enabled_vectors.append((vector, vector_path))
    cases = enabled_vectors
    if args.limit:
        cases = cases[: args.limit]
    if skipped:
        print(
            f"Skipped {skipped} vector(s) not enabled for codec {args.codec}",
            flush=True,
        )

    results: list[ComparisonResult] = []
    for index, (vector, vector_path) in enumerate(cases, start=1):
        print(f"[{index:03d}/{len(cases):03d}] {vector.filename}", flush=True)
        result = run_case(vector, vector_path, reference_encoder, args)
        results.append(result)
        print(
            "  mode={mode} baseline={backend} reference={cache} "
            "FrameForge={ff} byte(s), reference={ref} byte(s), "
            "ratio={ratio:.3f}x, encode={fps} fps".format(
                mode=result_mode(result),
                backend=args.reference_backend,
                cache="cached" if result.reference_cached else "fresh",
                ff=result.frameforge_bytes,
                ref=result.reference_bytes,
                ratio=result.ratio,
                fps=format_fps(result.frameforge_fps),
            ),
            flush=True,
        )

    print()
    print(
        "FrameForge media compression comparison: "
        f"{args.set} ({args.codec}, baseline={args.reference_backend})"
    )
    print(
        "| # | vector | mode | reference | FrameForge bytes | reference bytes | "
        "FF/reference | delta | FF encode fps | log |"
    )
    print("|---:|---|---|---|---:|---:|---:|---:|---:|---|")
    total_ff = 0
    total_ref = 0
    total_frames = 0
    total_frameforge_seconds = 0.0
    for index, result in enumerate(results, start=1):
        total_ff += result.frameforge_bytes
        total_ref += result.reference_bytes
        total_frames += result.frame_count
        total_frameforge_seconds += result.frameforge_seconds
        delta = result.frameforge_bytes - result.reference_bytes
        mode = result_mode(result)
        cache = "cached" if result.reference_cached else "fresh"
        print(
            f"| {index} | {result.vector_name} | {mode} | {cache} | {result.frameforge_bytes} | "
            f"{result.reference_bytes} | {result.ratio:.3f}x | {delta:+d} | "
            f"{format_fps(result.frameforge_fps)} | "
            f"{relpath(result.log)} |"
        )
    if results:
        total_ratio = total_ff / total_ref if total_ref else float("inf")
        total_fps = (
            total_frames / total_frameforge_seconds
            if total_frameforge_seconds > 0.0
            else float("inf")
        )
        print(
            f"| total | {len(results)} vector(s) | mixed | mixed | {total_ff} | {total_ref} | "
            f"{total_ratio:.3f}x | {total_ff - total_ref:+d} | {format_fps(total_fps)} | |"
        )
    print()
    print(
        "NOTE: FrameForge encode FPS is timed locally; reference encoders are only size baselines."
    )
    return 0


def normalize_reference_backend(value: str) -> str:
    normalized = value.strip().lower()
    if normalized in {"reference", "native", "avm", "vtm"}:
        return REFERENCE_BACKEND_NATIVE
    if normalized in {"rav1e", "av1-rav1e"}:
        return REFERENCE_BACKEND_RAV1E
    if normalized in {"ffmpeg-libaom", "ffmpeg_libaom", "libaom", "av1-libaom"}:
        return REFERENCE_BACKEND_FFMPEG_LIBAOM
    if normalized == "dav1d":
        raise SystemExit(
            "dav1d is an AV1 decoder, not an encoder; use "
            "COMPRESSION_REFERENCE_BACKEND=rav1e for the AV1 encode baseline"
        )
    raise SystemExit(
        "unsupported compression reference backend "
        f"'{value}'; expected reference, rav1e, or ffmpeg-libaom"
    )


def resolve_reference_encoder(codec: str, reference_backend: str) -> str:
    if reference_backend == REFERENCE_BACKEND_FFMPEG_LIBAOM:
        ffmpeg = shutil.which("ffmpeg")
        if ffmpeg is None:
            raise SystemExit("ffmpeg/libaom baseline requested, but ffmpeg is not in PATH")
        return ffmpeg

    reference_codec = codec if reference_backend == REFERENCE_BACKEND_NATIVE else reference_backend
    command = [
        sys.executable,
        str(REFERENCE_TOOLS),
        "encoder",
        "--codec",
        reference_codec,
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


def comparison_cases(args: argparse.Namespace) -> list[tuple[generate_test_vectors.TestVector, Path]]:
    sets = generate_test_vectors.vector_sets(args.set_dir)
    if args.set not in sets:
        choices = ", ".join(sorted(sets)) or "<none>"
        raise SystemExit(f"unknown test vector set '{args.set}'; choices: {choices}")

    vector_set = sets[args.set]
    args.vector_dir.mkdir(parents=True, exist_ok=True)
    cases: list[tuple[generate_test_vectors.TestVector, Path]] = []
    for vector in vector_set.vectors:
        if args.direct_source_files and vector.pattern == "source_file" and vector.source_path:
            cases.append((vector, source_file_path(vector)))
            continue
        path = args.vector_dir / vector.filename
        path.write_bytes(generate_test_vectors.generate_yuv(vector, vector_set.sources))
        cases.append((vector, path))
    return cases


def source_file_path(vector: generate_test_vectors.TestVector) -> Path:
    assert vector.source_path is not None
    path = vector.source_path
    if path.is_absolute():
        return path
    return (REPO_ROOT / path).resolve(strict=False)


def effective_frameforge_lossless(
    vector: generate_test_vectors.TestVector, args: argparse.Namespace
) -> bool:
    return vector.lossless and args.qp is None


def run_case(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    reference_encoder: str,
    args: argparse.Namespace,
) -> ComparisonResult:
    frameforge_output, reference_output, log = case_paths(Path(vector.filename).stem, args)
    if frameforge_output.exists():
        frameforge_output.unlink()
    lossless = effective_frameforge_lossless(vector, args)

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
    if lossless:
        frameforge_cmd.extend(["--set", "lossless"])
    for setting in args.setting:
        frameforge_cmd.extend(["--set", setting])
    if args.qp is not None:
        frameforge_cmd.extend(["--qp", str(args.qp)])
    reference_cmd = reference_encode_command(
        vector,
        vector_path,
        reference_output,
        reference_encoder,
        args,
    )
    metadata = reference_cache_metadata(vector, vector_path, reference_encoder, reference_cmd, args)
    metadata_path = reference_metadata_path(reference_output)

    frameforge_result, frameforge_seconds = run_logged_timed(frameforge_cmd)
    frameforge_fps = (
        vector.frames / frameforge_seconds if frameforge_seconds > 0.0 else float("inf")
    )
    reference_cached = False
    reference_output_text = ""
    reference_run_cmd = reference_cmd
    reference_run_output = reference_output
    if not args.refresh_reference and reference_cache_valid(
        reference_output, metadata_path, metadata, args
    ):
        reference_cached = True
        reference_output_text = (
            f"cached reference output: {relpath(reference_output)}\n"
            f"cache metadata: {relpath(metadata_path)}\n"
        )
        reference_returncode = 0
    else:
        reference_run_output = temporary_reference_output(reference_output)
        reference_run_cmd = reference_encode_command(
            vector,
            vector_path,
            reference_run_output,
            reference_encoder,
            args,
        )
        remove_reference_outputs(reference_run_output, args)
        reference_result = run_logged(reference_run_cmd)
        reference_output_text = reference_result.stdout
        reference_returncode = reference_result.returncode

    reference_log_header = f"$ {shlex.join(reference_run_cmd)}"
    if reference_run_cmd != reference_cmd:
        reference_log_header += f"\n# cache command: {shlex.join(reference_cmd)}"
    log.write_text(
        f"$ {shlex.join(frameforge_cmd)}\n\n{frameforge_result.stdout}\n"
        f"FrameForge timing: elapsed_seconds={frameforge_seconds:.6f} "
        f"encode_fps={format_fps(frameforge_fps)}\n\n"
        f"{reference_log_header}\n\n{reference_output_text}"
    )

    if frameforge_result.returncode != 0:
        raise SystemExit(f"FrameForge encode failed for {vector.filename}; see {relpath(log)}")
    if reference_returncode != 0:
        if not reference_cached:
            remove_reference_outputs(reference_run_output, args)
        raise SystemExit(f"reference encode failed for {vector.filename}; see {relpath(log)}")
    require_non_empty(frameforge_output, "FrameForge", vector.filename, log)
    if reference_cached:
        require_non_empty(reference_output, "reference", vector.filename, log)
        require_reference_outputs(reference_output, args, vector.filename, log)
    else:
        require_non_empty(reference_run_output, "reference", vector.filename, log)
        require_reference_outputs(reference_run_output, args, vector.filename, log)
        promote_reference_outputs(reference_run_output, reference_output, args)
        metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n")

    frameforge_bytes = frameforge_output.stat().st_size
    reference_bytes = reference_output.stat().st_size
    ratio = frameforge_bytes / reference_bytes if reference_bytes else float("inf")
    return ComparisonResult(
        vector_name=vector.filename,
        frameforge_output=frameforge_output,
        reference_output=reference_output,
        frameforge_bytes=frameforge_bytes,
        reference_bytes=reference_bytes,
        ratio=ratio,
        frameforge_seconds=frameforge_seconds,
        frameforge_fps=frameforge_fps,
        frame_count=vector.frames,
        lossless=lossless,
        qp=args.qp,
        reference_cached=reference_cached,
        log=log,
    )


def case_paths(stem: str, args: argparse.Namespace) -> tuple[Path, Path, Path]:
    output_dir = (args.out_dir / args.codec / args.set).resolve()
    log_dir = (args.log_dir / args.codec).resolve()
    if args.reference_backend != REFERENCE_BACKEND_NATIVE:
        output_dir = output_dir / args.reference_backend
        log_dir = log_dir / args.reference_backend
    output_dir.mkdir(parents=True, exist_ok=True)
    log_dir.mkdir(parents=True, exist_ok=True)
    frameforge_extension = codec_extension(args.codec)
    reference_extension = reference_extension_for_args(args)
    frameforge_output = output_dir / f"{stem}_frameforge.{frameforge_extension}"
    reference_output = output_dir / f"{stem}_reference.{reference_extension}"
    log = log_dir / f"{args.set}_{stem}.log"
    return frameforge_output, reference_output, log


def reference_recon_path(output: Path) -> Path:
    return output.with_name(f"{output.stem}_recon.yuv")


def reference_metadata_path(output: Path) -> Path:
    return output.with_name(f"{output.stem}_metadata.json")


def temporary_reference_output(output: Path) -> Path:
    return output.with_name(f".{output.stem}.{os.getpid()}.tmp{output.suffix}")


def reference_outputs(output: Path, args: argparse.Namespace) -> list[Path]:
    outputs = [output]
    if args.reference_backend == REFERENCE_BACKEND_NATIVE and args.codec == "vvc":
        outputs.append(reference_recon_path(output))
    return outputs


def remove_reference_outputs(output: Path, args: argparse.Namespace) -> None:
    for path in [*reference_outputs(output, args), reference_metadata_path(output)]:
        if path.exists():
            path.unlink()


def promote_reference_outputs(temp_output: Path, final_output: Path, args: argparse.Namespace) -> None:
    temp_outputs = reference_outputs(temp_output, args)
    final_outputs = reference_outputs(final_output, args)
    for path in final_outputs:
        if path.exists():
            path.unlink()
    for temp_path, final_path in zip(temp_outputs, final_outputs, strict=True):
        temp_path.replace(final_path)


def require_reference_outputs(
    output: Path, args: argparse.Namespace, vector_name: str, log: Path
) -> None:
    for path in reference_outputs(output, args):
        require_non_empty(path, "reference", vector_name, log)


def reference_cache_valid(
    output: Path,
    metadata_path: Path,
    expected_metadata: dict,
    args: argparse.Namespace,
) -> bool:
    if not metadata_path.exists():
        return False
    for path in reference_outputs(output, args):
        if not path.exists() or path.stat().st_size == 0:
            return False
    try:
        existing_metadata = json.loads(metadata_path.read_text())
    except json.JSONDecodeError:
        return False
    return cache_metadata_equivalent(existing_metadata, expected_metadata)


def cache_metadata_equivalent(existing_metadata: dict, expected_metadata: dict) -> bool:
    return normalize_cache_metadata(existing_metadata) == normalize_cache_metadata(
        expected_metadata
    )


def normalize_cache_metadata(metadata: dict) -> dict:
    normalized = copy.deepcopy(metadata)
    if isinstance(normalized.get("input"), dict) and "path" in normalized["input"]:
        normalized["input"]["path"] = normalize_cache_path(normalized["input"]["path"])
    if (
        isinstance(normalized.get("reference_encoder"), dict)
        and "path" in normalized["reference_encoder"]
    ):
        normalized["reference_encoder"]["path"] = normalize_cache_path(
            normalized["reference_encoder"]["path"]
        )
    if isinstance(normalized.get("reference_command"), list):
        normalized["reference_command"] = [
            normalize_cache_path(token) if isinstance(token, str) else token
            for token in normalized["reference_command"]
        ]
    return normalized


def normalize_cache_path(value: str) -> str:
    if value.startswith("--"):
        return value
    path = Path(value)
    if path.is_absolute():
        return str(path.resolve(strict=False))
    if (
        value.startswith("./")
        or value.startswith("../")
        or "/" in value
        or (REPO_ROOT / path).exists()
    ):
        return str((REPO_ROOT / path).resolve(strict=False))
    return value


def reference_cache_metadata(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    reference_encoder: str,
    reference_cmd: list[str],
    args: argparse.Namespace,
) -> dict:
    encoder_path = Path(reference_encoder)
    encoder_stat = encoder_path.stat()
    metadata = {
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
            "lossless": effective_frameforge_lossless(vector, args),
        },
    }
    if args.reference_backend != REFERENCE_BACKEND_NATIVE:
        metadata["reference_backend"] = args.reference_backend
    if args.direct_source_files:
        metadata["direct_source_files"] = True
    return metadata


def reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    if args.reference_backend == REFERENCE_BACKEND_RAV1E:
        return rav1e_reference_encode_command(vector, vector_path, output, encoder, args)
    if args.reference_backend == REFERENCE_BACKEND_FFMPEG_LIBAOM:
        return ffmpeg_libaom_reference_encode_command(vector, vector_path, output, encoder, args)
    if args.reference_backend != REFERENCE_BACKEND_NATIVE:
        raise SystemExit(f"unsupported compression reference backend: {args.reference_backend}")
    if args.codec == "av2":
        return av2_reference_encode_command(vector, vector_path, output, encoder, args)
    if args.codec == "vvc":
        return vvc_reference_encode_command(vector, vector_path, output, encoder, args)
    raise SystemExit(
        "reference compression comparison currently supports codecs av2 and vvc; "
        f"got {args.codec}"
    )


def rav1e_reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    bit_depth, _chroma = av1_pixel_format(vector.fmt)
    if bit_depth not in {8, 10, 12}:
        raise SystemExit(f"unsupported rav1e reference encode pixel format: {vector.fmt}")
    if effective_frameforge_lossless(vector, args):
        raise SystemExit(
            "rav1e does not implement lossless encoding; use "
            "COMPRESSION_REFERENCE_BACKEND=reference for lossless manifests"
        )

    y4m_input = rav1e_y4m_path(output)
    ensure_y4m_input(vector, vector_path, y4m_input)
    command = [encoder, str(y4m_input), "-o", str(output)]
    if args.reference_preset == "fast":
        command.extend(["--speed", "10"])
    if args.reference_threads is not None:
        command.extend(["--threads", str(args.reference_threads)])
    if args.reference_args:
        command.extend(shlex.split(args.reference_args))
    return command


def rav1e_y4m_path(output: Path) -> Path:
    return output.with_name(f"{output.stem}_input.y4m")


def ffmpeg_libaom_reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    bit_depth, _chroma = av1_pixel_format(vector.fmt)
    if bit_depth not in {8, 10, 12}:
        raise SystemExit(f"unsupported ffmpeg/libaom reference encode pixel format: {vector.fmt}")

    y4m_input = ffmpeg_libaom_input_path(vector, vector_path, output)
    command = [
        encoder,
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        str(y4m_input),
        "-frames:v",
        str(vector.frames),
        "-c:v",
        "libaom-av1",
    ]
    command.extend(ffmpeg_libaom_preset_args(vector, args))
    if args.reference_args:
        command.extend(shlex.split(args.reference_args))
    command.append(str(output))
    return command


def ffmpeg_libaom_input_path(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
) -> Path:
    if vector_path.suffix.lower() == ".y4m":
        return vector_path
    y4m_input = output.with_name(f"{output.stem}_input.y4m")
    ensure_y4m_input(vector, vector_path, y4m_input)
    return y4m_input


def ffmpeg_libaom_preset_args(
    vector: generate_test_vectors.TestVector,
    args: argparse.Namespace,
) -> list[str]:
    if args.reference_preset == "lossless":
        return [
            "-cpu-used",
            "8",
            "-threads",
            str(reference_thread_count(args.reference_threads)),
            "-row-mt",
            "1",
            "-tiles",
            ffmpeg_libaom_tiles(vector, args),
            "-lag-in-frames",
            "0",
            "-auto-alt-ref",
            "0",
            "-b:v",
            "0",
            "-crf",
            "0",
            "-aom-params",
            "lossless=1",
        ]

    if args.reference_preset not in {"default", "fast", "realtime-screen"}:
        raise SystemExit(
            "ffmpeg/libaom baseline supports presets default, fast, "
            "realtime-screen, and lossless"
        )

    return [
        "-usage",
        "realtime",
        "-cpu-used",
        "8",
        "-threads",
        str(reference_thread_count(args.reference_threads)),
        "-row-mt",
        "1",
        "-tiles",
        ffmpeg_libaom_tiles(vector, args),
        "-lag-in-frames",
        "0",
        "-auto-alt-ref",
        "0",
        "-b:v",
        "4M",
        "-maxrate",
        "4M",
        "-bufsize",
        "4M",
        "-g",
        "300",
        "-aq-mode",
        "cyclic",
        "-enable-cdef",
        "1",
        "-enable-restoration",
        "0",
        "-enable-global-motion",
        "0",
        "-enable-obmc",
        "0",
        "-enable-palette",
        "1",
        "-enable-cfl-intra",
        "0",
        "-enable-smooth-intra",
        "0",
        "-enable-angle-delta",
        "0",
        "-enable-filter-intra",
        "0",
        "-use-intra-default-tx-only",
        "1",
        "-enable-ref-frame-mvs",
        "0",
        "-enable-dual-filter",
        "0",
        "-enable-interintra-comp",
        "0",
        "-enable-masked-comp",
        "0",
        "-enable-paeth-intra",
        "0",
        "-enable-rect-partitions",
        "0",
        "-enable-tx64",
        "0",
        "-aom-params",
        "tune-content=screen",
    ]


def ffmpeg_libaom_tiles(
    vector: generate_test_vectors.TestVector,
    args: argparse.Namespace,
) -> str:
    if args.avm_tile_columns is not None:
        return f"{1 << args.avm_tile_columns}x{1 << (args.avm_tile_rows or 0)}"
    if vector.width >= 1920:
        return "8x1"
    if vector.width >= 1280:
        return "4x1"
    return "2x1"


def ensure_y4m_input(
    vector: generate_test_vectors.TestVector,
    raw_path: Path,
    y4m_path: Path,
) -> None:
    if (
        y4m_path.exists()
        and y4m_path.stat().st_size > 0
        and y4m_path.stat().st_mtime_ns >= raw_path.stat().st_mtime_ns
    ):
        return

    frame_len = generate_test_vectors.raw_frame_len(vector)
    y4m_path.parent.mkdir(parents=True, exist_ok=True)
    fps = fps_fraction(vector)
    header = (
        f"YUV4MPEG2 W{vector.width} H{vector.height} "
        f"F{fps.numerator}:{fps.denominator} Ip A0:0 C{y4m_chroma_tag(vector.fmt)}\n"
    ).encode("ascii")
    with raw_path.open("rb") as source, y4m_path.open("wb") as output:
        output.write(header)
        for frame_index in range(vector.frames):
            frame = source.read(frame_len)
            if len(frame) != frame_len:
                raise SystemExit(
                    f"{raw_path} is too short for rav1e Y4M wrapper: "
                    f"missing frame {frame_index + 1}"
                )
            output.write(b"FRAME\n")
            output.write(frame)


def y4m_chroma_tag(fmt: str) -> str:
    bit_depth, chroma = av1_pixel_format(fmt)
    if bit_depth == 8 and chroma == "420":
        return "420jpeg"
    if bit_depth == 8:
        return chroma
    return f"{chroma}p{bit_depth}"


def av1_pixel_format(fmt: str) -> tuple[int, str]:
    bit_depth = generate_test_vectors.yuv420_bit_depth(fmt)
    if bit_depth is not None:
        return bit_depth, "420"
    bit_depth = generate_test_vectors.yuv422_bit_depth(fmt)
    if bit_depth is not None:
        return bit_depth, "422"
    bit_depth = generate_test_vectors.yuv444_bit_depth(fmt)
    if bit_depth is not None:
        return bit_depth, "444"
    raise SystemExit(f"unsupported rav1e reference encode pixel format: {fmt}")


def av2_reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    bit_depth = generate_test_vectors.yuv420_bit_depth(vector.fmt)
    chroma_flag = "--i420"
    profile_args: list[str] = []
    if bit_depth is None:
        bit_depth = generate_test_vectors.yuv422_bit_depth(vector.fmt)
        chroma_flag = "--i422"
        profile_args = ["--profile=3"]
    if bit_depth is None:
        bit_depth = generate_test_vectors.yuv444_bit_depth(vector.fmt)
        chroma_flag = "--i444"
        profile_args = ["--profile=4"]
    if bit_depth is None or bit_depth not in {8, 10}:
        raise SystemExit(f"unsupported AV2 reference encode pixel format: {vector.fmt}")

    command = [
        encoder,
        "--codec=av2",
        "--obu",
        f"--limit={vector.frames}",
        f"--width={vector.width}",
        f"--height={vector.height}",
        f"--fps={reference_fps_ratio(vector)}",
        f"--input-bit-depth={bit_depth}",
        f"--bit-depth={bit_depth}",
        "--cpu-used=9",
        "--psnr=0",
        "--quiet",
        "--disable-warning-prompt",
    ]
    command.extend(avm_reference_preset_args(vector, args))
    if effective_frameforge_lossless(vector, args):
        command.append("--lossless=1")
    command.append(chroma_flag)
    command.extend(profile_args)
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
        if not vvc_bit_depth_is_supported(bit_depth):
            raise SystemExit(
                f"unsupported VVC reference encode pixel format for native FrameForge comparison: {vector.fmt}"
            )
    else:
        bit_depth = generate_test_vectors.yuv422_bit_depth(vector.fmt)
        if bit_depth is not None:
            chroma_format = "422"
            if not vvc_bit_depth_is_supported(bit_depth):
                raise SystemExit(
                    f"unsupported VVC reference encode pixel format for native FrameForge comparison: {vector.fmt}"
                )

    if bit_depth is None:
        bit_depth = generate_test_vectors.yuv444_bit_depth(vector.fmt)
        if bit_depth is None:
            raise SystemExit(f"unsupported VVC reference encode pixel format: {vector.fmt}")
        chroma_format = "444"
        if not vvc_bit_depth_is_supported(bit_depth):
            raise SystemExit(
                f"unsupported VVC reference encode pixel format for native FrameForge comparison: {vector.fmt}"
            )

    command = [
        encoder,
        "-c",
        str(VTM_CFG_DIR / "encoder_intra_vtm.cfg"),
    ]
    if effective_frameforge_lossless(vector, args):
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


def vvc_bit_depth_is_supported(bit_depth: int) -> bool:
    return VVC_MIN_BIT_DEPTH <= bit_depth <= VVC_MAX_BIT_DEPTH


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


def parse_qp(value: str) -> int:
    try:
        qp = int(value, 10)
    except ValueError as err:
        raise argparse.ArgumentTypeError(
            f"QP expects an integer from 1 through 255, got '{value}'"
        ) from err
    if not (1 <= qp <= 255):
        raise argparse.ArgumentTypeError(
            f"QP expects an integer from 1 through 255, got '{value}'"
        )
    return qp


def result_mode(result: ComparisonResult) -> str:
    if result.lossless:
        return "lossless"
    if result.qp is not None:
        return f"qp={result.qp}"
    return "default"


def run_logged(command: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def run_logged_timed(command: list[str]) -> tuple[subprocess.CompletedProcess[str], float]:
    start = time.perf_counter()
    process = run_logged(command)
    return process, time.perf_counter() - start


def format_fps(value: float) -> str:
    if value == float("inf"):
        return "inf"
    if value >= 100:
        return f"{value:.1f}"
    return f"{value:.2f}"


def require_non_empty(output: Path, label: str, vector_name: str, log: Path) -> None:
    if not output.exists():
        raise SystemExit(f"{label} encode produced no output for {vector_name}; see {relpath(log)}")
    if output.stat().st_size == 0:
        raise SystemExit(f"{label} encode produced empty output for {vector_name}; see {relpath(log)}")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def codec_extension(codec: str) -> str:
    return {"av2": "obu", "vvc": "vvc"}.get(codec, codec)


def reference_extension_for_args(args: argparse.Namespace) -> str:
    if args.reference_backend in {REFERENCE_BACKEND_RAV1E, REFERENCE_BACKEND_FFMPEG_LIBAOM}:
        return "ivf"
    return codec_extension(args.codec)


def relpath(path: Path) -> Path:
    try:
        return path.resolve().relative_to(REPO_ROOT)
    except ValueError:
        return path


if __name__ == "__main__":
    raise SystemExit(main())
