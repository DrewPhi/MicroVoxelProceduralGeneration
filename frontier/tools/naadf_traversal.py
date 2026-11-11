#!/usr/bin/env python3
"""Capture exact traversal counts, and estimate raster depth cutoff opportunity.

Requires numpy and Pillow. Diagnostic counts use an unlit float attachment;
do not use these captures to measure timing. Raster/voxel layers share the
normal ownership mask and are rendered independently at the exact same pose.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]


def vector(value):
    values = np.array([float(x) for x in value.split(',')])
    if values.shape != (3,) or not np.isfinite(values).all():
        raise argparse.ArgumentTypeError('expected finite X,Y,Z')
    return values


def unit(v):
    return v / np.linalg.norm(v)


def capture(binary, args, layer, stem, short=False):
    path = args.output / f'{stem}.png'
    env = {**os.environ, 'FRONTIER_NAADF_CAPTURE_LAYER': layer,
           'FRONTIER_NAADF_SPARSE_ROOTS': str(int(args.sparse or short)),
           'FRONTIER_NAADF_SPARSE_SHORT': str(int(short))}
    cmd = [str(binary), '--preset', 'alpine', '--seed', '7', '--msaa', '1',
           '--naadf-radius', str(args.radius), '--naadf-follow', 'off',
           '--pixel-error', '5', '--capture-far', '6000', '--capture-linear',
           '--atmosphere', 'off', '--no-adaptive-disk-cache', '--capture', str(path),
           '--width', str(args.width), '--height', str(args.height),
           '--eye', ','.join(map(str, args.eye)), '--target', ','.join(map(str, args.target))]
    with path.with_suffix('.log').open('w') as log:
        subprocess.run(cmd, env=env, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
    return path


def stats(counts):
    result = {}
    for i, name in enumerate(('root', 'block', 'leaf')):
        values = counts[..., i]
        p50, p95, p99 = np.percentile(values, [50, 95, 99])
        result[name] = dict(mean=float(values.mean()), p50=float(p50), p95=float(p95),
                            p99=float(p99), max=int(values.max()))
    return result


def heatmap(counts, path):
    height, width, _ = counts.shape
    canvas = Image.new('RGB', (3 * width, height + 28), (20, 24, 31))
    draw = ImageDraw.Draw(canvas)
    for i, name in enumerate(('Root visits', 'Block visits', 'Leaf visits')):
        values = counts[..., i]
        limit = max(1, np.percentile(values, 99))
        intensity = np.clip(np.log1p(values) / np.log1p(limit), 0, 1)
        rgb = np.stack((intensity, intensity ** 1.7 * 0.8, intensity ** 3 * 0.4), -1)
        canvas.paste(Image.fromarray((rgb * 255).astype(np.uint8)), (i * width, 28))
        draw.text((i * width + 8, 8), f'{name} | log scale, max color at {limit:.0f}',
                  fill=(237, 240, 248))
    canvas.save(path)


def depth_opportunity(meta, raster, voxels):
    eye, target = (np.array(meta[key]) for key in ("eye", "target"))
    width, height = meta["width"], meta["height"]
    forward = unit(target - eye)
    right = unit(np.cross(forward, [0., 1., 0.]))
    up = np.cross(right, forward)
    x = 2 * (np.arange(width) + .5) / width - 1
    y = 1 - 2 * (np.arange(height) + .5) / height
    tan = np.tan(np.deg2rad(meta["fov_y_degrees"]) / 2)
    direction = (forward + x[None, :, None] * right * tan * width / height
                 + y[:, None, None] * up * tan)
    # Parameter is forward depth, not Euclidean distance.
    low, high = np.array(meta["naadf"]["world_bounds_m"]) - eye
    moving = np.abs(direction) > 1e-12
    denominator = np.where(moving, direction, 1.)
    a = low / denominator
    b = high / denominator
    near, far = meta["near"], meta["far"]
    enter = np.maximum(near, np.where(moving, np.minimum(a, b), -np.inf).max(axis=2))
    leave = np.minimum(far, np.where(moving, np.maximum(a, b), np.inf).min(axis=2))
    intersects = (enter < leave) & np.all(moving | ((low <= 0) & (0 < high)), axis=2)
    linear = lambda d: near * far / (far - d.astype(np.float64) * (far - near))
    # Bias outward to avoid counting float32 ties as an opportunity.
    stop = linear(np.minimum(raster.astype(np.float64) + 2e-6, 1.))
    hit = np.where(voxels < 1., linear(voxels), np.inf)
    eligible = intersects & (raster < 1.) & (stop < np.minimum(leave, hit))
    before = eligible & (stop <= enter)
    return dict(pixels=raster.size, rays_entering_cube=int(intersects.sum()),
                cutoff_candidates=int(eligible.sum()), before_cube=int(before.sum()),
                partial=int((eligible & ~before).sum()),
                candidates_percent=100. * float(eligible.mean()), ndc_safety_bias=2e-6)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--radius', type=int, default=64)
    parser.add_argument('--width', type=int, default=640)
    parser.add_argument('--height', type=int, default=360)
    parser.add_argument('--eye', type=vector, default=vector('-41.06016,28.95,-250'))
    parser.add_argument('--target', type=vector, default=vector('-39,28.1,0'))
    parser.add_argument('--sparse', action='store_true')
    parser.add_argument('--short', action='store_true', help='Sparse atlas control without page skips')
    parser.add_argument('--depth', action='store_true')
    args = parser.parse_args()
    if not (1 <= args.radius <= 256 and args.width > 0 and args.height > 0):
        parser.error('radius must be 1..256 and dimensions must be positive')
    if np.linalg.norm(np.cross(args.target - args.eye, [0., 1., 0.])) < 1e-9:
        parser.error('target must specify a nonvertical camera heading')
    args.output = args.output.resolve()
    args.output.mkdir(exist_ok=False, parents=True)
    binary = args.binary.resolve()
    path = capture(binary, args, 'steps', 'steps', args.short)
    meta = json.loads(path.with_suffix(".capture.json").read_text())
    shape = (meta["height"], meta["width"])
    counts = np.fromfile(path.with_suffix('.linear_rgb.f32'), '<f4').reshape(*shape, 3)
    if not np.isfinite(counts).all() or np.any(counts != np.floor(counts)) or not counts.any():
        raise RuntimeError('expected unclamped integer GPU visit counts')
    heatmap(counts, args.output / 'heatmap.png')
    result = dict(binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  eye=args.eye.tolist(), target=args.target.tolist(), dimensions=shape[::-1],
                  adapter=meta["adapter"], terrain_recipe=meta["terrain_recipe"],
                  native_voxel_m=meta["native_voxel_m"], radius_m=args.radius, root_format='short' if args.short else 'sparse' if args.sparse else 'dense',
                  visits=stats(counts))
    if args.depth:
        layers = []
        for layer in ('raster', 'voxels'):
            path = capture(binary, args, layer, layer, args.short)
            other = json.loads(path.with_suffix('.capture.json').read_text())
            if (any(other[k] != meta[k] for k in ('eye', 'target', 'fov_y_degrees', 'near', 'far'))
                    or other['naadf']['world_bounds_m'] != meta['naadf']['world_bounds_m']):
                raise RuntimeError('unmatched camera or exact volume')
            layers.append(np.fromfile(path.with_suffix('.depth.f32'), '<f4').reshape(shape))
        result['raster_depth_opportunity'] = depth_opportunity(meta, *layers)
    (args.output / 'summary.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
