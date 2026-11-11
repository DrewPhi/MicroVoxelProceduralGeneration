#!/usr/bin/env python3
"""Sequential matched-view matrix. Plans by default; never overlaps GPU jobs."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
VIEWS = {
    'mountain-heading': ('-41.06016,28.95,-250', '-39,76,0', 'static', 10),
    'reverse-heading': ('-41.06016,28.95,-250', '-43.12032,28.1,-500', 'static', 10),
    'sky-heading': ('-41.06016,28.95,-250', '-39,200,0', 'static', 10),
    'walking-circuit': ('-41.06016,28.95,-250', '-39,28.1,0', 'walk-circuit', 45),
}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, required=True)
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--radii', type=int, nargs='+', default=[128, 256])
    p.add_argument('--views', nargs='+', choices=VIEWS, default=list(VIEWS))
    p.add_argument('--repeat', type=int, default=2)
    p.add_argument('--run', action='store_true')
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    records = []
    for radius in a.radii:
        for name in a.views:
            eye, target, route, seconds = VIEWS[name]
            cmd = [sys.executable, str(ROOT/'tools/naadf_experiments.py'),
                   '--binary', str(a.binary.resolve()), '--config', str(a.config.resolve()),
                   '--output', str((a.output/f'r{radius}-{name}').resolve()),
                   '--mode', 'timing', '--radius', str(radius), '--seconds', str(seconds),
                   '--route', route, '--repeat', str(a.repeat), f'--eye={eye}', f'--target={target}']
            if a.run:
                cmd.append('--run')
            record = {'radius_m': radius, 'view': name, 'command': cmd}
            records.append(record)
            print(radius, name, flush=True)
            record['returncode'] = subprocess.run(cmd, cwd=ROOT).returncode
            (a.output/'matrix.json').write_text(json.dumps(records, indent=2)+'\n')
            if record['returncode']:
                raise SystemExit(record['returncode'])

if __name__ == '__main__':
    main()
