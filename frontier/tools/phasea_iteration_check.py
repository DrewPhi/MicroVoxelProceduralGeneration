#!/usr/bin/env python3
"""Matched primary Phase A iteration: no builds; sequential settled window runs.

Dry-run does not write files or launch a GPU. Output must be a new directory.
Performance and image diagnostics never imply visual acceptance.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import time

import numpy as np
from PIL import Image
from compare_capture import compare as compare_linear, linear_rgb
from compare_volume_capture import compare as compare_bytes
from types import SimpleNamespace

from adaptive_motion_check import EXPERIMENT_ENV_KEYS, capture_provenance
from phasea_pages_check import CAMERA_MANIFEST, ROOT, commands, cpu_model
from phasea_report import report as compare_report

ITERATION_ENV_KEYS = (*EXPERIMENT_ENV_KEYS, "FRONTIER_INDEXED_PROXY")


def environment(entries):
    result = {}
    for entry in entries:
        key, separator, value = entry.partition('=')
        if not separator or key not in ITERATION_ENV_KEYS or key in result:
            raise ValueError('environment entries require unique allowlisted KEY=VALUE: ' + entry)
        result[key] = value
    return result


def match_metadata(side, benchmark, headless=False):
    """Read actual metadata; capture_pair independently checks it against artifacts."""
    source = benchmark.get('source', {})
    merged = dict(side)
    for key, value in dict(terrain_recipe=benchmark.get('terrain_recipe'),
                           seed=benchmark.get('seed'), source_version=source.get('version'),
                           native_voxel_m=source.get('native_voxel_m'),
                           appearance_version=benchmark.get('appearance_version'),
                           render_appearance_version=benchmark.get('render_appearance_version'),
                           samples=benchmark.get('msaa_samples'),
                           voxel_filter=benchmark.get('voxel_filter'), visual=benchmark.get('visual')).items():
        if key not in merged and value is not None:
            merged[key] = value
    visual = merged.get('visual', {})
    return dict(source=merged.get('terrain_recipe'), seed=merged.get('seed'),
                world_version=merged.get('source_version'), voxel_m=merged.get('native_voxel_m'),
                camera={key: merged.get(key) for key in ('eye', 'target', 'fov_y_degrees', 'near')},
                far_m=merged.get('far'), width=merged.get('width'), height=merged.get('height'),
                materials={key: merged.get(key) for key in ('appearance_version', 'render_appearance_version')},
                sampling=dict(samples=merged.get('samples'), voxel_filter=merged.get('voxel_filter'),
                              color_encoding=merged.get('color_encoding'), render_scale=visual.get('render_scale')),
                lighting={key: visual.get(key) for key in ('atmosphere', 'exposure', 'fog_density',
                          'fog_height_falloff', 'sun_elevation', 'sun_azimuth', 'cloud_coverage', 'cloud_speed', 'overcast')},
                route=dict(name='static', clock='headless_still' if headless else 'realtime',
                           start='frozen-primary', warmup_frames=0 if headless else 60,
                           readiness='headless_capture' if headless else 'adaptive-wait-quality'))



def artifact_diagnostics(root, reference, motion=False):
    """Validate buffers independently of unavailable appearance provenance."""
    sides, depths = [], []
    expected = dict(width=reference['width'], height=reference['height'], seed=reference['seed'],
                    eye=reference['eye'], target=reference['target'], near=reference['near'],
                    far=reference['far'], fov_y_degrees=reference['fov_y_degrees'],
                    native_voxel_m=.0625, samples=1, voxel_filter=True,
                    horizontal_scale_m=reference['params']['horizontal_scale'],
                    amplitude_m=reference['params']['amplitude'], octaves=reference['params']['octaves'],
                    roughness=reference['params']['roughness'])
    if motion:
        expected.pop('eye')
        expected.pop('target')
    observed_failures, unavailable, camera_roundtrip = [], [], []
    for name in ('baseline', 'candidate'):
        side = json.loads((root / (name + '.capture.json')).read_text())
        sides.append(side)
        with Image.open(root / (name + '.png')) as image:
            image.load()
            if image.size != (expected['width'], expected['height']):
                raise ValueError(name + ': unexpected PNG dimensions')
        path = root / (name + '.depth.f32')
        if path.stat().st_size != expected['width'] * expected['height'] * 4:
            raise ValueError(name + ': unexpected depth size')
        depth = np.fromfile(path, dtype='<f4')
        if not np.isfinite(depth).all() or ((depth < 0) | (depth > 1)).any():
            raise ValueError(name + ': depth must be finite normalized [0,1]')
        depths.append(depth)
        for key, value in expected.items():
            if key not in side:
                unavailable.append(name + ': ' + key)
            elif key in ('eye', 'target'):
                actual = np.asarray(side[key], dtype=float)
                frozen = np.asarray(value, dtype=float)
                delta = float(np.max(np.abs(actual - frozen))) if actual.shape == frozen.shape else None
                tolerance = 1e-6
                camera_roundtrip.append(dict(run=name, field=key, max_component_difference_m=delta,
                                             tolerance_m=tolerance))
                if delta is None or not math.isfinite(delta) or delta > tolerance:
                    observed_failures.append(name + ': capture disagrees with frozen settings: ' + key)
            elif side[key] != value:
                observed_failures.append(name + ': capture disagrees with frozen settings: ' + key)
        if 'terrain_recipe' in side and str(side['terrain_recipe']).lower() != 'legacy':
            observed_failures.append(name + ': non-Legacy source')
        # Never infer missing exposure/material provenance from command-line flags.
        for key in ('terrain_recipe', 'source_version', 'visual', 'render_appearance_version',
                    'appearance_version', 'depth_encoding', 'color_encoding'):
            if key not in side:
                unavailable.append(name + ': ' + key)
    for key in set(sides[0]) & set(sides[1]):
        if key in (*expected, 'terrain_recipe', 'source_version', 'visual', 'render_appearance_version',
                   'appearance_version', 'depth_encoding', 'color_encoding') and sides[0][key] != sides[1][key]:
            observed_failures.append('capture arms disagree: ' + key)
    if motion:
        return dict(comparison=None, comparison_unavailable='Realtime endpoints are unmatched trajectory frames',
                    observed_provenance_failures=observed_failures, unavailable_provenance=unavailable,
                    camera_roundtrip=camera_roundtrip, quality_pass=None)
    result = compare_bytes(root / 'baseline.png', root / 'candidate.png', tolerance=0,
                           mesh_depth=root / 'baseline.depth.f32', ray_depth=root / 'candidate.depth.f32')
    result['linear_rgb'] = compare_linear(linear_rgb(root / 'baseline.png'), linear_rgb(root / 'candidate.png'))
    if result['depth']['common_hit_pixels'] == 0:
        result['depth']['max_normalized_error'] = None
        result['depth']['p99_normalized_error'] = None
    return dict(comparison=result, observed_provenance_failures=observed_failures,
                unavailable_provenance=unavailable, camera_roundtrip=camera_roundtrip,
                note='Raw matched-dimension buffer diagnostics; unavailable provenance remains unverified.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--baseline-binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--candidate-binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--baseline-env', action='append', default=[])
    parser.add_argument('--candidate-env', action='append', default=[])
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--route', choices=('static', 'turn-return', 'fly'), default='static')
    parser.add_argument('--speed', type=float, default=3)
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--headless', action='store_true', help='Separate primary still; no window timings')
    args = parser.parse_args()
    if args.headless and args.route != 'static':
        parser.error('--headless requires --route static')
    if not math.isfinite(args.speed) or args.speed <= 0:
        parser.error('--speed must be finite and positive')
    if not math.isfinite(args.seconds) or args.seconds <= 0:
        parser.error('--seconds must be finite and positive')
    try:
        overrides = [environment(args.baseline_env), environment(args.candidate_env)]
    except ValueError as exc:
        parser.error(str(exc))
    out = args.output.resolve()
    if out.exists() and not args.dry_run:
        parser.error('--output must not exist; preserve previous evidence')
    reference_text = CAMERA_MANIFEST.read_text()
    reference = json.loads(reference_text)['reference']
    planned = []
    for name, binary, override in zip(('baseline', 'candidate'),
                                    (args.baseline_binary, args.candidate_binary), overrides):
        frozen = out / 'bin' / name
        options = SimpleNamespace(binary=frozen, output=out, scenes='adaptive', static_only=True,
                                  headless=args.headless, seconds=args.seconds, speeds=[])
        command = next(commands(options, reference))[3]
        command = [value.replace(str(out / ('adaptive-primary' if args.headless else 'adaptive-static')), str(out / name)) for value in command]
        if args.headless:
            command.remove('--adaptive-wait-quality')
        else:
            command[command.index('--route') + 1] = args.route
            command[command.index('--route-speed') + 1] = str(args.speed)
        env = os.environ.copy()
        # Every experiment is reset before applying explicit arm settings.
        for key in ITERATION_ENV_KEYS:
            env.pop(key, None)
        env.update(override)
        planned.append((name, binary.resolve(), frozen, command, env))
        print('+', shlex.join([f'{key}={value}' for key, value in override.items()] + command), flush=True)
    if args.dry_run:
        return
    (out / 'bin').mkdir(parents=True)
    (out / 'primary-camera.json').write_text(reference_text)
    # Freeze both arms before launching either: rebuilding the original cannot change a run.
    for _, binary, frozen, _, _ in planned:
        shutil.copy2(binary, frozen)
    manifest = dict(schema='phasea_iteration_v1', quality_pass=None, runs=[],
                    mode='headless_primary' if args.headless else 'settled_window_' + args.route,
                    route=args.route, speed_mps=None if args.headless else args.speed,
                    requested_seconds=None if args.headless else args.seconds,
                    primary_camera_sha256=hashlib.sha256(reference_text.encode()).hexdigest(),
                    reference=reference, timing_scope=None if args.headless else 'frame intervals after readiness and 60 warmup frames',
                    memory_scope='total residency unavailable; original scoped counters preserved',
                    experiment_environment_policy='all allowlisted experiment keys unset unless explicitly provided')
    for name, original, frozen, command, env in planned:
        row = dict(id=name, rgb=name + '.png', depth=name + '.depth.f32', report=name + '.json',
                   original_binary=str(original), command=command, errors=[])
        if name == 'candidate':
            row['compare_to'] = 'baseline'
        manifest['runs'].append(row)
        provenance = capture_provenance(frozen, environ=env)
        provenance['experiments'] = {key: env.get(key) for key in ITERATION_ENV_KEYS}
        row['provenance'] = provenance
        start = time.monotonic()
        with (out / (name + '.log')).open('w') as log:
            result = subprocess.run(command, cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT)
        row['wall_seconds'] = time.monotonic() - start
        row['exit_code'] = result.returncode
        if result.returncode:
            row['errors'].append(f'process exited {result.returncode}')
        try:
            benchmark = {} if args.headless else json.loads((out / row['report']).read_text())
            if args.headless:
                row.pop('report')
            side = json.loads((out / (name + '.capture.json')).read_text())
            row['match'] = match_metadata(side, benchmark, args.headless)
            row['match']['route'].update(name=args.route, speed_mps=None if args.headless else args.speed)
            if benchmark.get('benchmark_failed') or benchmark.get('benchmark_interrupted'):
                row['errors'].append('benchmark failed or interrupted')
            frames, adaptive = benchmark.get('frames', {}), benchmark.get('adaptive', {})
            row['measurements'] = dict(cpu_model=cpu_model(), gpu_model=benchmark.get('adapter'),
                binary_sha256=provenance['binary_sha256'], command=command,
                p50_ms=frames.get('p50_ms'), p95_ms=frames.get('p95_ms'), p99_ms=frames.get('p99_ms'),
                cold_ready_ms=adaptive.get('elapsed_preparation_ms'), resident_bytes=None,
                pending_bytes=None, page_or_mesh_count=adaptive.get('resident_clusters'))
            row['scoped_counters'] = {key: benchmark.get(key) for key in
                                      ('adaptive', 'adaptive_route_quality', 'streaming', 'native_streaming', 'pages', 'page_experiment', 'route')}
        except (OSError, ValueError) as exc:
            row['errors'].append(str(exc))
        (out / 'manifest.json').write_text(json.dumps(manifest, indent=2, allow_nan=False) + '\n')
    motion = args.route != 'static'
    comparison = (dict(schema='phasea_iteration_motion_v1', comparisons=[], evidence_complete=False,
                       quality_pass=None, comparison=None,
                       comparison_unavailable='Realtime endpoints are unmatched trajectory frames')
                  if motion else compare_report(manifest, out))
    artifact_errors = []
    try:
        comparison['raw_artifact_diagnostics'] = artifact_diagnostics(out, reference, motion=motion)
    except (OSError, ValueError) as exc:
        artifact_errors.append(str(exc))
    complete = not artifact_errors and not any(r['errors'] for r in manifest['runs'])
    comparison['artifact_errors'] = artifact_errors
    comparison['artifacts_complete'] = complete
    comparison['provenance_complete'] = False if motion else not any(
        ': unavailable measurement: ' not in failure
        for pair in comparison['comparisons'] for failure in pair['failures'])
    (out / 'comparison.json').write_text(json.dumps(comparison, indent=2, allow_nan=False) + '\n')
    raise SystemExit(0 if complete else 1)


if __name__ == '__main__':
    main()
