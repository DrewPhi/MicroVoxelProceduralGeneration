#!/usr/bin/env python3
"""Reproduce the fixed, user-selected player-view reference comparison."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'output/primary-check')
    parser.add_argument('--skip-build', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    baseline = json.loads((ROOT / 'reference/gallery/player-spiky.json').read_text())
    ref, gpu = baseline['reference'], baseline['gpu']
    out = args.output.resolve()
    vector = lambda values: ','.join(map(str, values))
    camera = ['--eye', vector(ref['eye']), '--target', vector(ref['target']),
              '--fov-y', str(ref['fov_y_degrees']), '--width', str(ref['width']),
              '--height', str(ref['height']), '--seed', str(ref['seed'])]
    commands = []
    if not args.skip_build:
        commands.append(['cargo', 'build', '--release', '--locked', '--workspace'])
    commands.extend([
        [str(ROOT / 'target/release/frontier'), '--scene', 'hybrid',
         '--terrain-recipe', 'legacy', '--view-levels', str(gpu['view_levels']),
         '--voxel-filter', 'on', '--msaa', '1', '--capture-linear',
         '--capture-far', str(ref['far']), '--capture', str(out / 'realtime.png')] + camera,
        [str(ROOT / 'target/release/landscape'), '--terrain-recipe', 'legacy',
         '--center', '0,0', '--extent', '1024', '--near', str(ref['near']),
         '--far', str(ref['far']), '--spp', str(ref['spp']), '--sample-seed', str(ref['sample_seed']),
         '--threads', '8', '--cache', str(out / 'native-columns.bin'),
         '--output', str(out / 'reference256')] + camera,
        [sys.executable, str(ROOT / 'tools/landscape_pair.py'), str(out / 'reference256'),
         str(out / 'realtime.png'), '--output', str(out / 'side-by-side.png')],
    ])
    if not args.dry_run:
        out.mkdir(parents=True, exist_ok=True)
    for command in commands:
        print('+', ' '.join(command), flush=True)
        if not args.dry_run:
            subprocess.run(command, cwd=ROOT, check=True)


if __name__ == '__main__':
    main()
