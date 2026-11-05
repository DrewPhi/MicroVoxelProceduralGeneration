#!/usr/bin/env python3
"""Matched LOD captures and optional sequential GPU route timings.

Run from any directory with a built release binary. Requires a GPU; timed runs
also require a display. Pillow/NumPy only used for offline image comparison.
A fixed-camera coverage shift measures representation change, not motion blur.
"""
import argparse
import json
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'output/lod-polish'


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('tag')
    p.add_argument('--filter', choices=['on', 'off'], default='on')
    p.add_argument('--benchmark', action='store_true')
    args = p.parse_args()
    if not args.tag.replace('-', '').replace('_', '').isalnum():
        p.error('tag must contain only letters, numbers, hyphens or underscores')
    OUT.mkdir(parents=True, exist_ok=True)
    binary = OUT / (args.tag + '-bin')
    shutil.copy2(ROOT / 'target/release/frontier', binary)
    base = [str(binary), '--scene', 'hybrid', '--seed', '7', '--width', '1920',
            '--height', '1080', '--msaa', '4', '--voxel-filter', args.filter]
    views = [('ground', []), ('aerial', ['--camera', 'aerial']),
             ('moved', ['--route', 'fly', '--route-speed', '3', '--capture-route-time', '8']),
             ('shift8', ['--capture-coverage-shift', '8'])]
    for name, flags in views:
        stem = OUT / (args.tag + '-' + name)
        with stem.with_suffix('.log').open('w') as log:
            subprocess.run(base + flags + ['--capture', str(stem.with_suffix('.png'))],
                           cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
        print(stem.name, flush=True)
    from PIL import Image
    import numpy as np
    images = [np.asarray(Image.open(OUT / (args.tag + '-' + v + '.png')).convert('RGB'),
                         dtype=np.float32) / 255 for v in ('ground', 'shift8')]
    images = [np.where(x <= .04045, x / 12.92, ((x + .055) / 1.055) ** 2.4) for x in images]
    delta = images[0] - images[1]
    metrics = {'fixed_camera_shift8_linear_rmse': float(np.sqrt(np.mean(delta ** 2))),
               'fixed_camera_shift8_linear_mae': float(np.mean(np.abs(delta))),
               'note': 'All-image representation change, not an exact voxel or silhouette oracle.',
               'filter': args.filter, 'size': [1920, 1080], 'msaa': 4, 'seed': 7}
    (OUT / (args.tag + '-comparison.json')).write_text(json.dumps(metrics, indent=2) + '\n')
    print(metrics, flush=True)
    if args.benchmark:
        for route, speed in [('stop-resume', 3), ('turn-return', 15)]:
            stem = OUT / (args.tag + '-' + route)
            with stem.with_suffix('.log').open('w') as log:
                subprocess.run(base + ['--route', route, '--route-speed', str(speed),
                    '--route-clock', 'realtime', '--seconds', '12', '--warmup', '60',
                    '--uncapped', '--report', str(stem.with_suffix('.json'))],
                    cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, check=True)
            print(stem.name, flush=True)


if __name__ == '__main__':
    main()
