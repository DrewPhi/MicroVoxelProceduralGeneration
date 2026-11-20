#!/usr/bin/env python3
"""Record a real desert walk, scoop, and stumble using normal X11 game input."""
import argparse
import hashlib
import json
import math
import os
import subprocess
import time
from pathlib import Path

from record_walk import I, P, ROOT, U, X11


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("view", choices=["first-person", "third-person"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=720)
    args = parser.parse_args()
    out = args.output.resolve() / args.view
    out.mkdir(parents=True, exist_ok=False)
    width, height = args.width, args.height
    command = [
        str(ROOT / "target/release/frontier"), "--preset", "desert", "--seed", "19",
        "--width", str(width), "--height", str(height),
        "--naadf-radius", "16", "--capture-far", "300", "--pixel-error", "6",
        "--msaa", "1", "--no-adaptive-disk-cache", "--sun-elevation", "28",
        "--report", str(out / "game.json"), "--bowl-save", str(out / "world.bowl"),
    ]
    env = dict(os.environ, FRONTIER_PLAYER_TRACE="1")
    binary_sha256 = hashlib.sha256(Path(command[0]).read_bytes()).hexdigest()
    log_path = out / "game.log"
    events = []
    recorder = window = None
    x = X11()
    with log_path.open("w") as log, (out / "record.log").open("w") as video_log:
        app = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        relative = x.t.XTestFakeRelativeMotionEvent
        relative.restype = I
        relative.argtypes = [P, I, I, U]

        def lines():
            return log_path.read_text().splitlines()

        def latest(prefix):
            for line in reversed(lines()):
                if line.startswith(prefix) and line.endswith("]"):
                    return json.loads(line[len(prefix):])
            return None

        def wait_for(check, timeout, error):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if app.poll() is not None:
                    raise RuntimeError("Game exited. See " + str(log_path))
                value = check()
                if value:
                    return value
                time.sleep(0.045)
            raise RuntimeError(error + f" (last pose {latest('player ')})")

        def pose():
            return wait_for(lambda: latest("player "), 3, "No player telemetry")

        def tap(key):
            x.key(key, True)
            time.sleep(0.08)
            x.key(key, False)

        def button(down):
            x.button(x.d, 1, int(down), 0)
            x.flush(x.d)

        def mouse(dx, dy, per_step=16):
            count = max(1, math.ceil(max(abs(dx), abs(dy)) / per_step))
            px = py = 0
            for step in range(1, count + 1):
                nx, ny = round(dx * step / count), round(dy * step / count)
                relative(x.d, nx - px, ny - py, 0)
                x.flush(x.d)
                time.sleep(0.016)
                px, py = nx, ny

        def aim(yaw, pitch, per_step=16):
            for _ in range(4):
                current = pose()
                d_yaw = math.remainder(yaw - current[3], math.tau)
                d_pitch = pitch - current[4]
                if abs(d_yaw) < 0.012 and abs(d_pitch) < 0.012:
                    return
                mouse(d_yaw / sensitivity[0], d_pitch / sensitivity[1], per_step)
                time.sleep(0.16)
            raise RuntimeError("Could not aim with relative mouse input")

        def walk_to(target, tolerance, careful=False):
            current = pose()
            aim(math.atan2(target[0] - current[0], target[1] - current[2]), current[4])
            if careful:
                x.key("Control_L", True)
            try:
                x.key("w", True)
                return wait_for(
                    lambda: (p if math.hypot(p[0] - target[0], p[2] - target[1]) < tolerance else None)
                    if (p := latest("player ")) else None,
                    22, f"Did not reach {target}",
                )
            finally:
                x.key("w", False)
                if careful:
                    x.key("Control_L", False)
                time.sleep(0.18)

        def note(name):
            p = pose()
            event = {"time": round(time.monotonic() - started, 3), "event": name, "player": p}
            events.append(event)
            print(event, flush=True)

        try:
            def active_window():
                candidate = x.find(app.pid)
                return candidate if candidate and "fps" in x.title(candidate).lower() else None
            window = wait_for(active_window, 100, "No rendered game window")
            course = wait_for(lambda: latest("player course "), 3, "No playable pond approach")
            x.focus(window, app.pid)
            x.click(window)
            time.sleep(0.35)
            tap("2")
            if args.view == "third-person":
                tap("c")
            first = pose()
            mouse(40, 40)
            time.sleep(0.2)
            second = pose()
            sensitivity = [(second[i] - first[i]) / 40 for i in [3, 4]]
            if any(abs(value) < 0.0001 for value in sensitivity):
                raise RuntimeError(f"Mouse could not be calibrated: {sensitivity}")
            sx, sz, wx, wz, rx, rz = course
            length = math.hypot(wx - sx, wz - sz)
            fx, fz = (wx - sx) / length, (wz - sz) / length
            inland = lambda distance: (wx - distance * fx, wz - distance * fz)
            inbound = math.atan2(fx, fz)
            aim(inbound, -0.26)
            time.sleep(1.0)
            recorder = subprocess.Popen([
                "ffmpeg", "-y", "-f", "x11grab", "-framerate", "30",
                "-video_size", f"{width}x{height}", "-window_id", str(window),
                "-draw_mouse", "0", "-i", os.environ["DISPLAY"], "-an",
                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "21",
                "-threads", "2", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(out / "take.mp4"),
            ], stdout=video_log, stderr=subprocess.STDOUT)
            started = time.monotonic()
            note("walk_to_pond")
            time.sleep(0.7)
            walk_to(inland(4.3), 0.28)
            note("slow_down_for_rocks")
            walk_to(inland(0.78), 0.18, careful=True)
            if any(line.startswith("player toe contact") for line in lines()):
                raise RuntimeError("Careful rock crossing should not trip")
            x.key("Control_L", True)
            aim(inbound, -1.04)
            note("dip_rim")
            button(True)
            try:
                for _ in range(6):
                    time.sleep(0.34)
                    if pose()[-2] == 20:
                        break
                    x.key("w", True)
                    time.sleep(0.13)
                    x.key("w", False)
                wait_for(lambda: pose()[-2] == 20, 2, "Rim did not fill the bowl at the pond")
                note("water_in_bowl")
                time.sleep(0.45)
            finally:
                button(False)
            aim(inbound, -0.43, per_step=8)
            time.sleep(0.48)
            x.key("Control_L", False)
            time.sleep(0.52)
            current = pose()
            return_yaw = math.atan2(rx - current[0], rz - current[2])
            aim(return_yaw, -0.41, per_step=9)
            time.sleep(0.45)
            before_trip = pose()
            if before_trip[-2:] != [20, 0]:
                raise RuntimeError("The bowl should stay full until the rock contact")
            note("carry_toward_rock")
            x.key("w", True)
            try:
                wait_for(lambda: any(line.startswith("player toe contact") for line in lines()), 7,
                         "The return step did not hit the rock")
                note("toe_hits_rock")
                time.sleep(0.48)
            finally:
                x.key("w", False)
            wait_for(lambda: pose()[-2] == 0, 3, "The stumble did not empty the bowl")
            note("bowl_empty")
            time.sleep(2.)
            if recorder.poll() is not None:
                raise RuntimeError("Recording process exited early")
            recorder.terminate()
            recorder.wait(timeout=15)
            x.request_close(window, app.pid)
            if app.wait(timeout=45):
                raise RuntimeError("Game failed on save")
            report = json.loads((out / "game.json").read_text())
            bowl = report["bowl"]
            if not bowl["conserved"] or bowl["spilled_units"] != 20 or bowl["units"]:
                raise RuntimeError(f"Trip or conservation check failed: {bowl}")
        finally:
            x.release_all()
            if recorder and recorder.poll() is None:
                recorder.terminate()
                recorder.wait(timeout=15)
            if app.poll() is None:
                if window:
                    x.request_close(window, app.pid)
                try:
                    app.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    app.terminate()
                    app.wait(timeout=10)
            x.close(x.d)
            (out / "events.json").write_text(json.dumps({
                "command": command, "binary_sha256": binary_sha256, "view": args.view,
                "events": events,
            }, indent=2) + "\n")


if __name__ == "__main__":
    main()
