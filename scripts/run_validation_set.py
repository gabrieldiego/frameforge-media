#!/usr/bin/env python3
"""Run a generated vector set through the FrameForge CLI encoder."""

from __future__ import annotations

import argparse
import hashlib
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

import generate_test_vectors


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_VECTOR_DIR = REPO_ROOT / "verification" / "generated" / "test_vectors"
DEFAULT_ENCODED_DIR = REPO_ROOT / "verification" / "generated" / "encoded"
DEFAULT_RECON_DIR = REPO_ROOT / "verification" / "generated" / "recon"
DEFAULT_LOG_DIR = REPO_ROOT / "verification" / "generated" / "validation_logs"
REFERENCE_TOOLS = REPO_ROOT / "scripts" / "reference_tools.py"


@dataclass(frozen=True)
class ValidationResult:
    vector_name: str
    output: Path
    recon: Path
    reference_recon: Path | None
    log: Path
    status: str
    reason: str
    bytes_written: int | None
    sha256: str
    recon_sha256: str
    reference_sha256: str


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("set", nargs="?", default="smoke", help="test vector set name")
    parser.add_argument("--codec", required=True, help="codec name accepted by ff encode")
    parser.add_argument("--ff", type=Path, default=REPO_ROOT / "ff")
    parser.add_argument("--set-dir", type=Path, default=generate_test_vectors.DEFAULT_SET_DIR)
    parser.add_argument("--vector-dir", type=Path, default=DEFAULT_VECTOR_DIR)
    parser.add_argument("--encoded-dir", type=Path, default=DEFAULT_ENCODED_DIR)
    parser.add_argument("--recon-dir", type=Path, default=DEFAULT_RECON_DIR)
    parser.add_argument("--log-dir", type=Path, default=DEFAULT_LOG_DIR)
    parser.add_argument("--limit", type=int, default=0, help="run only the first N vectors")
    parser.add_argument(
        "--reference-mode",
        choices=("auto", "required", "off"),
        default="auto",
        help="decode and compare with reference tools when available",
    )
    parser.add_argument("--setting", action="append", default=[], help="extra --set key[=value]")
    parser.add_argument(
        "--source-filters",
        action="store_true",
        help="run manifest patterns directly through --filter pattern=... without input files",
    )
    parser.add_argument("--stop-on-fail", action="store_true")
    args = parser.parse_args()

    if not args.ff.exists():
        print(f"error: missing CLI binary: {args.ff}; run 'make build' first", file=sys.stderr)
        return 2
    args.ff = args.ff.resolve()

    vector_set = load_vector_set(args.set, args.set_dir)
    if args.source_filters:
        cases = [(vector, None) for vector in vector_set.vectors]
    else:
        paths = generate_test_vectors.generate_vectors(args.set, args.vector_dir, args.set_dir)
        vectors_by_filename = {vector.filename: vector for vector in vector_set.vectors}
        cases = [(vectors_by_filename[path.name], path) for path in paths]
    skipped_by_codec = [
        vector for vector, _ in cases if not vector_enabled_for_codec(vector, args.codec)
    ]
    cases = [
        (vector, path) for vector, path in cases if vector_enabled_for_codec(vector, args.codec)
    ]
    if args.limit:
        cases = cases[: args.limit]
    if skipped_by_codec:
        print(
            f"Skipped {len(skipped_by_codec)} vector(s) not enabled for codec {args.codec}",
            flush=True,
        )

    results: list[ValidationResult] = []
    for index, (vector, vector_path) in enumerate(cases, start=1):
        name = vector.filename if args.source_filters else vector.name
        print(f"[{index:03d}/{len(cases):03d}] {name}", flush=True)
        if args.source_filters:
            result = run_source_case(vector, args)
        else:
            assert vector_path is not None
            result = run_file_case(vector, vector_path, args)
        results.append(result)
        size = "n/a" if result.bytes_written is None else str(result.bytes_written)
        print(f"  {result.status}: {result.reason} ({size} byte(s))", flush=True)
        if result.status != "PASS" and args.stop_on_fail:
            break

    print()
    print(f"FrameForge media validation set: {args.set} ({args.codec})")
    print("| # | vector | result | bytes | sha256 | recon_sha256 | reference_sha256 | reason | log |")
    print("|---:|---|---|---:|---|---|---|---|---|")
    for index, result in enumerate(results, start=1):
        print(
            f"| {index} | {result.vector_name} | {result.status} | "
            f"{result.bytes_written if result.bytes_written is not None else 'n/a'} | "
            f"{result.sha256} | {result.recon_sha256} | {result.reference_sha256} | "
            f"{markdown_escape(result.reason)} | {relpath(result.log)} |"
        )

    failed = [result for result in results if result.status != "PASS"]
    if failed:
        print(f"\nFAIL: {len(failed)} of {len(results)} validation case(s) failed", file=sys.stderr)
        return 1
    print(f"\nOK: {len(results)} validation case(s) passed")
    return 0


