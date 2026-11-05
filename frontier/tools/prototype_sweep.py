#!/usr/bin/env python3
"""Bounded, sequential GPU comparisons. Run after cargo build --release --locked.

Requires an active display for timed window runs; captures are headless. Raw
artifacts stay under ignored output/. Never runs GPU comparisons concurrently.
"""
import argparse
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / "target/release/frontier"
OUT = ROOT / "output/prototype-3-5"


def run(name, flags, seconds=None):
    args = [str(BIN), "--seed", "7", "--width", "1920", "--height", "1080"] + flags
    if seconds is None:
        args += ["--capture", str(OUT / (name + ".png"))]
    else:
        args += ["--seconds", str(seconds), "--warmup", "60", "--uncapped",
                 "--route-clock", "realtime", "--report", str(OUT / (name + ".json"))]
    with (OUT / (name + ".log")).open("w") as log:
        subprocess.run(args, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
    if seconds is None:
        print(name + ": captured", flush=True)
    else:
        report = json.loads((OUT / (name + ".json")).read_text())
        print(name + ": " + json.dumps({"frame": report["frames"],
              "gpu": report.get("gpu_render_pass")}), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=["captures", "benchmarks", "edits", "all"], default="all")
    args = parser.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    if args.mode in ("captures", "all"):
        for camera in ("valley", "cave", "bridge"):
            for renderer in ("mesh", "brick", "brick-skip"):
                run(f"fixture-{camera}-{renderer}", ["--scene", "fixture", "--camera", camera,
                    "--renderer", renderer, "--msaa", "1"])
        for camera in ("topology", "cave", "bridge"):
            run(f"topology-{camera}", ["--scene", "hybrid-topology", "--camera", camera, "--msaa", "4"])
        for camera in ("valley", "aerial"):
            for layout in ("clipmap", "quadtree"):
                run(f"surface-{camera}-{layout}", ["--scene", "fbm", "--camera", camera,
                    "--surface-layout", layout, "--terrain-static", "--msaa", "1"])
    if args.mode in ("edits", "all"):
        for scene, camera, renderer, msaa in (
                ("hybrid-topology", "topology", "mesh", "4"),
                ("fixture", "valley", "mesh", "1"),
                ("fixture", "valley", "brick", "1"),
                ("fixture", "valley", "brick-skip", "1")):
            name = "edit-check-" + scene + "-" + renderer
            run(name, ["--scene", scene, "--camera", camera,
                "--renderer", renderer, "--msaa", msaa, "--edit-test",
                "--capture-on-exit", str(OUT / (name + ".png"))], 30)
            edits = json.loads((OUT / (name + ".json")).read_text())["edits"]
            if edits["published_count"] != 2 or not edits["test_restored"]:
                raise RuntimeError(name + ": add/restore did not finish; inspect timing log")
    if args.mode in ("benchmarks", "all"):
        for camera in ("valley", "cave"):
            for renderer in ("mesh", "brick", "brick-skip"):
                run(f"bench-{camera}-{renderer}", ["--scene", "fixture", "--camera", camera,
                    "--renderer", renderer, "--msaa", "1"], 6)
        for layout in ("clipmap", "quadtree"):
            run(f"bench-surface-{layout}", ["--scene", "fbm", "--camera", "aerial",
                "--surface-layout", layout, "--terrain-static", "--msaa", "1"], 6)
        for name, route, speed, seconds in (("walk", "stop-resume", 3, 15),
                ("sprint", "stop-resume", 15, 15), ("return", "turn-return", 15, 8),
                ("flight", "fly", 80, 10)):
            run("travel-" + name, ["--scene", "hybrid", "--route", route,
                "--route-speed", str(speed), "--msaa", "4"], seconds)
        run("teleport", ["--scene", "hybrid", "--teleport-test", "--msaa", "4"], 20)


if __name__ == "__main__":
    main()
