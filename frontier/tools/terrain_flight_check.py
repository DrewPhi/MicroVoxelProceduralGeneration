#!/usr/bin/env python3
"""Matched 1080p terrain comparisons on the current desktop GPU."""
import argparse
import json
import shlex
import subprocess
import time
from pathlib import Path

FRONTIER = Path(__file__).resolve().parents[1]
VIEWS = {
    "alpine": (19, "-1050,1280,-550", "1300,700,1200"),
    "desert": (997, "620,400,-2100", "-860,170,-750"),
    "plains": (83, "-4500,160,1300", "-2950,30,2900"),
    "coast": (40961, "-3300,250,800", "-2400,30,3300"),
    "heldout": (70231, "1000,360,4700", "4860,460,7120"),
}


def run(binary, mode, preset, name, args, extra=(), view=None):
    seed, eye, target = VIEWS[name] if view is None else view
    tag = args.output / f"{name}-{mode}"
    report = tag.with_suffix(".json")
    command = [
        str(binary), "--preset", preset, "--seed", str(seed), "--walk", "off",
        "--vegetation", "off", "--width", "1920", "--height", "1080", "--msaa", "1",
        "--capture-far", "6000", "--no-adaptive-disk-cache", "--eye", eye, "--target", target,
        "--warmup", "15", "--report", str(report), "--capture-on-exit", str(tag.with_suffix(".png")),
        *extra,
    ]
    started = time.monotonic()
    with tag.with_suffix(".log").open("w") as log:
        log.write(shlex.join(command) + "\n")
        log.flush()
        subprocess.run(command, cwd=FRONTIER, stdout=log, stderr=subprocess.STDOUT, check=True)
    raw = json.loads(report.read_text())
    tag.with_suffix(".linear_rgb.f32").unlink(missing_ok=True)
    full = raw["gpu_full_render"]
    frames = raw["frames"]
    results = {
        "view": name,
        "mode": mode,
        "seed": seed,
        "adapter": raw["adapter"],
        "renderer": raw["representation"]["name"],
        "gpu_samples": full["samples"],
        "gpu_ms": [full[f"p{q}_ms"] for q in (50, 95, 99)],
        "frame_ms": [frames[f"p{q}_ms"] for q in (50, 95, 99)],
        "process_peak_mib": round(raw["process_memory"]["peak_rss_bytes"] / 1048576, 1),
        "wall_seconds_including_capture": round(time.monotonic() - started, 2),
        "rolling_cache": raw.get("rolling_cache"),
    }
    if raw.get("benchmark_failed") or raw.get("benchmark_interrupted"):
        raise RuntimeError(f"Frontier did not complete {tag.name}")
    print(json.dumps(results), flush=True)
    return results


def contact_sheet(directory, view_names):
    from PIL import Image, ImageDraw

    columns = ("before", "landforms", "canopy")
    tile_w, tile_h = 640, 360
    title = 29
    sheet = Image.new("RGB", (tile_w * len(columns), (tile_h + title) * len(view_names)), "#121c24")
    draw = ImageDraw.Draw(sheet)
    for row, name in enumerate(view_names):
        for col, variant in enumerate(columns):
            tag = directory / f"{name}-{variant}.png"
            x, y = col * tile_w, row * (tile_h + title)
            if tag.exists():
                with Image.open(tag) as source:
                    sheet.paste(source.convert("RGB").resize((tile_w, tile_h), Image.Resampling.LANCZOS),
                                (x, y + title))
            draw.text((x + 12, y + 8), f"{name.capitalize()} / {variant}", fill="white")
    sheet.save(directory / "comparison.jpg", quality=91)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=lambda x: Path(x).resolve(),
                        help="Frozen release binary from before the changes")
    parser.add_argument("--candidate", default=FRONTIER / "target/release/frontier",
                        type=lambda x: Path(x).resolve())
    parser.add_argument("--output", default=FRONTIER / "output/terrain-checklist/flight",
                        type=lambda x: Path(x).resolve())
    parser.add_argument("--view", choices=VIEWS, action="append")
    parser.add_argument("--motion", action="store_true")
    parser.add_argument("--motion-only", action="store_true")
    parser.add_argument("--motion-case", action="append",
                        choices=["approach", "bank", "walk", "light"])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    views = args.view or list(VIEWS)
    with (args.output / "runs.jsonl").open("a") as journal:
        def record(*run_args, **kwargs):
            result = run(*run_args, **kwargs)
            journal.write(json.dumps(result) + "\n")
            journal.flush()

        for i, name in enumerate([] if args.motion_only else views):
            pairs = [
                (args.baseline, "before", "seeded-world"),
                (args.candidate, "landforms", "landscapes"),
            ]
            if i % 2:
                pairs.reverse()
            for binary, mode, preset in pairs:
                record(binary, mode, preset, name, args, ("--frames", "120"))
            record(args.candidate, "canopy", "landscapes", name, args,
                   ("--vegetation", "on", "--frames", "240"))

        if args.motion or args.motion_only or args.motion_case:
            cases = args.motion_case or ("approach", "bank", "walk", "light")
            parameters = ("--route-clock", "frame", "--route-step", "0.0166667")
            if "approach" in cases:
                approach = (*parameters, "--route", "fly", "--route-speed", "90", "--frames", "480")
                record(args.candidate, "approach", "landscapes", "alpine", args, approach)
                record(args.baseline, "approach-before", "seeded-world", "alpine", args, approach)
            if "bank" in cases:
                record(args.candidate, "bank", "landscapes", "coast", args,
                       (*parameters, "--route", "bank-return", "--route-speed", "72", "--frames", "560",
                        "--weather", "regional", "--weather-hour", "13", "--vegetation", "on"))
            if "walk" in cases:
                record(args.candidate, "walk", "landscapes", "plains", args,
                       (*parameters, "--route", "walk-stop-resume", "--route-speed", "2.5",
                        "--frames", "360", "--vegetation", "on"),
                       view=(83, "-4200,22.7,1800", "-4020,14,2070"))
            if "light" in cases:
                record(args.candidate, "dawn", "landscapes", "heldout", args,
                       ("--sun-elevation", "7", "--overcast", "0.14", "--frames", "120"))
                record(args.candidate, "overcast", "landscapes", "heldout", args,
                       ("--sun-elevation", "42", "--overcast", "0.82", "--cloud-coverage", "0.76", "--frames", "120"))
    complete = [name for name in VIEWS if (args.output / f"{name}-canopy.png").exists()]
    if complete:
        contact_sheet(args.output, complete)



if __name__ == "__main__":
    main()
