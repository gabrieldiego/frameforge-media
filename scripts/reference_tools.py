#!/usr/bin/env python3
"""Locate, build, and run codec reference tools declared by manifests."""

from __future__ import annotations

import argparse
import json
import os
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST_DIR = REPO_ROOT / "verification" / "reference_codecs"
GLOBAL_DECODER_ENV = "FRAMEFORGE_DECODER"
GLOBAL_ENCODER_ENV = "FRAMEFORGE_ENCODER"
GLOBAL_REFERENCE_DIR_ENV = "FRAMEFORGE_REFERENCE_DIR"
LEGACY_GLOBAL_REFERENCE_DIR_ENV = "FRAMEFORGE_REF_DIR"


@dataclass(frozen=True)
class ReferenceManifest:
    codec: str
    label: str
    repo: str
    repo_env: tuple[str, ...]
    ref_env: tuple[str, ...]
    root_env: tuple[str, ...]
    encoder_env: tuple[str, ...]
    decoder_env: tuple[str, ...]
    build_dir_env: tuple[str, ...]
    build_type_env: tuple[str, ...]
    cmake_args_env: tuple[str, ...]
    default_root: Path
    decoder_names: tuple[str, ...]
    encoder_names: tuple[str, ...]
    decode_style: str


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest-dir",
        type=Path,
        default=DEFAULT_MANIFEST_DIR,
        help="directory containing <codec>.json reference manifests",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    list_parser = subparsers.add_parser("list", help="list declared reference manifests")
    list_parser.add_argument("--codec", default="all", help="codec name or all")

    setup_parser = subparsers.add_parser("setup", help="clone and build declared reference tools")
    setup_parser.add_argument("--codec", default="all", help="codec name or all")

    decoder_parser = subparsers.add_parser("decoder", help="print a reference decoder path")
    decoder_parser.add_argument("--codec", required=True)
    decoder_parser.add_argument("--no-build", action="store_true")

    encoder_parser = subparsers.add_parser("encoder", help="print a reference encoder path")
    encoder_parser.add_argument("--codec", required=True)
    encoder_parser.add_argument("--no-build", action="store_true")

    decode_parser = subparsers.add_parser("decode", help="decode a FrameForge bitstream")
    decode_parser.add_argument("--codec", required=True)
    decode_parser.add_argument("--bitstream", required=True, type=Path)
    decode_parser.add_argument("--output", required=True, type=Path)
    decode_parser.add_argument("--no-build", action="store_true")

    args = parser.parse_args()
    manifests = load_manifests(args.manifest_dir)
    if args.command == "list":
        for manifest in selected_manifests(manifests, args.codec):
            print(f"{manifest.codec}\t{manifest.label}\t{manifest.repo}\t{manifest.default_root}")
        return 0
    if args.command == "setup":
        for manifest in selected_manifests(manifests, args.codec):
            setup_reference(manifest)
        return 0
    if args.command == "decoder":
        manifest = manifest_for_codec(manifests, args.codec)
        path = resolve_tool(manifest, "decoder", no_build=args.no_build)
        print(path)
        return 0
    if args.command == "encoder":
        manifest = manifest_for_codec(manifests, args.codec)
        path = resolve_tool(manifest, "encoder", no_build=args.no_build)
        print(path)
        return 0
    if args.command == "decode":
        manifest = manifest_for_codec(manifests, args.codec)
        decoder = resolve_tool(manifest, "decoder", no_build=args.no_build)
        return decode_bitstream(manifest, decoder, args.bitstream, args.output)
    raise AssertionError(f"unhandled command {args.command}")


