"""Build matched stills, depth error summaries and motion clips for the cached renderer."""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont


def load_mode(root: Path, name: str, mode: str = "dda"):
    base = root / name / mode
    report = json.loads((base / "report.json").read_text())
    shape = (report["height"], report["width"])
    return report, np.fromfile(base / "depth.f32", dtype="<f4").reshape(shape)


def distance(report, depth):
    """Convert forward depth to radial depth using the measured camera basis."""
    h, w = depth.shape
    x = (np.arange(w) + 0.5) * (2.0 / w) - 1.0
    y = 1.0 - (np.arange(h) + 0.5) * (2.0 / h)
    ray_sq = sum(
        (report["forward"][i] + x[None, :] * report["right"][i]
         + y[:, None] * report["up"][i]) ** 2
        for i in range(3)
    )
    return np.where(depth >= 0, depth * np.sqrt(ray_sq), np.nan)


def errors(candidate, reference):
    a, b = np.isfinite(candidate), np.isfinite(reference)
    both = a & b
    diff = abs(candidate[both] - reference[both])
    h = reference.shape[0]
    skyline_a = np.where(a.any(0), a.argmax(0), h)
    skyline_b = np.where(b.any(0), b.argmax(0), h)
    return {
        "missing_hits": int(np.sum(b & ~a)),
        "extra_hits": int(np.sum(a & ~b)),
        "common_hits": int(np.sum(both)),
        "radial_depth_error_m_p50_p95_p99_max":
            np.percentile(diff, [50, 95, 99, 100]).tolist() if diff.size else None,
        "depth_error_over_5m": int(np.sum(diff > 5)),
        "skyline_error_px_p50_p95_p99_max":
            np.percentile(abs(skyline_a - skyline_b), [50, 95, 99, 100]).tolist(),
    }


def error_image(candidate, reference, path):
    a, b = np.isfinite(candidate), np.isfinite(reference)
    image = np.zeros((*candidate.shape, 3), dtype=np.uint8)
    image[:] = (28, 32, 38)
    error = np.nan_to_num(abs(candidate - reference))
    image[a & b] = (52, 60, 69)
    image[a & b & (error > 5)] = (222, 164, 68)
    image[b & ~a] = (255, 83, 100)
    image[a & ~b] = (58, 185, 236)
    Image.fromarray(image).save(path)


def panels(path, entries, size=(768, 432)):
    w, h = size
    canvas = Image.new("RGB", (len(entries) * (w + 8) + 8, h + 52), "#181b21")
    draw = ImageDraw.Draw(canvas)
    font = ImageFont.truetype("DejaVuSans.ttf", 18)
    for i, (label, source) in enumerate(entries):
        at = 8 + i * (w + 8)
        draw.text((at + 6, 13), label, font=font, fill="#eceef2")
        with Image.open(source) as im:
            canvas.paste(im.convert("RGB").resize(size, Image.Resampling.LANCZOS), (at, 44))
    canvas.save(path)


