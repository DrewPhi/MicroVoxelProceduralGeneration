"""Compare the independent ray results and draw the matched screenshot panels."""
import argparse
import json
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont


def read_depth(root: Path, run: str) -> np.ndarray:
    directory = root / run
    report = json.loads((directory / "report.json").read_text())
    return np.fromfile(directory / "ray-distance.f32", dtype="<f4").reshape(
        report["height"], report["width"]
    )


def compare(reference: np.ndarray, candidate: np.ndarray) -> dict:
    reference_hit = reference >= 0
    candidate_hit = candidate >= 0
    both_hit = reference_hit & candidate_hit
    difference = np.abs(reference[both_hit] - candidate[both_hit])
    depth_error = (
        np.percentile(difference, [50, 95, 99, 100]).tolist() if difference.size else None
    )
    height = reference.shape[0]
    ref_skyline = np.where(reference_hit.any(axis=0), reference_hit.argmax(axis=0), height)
    candidate_skyline = np.where(candidate_hit.any(axis=0), candidate_hit.argmax(axis=0), height)
    skyline_error = np.abs(ref_skyline - candidate_skyline)
    return {
        "terrain_sky_disagreements": int(np.count_nonzero(reference_hit != candidate_hit)),
        "joint_terrain_pixels": int(both_hit.sum()),
        "depth_error_m_p50_p95_p99_max": depth_error,
        "depth_disagreements_over_5m": int(np.count_nonzero(difference > 5)),
        "depth_disagreements_over_20m": int(np.count_nonzero(difference > 20)),
        "skyline_error_px_p50_p95_p99_max": np.percentile(skyline_error, [50, 95, 99, 100]).tolist(),
        "unresolved_pixels": int(np.count_nonzero(candidate == -2)),
    }


def screenshot_panel(output: Path, entries: list[tuple[str, Path]]) -> None:
    width, height = 768, 432
    canvas = Image.new("RGB", (len(entries) * (width + 8) + 8, height + 52), "#181b21")
    draw = ImageDraw.Draw(canvas)
    font = ImageFont.truetype("DejaVuSans.ttf", 18)
    for index, (label, source) in enumerate(entries):
        with Image.open(source) as image:
            panel = image.convert("RGB").resize((width, height), Image.Resampling.LANCZOS)
        left = 8 + index * (width + 8)
        draw.text((left + 6, 13), label, font=font, fill="#eceef2")
        canvas.paste(panel, (left, 44))
    canvas.save(output)


def moving_comparison(root: Path) -> None:
    """Encode a matched split-screen playback without including encode time in GPU timings."""
    routes = {
        "walking": "Ground-following walk",
        "turning": "In-place turn",
        "flight": "Fast flight",
    }
    with tempfile.TemporaryDirectory(prefix="motion-", dir=root) as directory:
        temporary = Path(directory)
        for route, label in routes.items():
            baseline = root / f"baseline-{route}" / "frames" / "%04d.png"
            packets = root / f"packets-{route}" / "frames" / "%04d.png"
            filters = (
                "[0:v][1:v]hstack,pad=1280:420:0:32:color=0x181b21,"
                "drawtext=font=DejaVu Sans:text='Independent rays':x=16:y=7:fontsize=19:fontcolor=white,"
                "drawtext=font=DejaVu Sans:text='Packet prefix':x=656:y=7:fontsize=19:fontcolor=white,"
                f"drawtext=font=DejaVu Sans:text='{label}':x=(w-tw)/2:y=399:fontsize=16:fontcolor=white"
            )
            subprocess.run([
                "ffmpeg", "-v", "error", "-y", "-framerate", "24", "-i", str(baseline),
                "-framerate", "24", "-i", str(packets), "-filter_complex", filters,
                "-frames:v", "36", "-c:v", "libx264", "-crf", "17", "-pix_fmt", "yuv420p",
                str(temporary / f"{route}.mp4"),
            ], check=True)
        manifest = temporary / "routes.txt"
        manifest.write_text("".join(f"file '{route}.mp4'\n" for route in routes))
        subprocess.run([
            "ffmpeg", "-v", "error", "-y", "-f", "concat", "-i", str(manifest),
            "-c", "copy", "-movflags", "+faststart", str(root / "moving-comparison.mp4"),
        ], check=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--video", action="store_true", help="encode the captured moving comparisons")
    args = parser.parse_args()
    root = args.output

    pairs = {
        "stationary": ["baseline-1920-filtered", "packets-1920-filtered",
                       "preview-1920-filtered", "compact-1920-filtered", "repair-1920-filtered"],
        **{route: [f"{mode}-{route}" for mode in ("baseline", "packets", "compact", "repair")]
           for route in ("grazing", "walking", "turning", "flight")},
    }
    results = {}
    for label, runs in pairs.items():
        reference = read_depth(root, runs[0])
        results[label] = {name: compare(reference, read_depth(root, name)) for name in runs[1:]}
    (root / "visual-comparison.json").write_text(json.dumps(results, indent=2) + "\n")

    screenshot_panel(root / "stationary-comparison.png", [
        ("Frontier packed hybrid", root / "packed-1920.png"),
        ("Independent rays", root / "baseline-1920-filtered/terrain.png"),
        ("Packet prefix", root / "packets-1920-filtered/terrain.png"),
    ])
    screenshot_panel(root / "grazing-comparison.png", [
        ("Independent rays", root / "baseline-grazing/terrain.png"),
        ("Packet prefix", root / "packets-grazing/terrain.png"),
        ("Sparse preview", root / "compact-grazing/terrain.png"),
    ])
    if args.video:
        moving_comparison(root)


if __name__ == "__main__":
    main()
