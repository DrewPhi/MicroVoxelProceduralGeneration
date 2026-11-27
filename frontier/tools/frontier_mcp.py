#!/usr/bin/env python3
"""Local MCP tools for Frontier gameplay capture and reproducible aircraft inspection."""
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


PREVIEW_NAME = "render_aircraft_preview"
INSPECT_NAME = "inspect_aircraft_model"
AIRCRAFT_ONLY = "--aircraft-only" in sys.argv[1:]
VIEWS = ["orbit", "front", "side", "rear", "top", "underside", "pilot", "rear-seat", "panel", "controls"]
AIRCRAFT = {"type": "string", "enum": ["skyhawk", "da62"], "default": "skyhawk"}
PREVIEW_NUMBERS = {
    "yaw": (-360, 360), "pitch": (-89, 89), "distance": (0.55, 80),
    "light": (-180, 180), "door": (0, 1), "left_door": (0, 1), "right_door": (0, 1),
    "aileron": (-1, 1), "elevator": (-1, 1), "rudder": (-1, 1), "flaps": (0, 1),
    "power": (0, 1), "right_power": (0, 1), "rear_door": (0, 1), "gear": (0, 1),
    "wheel": (0, 360), "compression": (0, 0.13),
}
PREVIEW_FLAGS = ("orthographic", "no_glass", "engine")
PREVIEW_PROPERTIES = {
    "aircraft": AIRCRAFT,
    "views": {"type": "array", "items": {"type": "string", "enum": VIEWS},
              "minItems": 1, "maxItems": 6, "default": ["orbit"]},
    "width": {"type": "integer", "minimum": 320, "maximum": 1920, "default": 960},
    "height": {"type": "integer", "minimum": 180, "maximum": 1080, "default": 600},
    "isolate": {"type": "string", "description": "Part name returned by inspect_aircraft_model."},
    "cutaway": {"type": "string", "enum": ["left", "right"]},
    "target": {"type": "array", "items": {"type": "number", "minimum": -16, "maximum": 16},
               "minItems": 3, "maxItems": 3, "description": "Orbit target in model-local metres. With interior views, this is an eye offset."},
    **{name: {"type": "number", "minimum": low, "maximum": high}
       for name, (low, high) in PREVIEW_NUMBERS.items()},
    **{name: {"type": "boolean", "default": False} for name in PREVIEW_FLAGS},
}
PREVIEW_TOOL = {
    "name": PREVIEW_NAME,
    "description": (
        "Render up to six repeatable, antialiased images of the real in-engine aircraft without a game world or window. "
        "Use orthographic views for proportions or custom yaw, pitch, distance and target for close-ups. "
        "Pose doors, control surfaces, wheels and propellers, change the light, "
        "or inspect the cabin and model joints with a cutaway or named part isolation. "
        "Choose da62 for its rear door, retractable gear and second power lever."
    ),
    "inputSchema": {"type": "object", "properties": PREVIEW_PROPERTIES, "additionalProperties": False},
}
INSPECT_TOOL = {
    "name": INSPECT_NAME,
    "description": "Get the chosen aircraft's voxel scales, coordinate system, part names, voxel counts, triangle counts and rest-pose bounds.",
    "inputSchema": {"type": "object", "properties": {"aircraft": AIRCRAFT}, "additionalProperties": False},
}


def aircraft_binary(*args):
    if not BINARY.is_file() or not os.access(BINARY, os.X_OK):
        raise RuntimeError(f"Build the Frontier release binary first. Expected {BINARY}.")
    result = subprocess.run([str(BINARY), "--model-preview", *args], cwd=ROOT,
                            stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=45)
    if result.returncode:
        raise RuntimeError(result.stderr.strip()[-3000:] or f"Aircraft studio exited with status {result.returncode}.")
    return result.stdout


def inspect_aircraft(arguments):
    if not isinstance(arguments, dict) or arguments.keys() - {"aircraft"}:
        raise ValueError("inspect_aircraft_model accepts an optional aircraft name.")
    aircraft = json.loads(aircraft_binary("--aircraft", aircraft_name(arguments), "--inspect"))
    return {"content": [{"type": "text", "text": json.dumps(aircraft)}], "structuredContent": aircraft}


