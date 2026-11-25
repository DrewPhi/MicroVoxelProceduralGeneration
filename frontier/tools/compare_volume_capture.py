#!/usr/bin/env python3
"""Compare equally sized, matched mesh/brick PNG captures at native resolution.

No camera registration, rescaling or filtering is applied. RGB disagreement
measures image parity, not depth correctness. Optional --mesh-depth/--ray-depth
accept matching raw row-major float32 Depth32Float captures for depth parity.
Requires Pillow and NumPy (analysis tools only).
"""
import argparse
import json
from pathlib import Path
import numpy as np
from PIL import Image


def compare(mesh_path, ray_path, tolerance=1, mesh_depth=None, ray_depth=None):
    mesh = np.asarray(Image.open(mesh_path).convert("RGB"), dtype=np.int16)
    ray = np.asarray(Image.open(ray_path).convert("RGB"), dtype=np.int16)
    if mesh.shape != ray.shape:
        raise ValueError(f"matched dimensions required: {mesh.shape} != {ray.shape}")
    delta = np.abs(mesh - ray)
    disagreement = np.max(delta, axis=2) > tolerance
    result = {
        "width": mesh.shape[1], "height": mesh.shape[0],
        "tolerance_srgb_byte": tolerance,
        "different_pixels": int(disagreement.sum()),
        "different_fraction": float(disagreement.mean()),
        "max_channel_error_srgb_byte": int(delta.max()),
        "mae_srgb_byte": float(delta.mean()),
        "rmse_srgb_byte": float(np.sqrt(np.mean(delta.astype(np.float64) ** 2))),
    }
    if (mesh_depth is None) != (ray_depth is None):
        raise ValueError("supply both mesh and ray depth files")
    if mesh_depth is not None:
        shape = mesh.shape[:2]
        md = np.fromfile(mesh_depth, dtype="<f4").reshape(shape)
        rd = np.fromfile(ray_depth, dtype="<f4").reshape(shape)
        if not np.isfinite(md).all() or not np.isfinite(rd).all():
            raise ValueError("depth files contain nonfinite values")
        coverage_diff = (md < 1) != (rd < 1)
        common = (md < 1) & (rd < 1)
        depth_error = np.abs(md[common] - rd[common])
        result["depth"] = {
            "coverage_disagreement_pixels": int(coverage_diff.sum()),
            "common_hit_pixels": int(common.sum()),
            "max_normalized_error": float(depth_error.max()) if depth_error.size else 0,
            "p99_normalized_error": float(np.quantile(depth_error, .99)) if depth_error.size else 0,
        }
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mesh", type=Path)
    parser.add_argument("ray", type=Path)
    parser.add_argument("--tolerance", type=int, default=1)
    parser.add_argument("--mesh-depth", type=Path)
    parser.add_argument("--ray-depth", type=Path)
    args = parser.parse_args()
    if not 0 <= args.tolerance <= 255:
        parser.error("tolerance must be 0..255")
    print(json.dumps(compare(args.mesh, args.ray, args.tolerance,
                             args.mesh_depth, args.ray_depth), indent=2))


if __name__ == "__main__":
    main()
