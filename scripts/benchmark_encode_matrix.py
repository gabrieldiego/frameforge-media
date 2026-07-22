#!/usr/bin/env python3
"""Benchmark FrameForge encode speed over codec/mode/vector matrices."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import shlex
import subprocess
import sys
import time
from dataclasses import replace
from pathlib import Path
from typing import Any

import generate_test_vectors


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SET = "local-aomctc-b2-scc-1080p-lossless-50f"
DEFAULT_VECTOR_DIR = REPO_ROOT / "verification" / "generated" / "test_vectors"
DEFAULT_OUT_DIR = REPO_ROOT / "verification" / "generated" / "encode_matrix"
PSNR_ALL_RE = re.compile(r"\bpsnr_all=(inf|[-+]?[0-9]*\.?[0-9]+)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("set", nargs="?", default=DEFAULT_SET, help="test vector set name")
    parser.add_argument("--ff", type=Path, default=REPO_ROOT / "ff")
    parser.add_argument("--set-dir", type=Path, default=generate_test_vectors.DEFAULT_SET_DIR)
    parser.add_argument("--vector-dir", type=Path, default=DEFAULT_VECTOR_DIR)
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR)
    parser.add_argument("--run-name", default="", help="label for output files/directories")
    parser.add_argument("--codec", action="append", choices=("av2", "vvc"), default=[])
    parser.add_argument("--mode", action="append", choices=("lossless", "lossy"), default=[])
    parser.add_argument("--limit", type=int, default=0, help="run only the first N enabled rows")
    parser.add_argument(
        "--frames",
        type=parse_positive_int,
        default=0,
        help="override each vector's frame count, e.g. --frames 1 for I-frame checks",
    )
    parser.add_argument("--av2-lossy-qp", type=parse_qp, default=24)
    parser.add_argument("--av2-predictive", dest="av2_predictive", action="store_true", default=True)
    parser.add_argument("--no-av2-predictive", dest="av2_predictive", action="store_false")
    parser.add_argument(
        "--direct-source-files",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="feed source_file rows directly instead of materializing raw clips",
    )
    parser.add_argument(
        "--baseline-json",
        type=Path,
        help="optional previous JSON report to include byte/fps deltas",
    )
    args = parser.parse_args()

    if not args.ff.exists():
        print(f"error: missing CLI binary: {args.ff}; run 'make build' first", file=sys.stderr)
        return 2
    args.ff = args.ff.resolve()
    args.frames = args.frames or None
    codecs = args.codec or ["av2", "vvc"]
    modes = args.mode or ["lossless", "lossy"]
    run_name = args.run_name or time.strftime("%Y%m%d-%H%M%S")
    run_dir = (args.out_dir / run_name).resolve()
    log_dir = run_dir / "logs"
    run_dir.mkdir(parents=True, exist_ok=True)
    log_dir.mkdir(parents=True, exist_ok=True)

    vector_set = load_vector_set(args.set, args.set_dir)
    baseline = load_baseline(args.baseline_json)
    results: list[dict[str, Any]] = []
    skipped = 0
    cases = [
        (codec, mode, vector)
        for codec in codecs
        for mode in modes
        for vector in vector_set.vectors
        if vector_enabled_for_codec(vector, codec) and mode_supported(vector, codec, mode)
    ]
    if args.limit:
        cases = cases[: args.limit]
    total_cases = len(cases)

    for codec, mode, vector in cases:
        case_index = len(results) + 1
        print(
            f"[{case_index:03d}/{total_cases:03d}] {codec} {mode} {vector.name}",
            flush=True,
        )
        result = run_case(vector_set, vector, codec, mode, run_dir, log_dir, args)
        apply_baseline_delta(result, baseline)
        results.append(result)
        delta = delta_label(result)
        print(
            "  bytes={bytes} fps={fps:.2f} psnr={psnr}{delta}".format(
                bytes=result["bytes"],
                fps=result["fps"],
                psnr=format_optional_float(result.get("psnr_all_mean")),
                delta=delta,
            ),
            flush=True,
        )
    skipped = count_skipped(vector_set, codecs, modes, args.limit)

    report = {
        "set": args.set,
        "run_name": run_name,
        "ff": str(args.ff),
        "av2_predictive": args.av2_predictive,
        "av2_lossy_qp": args.av2_lossy_qp,
        "results": results,
    }
    json_path = args.out_dir / f"{run_name}.json"
    md_path = args.out_dir / f"{run_name}.md"
    json_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    md_path.write_text(markdown_report(report, skipped) + "\n")
    print()
    print(f"wrote {relpath(json_path)}")
    print(f"wrote {relpath(md_path)}")
    if skipped:
        print(f"skipped {skipped} unsupported codec/vector/mode combination(s)")
    return 0


def load_vector_set(set_name: str, set_dir: Path) -> generate_test_vectors.TestVectorSet:
    sets = generate_test_vectors.vector_sets(set_dir)
    if set_name not in sets:
        choices = ", ".join(sorted(sets)) or "<none>"
        raise SystemExit(f"unknown test vector set '{set_name}'; choices: {choices}")
    return sets[set_name]


def vector_enabled_for_codec(vector: generate_test_vectors.TestVector, codec: str) -> bool:
    return vector.codecs is None or codec.lower() in vector.codecs


def mode_supported(vector: generate_test_vectors.TestVector, codec: str, mode: str) -> bool:
    if codec == "vvc" and vector.fmt == "rgb24":
        return False
    if codec == "vvc" and mode == "lossy" and vector.fmt == "yuv422p10le":
        return True
    return True


def count_skipped(
    vector_set: generate_test_vectors.TestVectorSet,
    codecs: list[str],
    modes: list[str],
    limit: int,
) -> int:
    if limit:
        return 0
    skipped = 0
    for codec in codecs:
        for mode in modes:
            for vector in vector_set.vectors:
                if not vector_enabled_for_codec(vector, codec):
                    skipped += 1
                elif not mode_supported(vector, codec, mode):
                    skipped += 1
    return skipped


def run_case(
    vector_set: generate_test_vectors.TestVectorSet,
    vector: generate_test_vectors.TestVector,
    codec: str,
    mode: str,
    run_dir: Path,
    log_dir: Path,
    args: argparse.Namespace,
) -> dict[str, Any]:
    if args.frames is not None and args.frames != vector.frames:
        vector = replace(vector, frames=args.frames)
    source_path = source_path_for_vector(vector_set, vector, args)
    case_dir = run_dir / codec / mode
    case_dir.mkdir(parents=True, exist_ok=True)
    stem = Path(vector.filename).stem
    output = case_dir / f"{stem}.{codec_extension(codec)}"
    recon = case_dir / f"{stem}_recon.{raw_extension(vector)}"
    log = log_dir / f"{codec}_{mode}_{stem}.log"
    output.unlink(missing_ok=True)
    recon.unlink(missing_ok=True)

    command = [
        str(args.ff),
        "encode",
        str(source_path),
        "--video",
        f"{vector.width}x{vector.height}:{vector.fmt}",
        "--frames",
        str(vector.frames),
    ]
    if vector.fps is not None:
        command.extend(["--fps", vector.fps])
    command.extend(["--encode", f"{codec}:{output}", "--recon", str(recon)])
    settings: list[str] = []
    if mode == "lossless":
        settings.append("lossless")
    if codec == "av2" and args.av2_predictive:
        settings.append("predictive")
    for setting in settings:
        command.extend(["--set", setting])
    if codec == "av2" and mode == "lossy":
        command.extend(["--qp", str(args.av2_lossy_qp)])

    start = time.perf_counter()
    process = subprocess.run(
        command,
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    elapsed = time.perf_counter() - start
    log.write_text(f"$ {shlex.join(command)}\n\n{process.stdout}")
    if process.returncode != 0:
        print(process.stdout, file=sys.stderr, end="")
        raise SystemExit(f"encode failed for {codec} {mode} {vector.filename}; see {relpath(log)}")
    require_non_empty(output, "bitstream", vector.filename, log)
    require_non_empty(recon, "reconstruction", vector.filename, log)

    fps = vector.frames / elapsed if elapsed > 0.0 else math.inf
    result = {
        "codec": codec,
        "mode": mode_label(codec, mode, args),
        "mode_key": mode,
        "vector": vector.name,
        "filename": vector.filename,
        "format": vector.fmt,
        "width": vector.width,
        "height": vector.height,
        "frames": vector.frames,
        "bytes": output.stat().st_size,
        "seconds": elapsed,
        "fps": fps,
        "psnr_all_mean": mean_psnr_all(process.stdout),
        "bitstream_sha256": sha256_file(output),
        "recon_sha256": sha256_file(recon),
        "log": str(relpath(log)),
    }
    return result


def source_path_for_vector(
    vector_set: generate_test_vectors.TestVectorSet,
    vector: generate_test_vectors.TestVector,
    args: argparse.Namespace,
) -> Path:
    if args.direct_source_files and vector.pattern == "source_file" and vector.source_path:
        return source_file_path(vector)
    args.vector_dir.mkdir(parents=True, exist_ok=True)
    path = args.vector_dir / vector.filename
    path.write_bytes(generate_test_vectors.generate_yuv(vector, vector_set.sources))
    return path


def source_file_path(vector: generate_test_vectors.TestVector) -> Path:
    assert vector.source_path is not None
    if vector.source_path.is_absolute():
        return vector.source_path
    return (REPO_ROOT / vector.source_path).resolve(strict=False)


def mean_psnr_all(output: str) -> float | None:
    values = []
    for match in PSNR_ALL_RE.finditer(output):
        value = match.group(1)
        if value == "inf":
            values.append(math.inf)
        else:
            values.append(float(value))
    if not values:
        return None
    if any(math.isinf(value) for value in values):
        return math.inf if all(math.isinf(value) for value in values) else None
    return sum(values) / len(values)


def load_baseline(path: Path | None) -> dict[tuple[str, str, str], dict[str, Any]]:
    if path is None:
        return {}
    report = json.loads(path.read_text())
    return {
        (row["codec"], row["mode_key"], row["filename"]): row
        for row in report.get("results", [])
    }


def apply_baseline_delta(
    result: dict[str, Any],
    baseline: dict[tuple[str, str, str], dict[str, Any]],
) -> None:
    previous = baseline.get((result["codec"], result["mode_key"], result["filename"]))
    if previous is None:
        return
    result["delta_bytes"] = result["bytes"] - previous["bytes"]
    result["delta_fps"] = result["fps"] - previous["fps"]
    previous_psnr = previous.get("psnr_all_mean")
    current_psnr = result.get("psnr_all_mean")
    if previous_psnr is not None and current_psnr is not None:
        if math.isfinite(previous_psnr) and math.isfinite(current_psnr):
            result["delta_psnr_all_mean"] = current_psnr - previous_psnr


def markdown_report(report: dict[str, Any], skipped: int) -> str:
    lines = [
        f"# Encode Matrix: {report['run_name']}",
        "",
        f"- Set: `{report['set']}`",
        f"- AV2 predictive: `{report['av2_predictive']}`",
        f"- AV2 lossy QP: `{report['av2_lossy_qp']}`",
        f"- Skipped combinations: `{skipped}`",
        "",
        "| Codec | Mode | Vector | Format | Frames | Bytes | FPS | PSNR mean | Delta bytes | Delta FPS | Log |",
        "|---|---|---|---|---:|---:|---:|---:|---:|---:|---|",
    ]
    for row in report["results"]:
        lines.append(
            "| {codec} | {mode} | {vector} | {format} | {frames} | {bytes} | {fps:.2f} | "
            "{psnr} | {delta_bytes} | {delta_fps} | {log} |".format(
                codec=row["codec"],
                mode=row["mode"],
                vector=row["vector"],
                format=row["format"],
                frames=row["frames"],
                bytes=row["bytes"],
                fps=row["fps"],
                psnr=format_optional_float(row.get("psnr_all_mean")),
                delta_bytes=format_optional_int(row.get("delta_bytes")),
                delta_fps=format_optional_delta_float(row.get("delta_fps")),
                log=row["log"],
            )
        )
    lines.extend(total_rows(report["results"]))
    return "\n".join(lines)


def total_rows(results: list[dict[str, Any]]) -> list[str]:
    totals: dict[tuple[str, str], dict[str, float]] = {}
    for row in results:
        key = (row["codec"], row["mode"])
        total = totals.setdefault(key, {"frames": 0.0, "bytes": 0.0, "seconds": 0.0})
        total["frames"] += row["frames"]
        total["bytes"] += row["bytes"]
        total["seconds"] += row["seconds"]
    if not totals:
        return []
    lines = ["", "## Totals", "", "| Codec | Mode | Frames | Bytes | FPS |", "|---|---|---:|---:|---:|"]
    for (codec, mode), total in sorted(totals.items()):
        fps = total["frames"] / total["seconds"] if total["seconds"] > 0.0 else math.inf
        lines.append(f"| {codec} | {mode} | {int(total['frames'])} | {int(total['bytes'])} | {fps:.2f} |")
    return lines


def mode_label(codec: str, mode: str, args: argparse.Namespace) -> str:
    if codec == "av2" and args.av2_predictive:
        if mode == "lossy":
            return f"qp={args.av2_lossy_qp}+predictive"
        return "lossless+predictive"
    if codec == "av2" and mode == "lossy":
        return f"qp={args.av2_lossy_qp}"
    return mode


def codec_extension(codec: str) -> str:
    return {"av2": "obu", "vvc": "vvc"}.get(codec, codec)


def raw_extension(vector: generate_test_vectors.TestVector) -> str:
    return "rgb" if vector.fmt in {"gbrp8", "rgb24"} else "yuv"


def require_non_empty(path: Path, label: str, vector_name: str, log: Path) -> None:
    if not path.exists() or path.stat().st_size == 0:
        raise SystemExit(f"{label} missing or empty for {vector_name}; see {relpath(log)}")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        while chunk := file.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


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


def parse_positive_int(value: str) -> int:
    try:
        parsed = int(value, 10)
    except ValueError as err:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got '{value}'") from err
    if parsed <= 0:
        raise argparse.ArgumentTypeError(f"expected a positive integer, got '{value}'")
    return parsed


def delta_label(result: dict[str, Any]) -> str:
    parts = []
    if "delta_bytes" in result:
        parts.append(f"bytes_delta={result['delta_bytes']:+d}")
    if "delta_fps" in result:
        parts.append(f"fps_delta={result['delta_fps']:+.2f}")
    return " " + " ".join(parts) if parts else ""


def format_optional_float(value: Any) -> str:
    if value is None:
        return "n/a"
    if isinstance(value, float) and math.isinf(value):
        return "inf"
    return f"{value:.3f}"


def format_optional_delta_float(value: Any) -> str:
    if value is None:
        return "n/a"
    return f"{value:+.2f}"


def format_optional_int(value: Any) -> str:
    if value is None:
        return "n/a"
    return f"{value:+d}"


def relpath(path: Path) -> Path:
    try:
        return path.resolve().relative_to(REPO_ROOT)
    except ValueError:
        return path


if __name__ == "__main__":
    raise SystemExit(main())
