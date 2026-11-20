#!/usr/bin/env python3
"""Local MCP sidecar for file-free, live Frontier frame capture on X11."""
import base64
from fractions import Fraction
import json
import math
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

# Reuse the existing PID-scoped window lookup without generating __pycache__.
sys.dont_write_bytecode = True
from record_walk import ROOT, X11

BINARY = ROOT / "target/release/frontier"
TOOL_NAME = "capture_game_frames"
PNG_HEADER = b"\x89PNG\r\n\x1a\n"
PROTOCOLS = ("2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05")
PROPERTIES = {
    "wait_seconds": {
        "type": "number", "minimum": 0, "maximum": 60, "default": 3,
        "description": "Wait this long after the first rendered game window is ready.",
    },
    "frame_count": {
        "type": "integer", "minimum": 1, "maximum": 12, "default": 6,
        "description": "Number of PNG images to return in temporal order.",
    },
    "delay_ms": {
        "type": "number", "minimum": 10, "maximum": 5000, "default": 125,
        "description": "Requested spacing between sampled frames in milliseconds.",
    },
    "preset": {"type": "string", "enum": ["alpine", "seeded-world"], "default": "alpine"},
    "seed": {"type": "integer", "minimum": 0, "maximum": 2**64 - 1, "default": 7},
    "route": {
        "type": "string",
        "enum": ["static", "walk-stop-resume", "walk-turn-return", "walk-circuit",
                 "fly", "stop-resume", "turn-return"],
        "default": "static",
        "description": "Optional built-in automatic camera route, started by the game.",
    },
    "width": {"type": "integer", "minimum": 320, "maximum": 1920, "default": 960},
    "height": {"type": "integer", "minimum": 180, "maximum": 1080, "default": 540},
}
TOOL = {
    "name": TOOL_NAME,
    "description": (
        "Launch a separate local Frontier process, wait after its first rendered window, "
        "then return sampled gameplay frames as PNG image content with relative timestamps. "
        "Capture a static view or a built-in walking route. Closes the process when done."
    ),
    "inputSchema": {"type": "object", "properties": PROPERTIES, "additionalProperties": False},
}


def validated(arguments):
    if not isinstance(arguments, dict):
        raise ValueError("Tool arguments must be an object.")
    if unknown := arguments.keys() - PROPERTIES.keys():
        raise ValueError(f"Unknown arguments: {', '.join(sorted(unknown))}")
    options = {}
    for name, spec in PROPERTIES.items():
        value = arguments.get(name, spec["default"])
        if "enum" in spec:
            if value not in spec["enum"]:
                raise ValueError(f"{name} must be one of {', '.join(spec['enum'])}.")
        else:
            types = (int,) if spec["type"] == "integer" else (int, float)
            if type(value) not in types or not spec["minimum"] <= value <= spec["maximum"]:
                raise ValueError(f"{name} requires {spec['type']} values in {spec['minimum']}..{spec['maximum']}.")
        options[name] = value
    return options


def png_frames(data):
    """Split image2pipe output at real PNG chunk boundaries."""
    start = 0
    while start < len(data):
        if not data.startswith(PNG_HEADER, start):
            raise RuntimeError("FFmpeg returned an invalid PNG frame.")
        offset = start + len(PNG_HEADER)
        while offset + 12 <= len(data):
            size = int.from_bytes(data[offset:offset + 4], "big")
            kind = data[offset + 4:offset + 8]
            offset += size + 12
            if offset > len(data):
                break
            if kind == b"IEND":
                yield data[start:offset]
                start = offset
                break
        else:
            raise RuntimeError("FFmpeg returned a partial PNG frame.")
        if start != offset:
            raise RuntimeError("FFmpeg returned a partial PNG frame.")


def wait_for_window(x11, game):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if game.poll() is not None:
            raise RuntimeError(f"Frontier exited during startup (status {game.returncode}).")
        window = x11.find(game.pid)
        if window and "fps" in x11.title(window).lower():
            return window
        time.sleep(0.05)
    raise RuntimeError("Frontier did not present a window within 30 seconds.")


def capture_window(ffmpeg, display, window, options):
    count, delay = options["frame_count"], options["delay_ms"]
    # The one grabber samples the window at the requested interval. Its output
    # and timing metadata travel over pipes, including at full frame cadence.
    cadence = Fraction(1000) / Fraction(str(delay))
    result = subprocess.run([
        ffmpeg, "-hide_banner", "-loglevel", "info", "-nostdin",
        "-probesize", "32", "-analyzeduration", "0",
        "-f", "x11grab", "-window_id", str(window), "-draw_mouse", "0",
        "-framerate", str(cadence), "-i", display,
        "-frames:v", str(count), "-vf", "showinfo=checksum=0",
        "-fps_mode", "passthrough", "-c:v", "png", "-compression_level", "2",
        "-threads", "1", "-f", "image2pipe", "pipe:1",
    ], capture_output=True, timeout=10 + count * delay / 1000)
    stderr = result.stderr.decode(errors="replace")
    if result.returncode:
        raise RuntimeError(f"Window capture failed. {stderr.strip().splitlines()[-1] if stderr.strip() else result.returncode}")
    frames = list(png_frames(result.stdout))
    times = [float(t) for t in re.findall(r"\bn:\s*\d+\s+pts:\s*-?\d+\s+pts_time:([-+\d.eE]+)", stderr)]
    if len(frames) != count or len(times) < count or not all(map(math.isfinite, times[:count])):
        raise RuntimeError(f"Capture was incomplete ({len(frames)}/{count} images).")
    return frames, [round(t - times[0], 6) for t in times[:count]]


