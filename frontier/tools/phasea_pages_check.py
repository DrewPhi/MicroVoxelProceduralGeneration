#!/usr/bin/env python3
"""Sequential frozen-camera Phase A diagnostics; never assert quality acceptance.

No builds. --dry-run prints commands without creating output or launching a GPU.
Window mode records routes; --headless renders the separate fixed primary still.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import shlex
import subprocess
import time

from adaptive_motion_check import capture_provenance

ROOT = Path(__file__).resolve().parents[1]
CAMERA_MANIFEST = ROOT / 'reference/gallery/player-spiky.json'


def cpu_model():
    try:
        for line in Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                return line.split(':', 1)[1].strip()
    except OSError:
        pass
    return None


def commands(args, reference):
    vector = lambda values: ','.join(map(str, values))
    params = reference['params']
    for scene in args.scenes.split(','):
        for speed in ([None] if args.static_only or args.headless else args.speeds):
            name = f'{scene}-' + ('primary' if args.headless else 'static' if speed is None else f'{speed:g}mps'.replace('.', 'p'))
            prefix = args.output / name
            command = [str(args.binary), '--scene', scene, '--terrain-recipe', 'legacy',
                       '--seed', str(reference['seed']), '--scale', str(params['horizontal_scale']),
                       '--amplitude', str(params['amplitude']), '--octaves', str(params['octaves']),
                       '--roughness', str(params['roughness']), '--width', '1920', '--height', '1080',
                       '--msaa', '1', '--voxel-filter', 'on', '--atmosphere', 'off', '--render-scale', '1',
                       '--view-levels', '11', '--capture-far', '300',
                       '--eye', vector(reference['eye']), '--target', vector(reference['target']),
                       '--fov-y', str(reference['fov_y_degrees'])]
            if scene == 'pages':
                command += ['--page-spacing', '1']
            if scene == 'adaptive':
                command += ['--pixel-error', '5', '--adaptive-workers', '4',
                            '--adaptive-refinements', '8', '--adaptive-wait-quality']
            if args.headless:
                command += ['--capture-linear', '--capture', str(prefix.with_suffix('.png'))]
            else:
                command += ['--uncapped', '--warmup', '60', '--benchmark-input-lock',
                            '--route', 'static' if speed is None else 'turn-return',
                            '--route-clock', 'realtime', '--route-speed', str(speed or 3),
                            '--seconds', str(args.seconds), '--capture-on-exit', str(prefix.with_suffix('.png')),
                            '--report', str(prefix.with_suffix('.json'))]
            yield name, speed, prefix, command


def read_artifact(path, errors, required=True):
    if not path.is_file():
        if required:
            errors.append(f'missing artifact: {path}')
        return None
    try:
        return json.loads(path.read_text(), parse_constant=lambda value: None)
    except (OSError, ValueError) as exc:
        errors.append(f'unreadable artifact {path}: {exc}')
        return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--scenes', default='pages,hybrid')
    parser.add_argument('--static-only', action='store_true')
    parser.add_argument('--speeds', default='3,15,80')
    parser.add_argument('--seconds', type=float, default=6.)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--headless', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    scenes = args.scenes.split(',')
    if not scenes or any(scene not in ('pages', 'hybrid', 'adaptive') for scene in scenes) or len(set(scenes)) != len(scenes):
        parser.error('--scenes must list unique pages,hybrid,adaptive values')
    try:
        args.speeds = [float(value) for value in args.speeds.split(',')]
    except ValueError:
        parser.error('--speeds must be comma-separated numbers')
    if not args.speeds or any(not math.isfinite(s) or s <= 0 for s in args.speeds) or len(set(args.speeds)) != len(args.speeds):
        parser.error('--speeds must be unique finite positive numbers')
    if not math.isfinite(args.seconds) or args.seconds <= 0:
        parser.error('--seconds must be finite and positive')
    args.output, args.binary = args.output.resolve(), args.binary.resolve()
    reference_text = CAMERA_MANIFEST.read_text()
    reference = json.loads(reference_text)['reference']
    planned = list(commands(args, reference))
    for _, _, _, command in planned:
        print('+', shlex.join(command), flush=True)
    if args.dry_run:
        return
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / 'primary-camera.json').write_text(reference_text)
    summary = {'schema': 'phasea_pages_runs_v1', 'quality_pass': None,
               'mode': 'headless_primary_still' if args.headless else 'window_routes',
               'cpu_model': cpu_model(), 'camera_manifest': str(CAMERA_MANIFEST),
               'camera_manifest_sha256': hashlib.sha256(reference_text.encode()).hexdigest(),
               'reference': reference, 'runs': [],
               'limitations': ['No visual or temporal acceptance inferred from timings or end captures.',
                               'Headless stills and window route endpoints are separate experiments.',
                               'Memory/readiness fields retain actual report scopes; unavailable values stay absent.']}
    for name, speed, prefix, command in planned:
        # These exact generated paths must not attach stale evidence to a new failed run.
        for suffix in ('.json', '.png', '.capture.json', '.depth.f32', '.coverage.f32', '.linear_rgb.f32'):
            prefix.with_suffix(suffix).unlink(missing_ok=True)
        errors = []
        row = {'id': name, 'speed_mps': speed, 'command': command, 'quality_pass': None,
               'timestamp_utc': datetime.now(timezone.utc).isoformat(),
               'provenance': capture_provenance(args.binary), 'errors': errors}
        summary['runs'].append(row)
        started = time.monotonic()
        with prefix.with_suffix('.log').open('w') as log:
            try:
                result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
                row['exit_code'] = result.returncode
                if result.returncode:
                    errors.append(f'process exited {result.returncode}')
            except OSError as exc:
                row['exit_code'] = None
                errors.append(str(exc))
        row['wall_seconds'] = time.monotonic() - started
        row['report_path'] = str(prefix.with_suffix('.json'))
        row['capture_path'] = str(prefix.with_suffix('.png'))
        row['actual_report'] = read_artifact(prefix.with_suffix('.json'), errors, required=not args.headless)
        row['actual_capture_metadata'] = read_artifact(prefix.with_suffix('.capture.json'), errors)
        for suffix in ('.png', '.depth.f32'):
            if not prefix.with_suffix(suffix).is_file():
                errors.append(f'missing artifact: {prefix.with_suffix(suffix)}')
        actual = row['actual_report'] or {}
        if actual.get('benchmark_failed') or actual.get('benchmark_interrupted'):
            errors.append('benchmark failed or interrupted')
        row['diagnostics'] = {key: actual.get(key) for key in
                              ('frames', 'gpu_render_pass', 'streaming', 'pages', 'page_experiment', 'adaptive',
                               'adaptive_route_quality', 'topology_owner', 'edits', 'representation')}
        row['evidence_collected'] = not errors
        (args.output / 'runs.json').write_text(json.dumps(summary, indent=2, allow_nan=False) + '\n')
    raise SystemExit(1 if any(row['errors'] for row in summary['runs']) else 0)


if __name__ == '__main__':
    main()