def load_manifests(manifest_dir: Path) -> dict[str, ReferenceManifest]:
    manifests: dict[str, ReferenceManifest] = {}
    if not manifest_dir.exists():
        return manifests
    for path in sorted(manifest_dir.glob("*.json")):
        data = json.loads(path.read_text())
        manifest = ReferenceManifest(
            codec=required_str(data, "codec", path),
            label=required_str(data, "label", path),
            repo=required_str(data, "repo", path),
            repo_env=tuple(data.get("repo_env", [])),
            ref_env=tuple(data.get("ref_env", [])),
            root_env=tuple(data.get("root_env", [])),
            encoder_env=tuple(data.get("encoder_env", [])),
            decoder_env=tuple(data.get("decoder_env", [])),
            build_dir_env=tuple(data.get("build_dir_env", [])),
            build_type_env=tuple(data.get("build_type_env", [])),
            cmake_args_env=tuple(data.get("cmake_args_env", [])),
            default_root=resolve_repo_path(required_str(data, "default_root", path)),
            decoder_names=tuple(data.get("decoder_names", [])),
            encoder_names=tuple(data.get("encoder_names", [])),
            decode_style=required_str(data, "decode_style", path),
        )
        manifests[manifest.codec] = manifest
    return manifests


def required_str(data: dict, key: str, path: Path) -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value:
        raise SystemExit(f"{path}: missing string field '{key}'")
    return value


def resolve_repo_path(value: str) -> Path:
    path = Path(value)
    return path if path.is_absolute() else REPO_ROOT / path


def selected_manifests(
    manifests: dict[str, ReferenceManifest], codec: str
) -> list[ReferenceManifest]:
    if codec == "all":
        return [manifests[name] for name in sorted(manifests)]
    return [manifest_for_codec(manifests, codec)]


def manifest_for_codec(manifests: dict[str, ReferenceManifest], codec: str) -> ReferenceManifest:
    try:
        return manifests[codec]
    except KeyError:
        choices = ", ".join(sorted(manifests)) or "<none>"
        raise SystemExit(f"no reference manifest declared for codec '{codec}'; choices: {choices}")


def resolve_tool(manifest: ReferenceManifest, kind: str, no_build: bool) -> str:
    configured = configured_tool(manifest, kind)
    if configured is not None:
        return configured

    names = manifest.decoder_names if kind == "decoder" else manifest.encoder_names
    if not names:
        raise SystemExit(f"{manifest.codec} manifest does not declare {kind} tools")

    root = reference_root(manifest)
    found = find_tool(root, names)
    if found is not None:
        return str(found)

    if no_build:
        names_label = ", ".join(names)
        print(
            f"no {manifest.label} {kind} found. Run 'make reference-setup "
            f"REFERENCE_CODEC={manifest.codec}' or set one of: "
            f"{', '.join(tool_env_names(manifest, kind))}. "
            f"Looked for {names_label} under {root}.",
            file=sys.stderr,
        )
        raise SystemExit(2)

    setup_reference(manifest)
    found = find_tool(root, names)
    if found is None:
        raise SystemExit(
            f"{manifest.label} build completed but no {kind} executable was found under {root}"
        )
    return str(found)


def configured_tool(manifest: ReferenceManifest, kind: str) -> str | None:
    global_env = GLOBAL_DECODER_ENV if kind == "decoder" else GLOBAL_ENCODER_ENV
    if value := os.environ.get(global_env):
        return first_shell_word_or_path(value)
    for env_name in tool_env_names(manifest, kind):
        if value := os.environ.get(env_name):
            path = Path(value)
            if path.exists():
                return str(path)
            raise SystemExit(f"{env_name} does not exist: {path}")
    return None


def first_shell_word_or_path(value: str) -> str:
    parts = shlex.split(value)
    return parts[0] if parts else value


def tool_env_names(manifest: ReferenceManifest, kind: str) -> tuple[str, ...]:
    return manifest.decoder_env if kind == "decoder" else manifest.encoder_env


def reference_root(manifest: ReferenceManifest) -> Path:
    for env_name in manifest.root_env:
        if value := os.environ.get(env_name):
            return Path(value)
    for env_name in (GLOBAL_REFERENCE_DIR_ENV, LEGACY_GLOBAL_REFERENCE_DIR_ENV):
        if value := os.environ.get(env_name):
            return Path(value) / manifest.codec
    return manifest.default_root


