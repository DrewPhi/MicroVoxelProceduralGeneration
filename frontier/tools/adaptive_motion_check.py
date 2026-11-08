#!/usr/bin/env python3
"""Sequential hardware motion diagnostics from the frozen primary camera.

Run with DISPLAY pointing to the laptop session and the hardware GPU accessible.
Quality/readiness in each report are acceptance conditions, not implied by FPS.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import os
import json
import math
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
# Explicit allowlist: do not serialize the environment or arbitrary FRONTIER_* keys.
EXPERIMENT_ENV_KEYS = (
    "FRONTIER_MAP_NODE_REUSE", "FRONTIER_VEGETATION_REFERENCE", "FRONTIER_VEGETATION_TRACE", "FRONTIER_BEAUTY_REFERENCE", "FRONTIER_ATMOSPHERE_DEBUG",
    "FRONTIER_MAP_ANISOTROPY", "FRONTIER_MAP_FILTER_SURFACES",
    "FRONTIER_NEAR_APPEARANCE", "FRONTIER_NEAR_BLEND",
    "FRONTIER_CAPTURE_DIAGNOSTICS", "FRONTIER_APPEARANCE_DEBUG", "FRONTIER_MAP_PREFILTER", "FRONTIER_MAP_PREFILTER_RADIUS_CELLS",
    "FRONTIER_VOXEL_GRAIN",
    "FRONTIER_VISIBILITY_GRID", "FRONTIER_VISIBILITY_SPATIAL", "FRONTIER_LOD_APPEARANCE", "FRONTIER_APPEARANCE_MAP_INTERVALS",
    'FRONTIER_NATIVE_PROFILE_REUSE', 'FRONTIER_PROFILE_CENTER_REUSE',
    'FRONTIER_PROFILE_SOURCE', 'FRONTIER_RENDER_BUNDLES',
    'FRONTIER_PREPARED_VIEW_CACHE', 'FRONTIER_GPU_SUMMARY_CONTENTION',
    'FRONTIER_VISIBLE_ERROR_BOUND', 'FRONTIER_DEBT_PREDICTION',
    'FRONTIER_NATIVE_SUMMARY_REUSE', 'FRONTIER_SOURCE_CACHE_MIB',
    'FRONTIER_SOURCE_COLD_CODEC',
    'FRONTIER_OBSOLETE_TRANSACTION_PREEMPT', 'FRONTIER_COALESCE_VERTEX_PAGES',
    'FRONTIER_NEAR_DETAIL', 'FRONTIER_PROFILE_PROBE_CELLS',
    'FRONTIER_SPATIAL_PROFILES', 'FRONTIER_SPATIAL_FOOTPRINT',
)


def capture_provenance(binary, root=ROOT, environ=None):
    environ = os.environ if environ is None else environ
    digest = None
    error = None
    try:
        with Path(binary).open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    except OSError as exc:
        error = str(exc)
    commit = dirty = None
    try:
        head = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=root,
                              capture_output=True, text=True, check=True)
        status = subprocess.run(['git', 'status', '--porcelain'], cwd=root,
                                capture_output=True, text=True, check=True)
        commit, dirty = head.stdout.strip(), bool(status.stdout.strip())
    except (OSError, subprocess.CalledProcessError):
        pass
    return {
        'timestamp_utc': datetime.now(timezone.utc).isoformat(),
        'binary_path': str(Path(binary).resolve()), 'binary_sha256': digest,
        'binary_hash_error': error,
        'observed_checkout_commit': commit, 'observed_checkout_dirty': dirty,
        'checkout_identity_note': 'Checkout observed before run; not a guaranteed binary build revision.',
        'experiments': {key: environ.get(key) for key in EXPERIMENT_ENV_KEYS},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'output/adaptive-motion')
    parser.add_argument('--seconds', type=float, default=10.)
    parser.add_argument('--speeds', default='3,15,80')
    parser.add_argument('--route', choices=('turn-return', 'stop-resume', 'fly'), default='turn-return')
    parser.add_argument('--pixel-error', type=float, default=2.)
    parser.add_argument('--cache-mib', type=int, default=512)
    parser.add_argument('--msaa', type=int, choices=(1, 4), default=1)
    parser.add_argument('--native-bins', choices=('on', 'off'))
    parser.add_argument('--settled', action='store_true', help='Require initial quality before starting benchmark clocks')
    parser.add_argument('--static-only', action='store_true', help='Measure the fixed primary pose instead of routes')
    parser.add_argument('--direct-native', choices=('on', 'off'), default='off')
    parser.add_argument('--refinements', type=int, choices=range(1,33), default=4)
    parser.add_argument('--workers', type=int, choices=range(1, 17), default=1)
    parser.add_argument('--far', type=float, default=300., help='Only change for explicitly labelled distance-scaling tests')
    parser.add_argument('--disk-cache', type=Path, help='Opt-in persistent cache; use an empty new directory for a cold disk-cache test')
    parser.add_argument('--disk-mib', type=int, default=2048)
    args = parser.parse_args()
    if not math.isfinite(args.far) or args.far <= 0.025:
        parser.error('--far must be finite and greater than 0.025')
    if args.disk_mib <= 0:
        parser.error('--disk-mib must be positive')
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    reference = json.loads((ROOT / 'reference/gallery/player-spiky.json').read_text())['reference']
    vector = lambda values: ','.join(map(str, values))
    runs = []
    for speed in ([3.] if args.static_only else map(float, args.speeds.split(','))):
        name = 'static' if args.static_only else f'{speed:g}mps'
        command = [str(ROOT / 'target/release/frontier'), '--scene', 'adaptive',
                   '--pixel-error', str(args.pixel_error), '--adaptive-cache-mib', str(args.cache_mib), '--terrain-recipe', 'legacy', '--seed', '7',
                   '--width', '1920', '--height', '1080', '--msaa', str(args.msaa),
                   '--voxel-filter', 'on', '--uncapped', '--warmup', '60', '--benchmark-input-lock',
                   '--route', 'static' if args.static_only else args.route, '--route-clock', 'realtime',
                   '--route-speed', str(speed), '--seconds', str(args.seconds),
                   '--capture-far', str(args.far), '--adaptive-workers', str(args.workers), '--adaptive-refinements', str(args.refinements),
                   '--eye', vector(reference['eye']), '--target', vector(reference['target']),
                   '--fov-y', str(reference['fov_y_degrees']),
                   '--capture-on-exit', str(out / f'{name}.png'),
                   '--report', str(out / f'{name}.json')]
        if args.direct_native == 'on':
            command += ['--adaptive-direct-native', 'on']
        if args.disk_cache is not None:
            command += ['--adaptive-disk-cache', str(args.disk_cache.resolve()), '--adaptive-disk-mib', str(args.disk_mib)]
        if args.settled:
            command.append('--adaptive-wait-quality')
        if args.native_bins is not None:
            command += ['--native-bins', args.native_bins]
        print('+', ' '.join(command), flush=True)
        # Never attach a stale successful report to a failed new run.
        for suffix in ('.json', '.png'):
            (out / f'{name}{suffix}').unlink(missing_ok=True)
        provenance = capture_provenance(command[0])
        started = time.monotonic()
        with (out / f'{name}.log').open('w') as log:
            result = subprocess.run(['/usr/bin/time', '-v', '-o', str(out / f'{name}.time.txt')]
                                    + command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        report_path = out / f'{name}.json'
        if report_path.exists():
            report = json.loads(report_path.read_text())
            log_text = (out / f'{name}.log').read_text()
            report['benchmark_interrupted'] = ('benchmark interrupted' in log_text
                                               or result.returncode < 0 or result.returncode >= 128)
            report['benchmark_requested_seconds'] = args.seconds
            report['benchmark_run_exit_code'] = result.returncode
            report['capture_provenance'] = provenance
            report_path.write_text(json.dumps(report, indent=2) + '\n')
        runs.append({'speed_mps': speed, 'command': command, 'capture_provenance': provenance,
                     'wall_seconds': time.monotonic() - started, 'exit_code': result.returncode})
        (out / 'runs.json').write_text(json.dumps(runs, indent=2) + '\n')
        if result.returncode:
            raise SystemExit(result.returncode)


if __name__ == '__main__':
    main()
