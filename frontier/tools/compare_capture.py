#!/usr/bin/env python3
"""Compare matched captures against an integer-supersampled reference.

Analysis only: image dimensions, camera, scene, and settings must match except
for sampling/filtering. Decode sRGB before averaging reference pixels. These
metrics measure image agreement, not geometric correctness or temporal quality.
Requires Pillow and NumPy; neither is a game dependency.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image


def linear_rgb(path):
    srgb = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0
    return np.where(srgb <= 0.04045, srgb / 12.92, ((srgb + 0.055) / 1.055) ** 2.4)


def compare(reference, candidate):
    h, w, _ = candidate.shape
    rh, rw, _ = reference.shape
    if rh % h or rw % w or rh // h != rw // w:
        raise ValueError("reference must be an integer, isotropic supersample")
    scale = rh // h
    if scale < 1:
        raise ValueError("reference must not be smaller than candidate")
    target = reference.reshape(h, scale, w, scale, 3).mean(axis=(1, 3))
    error = np.abs(candidate - target)
    # The seed-7 ground view contains native geometry in the bottom corners.
    # Other camera views must not interpret this ROI as a native-only mask.
    corners = np.concatenate((error[3*h//4:, :w//4],
                              error[3*h//4:, 3*w//4:]), axis=1)
    regions = {"full_frame": error, "lower_half": error[h // 2:],
               "bottom_corners": corners}
    return {
        name: {
            "linear_rgb_rmse": float(np.sqrt(np.mean(values ** 2))),
            "linear_rgb_mae": float(np.mean(values)),
            "p95_max_channel_error": float(np.quantile(values.max(axis=2), 0.95)),
        }
        for name, values in regions.items()
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("candidates", type=Path, nargs="+")
    args = parser.parse_args()
    reference = linear_rgb(args.reference)
    print(json.dumps({str(p): compare(reference, linear_rgb(p))
                      for p in args.candidates}, indent=2))


if __name__ == "__main__":
    main()
