#!/usr/bin/env python3
"""Summarize encoder instrumentation traces for comparative AV2 work."""

from __future__ import annotations

import argparse
import json
import math
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


CATEGORY_KEYS = (
    "partition_bits",
    "luma_mode_bits",
    "chroma_mode_bits",
    "residual_bits",
    "intrabc_bits",
    "inter_bits",
    "palette_bits",
    "other_bits",
)


@dataclass(frozen=True)
class TraceSpec:
    case: str
    source: str | None
    path: Path


@dataclass(frozen=True)
class SbRow:
    case: str
    source: str
    frame_index: int
    sb_x: int
    sb_y: int
    x: int | None
    y: int | None
    width: int | None
    height: int | None
    total_bits: int
    categories: dict[str, int]


@dataclass(frozen=True)
class VvcStageRow:
    case: str
    source: str
    frame_index: int
    width: int
    height: int
    chroma_sampling: str
    bit_depth: int
    lossless: bool
    bitstream_bytes: int
    stage: str
    nanos: int
    count: int


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sb-bits",
        action="append",
        default=[],
        metavar="[CASE/]SOURCE=PATH",
        help=(
            "per-superblock JSONL from FRAMEFORGE_AV2_SB_BITS, "
            "FRAMEFORGE_LIBAOM_SB_BITS, or FRAMEFORGE_AVM_SB_BITS"
        ),
    )
    parser.add_argument(
        "--field-trace",
        action="append",
        default=[],
        metavar="[CASE/]SOURCE=PATH",
        help="FrameForge entropy field JSONL trace with name and bit_count fields",
    )
    parser.add_argument(
        "--lossy-stats-log",
        action="append",
        default=[],
        metavar="[CASE/]SOURCE=PATH",
        help="stderr log containing gated FRAMEFORGE_AV2_LOSSY_STATS lines",
    )
    parser.add_argument(
        "--vvc-stats",
        action="append",
        default=[],
        metavar="[CASE/]SOURCE=PATH",
        help="VVC stage timing JSONL from FRAMEFORGE_VVC_STATS",
    )
    parser.add_argument(
        "--baseline-source",
        default="frameforge",
        help="source name used as the numerator for pairwise SB ratios",
    )
    parser.add_argument("--top", type=int, default=12, help="number of hot rows to print")
    args = parser.parse_args()

    sb_specs = [parse_spec(value) for value in args.sb_bits]
    field_specs = [parse_spec(value) for value in args.field_trace]
    lossy_specs = [parse_spec(value) for value in args.lossy_stats_log]
    vvc_specs = [parse_spec(value) for value in args.vvc_stats]

    if not sb_specs and not field_specs and not lossy_specs and not vvc_specs:
        parser.error("at least one instrumentation input is required")

    sb_rows = [row for spec in sb_specs for row in read_sb_rows(spec)]
    field_rows = [row for spec in field_specs for row in read_field_rows(spec)]
    lossy_rows = [row for spec in lossy_specs for row in read_lossy_stats_rows(spec)]
    vvc_rows = [row for spec in vvc_specs for row in read_vvc_stage_rows(spec)]

    if sb_rows:
        print_sb_summary(sb_rows)
        print_pairwise_sb_summary(sb_rows, args.baseline_source)
        print_hot_sbs(sb_rows, args.top)
    if field_rows:
        print_field_summary(field_rows, args.top)
    if lossy_rows:
        print_lossy_stats_summary(lossy_rows)
    if vvc_rows:
        print_vvc_stage_summary(vvc_rows, args.top)
    return 0


def parse_spec(value: str) -> TraceSpec:
    label = None
    path_text = value
    if "=" in value:
        label, path_text = value.split("=", 1)
    path = Path(path_text)
    if label:
        if "/" in label:
            case, source = label.split("/", 1)
        else:
            case, source = path.stem, label
    else:
        case, source = path.stem, None
    case = case.strip() or path.stem
    source = source.strip() if source is not None else None
    if source == "":
        source = None
    return TraceSpec(case=case, source=source, path=path)


def read_jsonl(path: Path) -> Iterable[dict]:
    with path.open("r", encoding="utf-8") as source:
        for line_number, line in enumerate(source, start=1):
            stripped = line.strip()
            if not stripped:
                continue
            try:
                yield json.loads(stripped)
            except json.JSONDecodeError as exc:
                raise SystemExit(f"{path}:{line_number}: invalid JSONL: {exc}") from exc


