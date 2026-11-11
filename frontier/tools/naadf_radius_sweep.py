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
    parser.add_argument('--route', choices=['static', 'walk-stop-resume', 'walk-turn-return'], default='static')
    parser.add_argument('--route-speed', type=float, default=2.5)
    parser.add_argument('--atmosphere', choices=['on', 'off'], default='on')
    parser.add_argument('--naadf-budget-mib', type=int,
                        help='Explicit conservative build-estimate ceiling; actual RAM watchdog remains active')
    parser.add_argument('--ray-profile', choices=['full', 'primary', 'voxel-shadows', 'mesh-shadows'], default='full',
                        help='Ablation only: primary omits both NAADF shadow query types; other modes keep one')
    parser.add_argument('--gpu-distance', choices=['on', 'off'], default='off')
    parser.add_argument('--leaf-format', choices=['auto', 'wide'], default='auto')
    parser.add_argument('--root-format', choices=['dense', 'sparse'], default='dense',
                        help='Sparse atlas prototype (fixed cube, CPU root distances only)')
    parser.add_argument('--watch-xz', help='Track published tile width at fixed world x,z in meters')
    parser.add_argument('--eye', help='Optional explicit x,y,z start (requires --target)')
    parser.add_argument('--target', help='Optional explicit x,y,z heading target (requires --eye)')
    parser.add_argument('--run', action='store_true')
    args = parser.parse_args()
    if args.root_format == 'sparse' and (args.route != 'static' or args.gpu_distance == 'on'):
        parser.error('sparse roots require a static route and GPU distance off')
    if (args.eye is None) != (args.target is None):
        parser.error('--eye and --target must be given together')
    if args.naadf_budget_mib is not None and not 16 <= args.naadf_budget_mib <= 16384:
        parser.error('NAADF estimate ceiling must be 16..16384MiB')
    if not math.isfinite(args.route_speed) or args.route_speed <= 0:
        parser.error('route speed must be finite and positive')
    if not math.isfinite(args.seconds) or not .1 <= args.seconds <= 180:
        parser.error('sample seconds must be finite and between 0.1 and 180')
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
    reserve = max(1024, initial['MemTotal'] // 10)
    manifest = {
        'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
        'experiment_environment': {k: v for k, v in sorted(os.environ.items())
                                   if k.startswith('FRONTIER_NAADF_')},
        'scope': __doc__, 'initial_memory_mib': initial,
        'watchdog_reserve_mib': reserve,
        'conditions': {'width': 1920, 'height': 1080, 'msaa': 1,
                       'preset': 'alpine', 'seed': 7, 'clip_m': 6000,
                       'native_mesh_radius_m': 4, 'vegetation_near_m': 32,
                       'pixel_error': 5, 'route': args.route, 'route_speed_mps': args.route_speed,
                       'seconds': args.seconds,
                       'lighting': 'ordinary beauty and sun shadows',
                       'atmosphere': args.atmosphere, 'ray_profile': args.ray_profile,
                       'eye': args.eye, 'target': args.target, 'watch_xz': args.watch_xz,
                       'gpu_distance': args.gpu_distance, 'leaf_format': args.leaf_format,
                       'root_format': args.root_format,
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
                   '--naadf-radius', str(radius), '--naadf-follow', 'off' if args.route == 'static' else 'on', '--msaa', '1',
                   '--width', '1920', '--height', '1080', '--pixel-error', '5',
                   '--atmosphere', args.atmosphere,
                   '--native-radius', '4', '--vegetation-near', '32',
                   '--resource-profile', 'development', '--adaptive-workers',
                   str(max(1, min(16, (os.cpu_count() or 2) - 2))),
                   '--adaptive-refinements', '32', '--upload-mib', '16',
                   '--no-adaptive-disk-cache', '--adaptive-wait-quality',
                   '--benchmark-input-lock', '--uncapped', '--warmup', '30',
                   '--seconds', str(args.seconds), '--route', args.route,
                   '--route-clock', 'realtime', '--route-speed', str(args.route_speed),
                   '--report', str(report), '--capture-on-exit', str(out / (name + '.png'))]
        case['command'] = command
        if args.naadf_budget_mib is not None:
            command += ['--naadf-budget-mib', str(args.naadf_budget_mib)]
        if args.gpu_distance == 'on':
            command += ['--naadf-gpu-distance', 'on']
        if args.eye is not None:
            command += ['--eye', args.eye, '--target', args.target]
        print(shlex.join(command), flush=True)
        save()
        if not args.run:
            continue
        started = time.monotonic()
        with (out / (name + '.log')).open('w') as log:
            child = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                                     env={**os.environ, 'FRONTIER_NAADF_RAY_PROFILE': args.ray_profile,
                                          'FRONTIER_NAADF_WIDE_LEAVES': '1' if args.leaf_format == 'wide' else '0',
                                          'FRONTIER_TERRAIN_WATCH_XZ': args.watch_xz or '',
                                          'FRONTIER_NAADF_SPARSE_ROOTS': '1' if args.root_format == 'sparse' else '0'})
            low = current['MemAvailable']
            stopped = None
            try:
                while child.poll() is None:
                    available = memory()['MemAvailable']
                    low = min(low, available)
                    if available < reserve:
                        stopped = 'watchdog_low_available_ram'
                    elif time.monotonic() - started > 330 + args.seconds:
                        stopped = 'watchdog_preparation_timeout'
                    if stopped:
                        child.terminate()
                        try:
                            child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            child.kill()
                            child.wait()
                        break
                    time.sleep(.2)
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
                                     'gpu_full_render', 'naadf_gpu_pass', 'naadf_ray_profile',
                                     'hybrid_mesh_residency',
                                     'camera_start', 'projection', 'adaptive_route_quality', 'adaptive')}
            trace = data.get('frame_observations', {}).get('frames', [])
            if trace:
                end = trace[-1].get('physical_eye') or trace[-1]['eye']
                start = data['camera_start']['position']
                case['motion'] = {'start_eye': start, 'end_eye': end,
                                  'horizontal_displacement_m': math.dist(start[::2], end[::2]),
                                  'elapsed_seconds': data.get('route_elapsed_seconds')}
            if args.watch_xz is not None:
                events = []
                for frame in trace:
                    width = frame.get('watched_tile_cells')
                    if not events or width != events[-1]['tile_width_cells']:
                        events.append({'seconds': frame['seconds'], 'tile_width_cells': width,
                                       'terrain_owner': frame.get('terrain_owner')})
                case['watched_tile'] = {'world_xz_m': args.watch_xz, 'events': events}
            snapshot = data.get('naadf', {})
            if child.returncode == 0 and snapshot.get('resident_radius_m') != radius:
                case['status'] = 'invalid_unpublished_requested_radius'
        case['log_tail'] = '\n'.join((out / (name + '.log')).read_text().splitlines()[-5:])
        print(name, case['status'], flush=True)
        save()


if __name__ == '__main__':
    main()
