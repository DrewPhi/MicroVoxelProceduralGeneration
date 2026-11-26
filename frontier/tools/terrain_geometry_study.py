#!/usr/bin/env python3
"""Matched gray terrain captures and an independent native-column reference."""
import argparse
import json
import shlex
import subprocess
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

from reference_compare import ray_distance

ROOT = Path(__file__).resolve().parents[1]
FRONTIER = ROOT / "target/release/frontier"
ORACLE = ROOT / "target/release/landscape"
WIDTH, HEIGHT = 640, 360
TARGET = np.array((1011.5, 734., 805.8))
DIRECTION = np.array((.81, -.43, .40))
DIRECTION /= np.linalg.norm(DIRECTION)
CACHE_CONFIGS = {
    "compact": [],
    "extended": ["--terrain-mid-detail", "on"],
    "strips": ["--terrain-mid-detail", "on", "--terrain-mid-strips", "on"],
    "one-metre": ["--terrain-mid-detail", "on", "--terrain-mid-strips", "on", "--terrain-mid-spacing", "1"],
    "sub-metre": ["--terrain-mid-detail", "on", "--terrain-mid-strips", "on", "--terrain-mid-spacing", ".75"],
    "bounded": ["--terrain-mid-detail", "on", "--terrain-mid-strips", "on", "--terrain-mid-error", "1"],
    "tiles-10cm": ["--terrain-mid-detail", "on", "--terrain-mid-strips", "on", "--terrain-mid-spacing", ".75",
                   "--terrain-mid-tile-error", ".10"],
    "tiles-20cm": ["--terrain-mid-detail", "on", "--terrain-mid-strips", "on", "--terrain-mid-spacing", ".75",
                   "--terrain-mid-tile-error", ".20"],
    "two-metres": ["--terrain-mid-detail", "on", "--terrain-mid-spacing", "2"],
    "four-metres": ["--terrain-mid-detail", "on", "--terrain-mid-spacing", "4"],
    "boundary": ["--terrain-mid-detail", "on", "--terrain-mid-boundary-ascent", "on"],
    "default": ["--terrain-detail", "off"],
    "extended-standard-near": ["--terrain-detail", "off", "--terrain-mid-detail", "on"],
    "two-metres-standard-near": ["--terrain-detail", "off", "--terrain-mid-detail", "on",
                                 "--terrain-mid-spacing", "2"],
    "three-metres": ["--terrain-mid-detail", "on", "--terrain-mid-spacing", "3"],
    "three-metres-standard-near": ["--terrain-detail", "off", "--terrain-mid-detail", "on",
                                   "--terrain-mid-spacing", "3"],
    "selective-15cm": ["--terrain-detail", "off", "--terrain-mid-detail", "on",
                      "--terrain-mid-tile-error", ".15"],
    "selective-30cm": ["--terrain-detail", "off", "--terrain-mid-detail", "on",
                      "--terrain-mid-tile-error", ".30"],
}
CACHE_ARGS = ["--preset", "landforms", "--terrain-detail", "on", "--naadf", "off",
              "--vegetation", "off", "--width", "1920", "--height", "1080", "--msaa", "1",
              "--capture-far", "6000", "--fov-y", "60"]


def vec(values):
    return ",".join(f"{x:.9f}" for x in values)


def shot(command, output):
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w") as log:
        print(shlex.join(str(s) for s in command), file=log, flush=True)
        subprocess.run(command, cwd=ROOT, check=True, stdout=log, stderr=subprocess.STDOUT)


def sheet(rows, path, width, height):
    columns = len(rows[0])
    strip = height + 31
    composite = Image.new("RGB", (width * columns, strip * len(rows)), "#131c24")
    labels = ImageDraw.Draw(composite)
    for y, row in enumerate(rows):
        for x, (title, filename) in enumerate(row):
            with Image.open(filename) as img:
                composite.paste(img.convert("RGB").resize((width, height), Image.Resampling.LANCZOS),
                                (x * width, y * strip + 31))
            labels.text((x * width + 13, y * strip + 9), title, fill="white")
    composite.save(path, quality=94)