def read_sb_rows(spec: TraceSpec) -> list[SbRow]:
    rows = []
    for record in read_jsonl(spec.path):
        source = spec.source or str(record.get("source") or spec.path.stem)
        case = spec.case
        total_bits = int(record.get("total_symbol_bits", record.get("bits", 0)))
        categories = {
            key: int(record.get(key, 0))
            for key in CATEGORY_KEYS
            if int(record.get(key, 0)) != 0
        }
        rows.append(
            SbRow(
                case=case,
                source=source,
                frame_index=int(record.get("frame_index", 0)),
                sb_x=int(record.get("sb_x", 0)),
                sb_y=int(record.get("sb_y", 0)),
                x=optional_int(record.get("x")),
                y=optional_int(record.get("y")),
                width=optional_int(record.get("width")),
                height=optional_int(record.get("height")),
                total_bits=total_bits,
                categories=categories,
            )
        )
    return rows


def optional_int(value: object) -> int | None:
    if value is None:
        return None
    return int(value)


def print_sb_summary(rows: list[SbRow]) -> None:
    grouped: dict[tuple[str, str, int], list[SbRow]] = defaultdict(list)
    for row in rows:
        grouped[(row.case, row.source, row.frame_index)].append(row)

    print("## Superblock Bit Summary")
    print(
        "| Case | Source | Frame | SBs | Total bits | Mean bits/SB | P95 bits/SB | Max bits/SB | Top categories |"
    )
    print("|---|---|---:|---:|---:|---:|---:|---:|---|")
    for key in sorted(grouped):
        group = grouped[key]
        values = sorted(row.total_bits for row in group)
        total = sum(values)
        category_totals = aggregate_categories(group)
        print(
            f"| {key[0]} | {key[1]} | {key[2]} | {len(group)} | {total} | "
            f"{mean(values):.1f} | {percentile(values, 0.95):.1f} | {max(values)} | "
            f"{format_categories(category_totals, total)} |"
        )
    print()


def print_pairwise_sb_summary(rows: list[SbRow], baseline_source: str) -> None:
    grouped: dict[tuple[str, int, str], list[SbRow]] = defaultdict(list)
    for row in rows:
        grouped[(row.case, row.frame_index, row.source)].append(row)

    by_case_frame: dict[tuple[str, int], dict[str, list[SbRow]]] = defaultdict(dict)
    for (case, frame, source), group in grouped.items():
        by_case_frame[(case, frame)][source] = group

    print("## Pairwise Superblock Ratios")
    print(
        "| Case | Frame | Baseline | Compared | Matched SBs | Baseline bits | Compared bits | Baseline/Compared | Largest excess SB |"
    )
    print("|---|---:|---|---|---:|---:|---:|---:|---|")
    emitted = False
    for (case, frame), sources in sorted(by_case_frame.items()):
        baseline = choose_baseline_source(sources, baseline_source)
        if baseline is None:
            continue
        baseline_map = sb_map(sources[baseline])
        for source in sorted(sources):
            if source == baseline:
                continue
            compared_map = sb_map(sources[source])
            matched = sorted(set(baseline_map) & set(compared_map))
            if not matched:
                continue
            baseline_bits = sum(baseline_map[key].total_bits for key in matched)
            compared_bits = sum(compared_map[key].total_bits for key in matched)
            ratio = baseline_bits / compared_bits if compared_bits else math.inf
            largest = max(
                matched,
                key=lambda key: baseline_map[key].total_bits - compared_map[key].total_bits,
            )
            largest_delta = baseline_map[largest].total_bits - compared_map[largest].total_bits
            print(
                f"| {case} | {frame} | {baseline} | {source} | {len(matched)} | "
                f"{baseline_bits} | {compared_bits} | {format_ratio(ratio)} | "
                f"({largest[0]},{largest[1]}) {largest_delta:+d} bits |"
            )
            emitted = True
    if not emitted:
        print("| n/a | 0 | n/a | n/a | 0 | 0 | 0 | n/a | no matching baseline/source rows |")
    print()