def aircraft_name(arguments):
    name = arguments.get("aircraft", AIRCRAFT["default"])
    if name not in AIRCRAFT["enum"]:
        raise ValueError("aircraft must be skyhawk or da62.")
    return name


def render_aircraft(arguments):
    if not isinstance(arguments, dict):
        raise ValueError("Tool arguments must be an object.")
    if unknown := arguments.keys() - PREVIEW_PROPERTIES.keys():
        raise ValueError(f"Unknown arguments: {', '.join(sorted(unknown))}")
    views = arguments.get("views", ["orbit"])
    if type(views) is not list or not 1 <= len(views) <= 6 or any(v not in VIEWS for v in views):
        raise ValueError("Provide 1 to 6 supported view names.")
    args = ["--aircraft", aircraft_name(arguments)]
    dimensions = {name: arguments.get(name, PREVIEW_PROPERTIES[name]["default"]) for name in ("width", "height")}
    for name, value in dimensions.items():
        spec = PREVIEW_PROPERTIES[name]
        if type(value) is not int or not spec["minimum"] <= value <= spec["maximum"]:
            raise ValueError(f"{name} must be an integer in {spec['minimum']}..{spec['maximum']}.")
        args.extend([f"--{name}", str(value)])
    for name, (low, high) in PREVIEW_NUMBERS.items():
        if name in arguments:
            value = arguments[name]
            if type(value) not in (int, float) or not low <= value <= high:
                raise ValueError(f"{name} must be a number in {low}..{high}.")
            args.extend(["--" + name.replace("_", "-"), str(value)])
    for name in PREVIEW_FLAGS:
        value = arguments.get(name, False)
        if type(value) is not bool:
            raise ValueError(f"{name} must be a boolean.")
        if value:
            args.append("--" + name.replace("_", "-"))
    if "target" in arguments:
        value = arguments["target"]
        if type(value) is not list or len(value) != 3 or any(type(v) not in (int, float) or not -16 <= v <= 16 for v in value):
            raise ValueError("target must contain three numbers in -16..16.")
        args.extend(["--target", ",".join(str(v) for v in value)])
    for name in ("cutaway", "isolate"):
        if name in arguments:
            value = arguments[name]
            if type(value) is not str or not re.fullmatch(r"[a-z][a-z-]{0,31}", value):
                raise ValueError(f"{name} must be a valid name.")
            if name == "cutaway" and value not in ("left", "right"):
                raise ValueError("cutaway must be left or right.")
            args.extend([f"--{name}", value])
    with tempfile.TemporaryDirectory(prefix="frontier-aircraft-") as directory:
        path = Path(directory) / "preview.png"
        content = []
        for index, view in enumerate(views, 1):
            aircraft_binary("--view", view, *args, "--capture", str(path))
            image = path.read_bytes()
            if not image.startswith(PNG_HEADER):
                raise RuntimeError("The aircraft studio produced an invalid image.")
            content.extend([
                {"type": "text", "text": f"{index}. {view}"},
                {"type": "image", "mimeType": "image/png", "data": base64.b64encode(image).decode("ascii")},
            ])
    return {"content": content, "structuredContent": {"views": views, **dimensions, "options": arguments}}


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
            "serverInfo": {"name": "frontier", "version": "0.2.0"},
        }}
    if method == "ping":
        return {"result": {}}
    if method == "tools/list":
        published = [PREVIEW_TOOL, INSPECT_TOOL] if AIRCRAFT_ONLY else [TOOL, PREVIEW_TOOL, INSPECT_TOOL]
        return {"result": {"tools": published}}
    if method == "tools/call":
        handlers = {TOOL_NAME: capture, PREVIEW_NAME: render_aircraft, INSPECT_NAME: inspect_aircraft}
        name = params.get("name")
        if not isinstance(name, str) or (AIRCRAFT_ONLY and name == TOOL_NAME):
            name = ""
        handler = handlers.get(name)
        if handler is None:
            return {"error": {"code": -32602, "message": "Unknown Frontier tool."}}
        try:
            return {"result": handler(params.get("arguments", {}))}
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
