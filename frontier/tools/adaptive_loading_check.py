#!/usr/bin/env python3
"""Sequential fixed-source preparation/distance sweep; never weakens quality gates.

Default: 300 m, worker counts 1/2/4. Use --distances 300,1000,4000,20000
for the explicit distance experiment, and --workers 2 to hold concurrency fixed.
A warm disk run requires a prior completed run with the same --disk-cache path;
this tool never deletes that cache or labels an existing directory cold.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

from adaptive_report import summarize

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--refinements', type=int, choices=range(1,33), default=4)
    parser.add_argument('--workers', default='1,2,4')
    parser.add_argument('--distances', default='300')
    parser.add_argument('--seconds', type=float, default=12.)
    parser.add_argument('--disk-cache', type=Path)
    parser.add_argument('--disk-mib', type=int, default=2048)
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    workers = [int(v) for v in args.workers.split(',')]
    distances = [int(v) for v in args.distances.split(',')]
    if any(w < 1 or w > 16 for w in workers) or any(d <= 0 for d in distances):
        parser.error('workers must be 1..16 and distances positive integer meters')
    if args.seconds <= 0 or args.disk_mib <= 0:
        parser.error('seconds and disk-mib must be positive')
    out = args.output.resolve()
    if not args.dry_run:
        out.mkdir(parents=True, exist_ok=True)
    binary = ROOT / 'target/release/frontier'
    digest = hashlib.file_digest(binary.open('rb'), 'sha256').hexdigest() if binary.exists() else None
    results = []
    for distance in distances:
        for worker in workers:
            directory = out / f'far{distance}-workers{worker}'
            command = [sys.executable, str(ROOT / 'tools/adaptive_motion_check.py'),
                       '--output', str(directory), '--static-only', '--settled',
                       '--seconds', str(args.seconds), '--pixel-error', '5', '--msaa', '4',
                       '--native-bins', 'on', '--workers', str(worker), '--far', str(distance), '--refinements', str(args.refinements)]
            if args.disk_cache:
                command += ['--disk-cache', str(args.disk_cache.resolve()), '--disk-mib', str(args.disk_mib)]
            print('+', ' '.join(command), flush=True)
            if args.dry_run:
                continue
            started = time.monotonic()
            run = subprocess.run(command, cwd=ROOT)
            result = {'far_m': distance, 'workers': worker, 'refinements': args.refinements, 'exit_code': run.returncode,
                      'wall_seconds': time.monotonic() - started, 'binary_sha256': digest,
                      'disk_cache_path': str(args.disk_cache.resolve()) if args.disk_cache else None,
                      'disk_cache_initial_state': 'not asserted; use a new path for cold cache',
                      'command': command}
            report = directory / 'static.json'
            if report.exists():
                data = json.loads(report.read_text())
                result['screen'] = summarize(data)
                result['preparation'] = {k: v for k, v in data.get('adaptive', {}).items()
                                         if any(s in k for s in ('first_', 'build', 'source_', 'generated', 'affine', 'pending'))}
            else:
                result['screen'] = {'numerical_screen_pass': False, 'failures': ['no completed report; inspect run log']}
            results.append(result)
            (out / 'sweep.json').write_text(json.dumps(results, indent=2) + '\n')
            if run.returncode < 0 or run.returncode >= 128:
                raise SystemExit(run.returncode)
    if any(result['exit_code'] != 0 for result in results):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