def stop_game(game, x11, window):
    if game.poll() is not None:
        return
    if window:
        x11.request_close(window, game.pid)
        try:
            game.wait(timeout=2)
            return
        except subprocess.TimeoutExpired:
            pass
    game.terminate()
    try:
        game.wait(timeout=2)
    except subprocess.TimeoutExpired:
        game.kill()
        game.wait()


def capture(arguments):
    options = validated(arguments)
    if not BINARY.is_file() or not os.access(BINARY, os.X_OK):
        raise RuntimeError(f"Build the Frontier release binary first. Expected {BINARY}.")
    display = os.environ.get("DISPLAY")
    ffmpeg = shutil.which("ffmpeg")
    if not display or not ffmpeg:
        raise RuntimeError("Capture needs a local X11 DISPLAY and FFmpeg.")
    x11 = X11()
    try:
        with tempfile.TemporaryDirectory(prefix="frontier-mcp-") as session:
            game = subprocess.Popen([
                str(BINARY), "--preset", options["preset"], "--seed", str(options["seed"]),
                "--width", str(options["width"]), "--height", str(options["height"]),
                "--route", options["route"], "--route-clock", "realtime",
                "--resource-profile", "development",
                "--no-adaptive-disk-cache",
            ], cwd=session, stdin=subprocess.DEVNULL, stdout=sys.stderr)
            window = None
            try:
                window = wait_for_window(x11, game)
                ready = time.monotonic()
                # Wait against the ready timestamp, then sample this exact PID's window.
                time.sleep(options["wait_seconds"])
                if game.poll() is not None or x11.pid(window) != game.pid:
                    raise RuntimeError("Frontier's capture window closed before sampling.")
                seconds_since_ready = round(time.monotonic() - ready, 3)
                frames, times = capture_window(ffmpeg, display, window, options)
            finally:
                stop_game(game, x11, window)
    finally:
        x11.close(x11.d)

    metadata = {
        "preset": options["preset"], "seed": options["seed"], "route": options["route"],
        "requested_wait_seconds": options["wait_seconds"],
        "grabber_started_seconds_after_window_ready": seconds_since_ready,
        "requested_delay_ms": options["delay_ms"],
        "width": int.from_bytes(frames[0][16:20], "big"),
        "height": int.from_bytes(frames[0][20:24], "big"),
        "frames": [{"index": i, "seconds_from_first_frame": t} for i, t in enumerate(times, 1)],
    }
    content = []
    for frame, timing in zip(frames, metadata["frames"]):
        content.extend([
            {"type": "text", "text": f"Frame {timing['index']} · +{timing['seconds_from_first_frame']:.3f} s"},
            {"type": "image", "mimeType": "image/png", "data": base64.b64encode(frame).decode("ascii")},
        ])
    return {"content": content, "structuredContent": metadata}


def respond(request):
    method, params = request.get("method"), request.get("params", {})
    if method == "initialize":
        requested = params.get("protocolVersion")
        return {"result": {
            "protocolVersion": requested if requested in PROTOCOLS else PROTOCOLS[0],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "frontier", "version": "0.1.0"},
        }}
    if method == "ping":
        return {"result": {}}
    if method == "tools/list":
        return {"result": {"tools": [TOOL]}}
    if method == "tools/call":
        if params.get("name") != TOOL_NAME:
            return {"error": {"code": -32602, "message": "Unknown Frontier tool."}}
        try:
            return {"result": capture(params.get("arguments", {}))}
        except (ValueError, RuntimeError, OSError, subprocess.TimeoutExpired) as error:
            return {"result": {"isError": True, "content": [{"type": "text", "text": str(error)}]}}
    return {"error": {"code": -32601, "message": "Method not found."}}


def serve():
    for line in sys.stdin:
        request_id = None
        try:
            request = json.loads(line)
            if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
                response = {"error": {"code": -32600, "message": "Invalid JSON-RPC request."}}
            elif "id" not in request:
                continue
            else:
                request_id = request["id"]
                if not isinstance(request.get("params", {}), dict):
                    response = {"error": {"code": -32602, "message": "Expected object parameters."}}
                else:
                    response = respond(request)
        except json.JSONDecodeError:
            response = {"error": {"code": -32700, "message": "Invalid JSON."}}
        print(json.dumps({"jsonrpc": "2.0", "id": request_id, **response}), flush=True)


if __name__ == "__main__":
    def terminate(_signum, _frame):
        raise SystemExit
    signal.signal(signal.SIGTERM, terminate)
    serve()