def print_hot_sbs(rows: list[SbRow], top: int) -> None:
    print("## Hottest Superblocks")
    print("| Case | Source | Frame | SB | Rect | Total bits | Top categories |")
    print("|---|---|---:|---|---|---:|---|")
    for row in sorted(rows, key=lambda row: row.total_bits, reverse=True)[:top]:
        rect = "n/a"
        if row.x is not None and row.y is not None and row.width is not None and row.height is not None:
            rect = f"{row.width}x{row.height}+{row.x},{row.y}"
        print(
            f"| {row.case} | {row.source} | {row.frame_index} | ({row.sb_x},{row.sb_y}) | "
            f"{rect} | {row.total_bits} | {format_categories(row.categories, row.total_bits)} |"
        )
    print()


def read_field_rows(spec: TraceSpec) -> list[tuple[str, str, str, str, int, int]]:
    rows = []
    for record in read_jsonl(spec.path):
        if "name" not in record or "bit_count" not in record:
            continue
        source = spec.source or str(record.get("source") or spec.path.stem)
        rows.append(
            (
                spec.case,
                source,
                str(record.get("phase", "")),
                str(record["name"]),
                int(record.get("bit_count", 0)),
                1,
            )
        )
    return rows


def print_field_summary(rows: list[tuple[str, str, str, str, int, int]], top: int) -> None:
    totals: dict[tuple[str, str, str, str], list[int]] = defaultdict(lambda: [0, 0])
    for case, source, phase, name, bits, count in rows:
        total = totals[(case, source, phase, name)]
        total[0] += bits
        total[1] += count

    print("## Entropy Field Summary")
    print("| Case | Source | Phase | Field | Count | Bits |")
    print("|---|---|---|---|---:|---:|")
    for (case, source, phase, name), (bits, count) in sorted(
        totals.items(), key=lambda item: item[1][0], reverse=True
    )[:top]:
        print(f"| {case} | {source} | {phase} | `{name}` | {count} | {bits} |")
    print()


LOSSY_STATS_RE = re.compile(r"^av2-lossy-stats\s+(?P<label>\S+)\s+(?P<body>.*)$")


def read_lossy_stats_rows(spec: TraceSpec) -> list[tuple[str, str, str, dict[str, int]]]:
    rows = []
    with spec.path.open("r", encoding="utf-8", errors="replace") as source:
        for line in source:
            match = LOSSY_STATS_RE.match(line.strip())
            if not match:
                continue
            values: dict[str, int] = {}
            for token in match.group("body").split():
                key, sep, value = token.partition("=")
                if sep and is_int(value):
                    values[key] = int(value)
            rows.append((spec.case, spec.source or "frameforge", match.group("label"), values))
    return rows


def print_lossy_stats_summary(rows: list[tuple[str, str, str, dict[str, int]]]) -> None:
    totals: dict[tuple[str, str, str], dict[str, int]] = defaultdict(lambda: defaultdict(int))
    for case, source, label, values in rows:
        target = totals[(case, source, label)]
        for key, value in values.items():
            target[key] += value

    print("## FrameForge Lossy Mode/TXB Stats")
    print("| Case | Source | Stat | Summary |")
    print("|---|---|---|---|")
    for (case, source, label), values in sorted(totals.items()):
        summary = ", ".join(f"{key}={value}" for key, value in sorted(values.items()))
        print(f"| {case} | {source} | `{label}` | {summary} |")
    print()


def read_vvc_stage_rows(spec: TraceSpec) -> list[VvcStageRow]:
    rows = []
    for record in read_jsonl(spec.path):
        if record.get("kind") != "frameforge.vvc.stats.v1":
            continue
        source = spec.source or str(record.get("source") or spec.path.stem)
        for stage in record.get("stages", []):
            rows.append(
                VvcStageRow(
                    case=spec.case,
                    source=source,
                    frame_index=int(record.get("frame_index", 0)),
                    width=int(record.get("width", 0)),
                    height=int(record.get("height", 0)),
                    chroma_sampling=str(record.get("chroma_sampling", "")),
                    bit_depth=int(record.get("bit_depth", 0)),
                    lossless=bool(record.get("lossless", False)),
                    bitstream_bytes=int(record.get("bitstream_bytes", 0)),
                    stage=str(stage.get("name", "")),
                    nanos=int(stage.get("ns", 0)),
                    count=int(stage.get("count", 0)),
                )
            )
    return rows