def load_vector_set(set_name: str, set_dir: Path) -> generate_test_vectors.TestVectorSet:
    sets = generate_test_vectors.vector_sets(set_dir)
    if set_name not in sets:
        choices = ", ".join(sorted(sets)) or "<none>"
        raise ValueError(f"unknown test vector set '{set_name}'; choices: {choices}")
    return sets[set_name]


def vector_enabled_for_codec(vector: generate_test_vectors.TestVector, codec: str) -> bool:
    return vector.codecs is None or codec.lower() in vector.codecs


def run_file_case(
    vector: generate_test_vectors.TestVector, vector_path: Path, args: argparse.Namespace
) -> ValidationResult:
    output, recon, reference_recon, log = case_paths(vector_path.stem, args)
    command = [
        str(args.ff),
        "encode",
        str(vector_path),
        "--video",
        f"{vector.width}x{vector.height}:{vector.fmt}",
        "--frames",
        str(vector.frames),
    ]
    if vector.fps is not None:
        command.extend(["--fps", vector.fps])
    command.extend(
        [
            "--encode",
            f"{args.codec}:{output}",
            "--recon",
            str(recon),
        ]
    )
    if vector.lossless:
        command.extend(["--set", "lossless"])
    return run_command(
        vector_path.name,
        output,
        recon,
        reference_recon,
        log,
        command,
        args,
        lossless_source=vector_path if vector.lossless else None,
    )


def run_source_case(vector: generate_test_vectors.TestVector, args: argparse.Namespace) -> ValidationResult:
    stem = Path(vector.filename).stem
    output, recon, reference_recon, log = case_paths(stem, args)
    command = [
        str(args.ff),
        "encode",
        "--filter",
        f"pattern={vector.pattern}",
        "--video",
        f"{vector.width}x{vector.height}:{vector.fmt}",
        "--frames",
        str(vector.frames),
    ]
    if vector.fps is not None:
        command.extend(["--fps", vector.fps])
    command.extend(["--encode", f"{args.codec}:{output}", "--recon", str(recon)])
    if vector.lossless:
        command.extend(["--set", "lossless"])
    return run_command(
        vector.filename,
        output,
        recon,
        reference_recon,
        log,
        command,
        args,
        lossless_source=vector if vector.lossless else None,
    )


def case_paths(stem: str, args: argparse.Namespace) -> tuple[Path, Path, Path, Path]:
    output_dir = args.encoded_dir / args.codec / args.set
    recon_dir = args.recon_dir / args.codec / args.set
    log_dir = args.log_dir / args.codec
    output_dir.mkdir(parents=True, exist_ok=True)
    recon_dir.mkdir(parents=True, exist_ok=True)
    log_dir.mkdir(parents=True, exist_ok=True)

    output = output_dir / f"{stem}.{codec_extension(args.codec)}"
    recon = recon_dir / f"{stem}_internal.yuv"
    reference_recon = recon_dir / f"{stem}_reference.yuv"
    log = log_dir / f"{args.set}_{stem}.log"
    return output, recon, reference_recon, log


