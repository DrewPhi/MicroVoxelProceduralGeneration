#!/usr/bin/env python3
"""Ten reproducible procedural views, including matched river/lake LOD pairs.

Run after building the native release binary. No terrain/settings are changed.
Outputs and complete command/log manifests remain under ignored output/.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
SCENES = [
    ("01_valley", 42, "temperate", [4250, 200, 6250, 125, -5.14], 75, False,
     "Broad landforms, material transitions, distant silhouettes and depth."),
    ("02_river_high", 137, "arid", [1168, 166, 3580, 51.34, -35.1], 95, False,
     "River continuity, bends, banks and tributaries at high requested detail."),
    ("03_river_low", 137, "arid", [1168, 166, 3580, 51.34, -35.1], 0, False,
     "Exactly the river camera from 02, with only LOD quality changed."),
    ("04_river_bank", 137, "arid", [1230, 100, 3670, 29, 5], 75, True,
     "Player-height dry bank: water height relative to surrounding ground."),
    ("05_lake_high", 42, "temperate", [1428, 680, 8596, 45, -21.4], 95, False,
     "Deep-basin lake: shoreline, surrounding slopes and water joins."),
    ("06_lake_low", 42, "temperate", [1428, 680, 8596, 45, -21.4], 0, False,
     "Exactly the lake camera from 05, with only LOD quality changed."),
    ("07_tropical_hills", 909, "tropical", [2080, 210, 4856, 51.34, -25.3], 75, False,
     "Humid low hills: material coherence and small-scale terrain variety."),
    ("08_arid_cliffs", 137, "arid", [7480, 543, 5280, 62.02, 37.9], 75, False,
     "Steep dry mountain: rock relief, repeated patterns and silhouette."),
    ("09_continental", 2027, "continental", [2200, 650, 4900, 46.87, 13.65], 75, False,
     "Held-out seed: mountain/lake relationships and generalization."),
    ("10_plain_ground", 42, "temperate", [4345, 170, 6725, 67, -8], 75, True,
     "Player-height plain: voxel color, grass/soil patches and horizon scale."),
]

RIVER_CHECKS = [
    ("11_valid_river_high", 137, "arid", [2768,120,2108,51.34,-35.1], 95, False,
     "Additional wet-channel regression; original invalid spillway can become dry."),
    ("12_valid_river_low", 137, "arid", [2768,120,2108,51.34,-35.1], 0, False,
     "Same additional channel camera, low detail."),
    ("13_humid_river_high", 909, "tropical", [3712,165,9312,45,-23], 95, False,
     "Humid river-to-lake junction and adjoining dry banks."),
    ("14_humid_river_low", 909, "tropical", [3712,165,9312,45,-23], 0, False,
     "Same humid river-to-lake camera, low detail."),
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "output/terrain_sweep")
    parser.add_argument("--game", type=Path, default=ROOT / "native/target/release/microvoxel-game")
    parser.add_argument("--only", nargs="*", help="Optional scene IDs to recapture")
    parser.add_argument("--river-checks", action="store_true", help="Include four additional wet-channel LOD checks")
    parser.add_argument("--renderer", choices=["mesh", "ray"], default="mesh")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.game = args.game.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    binary_sha256 = hashlib.sha256(args.game.read_bytes()).hexdigest()
    manifest = []
    for name, seed, climate, camera, quality, ground, purpose in SCENES + (RIVER_CHECKS if args.river_checks else []):
        image = args.output / f"{name}.png"
        command = [str(args.game), "--world", "procedural", "--headless",
                   "--renderer", args.renderer, "--seed", str(seed), "--climate", climate,
                   "--voxel-size", "1/16", "--size", "1280x720", "--no-hud",
                   "--quality", str(quality), "--native-radius", "6",
                   "--terrain-memory", "256", "--tiles", "2048", "--haze", "0.65",
                   "--frames", "3", "--camera", ",".join(map(str, camera)),
                   "--screenshot", str(image)]
        if ground:
            command.append("--camera-ground")
        manifest.append(dict(binary_sha256=binary_sha256, renderer=args.renderer, id=name, seed=seed, climate=climate, purpose=purpose,
                             camera_requested_m_degrees=camera, ground=ground,
                             quality=quality, command=command, image=image.name))
    (args.output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    for scene in manifest:
        if args.only and scene["id"] not in args.only:
            continue
        print(f"Capturing {scene['id']} ...", flush=True)
        start = time.monotonic()
        with (args.output / f"{scene['id']}.log").open("w") as log:
            subprocess.run(scene["command"], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                           check=True, timeout=360)
        print(f"Saved {scene['image']} ({time.monotonic() - start:.1f}s)", flush=True)


if __name__ == "__main__":
    main()
