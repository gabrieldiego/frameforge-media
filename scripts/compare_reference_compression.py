#!/usr/bin/env python3
"""Compare FrameForge bitstream sizes against a reference encoder."""

from __future__ import annotations

import argparse
import shlex
import subprocess
import sys
from dataclasses import dataclass
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
    args = parser.parse_args()

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
            "  mode={mode} FrameForge={ff} byte(s), reference={ref} byte(s), ratio={ratio:.3f}x".format(
                mode="lossless" if result.lossless else "default",
                ff=result.frameforge_bytes,
                ref=result.reference_bytes,
                ratio=result.ratio,
            ),
            flush=True,
        )

    print()
    print(f"FrameForge media compression comparison: {args.set} ({args.codec})")
    print("| # | vector | mode | FrameForge bytes | reference bytes | FF/reference | delta | log |")
    print("|---:|---|---|---:|---:|---:|---:|---|")
    total_ff = 0
    total_ref = 0
    for index, result in enumerate(results, start=1):
        total_ff += result.frameforge_bytes
        total_ref += result.reference_bytes
        delta = result.frameforge_bytes - result.reference_bytes
        mode = "lossless" if result.lossless else "default"
        print(
            f"| {index} | {result.vector_name} | {mode} | {result.frameforge_bytes} | "
            f"{result.reference_bytes} | {result.ratio:.3f}x | {delta:+d} | "
            f"{relpath(result.log)} |"
        )
    if results:
        total_ratio = total_ff / total_ref if total_ref else float("inf")
        print(
            f"| total | {len(results)} vector(s) | mixed | {total_ff} | {total_ref} | "
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
    if reference_output.exists():
        reference_output.unlink()
    reference_recon = reference_recon_path(reference_output)
    if reference_recon.exists():
        reference_recon.unlink()

    frameforge_cmd = [
        str(args.ff),
        "encode",
        str(vector_path),
        "--encode",
        f"{args.codec}:{frameforge_output}",
    ]
    if vector.lossless:
        frameforge_cmd.extend(["--set", "lossless"])
    reference_cmd = reference_encode_command(vector, vector_path, reference_output, reference_encoder, args)

    frameforge_result = run_logged(frameforge_cmd)
    reference_result = run_logged(reference_cmd)
    log.write_text(
        f"$ {shlex.join(frameforge_cmd)}\n\n{frameforge_result.stdout}\n\n"
        f"$ {shlex.join(reference_cmd)}\n\n{reference_result.stdout}"
    )

    if frameforge_result.returncode != 0:
        raise SystemExit(f"FrameForge encode failed for {vector_path.name}; see {relpath(log)}")
    if reference_result.returncode != 0:
        raise SystemExit(f"reference encode failed for {vector_path.name}; see {relpath(log)}")
    require_non_empty(frameforge_output, "FrameForge", vector_path, log)
    require_non_empty(reference_output, "reference", vector_path, log)

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
        f"--fps={vector.fps or 30}/1",
        "--input-bit-depth=8",
        "--bit-depth=8",
        "--cpu-used=9",
        "--psnr=0",
        "--quiet",
        "--disable-warning-prompt",
    ]
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


def vvc_reference_encode_command(
    vector: generate_test_vectors.TestVector,
    vector_path: Path,
    output: Path,
    encoder: str,
    args: argparse.Namespace,
) -> list[str]:
    if vector.fmt == "yuv420p8":
        chroma_format = "420"
    elif vector.fmt == "yuv444p8":
        chroma_format = "444"
    else:
        raise SystemExit(f"unsupported VVC reference encode pixel format: {vector.fmt}")

    command = [
        encoder,
        "-c",
        str(VTM_CFG_DIR / "encoder_intra_vtm.cfg"),
    ]
    if vector.lossless:
        command.extend(["-c", str(VTM_CFG_DIR / "lossless" / "lossless.cfg")])
        if vector.fmt == "yuv444p8":
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
            str(vector.fps or 30),
            "-f",
            str(vector.frames),
            "--InputBitDepth=8",
            "--InternalBitDepth=8",
            f"--InputChromaFormat={chroma_format}",
            f"--ChromaFormatIDC={chroma_format}",
            "--TemporalSubsampleRatio=1",
            "--Verbosity=0",
        ]
    )
    if args.reference_args:
        command.extend(shlex.split(args.reference_args))
    return command


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


def codec_extension(codec: str) -> str:
    return {"av2": "obu", "vvc": "vvc"}.get(codec, codec)


def relpath(path: Path) -> Path:
    try:
        return path.resolve().relative_to(REPO_ROOT)
    except ValueError:
        return path


if __name__ == "__main__":
    raise SystemExit(main())
