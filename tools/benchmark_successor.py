#!/usr/bin/env python3
"""Paired old/successor CLI benchmark runner; see successor/README.md."""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import statistics
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
SCENES = [
    ("valley", 42, "temperate", "4250,200,6250,125,-5.14", "75"),
    ("river", 137, "arid", "1168,166,3580,51.34,-35.1", "75"),
    ("lake", 42, "temperate", "1428,680,8596,45,-21.4", "75"),
    ("heldout", 2027, "continental", "2200,650,4900,46.87,13.65", "75"),
]

def sha256(path):
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--old", type=Path, default=ROOT / "output/redesign/baseline/microvoxel-game")
    p.add_argument("--new", type=Path, default=ROOT / "successor/target/release/microvoxel-successor")
    p.add_argument("--output", type=Path, default=ROOT / "output/redesign/benchmarks")
    p.add_argument("--trials", type=int, default=5)
    p.add_argument("--mode", choices=("screenshots", "movement", "both"), default="both")
    p.add_argument("--renderer", choices=("mesh", "ray"), default="mesh")
    p.add_argument("--old-prefix", default="", help="Extra shell-quoted args preceding common CLI args")
    p.add_argument("--new-prefix", default="", help="Extra shell-quoted args preceding common CLI args")
    p.add_argument("--size", default="1920x1080")
    p.add_argument("--frames", type=int, default=120)
    p.add_argument("--haze", default="0")
    p.add_argument("--dry-run", action="store_true", help="Write manifest/commands; do not launch GPU binaries")
    a = p.parse_args()
    if a.trials < 1:
        p.error("--trials must be >= 1")
    old, new, out = (x.resolve() for x in (a.old, a.new, a.output))
    for binary in (old, new):
        if not binary.is_file() and not a.dry_run:
            p.error(f"binary not found: {binary}")
    out.mkdir(parents=True, exist_ok=True)
    entries = []
    common = ["--world", "procedural", "--headless", "--renderer", a.renderer,
              "--seed", "{seed}", "--climate", "{climate}", "--voxel-size", "1/16",
              "--size", a.size, "--no-hud", "--quality", "{quality}",
              "--native-radius", "6", "--terrain-memory", "256", "--tiles", "2048",
              "--haze", a.haze]
    for engine, binary, extra in (("old", old, shlex.split(a.old_prefix)), ("successor", new, shlex.split(a.new_prefix))):
        digest = sha256(binary) if binary.is_file() else None
        if a.mode in ("screenshots", "both"):
            for name, seed, climate, camera, quality in SCENES:
                for trial in range(1, a.trials + 1):
                    image = out / "captures" / engine / name / f"trial-{trial:02}.png"
                    argv = [str(binary), *extra, *[v.format(seed=seed, climate=climate, quality=quality) for v in common],
                            "--frames", str(a.frames), "--camera", camera, "--screenshot", str(image)]
                    entries.append(dict(engine=engine, kind="screenshot", scene=name, trial=trial,
                                        binary=str(binary), binary_sha256=digest, argv=argv, output=str(image)))
        if a.mode in ("movement", "both"):
            for trial in range(1, a.trials + 1):
                argv = [str(binary), *extra, "--stream-test"]
                entries.append(dict(engine=engine, kind="movement", scene="walk_sprint_turn", trial=trial,
                                    binary=str(binary), binary_sha256=digest, argv=argv, output=None))
    # Pair each scene/trial, alternating engine order to reduce thermal bias.
    entries.sort(key=lambda e: (e["kind"], e["scene"], e["trial"],
                               (e["engine"] == "successor") ^ (e["trial"] % 2 == 0)))
    manifest = {"schema": 1, "created_unix": int(time.time()), "dry_run": a.dry_run,
                "settings": {"trials": a.trials, "mode": a.mode, "renderer": a.renderer,
                             "resolution": a.size, "frames": a.frames, "haze": a.haze, "voxel_size": "1/16", "quality": 75,
                             "native_radius": 6, "terrain_memory_mib": 256},
                "host": {"platform": platform.platform(), "python": platform.python_version(),
                         "processor": platform.processor(), "cpu_count": os.cpu_count()},
                "gpu": "record GPU model, driver, API/backend, and power mode in run-notes.json",
                "trials": entries}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    if a.dry_run:
        print(f"Wrote {out / 'manifest.json'} ({len(entries)} commands); binaries hashed; nothing launched.")
        return
    rows = []
    for item in entries:
        Path(item["output"]).parent.mkdir(parents=True, exist_ok=True) if item["output"] else None
        log_path = out / "logs" / item["engine"] / f"{item['kind']}-{item['scene']}-trial-{item['trial']:02}.log"
        log_path.parent.mkdir(parents=True, exist_ok=True)
        start = time.perf_counter()
        with log_path.open("w") as log:
            result = subprocess.run(item["argv"], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        if item["kind"] == "movement":
            source = ROOT / "output/streaming"
            destination = out / "movement" / item["engine"] / f"trial-{item['trial']:02}"
            if source.exists():
                shutil.copytree(source, destination, dirs_exist_ok=True)
        elapsed = (time.perf_counter() - start) * 1000
        rows.append({"engine": item["engine"], "kind": item["kind"], "scene": item["scene"],
                     "trial": item["trial"], "process_wall_ms": f"{elapsed:.3f}",
                     "returncode": result.returncode, "log": str(log_path), "output": item["output"] or ""})
        if result.returncode:
            raise SystemExit(f"command failed ({result.returncode}); see {log_path}")
    with (out / "trials.csv").open("w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=rows[0].keys()); w.writeheader(); w.writerows(rows)
    groups = {}
    for row in rows:
        groups.setdefault((row["engine"], row["kind"], row["scene"]), []).append(float(row["process_wall_ms"]))
    with (out / "summary.csv").open("w", newline="") as f:
        fields = ("engine", "kind", "scene", "trials", "median_process_wall_ms", "p95_process_wall_ms")
        w = csv.DictWriter(f, fieldnames=fields); w.writeheader()
        for (engine, kind, scene), values in groups.items():
            ordered = sorted(values)
            p95 = ordered[round((len(ordered) - 1) * .95)]
            w.writerow({"engine": engine, "kind": kind, "scene": scene, "trials": len(values),
                        "median_process_wall_ms": f"{statistics.median(values):.3f}", "p95_process_wall_ms": f"{p95:.3f}"})
    (out / "run-notes.json").write_text(json.dumps({"gpu": "", "driver": "", "backend_api": "",
        "power_mode": "", "ambient_conditions": "", "notes": "process_wall_ms measures whole CLI invocation, not frame time"}, indent=2) + "\n")
    print(f"Completed {len(rows)} process trials. Review logs and run-notes.json; process wall time is not frame time.")

if __name__ == "__main__":
    main()
