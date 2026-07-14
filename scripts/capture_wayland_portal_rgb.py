#!/usr/bin/env python3
"""Capture a Wayland screen-share stream as packed raw RGB frames.

This uses xdg-desktop-portal's ScreenCast API, so the compositor shows the same
share approval UI used by browsers and video-conferencing applications. The
approved PipeWire stream is then consumed through GStreamer.
"""

from __future__ import annotations

import argparse
import os
import shlex
import sys
import time
import uuid
from pathlib import Path

import dbus
import dbus.mainloop.glib
import gi

gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst  # noqa: E402


PORTAL_BUS = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"
SCREENCAST_IFACE = "org.freedesktop.portal.ScreenCast"
REQUEST_IFACE = "org.freedesktop.portal.Request"


def portal_token(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex}"


class PortalScreenCast:
    def __init__(self) -> None:
        dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
        self.bus = dbus.SessionBus()
        portal = self.bus.get_object(PORTAL_BUS, PORTAL_PATH)
        self.screencast = dbus.Interface(portal, SCREENCAST_IFACE)

    def _request_path(self, token: str) -> str:
        sender = self.bus.get_unique_name().lstrip(":").replace(".", "_")
        return f"{PORTAL_PATH}/request/{sender}/{token}"

    def _call_request(self, method_name: str, token: str, *args):
        request_path = self._request_path(token)
        loop = GLib.MainLoop()
        received = {}

        def on_response(response, results):
            received["response"] = int(response)
            received["results"] = results
            loop.quit()

        signal_match = self.bus.add_signal_receiver(
            on_response,
            signal_name="Response",
            dbus_interface=REQUEST_IFACE,
            path=request_path,
        )

        method = getattr(self.screencast, method_name)
        returned_path = str(method(*args))
        if returned_path != request_path:
            signal_match.remove()
            request_path = returned_path
            signal_match = self.bus.add_signal_receiver(
                on_response,
                signal_name="Response",
                dbus_interface=REQUEST_IFACE,
                path=request_path,
            )

        loop.run()
        signal_match.remove()

        response = received.get("response")
        if response is None:
            raise RuntimeError(f"{method_name} did not return a portal response")
        if response != 0:
            raise RuntimeError(f"{method_name} was denied or cancelled by the portal")
        return received["results"]

    def create_session(self) -> dbus.ObjectPath:
        request_token = portal_token("create")
        session_token = portal_token("session")
        options = dbus.Dictionary(
            {
                "handle_token": dbus.String(request_token),
                "session_handle_token": dbus.String(session_token),
            },
            signature="sv",
        )
        results = self._call_request("CreateSession", request_token, options)
        return results["session_handle"]

    def select_sources(self, session_handle: dbus.ObjectPath, source_types: int, cursor_mode: int) -> None:
        request_token = portal_token("select")
        options = dbus.Dictionary(
            {
                "handle_token": dbus.String(request_token),
                "types": dbus.UInt32(source_types),
                "multiple": dbus.Boolean(False),
                "cursor_mode": dbus.UInt32(cursor_mode),
            },
            signature="sv",
        )
        self._call_request("SelectSources", request_token, session_handle, options)

    def start(self, session_handle: dbus.ObjectPath):
        request_token = portal_token("start")
        options = dbus.Dictionary({"handle_token": dbus.String(request_token)}, signature="sv")
        return self._call_request("Start", request_token, session_handle, "", options)

    def open_pipewire_remote(self, session_handle: dbus.ObjectPath) -> int:
        options = dbus.Dictionary({}, signature="sv")
        fd = self.screencast.OpenPipeWireRemote(session_handle, options)
        if hasattr(fd, "take"):
            return fd.take()
        return int(fd)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Capture approved Wayland ScreenCast portal output as raw RGB.",
    )
    parser.add_argument("output", type=Path, help="Output .rgb rawvideo path")
    parser.add_argument("--frames", type=int, default=500, help="Number of frames to write")
    parser.add_argument("--fps", type=int, default=30, help="Output frame rate")
    parser.add_argument("--width", type=int, help="Force output width")
    parser.add_argument("--height", type=int, help="Force output height")
    parser.add_argument(
        "--source",
        choices=("monitor", "window", "either"),
        default="monitor",
        help="Portal source type to offer",
    )
    parser.add_argument(
        "--cursor",
        choices=("hidden", "embedded", "metadata"),
        default="embedded",
        help="Cursor mode requested from the portal",
    )
    return parser.parse_args()


def stream_size(stream_props) -> tuple[int | None, int | None]:
    size = stream_props.get("size")
    if size is None or len(size) != 2:
        return None, None
    return int(size[0]), int(size[1])


def build_pipeline(
    fd: int,
    node_id: int,
    output: Path,
    frames: int,
    fps: int,
    width: int,
    height: int,
) -> Gst.Pipeline:
    location = shlex.quote(str(output))
    pipeline = (
        f'pipewiresrc fd={fd} path="{node_id}" always-copy=true use-bufferpool=false do-timestamp=true '
        "! queue max-size-buffers=8 max-size-bytes=0 max-size-time=0 "
        "! videorate "
        f"! video/x-raw,framerate={fps}/1 "
        "! videoscale add-borders=false "
        "! videoconvert dither=none "
        f"! video/x-raw,format=RGB,width={width},height={height} "
        "! identity name=frame_counter signal-handoffs=true "
        f"! filesink location={location} sync=false async=false"
    )
    return Gst.parse_launch(pipeline)