def setup_reference(manifest: ReferenceManifest) -> None:
    root = reference_root(manifest)
    if not root.exists():
        clone_reference(manifest, root)
    build_reference(manifest, root)
    decoder = find_tool(root, manifest.decoder_names)
    encoder = find_tool(root, manifest.encoder_names) if manifest.encoder_names else None
    if decoder is None:
        raise SystemExit(f"{manifest.label} build produced no declared decoder under {root}")
    if manifest.encoder_names and encoder is None:
        raise SystemExit(f"{manifest.label} build produced no declared encoder under {root}")
    print(f"{manifest.codec}: decoder={decoder}")
    if encoder is not None:
        print(f"{manifest.codec}: encoder={encoder}")


def clone_reference(manifest: ReferenceManifest, root: Path) -> None:
    repo = first_env(manifest.repo_env) or manifest.repo
    ref = first_env(manifest.ref_env)
    root.parent.mkdir(parents=True, exist_ok=True)
    cmd = ["git", "clone", "--depth", "1"]
    if ref:
        cmd.extend(["--branch", ref])
    cmd.extend([repo, str(root)])
    print(f"cloning {manifest.label} reference into {root}", file=sys.stderr)
    run(cmd)


def build_reference(manifest: ReferenceManifest, root: Path) -> None:
    if not shutil.which("cmake"):
        raise SystemExit(f"cmake is required to build {manifest.label}")
    build_dir = build_dir_for(manifest, root)
    build_type = first_env(manifest.build_type_env) or "Release"
    configure = [
        "cmake",
        "-S",
        str(root),
        "-B",
        str(build_dir),
        f"-DCMAKE_BUILD_TYPE={build_type}",
    ]
    configure.extend(cmake_args(manifest))
    build = ["cmake", "--build", str(build_dir), "--config", build_type]
    if jobs := os.environ.get("FRAMEFORGE_BUILD_JOBS"):
        build.extend(["--parallel", jobs])
    print(f"configuring {manifest.label} in {build_dir}", file=sys.stderr)
    run(configure)
    print(f"building {manifest.label}", file=sys.stderr)
    run(build)


def build_dir_for(manifest: ReferenceManifest, root: Path) -> Path:
    if value := first_env(manifest.build_dir_env):
        return Path(value)
    return root / "build"


def cmake_args(manifest: ReferenceManifest) -> list[str]:
    if value := first_env(manifest.cmake_args_env):
        return shlex.split(value)
    if manifest.codec == "av2" and not shutil.which("yasm") and not shutil.which("nasm"):
        return ["-DAVM_TARGET_CPU=generic"]
    return []


def first_env(names: tuple[str, ...]) -> str | None:
    for name in names:
        if value := os.environ.get(name):
            return value
    return None


def find_tool(root: Path, names: tuple[str, ...]) -> Path | None:
    if not root.exists():
        return None
    for name in names:
        for path in root.rglob(name):
            if path.is_file() and os.access(path, os.X_OK):
                return path
    return None


def decode_bitstream(
    manifest: ReferenceManifest, decoder: str, bitstream: Path, output: Path
) -> int:
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        output.unlink()
    if manifest.decode_style == "avm":
        cmd = [decoder, "--rawvideo", "-o", str(output), str(bitstream)]
    elif manifest.decode_style == "vtm":
        cmd = [decoder, "-b", str(bitstream), "-o", str(output)]
        if Path(decoder).name.startswith("DecoderAnalyserApp"):
            cmd.append("--Stats=0")
    else:
        raise SystemExit(f"unsupported decode_style '{manifest.decode_style}'")
    return subprocess.run(cmd, check=False).returncode


def run(cmd: list[str]) -> None:
    completed = subprocess.run(cmd, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