def run_command(
    vector_name: str,
    output: Path,
    recon: Path,
    reference_recon: Path,
    log: Path,
    command: list[str],
    args: argparse.Namespace,
    lossless_source: Path | generate_test_vectors.TestVector | None = None,
) -> ValidationResult:
    if output.exists():
        output.unlink()
    if recon.exists():
        recon.unlink()
    if reference_recon.exists():
        reference_recon.unlink()

    for setting in args.setting:
        command.extend(["--set", setting])

    process = subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    log.write_text(f"$ {shlex.join(command)}\n\n{process.stdout}")
    for line in process.stdout.splitlines():
        if line.startswith("frame:"):
            print(f"  {line}", flush=True)

    if process.returncode != 0:
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            recon=recon,
            reference_recon=None,
            log=log,
            status="FAIL",
            reason=extract_failure_reason(process.stdout),
            bytes_written=output.stat().st_size if output.exists() else None,
            sha256=sha256_file(output) if output.exists() else "n/a",
            recon_sha256=sha256_file(recon) if recon.exists() else "n/a",
            reference_sha256="n/a",
        )
    if not output.exists():
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            recon=recon,
            reference_recon=None,
            log=log,
            status="FAIL",
            reason="encoder returned success but did not create output",
            bytes_written=None,
            sha256="n/a",
            recon_sha256=sha256_file(recon) if recon.exists() else "n/a",
            reference_sha256="n/a",
        )
    size = output.stat().st_size
    if size == 0:
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            recon=recon,
            reference_recon=None,
            log=log,
            status="FAIL",
            reason="encoder returned success but output is empty",
            bytes_written=size,
            sha256=sha256_file(output),
            recon_sha256=sha256_file(recon) if recon.exists() else "n/a",
            reference_sha256="n/a",
        )
    if not recon.exists():
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            recon=recon,
            reference_recon=None,
            log=log,
            status="FAIL",
            reason="encoder returned success but did not create internal reconstruction",
            bytes_written=size,
            sha256=sha256_file(output),
            recon_sha256="n/a",
            reference_sha256="n/a",
        )
    recon_sha = sha256_file(recon)
    lossless_status = validate_lossless_source(lossless_source, recon)
    if lossless_status is not None and lossless_status[0] == "FAIL":
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            recon=recon,
            reference_recon=None,
            log=log,
            status="FAIL",
            reason=lossless_status[1],
            bytes_written=size,
            sha256=sha256_file(output),
            recon_sha256=recon_sha,
            reference_sha256="n/a",
        )
    reference_status = validate_reference_decode(args, output, recon, reference_recon, log)
    if reference_status is not None and reference_status[0] == "FAIL":
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            recon=recon,
            reference_recon=reference_recon if reference_recon.exists() else None,
            log=log,
            status="FAIL",
            reason=reference_status[1],
            bytes_written=size,
            sha256=sha256_file(output),
            recon_sha256=recon_sha,
            reference_sha256=sha256_file(reference_recon) if reference_recon.exists() else "n/a",
        )
    reference_sha = sha256_file(reference_recon) if reference_recon.exists() else "n/a"
    reason = "encoded output and internal reconstruction were produced"
    if lossless_status is not None:
        reason = lossless_status[1]
    if reference_status is not None:
        reason = (
            f"{reason}; {reference_status[1]}"
            if lossless_status is not None
            else reference_status[1]
        )
    return ValidationResult(
        vector_name=vector_name,
        output=output,
        recon=recon,
        reference_recon=reference_recon if reference_recon.exists() else None,
        log=log,
        status="PASS",
        reason=reason,
        bytes_written=size,
        sha256=sha256_file(output),
        recon_sha256=recon_sha,
        reference_sha256=reference_sha,
    )


def validate_lossless_source(
    source: Path | generate_test_vectors.TestVector | None, recon: Path
) -> tuple[str, str] | None:
    if source is None:
        return None
    if isinstance(source, Path):
        source_bytes = source.read_bytes()
    else:
        source_bytes = generate_test_vectors.generate_yuv(source, {})
    recon_bytes = recon.read_bytes()
    if source_bytes != recon_bytes:
        if len(source_bytes) != len(recon_bytes):
            return (
                "FAIL",
                f"lossless reconstruction length differs from source ({len(recon_bytes)} != {len(source_bytes)})",
            )
        return ("FAIL", "lossless reconstruction differs from source")
    return ("PASS", "lossless reconstruction matches source")


def codec_extension(codec: str) -> str:
    return {"av2": "obu", "vvc": "vvc"}.get(codec, codec)


def validate_reference_decode(
    args: argparse.Namespace, bitstream: Path, internal_recon: Path, reference_recon: Path, log: Path
) -> tuple[str, str] | None:
    if args.reference_mode == "off":
        return None

    command = [
        sys.executable,
        str(REFERENCE_TOOLS),
        "decode",
        "--codec",
        args.codec,
        "--bitstream",
        str(bitstream),
        "--output",
        str(reference_recon),
        "--no-build",
    ]
    process = subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    with log.open("a") as file:
        file.write("\n\n")
        file.write(f"$ {shlex.join(command)}\n\n{process.stdout}")

    if process.returncode != 0:
        reason = extract_failure_reason(process.stdout)
        if process.returncode == 2 and args.reference_mode == "auto":
            return ("SKIP", f"reference decode skipped: {reason}")
        return ("FAIL", f"reference decode failed: {reason}")

    if not reference_recon.exists():
        return ("FAIL", "reference decoder returned success but did not create reconstruction")
    if reference_recon.stat().st_size == 0:
        return ("FAIL", "reference decoder returned success but reconstruction is empty")

    internal_sha = sha256_file(internal_recon)
    reference_sha = sha256_file(reference_recon)
    if internal_sha != reference_sha:
        return (
            "FAIL",
            "reference reconstruction checksum differs from internal reconstruction",
        )
    return ("PASS", "reference reconstruction matches internal reconstruction")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def extract_failure_reason(output: str) -> str:
    markers = ("error:", "Error:", "panic", "FAIL:")
    for line in output.splitlines():
        stripped = line.strip()
        if any(marker in stripped for marker in markers):
            return stripped
    lines = [line.strip() for line in output.splitlines() if line.strip()]
    return lines[-1] if lines else "encoder command failed"


def markdown_escape(value: str) -> str:
    return value.replace("|", "\\|")


def relpath(path: Path) -> Path:
    try:
        return path.resolve().relative_to(REPO_ROOT)
    except ValueError:
        return path


if __name__ == "__main__":
    raise SystemExit(main())