def bus_error(bus: Gst.Bus):
    msg = bus.pop_filtered(Gst.MessageType.ERROR | Gst.MessageType.EOS)
    if msg is None:
        return None
    if msg.type == Gst.MessageType.ERROR:
        err, debug = msg.parse_error()
        return f"{err.message}; {debug}"
    return "stream ended before the requested frame count"


def run_pipeline(pipeline: Gst.Pipeline, frames: int, fps: int) -> None:
    counter = {"frames": 0, "eos_sent": False, "eos_time": 0.0}

    def on_handoff(_identity, _buffer):
        counter["frames"] += 1
        count = counter["frames"]
        if count % max(fps, 1) == 0 or count == frames:
            print(f"captured {count}/{frames} frames", flush=True)
        if count >= frames and not counter["eos_sent"]:
            counter["eos_sent"] = True
            counter["eos_time"] = time.monotonic()
            pipeline.send_event(Gst.Event.new_eos())

    identity = pipeline.get_by_name("frame_counter")
    identity.connect("handoff", on_handoff)

    bus = pipeline.get_bus()
    state_change = pipeline.set_state(Gst.State.PLAYING)
    if state_change == Gst.StateChangeReturn.FAILURE:
        raise RuntimeError("failed to start GStreamer pipeline")

    while True:
        msg = bus.timed_pop_filtered(500 * Gst.MSECOND, Gst.MessageType.ERROR | Gst.MessageType.EOS)
        if msg is not None and msg.type == Gst.MessageType.ERROR:
            err, debug = msg.parse_error()
            raise RuntimeError(f"{err.message}; {debug}")
        if msg is not None and msg.type == Gst.MessageType.EOS:
            break
        if counter["frames"] >= frames and not counter["eos_sent"]:
            counter["eos_sent"] = True
            counter["eos_time"] = time.monotonic()
            pipeline.send_event(Gst.Event.new_eos())
        if counter["eos_sent"] and time.monotonic() - counter["eos_time"] > 3.0:
            print("forcing pipeline stop after requested frame count", flush=True)
            break

    if counter["frames"] < frames:
        raise RuntimeError(f"captured {counter['frames']} frame(s), expected {frames}")


def main() -> int:
    args = parse_args()
    if args.frames <= 0:
        raise SystemExit("--frames must be positive")
    if args.fps <= 0:
        raise SystemExit("--fps must be positive")
    if (args.width is None) != (args.height is None):
        raise SystemExit("--width and --height must be provided together")

    source_types = {"monitor": 1, "window": 2, "either": 3}[args.source]
    cursor_modes = {"hidden": 1, "embedded": 2, "metadata": 4}
    cursor_mode = cursor_modes[args.cursor]

    portal = PortalScreenCast()
    print("Requesting Wayland screen-share permission through xdg-desktop-portal...", flush=True)
    session = portal.create_session()
    print("Portal session created.", flush=True)
    portal.select_sources(session, source_types, cursor_mode)
    print("Portal source selected.", flush=True)
    start_results = portal.start(session)
    print("Portal stream approved.", flush=True)

    streams = start_results.get("streams")
    if not streams:
        raise RuntimeError("portal did not return any PipeWire streams")
    node_id = int(streams[0][0])
    props = streams[0][1]
    detected_width, detected_height = stream_size(props)
    width = args.width if args.width is not None else detected_width
    height = args.height if args.height is not None else detected_height

    if width is None or height is None:
        raise RuntimeError("portal did not report stream size; pass --width and --height")

    print(f"Using PipeWire node {node_id} at {width}x{height}.", flush=True)
    portal_fd = portal.open_pipewire_remote(session)
    fd = os.dup(portal_fd)
    os.close(portal_fd)

    Gst.init(None)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    pipeline = build_pipeline(fd, node_id, args.output, args.frames, args.fps, width, height)
    print("GStreamer pipeline starting.", flush=True)
    try:
        run_pipeline(pipeline, args.frames, args.fps)
    finally:
        pipeline.set_state(Gst.State.NULL)

    expected_bytes = width * height * 3 * args.frames
    actual_bytes = args.output.stat().st_size
    if actual_bytes > expected_bytes:
        with args.output.open("r+b") as output:
            output.truncate(expected_bytes)
        print(
            f"trimmed {actual_bytes - expected_bytes} trailing byte(s) after requested frame count",
            flush=True,
        )
        actual_bytes = expected_bytes
    print(
        f"wrote {args.output} as rgb24 {width}x{height} {args.fps}fps "
        f"{args.frames}f ({actual_bytes} bytes)",
        flush=True,
    )
    if actual_bytes != expected_bytes:
        raise RuntimeError(f"expected {expected_bytes} bytes, wrote {actual_bytes}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
    except Exception as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
