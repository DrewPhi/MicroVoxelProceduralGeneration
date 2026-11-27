"""Compare matched finite-world captures. Run from frontier/."""

import argparse
import json
from pathlib import Path
import subprocess

import numpy as np
from PIL import Image, ImageDraw, ImageFont


def coverage(file, shape):
    return np.fromfile(file, dtype=np.uint8).reshape(shape).astype(np.float32) / 255


def errors(candidate, reference):
    error = np.abs(candidate - reference)
    edge = (reference > 0) & (reference < 1)
    return {
        "mean_coverage_error": float(error.mean()),
        "edge_mean_coverage_error": float(error[edge].mean()) if edge.any() else None,
        "pixels_over_quarter_coverage_error": int(np.sum(error > 0.25)),
        "mixed_reference_pixels": int(edge.sum()),
    }


def image(folder):
    with Image.open(folder / "terrain.png") as source:
        return source.convert("RGB")


def color_error(candidate, reference):
    c = np.asarray(candidate, dtype=np.float32) / 255
    r = np.asarray(reference, dtype=np.float32) / 255
    return float(np.abs(c - r).mean())


def sheet(root, panels, mask):
    width, height = 640, 360
    canvas = Image.new("RGB", (2 * width + 24, 2 * height + 88), "#141b22")
    draw = ImageDraw.Draw(canvas)
    font = ImageFont.truetype("DejaVuSans.ttf", 16)
    for i, (label, folder) in enumerate(panels):
        x, y = 8 + (width + 8) * (i % 2), 6 + (height + 42) * (i // 2)
        draw.text((x, y + 5), label, font=font, fill="#f0f5f7")
        pixels = image(folder)
        if mask:
            values = coverage(folder / "coverage.u8", (pixels.height, pixels.width))
            pixels = Image.fromarray(np.uint8(values * 255), "L").convert("RGB")
        canvas.paste(pixels.resize((width, height), Image.Resampling.LANCZOS), (x, y + 30))
    canvas.save(root / ("coverage-comparison.png" if mask else "comparison.png"))


def compare(root):
    panels = [
        ("Coarse heightfield  •  4 m", root / "proxy" / "dda"),
        ("Exact voxels  •  1 sample per pixel", root / "native" / "dda"),
        ("Exact voxels  •  footprint-filtered appearance", root / "native" / "filtered"),
        ("Exact voxels  •  64-sample reference", root / "native" / "ssaa64"),
    ]
    reference_folder = panels[-1][1]
    report = json.loads((reference_folder / "report.json").read_text())
    shape = report["height"], report["width"]
    ref = coverage(reference_folder / "coverage.u8", shape)
    ref_image = image(reference_folder)
    comparison = {}
    for label, folder in panels[:-1] + [(mode, root / "native" / mode) for mode in (
        "adaptive", "silhouette", "packet4", "ssaa4",
    )]:
        if not (folder / "coverage.u8").exists():
            continue
        stats = errors(coverage(folder / "coverage.u8", shape), ref)
        stats["mean_srgb_error"] = color_error(image(folder), ref_image)
        frames = sorted((reference_folder / "frames").glob("*.u8"))
        if frames:
            history = []
            changes = []
            previous = None
            for frame in frames:
                target = coverage(frame, shape)
                candidate = coverage(folder / "frames" / frame.name, shape)
                history.append(errors(candidate, target))
                residual = candidate - target
                if previous is not None:
                    changes.append(float(np.abs(residual - previous).mean()))
                previous = residual
            edge_errors = [v["edge_mean_coverage_error"] for v in history if v["edge_mean_coverage_error"] is not None]
            stats["motion_frames"] = len(frames)
            stats["motion_edge_mean_error"] = float(np.mean(edge_errors)) if edge_errors else None
            stats["motion_worst_pixels_over_quarter_error"] = max(
                v["pixels_over_quarter_coverage_error"] for v in history
            )
            stats["motion_mean_adjacent_frame_change_in_coverage_error"] = (
                float(np.mean(changes)) if changes else None
            )
        comparison[label] = stats
    if (root / "native/packet4/coverage.u8").is_file():
        fixed_folder = root / "native" / "ssaa4"
        bundled_folder = root / "native" / "packet4"
        fixed = coverage(fixed_folder / "coverage.u8", shape)
        bundled = coverage(bundled_folder / "coverage.u8", shape)
        packet_errors = {
            "unequal_coverage_pixels": int(np.sum(fixed != bundled)),
        }
        frames = sorted((fixed_folder / "frames").glob("*.u8"))
        if frames:
            packet_errors["motion_max_unequal_coverage_pixels"] = max(
                int(np.sum(
                    coverage(frame, shape) != coverage(bundled_folder / "frames" / frame.name, shape)
                ))
                for frame in frames
            )
        comparison["packet4_vs_ssaa4"] = packet_errors
    (root / "coverage-metrics.json").write_text(json.dumps(comparison, indent=2) + "\n")
    sheet(root, panels, False)
    sheet(root, panels, True)


def movie(root):
    entries = (
        ("Coarse heightfield (4 m)", root / "proxy" / "dda"),
        ("Exact voxels (1 sample)", root / "native" / "dda"),
        ("Exact voxels + footprint lighting", root / "native" / "filtered"),
        ("Exact voxels (64-sample reference)", root / "native" / "ssaa64"),
    )
    # letterbox labels without covering the terrain image
    labels = ";".join(
        f"[{i}:v]pad=iw:ih+28:0:28:color=0x141b22,drawtext=text='{label}'"
        f":x=12:y=6:fontsize=15:fontcolor=white[view{i}]"
        for i, (label, _) in enumerate(entries)
    )
    layout = (
        "[view0][view1][view2][view3]"
        "xstack=inputs=4:layout=0_0|w0_0|0_h0|w0_h0[out]"
    )
    sources = [item for _, folder in entries for item in (
        "-framerate", "24", "-i", str(folder / "frames" / "%04d.png")
    )]
    subprocess.run([
        "ffmpeg", "-v", "error", "-y", *sources,
        "-filter_complex_threads", "2", "-filter_complex", f"{labels};{layout}",
        "-map", "[out]", "-c:v", "libx264", "-crf", "17", "-pix_fmt", "yuv420p",
        "-movflags", "+faststart", str(root / "comparison.mp4"),
    ], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="Capture directory containing proxy/ and native/")
    parser.add_argument("--movie", action="store_true")
    args = parser.parse_args()
    compare(args.root)
    if args.movie:
        movie(args.root)
