#!/usr/bin/env python3
"""Four serial matched Alpine GPU cases; bounded to 120 seconds total. No promotion gate."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]

def memory():
    rows = dict(line.split(':', 1) for line in Path('/proc/meminfo').read_text().splitlines())
    return {key: int(rows[key].split()[0]) for key in ('MemTotal', 'MemAvailable', 'SwapFree')}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    p.add_argument('--output', type=Path, default=ROOT / 'output/nov16/gpu-comparison')
    p.add_argument('--run', action='store_true')
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=True)
    manifest_path = a.output / 'manifest.json'
    if manifest_path.exists():
        p.error('output already has a manifest; preserve prior measurements')
    base = [str(a.binary.resolve()), '--preset', 'alpine', '--seed', '7', '--width', '1920', '--height', '1080',
            '--msaa', '1', '--naadf-radius', '64', '--naadf-follow', 'on', '--native-radius', '4',
            '--vegetation-near', '32', '--pixel-error', '5', '--capture-far', '1000', '--resource-profile', 'development',
            '--adaptive-workers', '4', '--adaptive-refinements', '32', '--upload-mib', '16', '--no-adaptive-disk-cache',
            '--benchmark-input-lock', '--adaptive-wait-quality', '--uncapped', '--warmup', '10',
            '--route-clock', 'realtime', '--route-speed', '2.5']
    cases = [('static-control', 'static', 2, {}),
             ('static-cover16-deferred', 'static', 2, {'FRONTIER_GROUND_COVER_PROBES': '16', 'FRONTIER_DEFERRED_GROUND_COVER': '1'}),
             ('motion-control', 'walk-turn-return', 10, {}),
             ('motion-tree-matched-aa05', 'walk-turn-return', 10, {'FRONTIER_VEGETATION_TREE_TRANSITION': 'matched', 'FRONTIER_POST_AA_STRENGTH': '0.5'})]
    initial = memory()
    manifest = {'scope': __doc__, 'binary_sha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(),
                'binary': str(a.binary.resolve()), 'binary_mtime_ns': a.binary.stat().st_mtime_ns,
                'initial_memory_kib': initial, 'global_timeout_seconds': 120, 'case_timeout_seconds': 40,
                'cwd': str(ROOT), 'cases': []}
    def save():
        manifest_path.write_text(json.dumps(manifest, indent=2) + '\n')
    started = time.monotonic()
    for name, route, seconds, overrides in cases:
        command = base + ['--route', route, '--seconds', str(seconds), '--report', str(a.output / (name + '.json')),
                          '--capture-on-exit', str(a.output / (name + '.png'))]
        env = {k: v for k, v in os.environ.items() if not k.startswith('FRONTIER_')}
        env.update(overrides)
        entry = {'name': name, 'route': route, 'requested_seconds': seconds, 'command': command,
                 'environment_overrides': overrides, 'status': 'planned', 'memory_before_kib': memory()}
        manifest['cases'].append(entry)
        save()
        if not a.run:
            continue
        remaining = 120 - (time.monotonic() - started)
        if remaining < seconds + 2:
            entry['status'] = 'global_budget_skipped'
            save()
            continue
        case_started = time.monotonic()
        low = memory()['MemAvailable']
        reserve = max(1024 * 1024, initial['MemTotal'] // 10)
        with (a.output / (name + '.log')).open('w') as log:
            child = subprocess.Popen(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
            stopped = None
            while child.poll() is None:
                available = memory()['MemAvailable']
                low = min(low, available)
                elapsed = time.monotonic() - case_started
                if available < reserve:
                    stopped = 'memory_watchdog'
                elif elapsed >= min(40, remaining):
                    stopped = 'preparation_or_run_timeout'
                if stopped:
                    child.terminate()
                    try:
                        child.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait()
                    break
                time.sleep(.2)
        entry.update(status=stopped or ('ok' if child.returncode == 0 else 'failed'),
                     returncode=child.returncode, elapsed_seconds=time.monotonic() - case_started,
                     minimum_available_kib=low)
        report = a.output / (name + '.json')
        if report.exists():
            d = json.loads(report.read_text())
            entry['metrics'] = {k: d.get(k) for k in ('adapter', 'frames', 'gpu_render_pass', 'gpu_full_render', 'vegetation_gpu_timing',
                               'process_memory', 'adaptive_route_quality', 'ground_cover', 'camera_start', 'projection',
                               'route_elapsed_seconds', 'benchmark_failed', 'visual', 'vegetation', 'naadf', 'experiments')}
            entry['metrics']['native_publication'] = {k: d.get('naadf', {}).get(k) for k in ('status', 'stage', 'published_world_revision', 'resident_radius_m', 'requested_radius_m', 'startup_expanding', 'content_update_pending')}
        capture = a.output / (name + '.capture.json')
        if capture.exists():
            c = json.loads(capture.read_text())
            entry['capture'] = {k: c.get(k) for k in ('eye', 'target', 'fov_y_degrees', 'far', 'width', 'height', 'samples', 'adapter', 'ground_cover', 'adaptive')}
        save()
        print(name, entry['status'], round(entry['elapsed_seconds'], 2), flush=True)
    manifest['elapsed_seconds'] = time.monotonic() - started
    save()

if __name__ == '__main__':
    main()
