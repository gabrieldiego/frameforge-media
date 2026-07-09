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
DEFAULT_LOG_DIR = REPO_ROOT / "verification" / "generated" / "validation_logs"


@dataclass(frozen=True)
class ValidationResult:
    vector_name: str
    output: Path
    log: Path
    status: str
    reason: str
    bytes_written: int | None
    sha256: str


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("set", nargs="?", default="smoke", help="test vector set name")
    parser.add_argument("--codec", required=True, help="codec name accepted by ff encode")
    parser.add_argument("--ff", type=Path, default=REPO_ROOT / "ff")
    parser.add_argument("--set-dir", type=Path, default=generate_test_vectors.DEFAULT_SET_DIR)
    parser.add_argument("--vector-dir", type=Path, default=DEFAULT_VECTOR_DIR)
    parser.add_argument("--encoded-dir", type=Path, default=DEFAULT_ENCODED_DIR)
    parser.add_argument("--log-dir", type=Path, default=DEFAULT_LOG_DIR)
    parser.add_argument("--limit", type=int, default=0, help="run only the first N vectors")
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

    if args.source_filters:
        vector_set = load_vector_set(args.set, args.set_dir)
        cases = vector_set.vectors
    else:
        cases = generate_test_vectors.generate_vectors(args.set, args.vector_dir, args.set_dir)
    if args.limit:
        cases = cases[: args.limit]

    results: list[ValidationResult] = []
    for index, vector in enumerate(cases, start=1):
        name = vector.filename if args.source_filters else vector.name
        print(f"[{index:03d}/{len(cases):03d}] {name}", flush=True)
        result = run_source_case(vector, args) if args.source_filters else run_file_case(vector, args)
        results.append(result)
        size = "n/a" if result.bytes_written is None else str(result.bytes_written)
        print(f"  {result.status}: {result.reason} ({size} byte(s))", flush=True)
        if result.status != "PASS" and args.stop_on_fail:
            break

    print()
    print(f"FrameForge media validation set: {args.set} ({args.codec})")
    print("| # | vector | result | bytes | sha256 | reason | log |")
    print("|---:|---|---|---:|---|---|---|")
    for index, result in enumerate(results, start=1):
        print(
            f"| {index} | {result.vector_name} | {result.status} | "
            f"{result.bytes_written if result.bytes_written is not None else 'n/a'} | "
            f"{result.sha256} | {markdown_escape(result.reason)} | {relpath(result.log)} |"
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


def run_file_case(vector: Path, args: argparse.Namespace) -> ValidationResult:
    output, log = case_paths(vector.stem, args)
    command = [
        str(args.ff),
        "encode",
        str(vector),
        "--encode",
        f"{args.codec}:{output}",
    ]
    return run_command(vector.name, output, log, command, args.setting)


def run_source_case(vector: generate_test_vectors.TestVector, args: argparse.Namespace) -> ValidationResult:
    stem = Path(vector.filename).stem
    output, log = case_paths(stem, args)
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
        command.extend(["--fps", str(vector.fps)])
    command.extend(["--encode", f"{args.codec}:{output}"])
    return run_command(vector.filename, output, log, command, args.setting)


def case_paths(stem: str, args: argparse.Namespace) -> tuple[Path, Path]:
    output_dir = args.encoded_dir / args.codec / args.set
    log_dir = args.log_dir / args.codec
    output_dir.mkdir(parents=True, exist_ok=True)
    log_dir.mkdir(parents=True, exist_ok=True)

    output = output_dir / f"{stem}.{codec_extension(args.codec)}"
    log = log_dir / f"{args.set}_{stem}.log"
    return output, log


def run_command(
    vector_name: str,
    output: Path,
    log: Path,
    command: list[str],
    settings: list[str],
) -> ValidationResult:
    if output.exists():
        output.unlink()

    for setting in settings:
        command.extend(["--set", setting])

    process = subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    log.write_text(f"$ {shlex.join(command)}\n\n{process.stdout}")

    if process.returncode != 0:
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            log=log,
            status="FAIL",
            reason=extract_failure_reason(process.stdout),
            bytes_written=output.stat().st_size if output.exists() else None,
            sha256=sha256_file(output) if output.exists() else "n/a",
        )
    if not output.exists():
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            log=log,
            status="FAIL",
            reason="encoder returned success but did not create output",
            bytes_written=None,
            sha256="n/a",
        )
    size = output.stat().st_size
    if size == 0:
        return ValidationResult(
            vector_name=vector_name,
            output=output,
            log=log,
            status="FAIL",
            reason="encoder returned success but output is empty",
            bytes_written=size,
            sha256=sha256_file(output),
        )
    return ValidationResult(
        vector_name=vector_name,
        output=output,
        log=log,
        status="PASS",
        reason="encoded output was produced",
        bytes_written=size,
        sha256=sha256_file(output),
    )


def codec_extension(codec: str) -> str:
    return {"av2": "obu", "vvc": "vvc"}.get(codec, codec)


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