def close(out, bank=False):
    seed = 70231 if bank else 19
    name = "riverbank" if bank else "cliff"
    clip = 100 if bank else 205
    reference_center = "3470,4580" if bank else "988,782"
    reference_extent = 256 if bank else 512
    cache = out / f"{name}-reference-columns.bin"
    gray = [] if bank else ["--terrain-gray"]
    comparison = []
    scores = []
    viewport = ["--width", str(WIDTH), "--height", str(HEIGHT), "--fov-y", "60"]
    for distance in ((52,) if bank else (20, 60, 150)):
        directory = out / f"{name}-{distance}m"
        eye, target = ((3440., 53., 4558.), (3484., 37., 4582.)) if bank else (TARGET-DIRECTION*distance, TARGET)
        cameras = ["--eye", vec(eye), "--target", vec(target)]
        reference = directory / "native"
        shot([ORACLE, "--terrain-recipe", "landforms", "--seed", str(seed), *(["--no-tint"] if bank else ["--gray"]), "--spp", "4",
              "--threads", "8", "--extent", str(reference_extent), "--center", reference_center, "--far", str(clip),
              "--cache", cache, "--output", reference, *viewport,
              *cameras], directory / "native.log")
        row = []
        coverage_ref = np.fromfile(reference / "coverage.f32", dtype="<f4").reshape(HEIGHT, WIDTH)
        depth_ref = np.fromfile(reference / "depth.f32", dtype="<f4").reshape(HEIGHT, WIDTH)
        for filename, title, toggle in [("original", "Previous rolling cache", "off"),
                                        ("adaptive", "Projected detail", "on")]:
            png = directory / f"{filename}.png"
            shot([FRONTIER, "--preset", "landforms", "--seed", str(seed), "--terrain-detail", toggle,
                  "--walk", "off", *gray, "--beauty", "off", "--vegetation", "off",
                  "--naadf", "off", "--atmosphere", "off", "--terrain-shadows", "off",
                  "--capture-linear", "--msaa", "1", "--capture-far", str(clip),
                  "--capture", png, *viewport, *cameras], directory / f"{filename}.log")
            data = json.loads(png.with_suffix(".capture.json").read_text())
            ndc = np.fromfile(png.with_suffix(".depth.f32"), dtype="<f4").reshape(HEIGHT, WIDTH)
            z = ray_distance(ndc, data)
            common = (coverage_ref >= .999) & (ndc < 1) & np.isfinite(depth_ref) & (depth_ref < .95 * clip)
            error = np.abs(z[common] - depth_ref[common])
            if not error.size:
                raise RuntimeError(f"No interior reference hits for {png}")
            scores.append(dict(
                distance_m=distance, mode=filename, common_pixels=int(error.size),
                absolute_depth_p50_m=float(np.percentile(error, 50)),
                absolute_depth_p95_m=float(np.percentile(error, 95)),
            ))
            row.append((f"{distance} m setup / {title}", png))
        row.append((f"{distance} m setup / Independent 6.25 cm reference", reference / "color.png"))
        comparison.append(row)
    sheet(comparison, out / f"{name}-comparison.jpg", WIDTH, HEIGHT)
    (out / f"{name}-depth.json").write_text(json.dumps(scores, indent=2) + "\n")
    (cache).unlink(missing_ok=True)
    print(json.dumps(scores), flush=True)


def gallery(out):
    views = [
        ("Alpine", "19", "600,936,580", "1150,758,1090"),
        ("Desert", "997", "620,400,-2100", "-860,170,-750"),
        ("Coast", "40961", "-3300,250,800", "-2400,30,3300"),
        ("Held-out seed", "70231", "1000,360,4700", "4860,460,7120"),
    ]
    rows = []
    for label, seed, eye, target in views:
        row = []
        for name, extra in [
            ("Gray landforms", ["--terrain-gray", "--atmosphere", "off", "--vegetation", "off"]),
            ("Finished landscape", ["--atmosphere", "on", "--vegetation", "on"]),
        ]:
            png = out / f"gallery-{seed}-{'gray' if extra[0] == '--terrain-gray' else 'finished'}.png"
            shot([FRONTIER, "--preset", "landforms", "--seed", seed, "--walk", "off", "--terrain-detail", "on",
                  "--width", "1280", "--height", "720", "--msaa", "1", "--capture-far", "6000",
                  "--sun-elevation", "27", "--sun-azimuth", "40", "--cloud-coverage", ".18",
                  "--eye", eye, "--target", target, "--capture", png, *extra], png.with_suffix(".log"))
            row.append((f"{label} seed {seed} / {name}", png))
        rows.append(row)
    sheet(rows, out / "landforms-gallery.jpg", 800, 450)


