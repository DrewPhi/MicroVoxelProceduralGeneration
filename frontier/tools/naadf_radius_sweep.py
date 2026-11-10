#!/usr/bin/env python3
"""Descending NAADF capacity sweep using the ordinary full-lit Alpine scene.

Plans by default; --run executes sequentially. Oversized requests are preflight
results, never FPS measurements. Existing OS processes remain untouched. A
watchdog stops only this tool's child if available system RAM falls below the
reserve. Radius is the fixed cube half-extent, not its diameter.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
MIB = 1024 * 1024


def memory():
    values = {}
    for line in Path('/proc/meminfo').read_text().splitlines():
        key, value, *_ = line.split()
        values[key.rstrip(':')] = int(value) // 1024
    return {k: values[k] for k in ('MemTotal', 'MemAvailable', 'SwapFree')}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--radii', nargs='+', type=int, default=[1024, 512, 256, 128, 64, 32, 16])
    parser.add_argument('--seconds', type=float, default=10.)
    parser.add_argument('--atmosphere', choices=['on', 'off'], default='on')
    parser.add_argument('--naadf-budget-mib', type=int,
                        help='Explicit conservative build-estimate ceiling; actual RAM watchdog remains active')
    parser.add_argument('--run', action='store_true')
    args = parser.parse_args()
    if args.naadf_budget_mib is not None and not 16 <= args.naadf_budget_mib <= 16384:
        parser.error('NAADF estimate ceiling must be 16..16384MiB')
    if not math.isfinite(args.seconds) or not .1 <= args.seconds <= 60:
        parser.error('sample seconds must be finite and between 0.1 and 60')
    if len(set(args.radii)) != len(args.radii):
        parser.error('radii must be unique to preserve each result')
    if any(not 1 <= r <= 1024 for r in args.radii):
        parser.error('radii must be 1..1024m')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    manifest_path = out / 'manifest.json'
    if manifest_path.exists():
        parser.error('use a new directory to preserve measurements')
    initial = memory()
    reserve = max(768, initial['MemTotal'] // 20)
    manifest = {
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        'scope': __doc__, 'initial_memory_mib': initial,
        'watchdog_reserve_mib': reserve,
        'conditions': {'width': 1920, 'height': 1080, 'msaa': 1,
                       'preset': 'alpine', 'seed': 7, 'clip_m': 6000,
                       'native_mesh_radius_m': 4, 'vegetation_near_m': 32,
                       'pixel_error': 5, 'route': 'static', 'seconds': args.seconds,
                       'lighting': 'ordinary beauty and sun shadows',
                       'atmosphere': args.atmosphere,
                       'naadf_estimate_ceiling_mib': args.naadf_budget_mib,
                       'resource_profile': 'development; available RAM with system headroom'},
        'cases': {},
    }

    def save():
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')

    for radius in args.radii:
        name = f'r{radius}'
        current = memory()
        top_mib = (2 * radius) ** 3 * 4 / MIB
        minimum_mib = top_mib * 3
        case = {'radius_m': radius, 'cube_width_m': radius * 2,
                'top_grid_gpu_mib': top_mib,
                'minimum_grid_build_copy_mib_before_payload': minimum_mib,
                'memory_before_mib': current, 'status': 'planned'}
        manifest['cases'][name] = case
        if minimum_mib > max(0, current['MemAvailable'] - reserve):
            case.update(status='preflight_memory_refusal',
                        reason='Grid/build-copy floor exceeds current available RAM minus headroom; no allocation attempted, no FPS result')
            print(name, case['status'], f'floor {minimum_mib:.0f}MiB', flush=True)
            save()
            continue
        if radius > 256:
            case.update(status='preflight_implementation_limit',
                        reason='Current fixed snapshot builder supports up to 256m radius; no allocation attempted')
            save()
            continue
        report = out / (name + '.json')
        command = [str(args.binary.resolve()), '--preset', 'alpine', '--seed', '7',
                   '--naadf-radius', str(radius), '--msaa', '1',
                   '--width', '1920', '--height', '1080', '--pixel-error', '5',
                   '--atmosphere', args.atmosphere,
                   '--native-radius', '4', '--vegetation-near', '32',
                   '--resource-profile', 'development', '--adaptive-workers',
                   str(max(1, min(16, (os.cpu_count() or 2) - 2))),
                   '--adaptive-refinements', '32', '--upload-mib', '16',
                   '--no-adaptive-disk-cache', '--adaptive-wait-quality',
                   '--benchmark-input-lock', '--uncapped', '--warmup', '30',
                   '--seconds', str(args.seconds), '--route', 'static',
                   '--report', str(report), '--capture-on-exit', str(out / (name + '.png'))]
        case['command'] = command
        if args.naadf_budget_mib is not None:
            command += ['--naadf-budget-mib', str(args.naadf_budget_mib)]
        print(shlex.join(command), flush=True)
        save()
        if not args.run:
            continue
        started = time.monotonic()
        with (out / (name + '.log')).open('w') as log:
            child = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
            low = current['MemAvailable']
            stopped = None
            try:
                while child.poll() is None:
                    available = memory()['MemAvailable']
                    low = min(low, available)
                    if available < reserve:
                        stopped = 'watchdog_low_available_ram'
                    elif time.monotonic() - started > 330:
                        stopped = 'watchdog_preparation_timeout'
                    if stopped:
                        child.terminate()
                        try:
                            child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait()
                        break
                    time.sleep(.5)
            finally:
                if child.poll() is None:
                    child.terminate()
                    child.wait(timeout=10)
        case.update(status=stopped or ('ok' if child.returncode == 0 else 'failed'),
                    returncode=child.returncode, wall_seconds=time.monotonic() - started,
                    minimum_available_system_ram_mib=low)
        if report.exists():
            data = json.loads(report.read_text())
            case['measurements'] = {k: data.get(k) for k in
                                    ('adapter', 'frames', 'process_memory', 'naadf',
                                     'gpu_full_render', 'naadf_gpu_pass', 'hybrid_mesh_residency',
                                     'camera_start', 'projection', 'adaptive_route_quality')}
            snapshot = data.get('naadf', {})
            if child.returncode == 0 and snapshot.get('resident_radius_m') != radius:
                case['status'] = 'invalid_unpublished_requested_radius'
        case['log_tail'] = '\n'.join((out / (name + '.log')).read_text().splitlines()[-5:])
        print(name, case['status'], flush=True)
        save()


if __name__ == '__main__':
    main()
