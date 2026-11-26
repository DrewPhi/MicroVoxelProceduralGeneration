#!/usr/bin/env python3
"""Repeatable camera comparisons for the procedural terrain polish study."""

import argparse
import json
import shlex
import subprocess
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]


def capture(args, views):
    args.output.mkdir(parents=True, exist_ok=True)
    base = [
        args.binary, "--preset", args.preset, "--terrain-detail", "off",
        "--terrain-mid-detail", "on", "--terrain-mid-spacing", str(args.spacing),
        "--naadf", "off", "--width", "1440", "--height", "960",
        "--msaa", "1", "--capture-far", "6000",
    ]
    for view in views:
        fixed_camera = "eye" in view
        airborne = "altitude_m" in view
        for variant in args.variants:
            if fixed_camera:
                camera = ["--eye", view["eye"], "--target", view["target"], "--walk", "off"]
            elif airborne:
                camera = ["--walk", "off", "--flight-view", "chase"]
                if view["altitude_m"] is not None:
                    camera.extend(["--flight-altitude", str(view["altitude_m"])])
                if "heading_degrees" in view:
                    camera.extend(["--flight-heading", str(view["heading_degrees"])])
            else:
                camera = ["--walk", "on"]
            if variant == "gray":
                appearance = [
                    "--terrain-gray", "--beauty", "off", "--atmosphere", "off",
                    "--terrain-shadows", "off", "--vegetation", "off",
                ]
            else:
                appearance = [
                    "--vegetation", "on", "--sun-elevation", str(args.sun_elevation),
                    "--sun-azimuth", str(args.sun_azimuth), "--cloud-coverage", str(args.clouds),
                    "--overcast", str(args.overcast),
                ]
                if fixed_camera:
                    camera.extend(["--flight-view", "chase"])
            image = args.output / f"{view['name']}-{variant}.png"
            command = [*base, "--seed", str(view["seed"]), *camera, *appearance, *args.override,
                       "--capture", image]
            with image.with_suffix(".log").open("w") as log:
                print(shlex.join(str(part) for part in command), file=log, flush=True)
                subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
            image.with_suffix(".linear_rgb.f32").unlink(missing_ok=True)
        print(f"{view['name']} seed {view['seed']}", flush=True)


def compare(args, views):
    for variant in args.variants:
        included = [
            v for v in views if all(
                (directory / f"{v['name']}-{variant}.png").exists()
                for directory in (args.before, args.output)
            )
        ]
        if not included:
            continue
        width, height, bar = 700, 467, 32
        picture = Image.new("RGB", (2 * width, len(included) * (height + bar)), "#141e28")
        draw = ImageDraw.Draw(picture)
        for i, view in enumerate(included):
            for j, directory in enumerate((args.before, args.output)):
                filename = directory / f"{view['name']}-{variant}.png"
                with Image.open(filename) as src:
                    cell = src.convert("RGB").resize((width, height), Image.Resampling.LANCZOS)
                x, y = j * width, i * (height + bar)
                picture.paste(cell, (x, y + bar))
                mode = args.labels[j]
                pose = "" if "eye" in view else " / automatic spawn"
                draw.text((x + 13, y + 9), f"{view['name']} / seed {view['seed']} / {mode}{pose}",
                          fill="white")
        picture.save(args.output / f"comparison-{variant}.jpg", quality=94, subsampling=0)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/frontier")
    parser.add_argument("--preset", default="eroded")
    parser.add_argument("--views", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--before", type=Path)
    parser.add_argument("--labels", nargs=2, default=("Landforms", "Eroded"))
    parser.add_argument("--only", nargs="+")
    parser.add_argument("--variants", nargs="+", choices=("gray", "color"),
                        default=("gray", "color"))
    parser.add_argument("--spacing", type=float, default=3.)
    parser.add_argument("--sun-elevation", type=float, default=33.)
    parser.add_argument("--sun-azimuth", type=float, default=42.)
    parser.add_argument("--clouds", type=float, default=.20)
    parser.add_argument("--overcast", type=float, default=.10)
    parser.add_argument("--compare-only", action="store_true")
    parser.add_argument("--override", nargs=argparse.REMAINDER, default=[])
    args = parser.parse_args()
    views = json.loads(args.views.read_text())
    if args.only:
        views = [v for v in views if v["name"] in args.only]
    if not views:
        parser.error("no matching views")
    if not args.compare_only:
        capture(args, views)
    if args.before:
        compare(args, views)


if __name__ == "__main__":
    main()