def source_comparison(out):
    row = []
    for preset, title in (("landscapes", "Landscapes / original heights"),
                          ("landforms", "Landforms / revised heights")):
        png = out / f"source-{preset}.png"
        shot([FRONTIER, "--preset", preset, "--seed", "19", "--walk", "off",
              "--terrain-detail", "on", "--terrain-gray", "--beauty", "off",
              "--naadf", "off", "--vegetation", "off", "--atmosphere", "off",
              "--terrain-shadows", "off", "--msaa", "1", "--width", "1066",
              "--height", "600", "--fov-y", "60", "--capture-far", "205",
              "--eye", vec(TARGET-DIRECTION*20), "--target", vec(TARGET),
              "--capture", png], png.with_suffix(".log"))
        row.append((title, png))
    sheet([row], out / "canonical-relief.jpg", 800, 450)


def timing(out):
    reports = []
    for run, mode in enumerate(("off", "on", "on", "off"), start=1):
        tag = out / f"{run}-{mode}"
        report = tag.with_suffix(".json")
        shot([FRONTIER, "--preset", "landforms", "--seed", "19",
              "--terrain-detail", mode, "--walk", "off", "--vegetation", "off",
              "--width", "1920", "--height", "1080", "--msaa", "1",
              "--capture-far", "6000", "--fov-y", "60", "--eye", "600,936,580",
              "--target", "1150,758,1090", "--route", "fly", "--route-clock", "frame",
              "--route-step", "0.0166667", "--route-speed", "80", "--warmup", "24",
              "--frames", "320", "--report", report], tag.with_suffix(".log"))
        data = json.loads(report.read_text())
        if data["benchmark_failed"] or data["benchmark_interrupted"]:
            raise RuntimeError(f"Benchmark did not complete {tag.name}")
        reports.append(dict(detail=mode, adapter=data["adapter"], frame=data["frames"],
                            gpu=data["gpu_full_render"], rolling=data["rolling_cache"]))
        print(f"{tag.name}: {data['gpu_full_render']['p95_ms']:.2f} ms GPU p95", flush=True)
    (out / "comparison.json").write_text(json.dumps(reports, indent=2) + "\n")


