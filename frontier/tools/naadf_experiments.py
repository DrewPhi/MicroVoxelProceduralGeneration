#!/usr/bin/env python3
"""Run isolated NAADF candidates sequentially with explicit environment provenance.

Config: {"control": {}, "candidate": {"FRONTIER_NAADF_CACHE_NODES": "1"}}.
Run performance and correctness separately: --mode timing runs the complete
1080p hybrid renderer; --mode capture freezes camera, lighting and sampling.
Never use diagnostic shaders for timing. No concurrent GPU runs are launched.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def environment(overrides):
    if not isinstance(overrides, dict) or any(
            not k.startswith('FRONTIER_NAADF_') or not isinstance(v, str)
            for k, v in overrides.items()):
        raise ValueError('variants must map FRONTIER_NAADF_* variables to strings')
    return {**{k: v for k, v in os.environ.items()
               if not k.startswith('FRONTIER_NAADF_')}, **overrides}


def compare(a, b):
    import numpy as np
    ma = json.loads(a.with_suffix('.capture.json').read_text())
    mb = json.loads(b.with_suffix('.capture.json').read_text())
    for key in ('width', 'height', 'eye', 'target', 'fov_y_degrees', 'near', 'far',
                'terrain_recipe', 'native_voxel_m'):
        if ma[key] != mb[key]:
            raise ValueError(f'unmatched capture {key}')
    if ma['naadf']['world_bounds_m'] != mb['naadf']['world_bounds_m']:
        raise ValueError('unmatched exact volume')
    for key in ('seed', 'source_version', 'edit_revision', 'samples', 'voxel_filter',
                'appearance_version', 'render_appearance_version', 'atmosphere', 'beauty'):
        if ma.get(key) != mb.get(key):
            raise ValueError(f'unmatched capture {key}')
    # Completed jobs alone do not prove equal resident tree selections: retained
    # detail can depend on build completion order. Do not label that a ray error.
    for key in ('enabled', 'source_version', 'selected_instances', 'triangles',
                'triangles_by_species', 'tree_lod_histogram', 'shadow_instances',
                'shadow_triangles', 'shadow_lod_histogram', 'naadf_omitted_groups'):
        if ma.get('vegetation', {}).get(key) != mb.get('vegetation', {}).get(key):
            raise ValueError(f'unmatched vegetation {key}')
    result = {}
    for kind in ('linear_rgb', 'depth'):
        left = np.fromfile(a.with_suffix(f'.{kind}.f32'), '<f4')
        right = np.fromfile(b.with_suffix(f'.{kind}.f32'), '<f4')
        if not left.size or left.shape != right.shape:
            raise ValueError(f'missing or mismatched {kind} payload')
        if not np.isfinite(left).all() or not np.isfinite(right).all():
            raise ValueError(f'nonfinite {kind} payload')
        error = np.abs(left.astype('float64') - right)
        result[kind] = {'equal': bool(np.array_equal(left, right)),
                        'different_values': int(np.count_nonzero(left != right)),
                        'max_absolute_error': float(error.max()),
                        'rmse': float(np.sqrt(np.mean(error * error)))}
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    p.add_argument('--mode', choices=['timing', 'capture'], default='timing')
    p.add_argument('--radius', type=int, default=64)
    p.add_argument('--seconds', type=float, default=10)
    p.add_argument('--route', choices=['static', 'walk-stop-resume', 'walk-turn-return', 'walk-circuit'], default='static')
    p.add_argument('--repeat', type=int, default=1)
    p.add_argument('--start-xz', help='Grounded preset X,Z start; timing mode only')
    p.add_argument('--eye', help='Explicit X,Y,Z; timing otherwise uses the ordinary preset camera')
    p.add_argument('--target', help='Explicit target X,Y,Z (requires --eye)')
    p.add_argument('--capture-size', default='640,360')
    p.add_argument('--allow-color-error', action='store_true', help='Capture-only approximate lighting experiment: depth must still match exactly')
    p.add_argument('--keep-going', action='store_true', help='Continue after a rejected comparison; default stops at the first failed gate')
    p.add_argument('--run', action='store_true')
    args = p.parse_args()
    if args.allow_color_error and args.mode != 'capture':
        p.error('--allow-color-error applies only to capture mode')
    if args.start_xz is not None and (args.eye is not None or args.mode != 'timing'):
        p.error('--start-xz requires timing mode without explicit --eye')
    if (args.eye is None) != (args.target is None):
        p.error('--eye and --target must be provided together')
    variants = json.loads(args.config.read_text())
    if not isinstance(variants, dict) or not variants or args.repeat < 1:
        p.error('provide variants and positive repeat count')
    if args.mode == 'timing' and any(
            values.get('FRONTIER_NAADF_DIAGNOSTICS') == '1'
            for values in variants.values()):
        p.error('diagnostic atomic shaders are not valid for timing runs')
    for name, overrides in variants.items():
        if not name.replace('-', '').replace('_', '').isalnum():
            p.error('variant names must be simple directory names')
        try:
            environment(overrides)
        except ValueError as error:
            p.error(str(error))
        if args.mode == 'timing':
            managed = {'FRONTIER_NAADF_WIDE_LEAVES', 'FRONTIER_NAADF_SPARSE_ROOTS',
                       'FRONTIER_NAADF_RAY_PROFILE', 'FRONTIER_NAADF_CAPTURE_LAYER',
                       'FRONTIER_NAADF_CPU_PROFILE', 'FRONTIER_NAADF_GPU_STAGES'}
            unsupported = managed.intersection(overrides)
            if unsupported:
                p.error(f"timing uses the full renderer with default leaf/root layouts; "
                        f"unsupported overrides: {', '.join(sorted(unsupported))}")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = {'binary_sha256': hashlib.sha256(args.binary.read_bytes()).hexdigest(),
                'variants': variants, 'mode': args.mode, 'radius_m': args.radius,
                'route': args.route, 'runs': []}
    control = next(iter(variants))
    reference = None
    failed = False
    for repeat in range(args.repeat):
        # Reverse alternate passes to reveal thermal/order effects.
        order = list(variants.items())
        if repeat % 2:
            order.reverse()
        for name, overrides in order:
            out = args.output / f'{repeat}-{name}'
            env = environment(overrides)
            if args.mode == 'timing':
                cmd = [sys.executable, str(ROOT / 'tools/naadf_radius_sweep.py'),
                       '--binary', str(args.binary.resolve()), '--radii', str(args.radius),
                       '--seconds', str(args.seconds), '--route', args.route,
                       '--output', str(out)]
                if args.start_xz is not None:
                    cmd += [f'--start-xz={args.start_xz}']
                if args.eye is not None:
                    cmd += [f'--eye={args.eye}', f'--target={args.target}']
                if overrides.get('FRONTIER_NAADF_GPU_DISTANCE') == '1':
                    cmd += ['--gpu-distance', 'on']
                if args.run:
                    cmd += ['--run']
            else:
                out.mkdir()
                w, h = args.capture_size.split(',')
                cmd = [str(args.binary.resolve()), '--preset', 'alpine', '--seed', '7',
                       '--msaa', '1', '--naadf-radius', str(args.radius), '--naadf-follow', 'off',
                       '--pixel-error', '5', '--capture-far', '6000', '--capture-linear',
                       '--atmosphere', 'off', '--no-adaptive-disk-cache',
                       '--capture', str(out / 'frame.png'), '--width', w, '--height', h,
                       '--eye', args.eye or '-41.06016,28.95,-250',
                       '--target', args.target or '-39,28.1,0']
                if overrides.get('FRONTIER_NAADF_GPU_DISTANCE') == '1':
                    cmd += ['--naadf-gpu-distance', 'on']
            record = {'name': name, 'repeat': repeat, 'environment': overrides, 'command': cmd}
            print(name, args.mode, flush=True)
            if args.run or args.mode == 'timing':
                log = args.output / f'{repeat}-{name}.log'
                try:
                    with log.open('w') as handle:
                        result = subprocess.run(cmd, cwd=ROOT, env=env, stdout=handle,
                                                stderr=subprocess.STDOUT, timeout=900)
                    record['returncode'] = result.returncode
                except subprocess.TimeoutExpired:
                    record['returncode'] = None
                    record['error'] = 'experiment exceeded 900 seconds'
                if record['returncode']:
                    record['error'] = log.read_text()[-2000:]
                elif 'error' not in record and args.run and args.mode == 'capture':
                    frame = out / 'frame.png'
                    try:
                        if repeat == 0 and name == control:
                            record['comparison_to_first'] = compare(frame, frame)
                            reference = frame
                        elif reference is None:
                            raise ValueError(f'initial reference {control!r} did not succeed')
                        else:
                            record['comparison_to_first'] = compare(reference, frame)
                            unequal = [kind for kind, value in record['comparison_to_first'].items()
                                       if not value['equal'] and not (args.allow_color_error and kind == 'linear_rgb')]
                            if unequal:
                                raise ValueError(f"exact capture mismatch: {', '.join(unequal)}")
                    except (OSError, ValueError, KeyError) as error:
                        record['error'] = str(error)
                elif 'error' not in record and args.run:
                    try:
                        measured = json.loads((out / 'manifest.json').read_text())
                        record['case'] = measured['cases'][f'r{args.radius}']
                        if record['case']['status'] != 'ok':
                            record['error'] = f"timing case {record['case']['status']}"
                    except (OSError, ValueError, KeyError) as error:
                        record['error'] = str(error)
            failed |= 'error' in record
            manifest['runs'].append(record)
            (args.output / 'summary.json').write_text(json.dumps(manifest, indent=2) + '\n')
            if 'error' in record and not args.keep_going:
                return 1
    return int(failed)


if __name__ == '__main__':
    sys.exit(main())
