#!/usr/bin/env python3
"""Run deterministic whole-voxel player transaction and disk checkpoint checks.

Exercises production transaction/worker/save code without X11 input timing.
This checks game-state correctness; it does not certify mouse handling or FPS.
"""
import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=float, default=600)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    cargo = shutil.which('cargo')
    env = os.environ.copy()
    if not cargo:
        toolchains = ROOT.parents[1] / '.toolchains'
        cargo = str(toolchains / 'rustup/toolchains/1.99.1-x86_64-unknown-linux-gnu/bin/cargo')
        env['PATH'] = str(Path(cargo).parent) + os.pathsep + env['PATH']
        env['CARGO_HOME'] = str(toolchains / 'cargo')
        env['RUSTUP_HOME'] = str(toolchains / 'rustup')
        env['RUSTUP_TOOLCHAIN'] = '1.99.1'
    env.setdefault('CARGO_BUILD_JOBS', '2')
    checks = [
        ('runtime', 'frontier-renderer', 'discrete_water_live::tests'),
        ('moving_runtime', 'frontier-renderer', 'moving_water_live::tests'),
        ('procedural_runtime', 'frontier-renderer', 'procedural_water_live::tests'),
        ('hand_and_transactions', 'frontier-world', 'edit::tests'),
        ('placement_geometry', 'frontier-renderer', 'player_edit::tests'),
    ]
    results = []
    try:
        for name, package, pattern in checks:
            command = [cargo, 'test', '-p', package, '--lib', pattern, '--', '--nocapture']
            started = time.monotonic()
            try:
                with (out / f'{name}.log').open('w') as log:
                    result = subprocess.run(command, cwd=ROOT, env=env, stdout=log,
                                            stderr=subprocess.STDOUT, timeout=args.timeout)
            except subprocess.TimeoutExpired:
                results.append({'check': name, 'command': command,
                                'seconds': time.monotonic()-started,
                                'passed': False, 'timeout': True})
                raise
            text = (out / f'{name}.log').read_text()
            counts = re.findall(r'test result: ok\. (\d+) passed;', text)
            passed = result.returncode == 0 and any(int(count) > 0 for count in counts)
            results.append({'check': name, 'command': command, 'seconds': time.monotonic()-started,
                            'returncode': result.returncode, 'passed': passed})
            if not passed:
                raise RuntimeError(f'{name} failed; inspect {out / (name + ".log")}')
    finally:
        (out / 'result.json').write_text(json.dumps({'scope': __doc__, 'checks': results,
            'passed': len(results) == len(checks) and all(r['passed'] for r in results)}, indent=2)+'\n')


if __name__ == '__main__':
    main()