def cache_images(out, configurations, ridge=False):
    name = "ridge" if ridge else "hill"
    reference = out / f"native-{name}"
    if ridge:
        seed, eye, target = "19", "600,936,580", "1150,758,1090"
        crop = (800, 440, 1120, 625)
        clip_lo, clip_hi = 510, 970
    else:
        seed, eye = "12611535567061277817", "1536.0,189.7625,-1024.0"
        target = "1536.7069546580315,189.78324158005415,-1024.7069546580315"
        crop = (0, 370, 444, 524)
        clip_lo, clip_hi = 290, 535
    names = (*configurations, "native")
    x0, y0, x1, y1 = crop
    shape = (y1-y0, x1-x0)
    depth = np.fromfile(reference / "depth.f32", dtype="<f4").reshape(shape)
    coverage = np.fromfile(reference / "coverage.f32", dtype="<f4").reshape(shape)
    interior = (coverage >= .999) & (depth > clip_lo) & (depth < clip_hi)
    rows, stats = [], []
    for mode in names:
        png = out / f"{name}-{mode}.png"
        if mode == "native":
            image = Image.open(reference / "color.png").convert("RGB")
        else:
            shot([FRONTIER, *CACHE_ARGS, *CACHE_CONFIGS[mode], "--seed", seed,
                  "--eye", eye, "--target", target, "--walk", "off" if ridge else "on", "--terrain-gray",
                  "--beauty", "off", "--atmosphere", "off", "--capture-linear",
                  "--capture", png], png.with_suffix(".log"))
            metadata = json.loads(png.with_suffix(".capture.json").read_text())
            ndc = np.fromfile(png.with_suffix(".depth.f32"), dtype="<f4").reshape((1080, 1920))
            distances = ray_distance(ndc, metadata)[y0:y1, x0:x1]
            mask = ndc[y0:y1, x0:x1] < 1
            common = interior & mask & (distances > clip_lo) & (distances < clip_hi)
            e = np.abs(distances[common] - depth[common])
            stats.append(dict(mode=mode, common_pixels=int(common.sum()), reference_pixels=int(interior.sum()),
                              unsupported_reference_pixels=int((interior & ~common).sum()),
                              median_error_m=float(np.percentile(e, 50)), p95_error_m=float(np.percentile(e, 95))))
            image = Image.open(png).convert("RGB").crop(crop)
            png.with_suffix(".linear_rgb.f32").unlink(missing_ok=True)
        cropped = out / f"{name}-crop-{mode}.png"
        image.save(cropped)
        rows.append((mode, cropped))
    sheet([rows[:(len(rows)+1)//2], rows[(len(rows)+1)//2:]],
          out/f"cache-{name}.jpg", shape[1], shape[0])
    (out/f"{name}-metrics.json").write_text(json.dumps(stats, indent=2) + "\n")
    print(json.dumps(stats), flush=True)


def cache_route(out, configurations, walking=False, repetitions=1):
    route = "walk" if walking else "fly"
    scene = (["--seed", "12611535567061277817", "--walk", "on", "--route", "walk-stop-resume",
              "--route-speed", "3"] if walking else
             ["--seed", "19", "--walk", "off", "--eye", "600,936,580", "--target", "1150,758,1090",
              "--route", "fly", "--route-speed", "80"])
    samples = []
    for repetition in range(repetitions):
        order = configurations[::-1] if repetition % 2 else configurations
        for mode in order:
            tag = out/f"{mode}-{route}-{repetition+1}"
            report = tag.with_suffix(".json")
            shot([FRONTIER, *CACHE_ARGS, *CACHE_CONFIGS[mode], *scene, "--route-clock", "frame",
                  "--route-step", "0.0166667", "--warmup", "24", "--frames", "320",
                  "--report", report], tag.with_suffix(".log"))
            data = json.loads(report.read_text())
            if data["benchmark_failed"] or data["benchmark_interrupted"]:
                raise RuntimeError(f"Benchmark did not complete {tag.name}")
            entry = dict(mode=mode, route=route, run=repetition+1, report=report.name,
                         render=data["gpu_full_render"], frame=data["frames"], cache=data["rolling_cache"])
            samples.append(entry)
            print(f"{tag.name}: GPU p95 {entry['render']['p95_ms']:.2f} ms, "
                  f"frame p95 {entry['frame']['p95_ms']:.2f} ms, "
                  f"rolling {entry['cache']['gpu_bytes']/1048576:.1f} MiB", flush=True)
    (out/f"cache-{route}-summary.json").write_text(json.dumps(samples, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("close", "riverbank", "gallery", "source", "timing",
                                        "cache-images", "cache-ridge", "cache-fly", "cache-walk"))
    parser.add_argument("--output", type=Path, default=ROOT / "output/geometry-study")
    parser.add_argument("--configurations", nargs="+", choices=CACHE_CONFIGS, default=list(CACHE_CONFIGS))
    parser.add_argument("--repetitions", type=int, default=1)
    args = parser.parse_args()
    out = args.output.resolve()
    if args.mode == "gallery":
        gallery(out)
    elif args.mode == "source":
        source_comparison(out)
    elif args.mode == "timing":
        timing(out)
    elif args.mode in ("cache-images", "cache-ridge"):
        cache_images(out, args.configurations, ridge=args.mode == "cache-ridge")
    elif args.mode in ("cache-fly", "cache-walk"):
        cache_route(out, args.configurations, args.mode == "cache-walk", args.repetitions)
    else:
        close(out, bank=args.mode == "riverbank")


if __name__ == "__main__":
    main()