def print_vvc_stage_summary(rows: list[VvcStageRow], top: int) -> None:
    totals: dict[tuple[str, str, str], list[int]] = defaultdict(lambda: [0, 0])
    total_nanos_by_case: dict[tuple[str, str], int] = defaultdict(int)
    frame_totals: dict[tuple[str, str], set[int]] = defaultdict(set)
    bytes_by_case: dict[tuple[str, str], int] = defaultdict(int)
    byte_frames: set[tuple[str, str, int]] = set()
    for row in rows:
        key = (row.case, row.source, row.stage)
        totals[key][0] += row.nanos
        totals[key][1] += row.count
        total_nanos_by_case[(row.case, row.source)] += row.nanos
        frame_totals[(row.case, row.source)].add(row.frame_index)
        frame_key = (row.case, row.source, row.frame_index)
        if frame_key not in byte_frames:
            bytes_by_case[(row.case, row.source)] += row.bitstream_bytes
            byte_frames.add(frame_key)

    print("## VVC Stage Timing Summary")
    print("| Case | Source | Stage | Count | Time ms | Share | Avg us/call |")
    print("|---|---|---|---:|---:|---:|---:|")
    sorted_rows = sorted(totals.items(), key=lambda item: item[1][0], reverse=True)
    for (case, source, stage), (nanos, count) in sorted_rows[:top]:
        total_nanos = total_nanos_by_case[(case, source)]
        share = nanos * 100.0 / total_nanos if total_nanos else 0.0
        avg_us = nanos / count / 1000.0 if count else 0.0
        print(
            f"| {case} | {source} | `{stage}` | {count} | {nanos / 1_000_000.0:.3f} | "
            f"{share:.1f}% | {avg_us:.3f} |"
        )
    print()

    print("## VVC Stage Trace Totals")
    print("| Case | Source | Frames | Encoded bytes | Timed ms |")
    print("|---|---|---:|---:|---:|")
    for case, source in sorted(total_nanos_by_case):
        print(
            f"| {case} | {source} | {len(frame_totals[(case, source)])} | "
            f"{bytes_by_case[(case, source)]} | {total_nanos_by_case[(case, source)] / 1_000_000.0:.3f} |"
        )
    print()


def aggregate_categories(rows: Iterable[SbRow]) -> dict[str, int]:
    totals: dict[str, int] = defaultdict(int)
    for row in rows:
        for key, value in row.categories.items():
            totals[key] += value
    return dict(totals)


def format_categories(categories: dict[str, int], total: int) -> str:
    if not categories:
        return "n/a"
    parts = []
    for key, value in sorted(categories.items(), key=lambda item: item[1], reverse=True)[:3]:
        percent = value * 100.0 / total if total else 0.0
        parts.append(f"{key.removesuffix('_bits')}={value} ({percent:.1f}%)")
    return ", ".join(parts)


def sb_map(rows: Iterable[SbRow]) -> dict[tuple[int, int], SbRow]:
    out = {}
    for row in rows:
        key = (row.sb_x, row.sb_y)
        if key in out:
            previous = out[key]
            categories = aggregate_categories([previous, row])
            out[key] = SbRow(
                case=row.case,
                source=row.source,
                frame_index=row.frame_index,
                sb_x=row.sb_x,
                sb_y=row.sb_y,
                x=row.x,
                y=row.y,
                width=row.width,
                height=row.height,
                total_bits=previous.total_bits + row.total_bits,
                categories=categories,
            )
        else:
            out[key] = row
    return out


def choose_baseline_source(sources: dict[str, list[SbRow]], preferred: str) -> str | None:
    preferred_lower = preferred.lower()
    for source in sources:
        if source.lower() == preferred_lower:
            return source
    for source in sources:
        lowered = source.lower()
        if preferred_lower in lowered or "frameforge" in lowered or lowered == "ff":
            return source
    return None


def mean(values: list[int]) -> float:
    return sum(values) / len(values) if values else 0.0


def percentile(values: list[int], fraction: float) -> float:
    if not values:
        return 0.0
    index = max(0, min(len(values) - 1, math.ceil(len(values) * fraction) - 1))
    return float(values[index])


def format_ratio(value: float) -> str:
    if math.isinf(value):
        return "inf"
    return f"{value:.2f}x"


def is_int(value: str) -> bool:
    try:
        int(value)
    except ValueError:
        return False
    return True


if __name__ == "__main__":
    raise SystemExit(main())
