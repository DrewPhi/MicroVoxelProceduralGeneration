#!/usr/bin/env python3
"""Plan/run matched exact-geometry checks; default prints commands without GPU work.

Run from any directory. First build the candidate separately, then:
  python3 tools/playtest_smoothness.py --run
  python3 tools/playtest_smoothness.py --only mountain-still --run

Primary-after duplicates baseline.py's frozen Legacy hybrid capture. Additional
adaptive primary/mountain/grazing stills compare unit and conforming topology at
identical source/camera/lighting/clip/MSAA. Mountain motion runs tighten error
from baseline 4.5px to 1px and add grounded approach, stop/resume and 180deg turn.
Adaptive pairs use custom 1024MiB CPU cache, 1536MiB GPU and 768MiB build
reservations: demanding validation, not the constrained default comparison.
Motion uses wall-clock routes: compare frame tails and readiness, not pixelwise
endpoint images. Use the fixed stills for pixel comparisons. Persistent disk
caches are isolated by mode; these runs do not claim cold-cache startup timings.
Extra primary-adaptive uses 4.5px after tighter 1px capacity failures; the official
primary-after remains frozen. Optional --only flat-native exercises compact
geometry on a low-amplitude fixture, not a replacement mountain benchmark.
The script does not build binaries or perform reference regeneration.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
HERE = ROOT / 'output/playtest-smoothness'
PRIMARY_EYE = '0,94.8875,0'
PRIMARY_TARGET = '-8.731370115810932e-08,94.83756238296628,-0.9987523555755615'
MOUNTAIN_EYE = '-650,587.825,2350'
MOUNTAIN_TARGET = '-649.4361380338669,587.8789355650545,2350.8241059184074'
# Same grounded start, almost horizontal direction: greater grazing sensitivity.
GRAZING_TARGET = '-593.61380338669,587.835,2432.41059184074'
CASES = ['primary-after', 'primary-adaptive', 'mountain-still', 'grazing-still',
         'mountain-approach', 'mountain-turn', 'flat-native']


def commands(args, output):
    size = ['--width', '1920', '--height', '1080']
    adaptive_budgets = ['--resource-profile', 'custom', '--terrain-cache-mib', '1024',
                        '--terrain-gpu-mib', '1536', '--build-memory-mib', '768']
    primary = ['--terrain-recipe', 'legacy', '--view-levels', '11',
               '--voxel-filter', 'on', '--msaa', '1', '--capture-linear',
               '--capture-far', '300', '--eye', PRIMARY_EYE,
               '--target', PRIMARY_TARGET] + size
    alpine = ['--preset', 'alpine', '--pixel-error', '1', '--msaa', '1',
              '--seed', '7', '--voxel-filter', 'on', '--render-scale', '1',
              '--eye', MOUNTAIN_EYE] + size
    for case in CASES:
        if args.only and case not in args.only:
            continue
        if case == 'flat-native' and not args.include_flat_native and not args.only:
            continue
        modes = ['conforming'] if case == 'primary-after' else ['conforming', 'unit']
        for mode in modes:
            name = case if case == 'primary-after' else f'{case}-{mode}'
            png = output / f'{name}.png'
            if case == 'primary-after':
                flags = ['--scene', 'hybrid'] + primary + ['--capture', str(png)]
            elif case == 'primary-adaptive':
                flags = ['--scene', 'adaptive', '--pixel-error', '4.5'] + adaptive_budgets + primary + [
                    '--capture', str(png)]
            elif case == 'flat-native':
                flags = ['--scene', 'adaptive', '--terrain-recipe', 'legacy',
                         '--amplitude', '1', '--octaves', '2', '--pixel-error', '4.5', '--native-radius', '8',
                         '--capture-far', '64', '--eye', '0,2,0', '--target', '0,0,10',
                         '--seed', '7', '--msaa', '1', '--voxel-filter', 'on',
                         '--beauty', 'off', '--vegetation', 'off', '--atmosphere', 'off',
                         '--terrain-shadows', 'off', '--walk', 'off', '--capture-linear',
                         '--capture', str(png)] + adaptive_budgets + size
            else:
                cache = output / 'disk-cache' / mode
                flags = alpine + adaptive_budgets + ['--adaptive-disk-cache', str(cache),
                                  '--adaptive-disk-mib', '1024']
                target = GRAZING_TARGET if case in ['grazing-still', 'mountain-turn'] else MOUNTAIN_TARGET
                flags += ['--target', target]
                if case.endswith('-still'):
                    flags += ['--capture-linear', '--capture', str(png)]
                else:
                    route = 'walk-stop-resume' if case == 'mountain-approach' else 'walk-turn-return'
                    flags += ['--uncapped', '--warmup', '60', '--benchmark-input-lock',
                              '--adaptive-wait-quality', '--seconds', str(args.seconds),
                              '--route', route, '--route-clock', 'realtime',
                              '--route-speed', '6', '--report', str(output / f'{name}.json'),
                              '--capture-on-exit', str(png)]
            yield name, mode, [str(args.binary)] + flags


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--output', type=Path, default=HERE / 'validation-after')
    parser.add_argument('--only', choices=CASES, action='append')
    parser.add_argument('--include-flat-native', action='store_true',
                        help='Include the small compact-path fixture in the default suite')
    parser.add_argument('--seconds', type=float, default=12.)
    parser.add_argument('--timeout', type=float, default=420.)
    parser.add_argument('--run', action='store_true', help='Execute GPU work; omitted means print only')
    parser.add_argument('--overwrite', action='store_true')
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    output = args.output.resolve()
    if args.seconds <= 0 or args.timeout <= 0:
        parser.error('seconds and timeout must be positive')
    jobs = list(commands(args, output))
    for name, mode, command in jobs:
        print(f'{name}: FRONTIER_NATIVE_GEOMETRY={mode} {shlex.join(command)}', flush=True)
    if not args.run:
        return
    if not args.binary.is_file():
        parser.error(f'candidate binary missing: {args.binary}; build separately')
    conflicts = [output / f'{name}.command.json' for name, _, _ in jobs
                 if (output / f'{name}.command.json').exists()]
    if conflicts and not args.overwrite:
        parser.error(f'evidence already exists: {conflicts[0]}; choose new --output or --overwrite')
    binary_sha256 = hashlib.sha256(args.binary.read_bytes()).hexdigest()
    output.mkdir(parents=True, exist_ok=True)
    for name, mode, command in jobs:
        env = os.environ.copy()
        env['FRONTIER_NATIVE_GEOMETRY'] = mode
        metadata = {'command': command, 'cwd': str(ROOT), 'binary_sha256': binary_sha256,
                    'environment_overrides': {'FRONTIER_NATIVE_GEOMETRY': mode},
                    'comparison': 'same candidate binary; topology mode only',
                    'adaptive_resource_budgets_mib': None if name == 'primary-after' else {
                        'terrain_cache': 1024, 'terrain_gpu': 1536, 'build_memory': 768},
                    'cache_policy': 'persistent, isolated mode directories; not a cold-cache proof'}
        path = output / f'{name}.command.json'
        path.write_text(json.dumps(metadata, indent=2) + '\n')
        started = time.monotonic()
        try:
            with (output / f'{name}.log').open('w') as log:
                result = subprocess.run(command, cwd=ROOT, env=env, stdout=log,
                                        stderr=subprocess.STDOUT, timeout=args.timeout)
            metadata['returncode'] = result.returncode
            if result.returncode:
                raise RuntimeError(f'{name} failed ({result.returncode}); inspect its log')
        finally:
            metadata['wall_seconds'] = time.monotonic() - started
            path.write_text(json.dumps(metadata, indent=2) + '\n')
        print(f'completed {name}', flush=True)


if __name__ == '__main__':
    main()