def movie(root):
    with tempfile.TemporaryDirectory(prefix="cached-motion-", dir=root) as folder:
        temporary = Path(folder)
        for route, label in (("walk", "Ground-level walk"), ("bank", "Banked flight")):
            inputs = [root / f"video-{tier}-{route}" / "dda/frames/%04d.png"
                      for tier in ("finite", "nested")]
            labels = (
                "[0:v][1:v]hstack,pad=1920:600:0:32:color=0x181b21,"
                "drawtext=font=DejaVu Sans:text='Fixed 8 m cache':x=16:y=6:fontsize=20:fontcolor=white,"
                "drawtext=font=DejaVu Sans:text='Rolling 8 m and 0.5 m caches':x=976:y=6:fontsize=20:fontcolor=white,"
                f"drawtext=font=DejaVu Sans:text='{label}':x=(w-tw)/2:y=576:fontsize=16:fontcolor=white"
            )
            subprocess.run([
                "ffmpeg", "-v", "error", "-y", "-framerate", "24", "-i", str(inputs[0]),
                "-framerate", "24", "-i", str(inputs[1]), "-filter_complex", labels,
                "-frames:v", "60", "-c:v", "libx264", "-crf", "17", "-pix_fmt", "yuv420p",
                str(temporary / f"{route}.mp4"),
            ], check=True)
        manifest = temporary / "clips.txt"
        manifest.write_text("file 'walk.mp4'\nfile 'bank.mp4'\n")
        subprocess.run([
            "ffmpeg", "-v", "error", "-y", "-f", "concat", "-i", str(manifest),
            "-c", "copy", "-movflags", "+faststart", str(root / "motion.mp4"),
        ], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--direct", type=Path, default=Path(
        "output/procedural-coherence/baseline-1920-filtered"))
    parser.add_argument("--packed", type=Path, default=Path(
        "reference/procedural-raycast/packed-1080.png"))
    parser.add_argument("--video", action="store_true")
    args = parser.parse_args()
    root = args.output
    native_report, native = load_mode(root, "native")
    reference = np.fromfile(root / "native-reference/depth.f32", dtype="<f4").reshape(native.shape)
    stats = {"native_full_frame": errors(distance(native_report, native), reference)}
    error_image(distance(native_report, native), reference, root / "native-depth-map.png")
    panels(root / "native-comparison.png", [
        ("Independent 6.25 cm voxel reference", root / "native-reference/color.png"),
        ("Cached exact 6.25 cm columns", root / "native/dda/terrain.png"),
    ])
    posed_report, posed = load_mode(root, "shaded-1920")
    if (args.direct / "ray-distance.f32").exists():
        raw = np.fromfile(args.direct / "ray-distance.f32", dtype="<f4").reshape(posed.shape)
        direct = np.where(raw >= 0, raw, np.nan)
        stats["full_frame_against_direct_fbm"] = {}
        for name in ("pitch-16", "shaded-1920", "pitch-4", "nested-still"):
            report, depths = load_mode(root, name)
            stats["full_frame_against_direct_fbm"][name] = errors(distance(report, depths), direct)
        error_image(distance(posed_report, posed), direct, root / "horizon-error-map.png")
        panels(root / "horizon-comparison.png", [
            ("Frontier packed hybrid", args.packed),
            ("Direct procedural fBm", args.direct / "terrain.png"),
            ("Cached bilinear heightfield, 8 m samples", root / "shaded-1920/dda/terrain.png"),
        ])
    report, normal = load_mode(root, "nested-still")
    shifted_report, shifted = load_mode(root, "nested-shifted")
    stats["nested_recenter_fixed_pose"] = errors(distance(shifted_report, shifted), distance(report, normal))
    revisit_report, revisit = load_mode(root, "nested-revisit")
    stats["nested_revisit_same_pose"] = errors(distance(revisit_report, revisit), distance(report, normal))
    report, level = load_mode(root, "level-1920")
    vertical_report, vertical = load_mode(root, "level-1920", "vertical")
    stats["vertical_same_stepped_surface"] = errors(distance(vertical_report, vertical), distance(report, level))
    report, level = load_mode(root, "level-bilinear-1920")
    vertical_report, vertical = load_mode(root, "level-bilinear-1920", "vertical")
    stats["vertical_same_bilinear_surface"] = errors(distance(vertical_report, vertical), distance(report, level))
    panels(root / "banked-comparison.png", [
        ("Fixed 8 m cache, 33.8° bank", root / "route-banked/dda/terrain.png"),
        ("Rolling 8 m and 0.5 m caches, same pose", root / "nested-banked/dda/terrain.png"),
    ])
    for route in ("turn", "bank", "teleport"):
        off_report, off = load_mode(root, f"history-{route}-0")
        on_report, on = load_mode(root, f"history-{route}-1")
        stats[f"history_{route}_last_frame"] = errors(distance(on_report, on), distance(off_report, off))
    (root / "visual-validation.json").write_text(json.dumps(stats, indent=2) + "\n")
    if args.video:
        movie(root)


if __name__ == "__main__":
    main()
