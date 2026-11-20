#!/usr/bin/env python3
"""Sequential bounded fixture comparison; builds nothing and infers no acceptance.

Example: DISPLAY=:1 python3 tools/phasea_volume_check.py --output output/phasea-volume --edits
Box splats: pass --backends mesh brick box-splat --views overview cave bridge.
Use --capture-only for headless float32 comparisons without timing samples.
Use --dry-run to print the exact commands without starting GPU work. The optional
edit case uses the existing --edit-test add/restore transaction; its captures are
compared only when all backends report completed restoration. Requires Pillow
and NumPy for capture comparisons, as does compare_volume_capture.py.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
BACKENDS = ('mesh', 'brick', 'brick-skip', 'brick-directional')
VIEWS = ('overview', 'cave', 'bridge')


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        for block in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def cpu_model():
    try:
        for line in Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                return line.split(':', 1)[1].strip()
    except OSError:
        pass
    return platform.processor() or None


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + '\n')


def matched_capture(reference, candidate, capture_only):
    a, b = [json.loads(path.with_suffix('.capture.json').read_text())
            for path in (reference, candidate)]
    # These are observations written by the renderer, not declarations inferred
    # solely from matching launch arguments. Different backends are intentional.
    keys = ('width', 'height', 'eye', 'target', 'fov_y_degrees', 'near', 'far',
            'samples', 'voxel_filter', 'depth_encoding', 'color_encoding',
            'atmosphere', 'render_scale', 'postprocessed', 'depth_available')
    for key in keys:
        if key not in a or key not in b or a[key] != b[key]:
            raise ValueError(f'missing or unmatched capture provenance: {key}')
    if a['depth_encoding'] != 'ndc_0_1':
        raise ValueError('unsupported depth encoding')
    if capture_only:
        for key in ('scene', 'seed', 'source_version', 'edit_revision', 'native_voxel_m',
                    'native_min_cell', 'native_size_cells', 'appearance_version', 'adapter'):
            if key not in a or key not in b or a[key] != b[key]:
                raise ValueError(f'missing or unmatched capture provenance: {key}')
        if a['color_encoding'] != 'linear_rgb_float32_render_target':
            raise ValueError('capture-only comparison requires a float32 color target')
        return
    reports = [json.loads(path.with_suffix('.json').read_text())
               for path in (reference, candidate)]
    for key in ('scene', 'seed', 'source', 'terrain_recipe', 'appearance_version',
                'render_appearance_version', 'visual', 'camera_start', 'output',
                'adapter', 'vsync', 'warmup', 'route', 'projection'):
        if key not in reports[0] or key not in reports[1] or reports[0][key] != reports[1][key]:
            raise ValueError(f'missing or unmatched report provenance: {key}')
    if reports[0].get('edits', {}).get('world_revision') != reports[1].get('edits', {}).get('world_revision'):
        raise ValueError('captures use different final world revisions')


def linear_error(reference, candidate, dimensions):
    import numpy as np

    shape = (dimensions['height'], dimensions['width'], 3)
    rgb = [np.fromfile(path.with_suffix('.linear_rgb.f32'), dtype='<f4').reshape(shape)
           for path in (reference, candidate)]
    error = np.abs(rgb[0] - rgb[1])
    if not np.isfinite(error).all():
        raise ValueError('nonfinite linear color disagreement')
    return {'rmse': float(np.sqrt(np.mean(np.square(error, dtype=np.float64)))),
            'mae': float(error.mean()), 'max_channel_error': float(error.max())}


def contact_sheet(out, cases, backends):
    from PIL import Image, ImageDraw

    with Image.open(out / cases[0][0] / f'{backends[0]}.png') as original:
        original.thumbnail((640, 640))
        width, height = original.size
    label = 24
    canvas = Image.new('RGB', (width * len(backends), (height + label) * len(cases)), '#18212a')
    draw = ImageDraw.Draw(canvas)
    for row, (case, view) in enumerate(cases):
        for column, backend in enumerate(backends):
            x, y = column * width, row * (height + label)
            name = case if case == 'edits' else view
            draw.text((x + 8, y + 5), f'{name} / {backend}', fill='white')
            with Image.open(out / case / f'{backend}.png') as original:
                original.thumbnail((width, height))
                canvas.paste(original.convert('RGB'), (x, y + label))
    path = out / 'comparison.png'
    canvas.save(path)
    return str(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/release/frontier')
    parser.add_argument('--backends', nargs='+', choices=(*BACKENDS, 'box-splat'), default=BACKENDS)
    parser.add_argument('--views', nargs='+', choices=VIEWS, default=('overview',))
    parser.add_argument('--capture-only', action='store_true')
    parser.add_argument('--seconds', type=float, default=4.)
    parser.add_argument('--edits', action='store_true', help='also run the existing add/restore transaction cases')
    parser.add_argument('--edit-seconds', type=float, default=10.)
    parser.add_argument('--warmup', type=int, default=60)
    parser.add_argument('--width', type=int, default=1920)
    parser.add_argument('--height', type=int, default=1080)
    parser.add_argument('--dry-run', action='store_true')
    args = parser.parse_args()
    if any(not math.isfinite(t) or t <= 0 for t in (args.seconds, args.edit_seconds)):
        parser.error('durations must be finite and positive')
    if args.warmup < 0 or min(args.width, args.height) < 1:
        parser.error('warmup must be nonnegative and dimensions positive')
    if len(set(args.backends)) != len(args.backends) or len(set(args.views)) != len(args.views):
        parser.error('specify each backend and view only once')
    if 'mesh' not in args.backends or len(args.backends) < 2:
        parser.error('include mesh and at least one candidate backend')
    if args.edits and args.capture_only:
        parser.error('the edit transaction requires a window run')
    binary, out = args.binary.resolve(), args.output.resolve()
    if not args.dry_run and out.exists() and any(out.iterdir()):
        parser.error('output must be new or empty to prevent stale report/capture reuse')
    cases = [('static' if view == 'overview' else view, view) for view in args.views]
    if args.edits:
        cases.append(('edits', 'overview'))
    commands = []
    for case, view in cases:
        for backend in args.backends:
            prefix = out / case / backend
            command = [str(binary), '--scene', 'fixture', '--renderer', backend,
                       '--seed', '7', '--width', str(args.width), '--height', str(args.height),
                       '--msaa', '1', '--voxel-filter', 'off', '--atmosphere', 'off',
                       '--fov-y', '60', '--capture-far', '20000']
            if view == 'overview':
                command.extend(['--eye', '11,9,-15', '--target', '-1,3,1'])
            else:
                command.extend(['--camera', view])
            if args.capture_only:
                command.extend(['--capture-linear', '--capture', str(prefix.with_suffix('.png'))])
            else:
                command.extend(['--benchmark-input-lock', '--uncapped', '--warmup', str(args.warmup),
                                '--seconds', str(args.edit_seconds if case == 'edits' else args.seconds),
                                '--report', str(prefix.with_suffix('.json')),
                                '--capture-on-exit', str(prefix.with_suffix('.png'))])
            if case == 'edits':
                command.append('--edit-test')
            commands.append((case, backend, prefix, command))
            print('+', shlex.join(command), flush=True)
    if args.dry_run:
        return 0

    # Import only when executing, so --dry-run does not require analysis packages.
    from compare_volume_capture import compare
    digest = sha256(binary)
    summary = {'schema': 'phasea_volume_check_v1', 'binary': str(binary),
               'binary_sha256': digest, 'cpu_model': cpu_model(),
               'platform': platform.platform(), 'cwd': str(ROOT),
               'capture_only': args.capture_only,
               'environment': {key: os.environ.get(key) for key in
                               ('DISPLAY', 'WGPU_BACKEND', 'VK_ICD_FILENAMES')},
               'scope': 'bounded 16x14x16m native fixture; GPU timings cover terrain pass only',
               'runs': [], 'comparisons': [], 'quality_pass': None,
               'note': 'Short static/edit evidence does not establish G2, streaming or temporal acceptance.'}
    out.mkdir(parents=True, exist_ok=True)
    for case, backend, prefix, command in commands:
        prefix.parent.mkdir(exist_ok=True)
        run = {'case': case, 'backend': backend, 'command': command,
               'shell_command': shlex.join(command), 'binary_sha256': sha256(binary)}
        if run['binary_sha256'] != digest:
            raise RuntimeError('binary changed during comparison')
        started = time.monotonic()
        with prefix.with_suffix('.log').open('w') as log:
            process = subprocess.run(command, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
        run.update(exit_code=process.returncode, wall_seconds=time.monotonic() - started)
        report = prefix.with_suffix('.capture.json' if args.capture_only else '.json')
        try:
            data = json.loads(report.read_text())
            for key in ('frames', 'adapter', 'gpu_render_pass', 'gpu_timing_dropped',
                        'representation', 'brick_gpu_bytes', 'brick_directional_cache',
                        'box_splat', 'edits',
                        'benchmark_failed', 'benchmark_interrupted'):
                run[key] = data.get(key)
            run['report'] = str(report)
            if args.capture_only:
                run['complete'] = process.returncode == 0 and data.get('renderer') == backend
            else:
                run['complete'] = (process.returncode == 0 and data.get('renderer_backend') == backend
                                   and data.get('benchmark_failed') is False
                                   and data.get('benchmark_interrupted') is False
                                   and data.get('frames', {}).get('sample_count', 0) > 0)
            if case == 'edits' and not data.get('edits', {}).get('test_restored', False):
                run['complete'] = False
                run['error'] = 'edit transaction did not finish restoring original materials'
        except (OSError, ValueError) as error:
            run.update(complete=False, error=str(error))
        summary['runs'].append(run)
        write_json(out / 'summary.json', summary)

    indexed = {(run['case'], run['backend']): run for run in summary['runs']}
    pairs = [('mesh', backend) for backend in args.backends if backend != 'mesh']
    if 'brick' in args.backends:
        pairs.extend(('brick', backend) for backend in args.backends if backend not in ('mesh', 'brick'))
    for case, _ in cases:
        for reference, candidate in pairs:
            result = {'case': case, 'reference': reference, 'candidate': candidate}
            try:
                if not all(indexed[(case, backend)]['complete'] for backend in (reference, candidate)):
                    raise ValueError('comparison requires both complete benchmark runs')
                a, b = [out / case / f'{backend}.png' for backend in (reference, candidate)]
                matched_capture(a, b, args.capture_only)
                result['parity'] = compare(a, b, tolerance=1,
                                          mesh_depth=a.with_suffix('.depth.f32'),
                                          ray_depth=b.with_suffix('.depth.f32'))
                if args.capture_only:
                    result['parity']['float32_linear_rgb'] = linear_error(a, b, result['parity'])
                result['complete'] = True
            except (OSError, ValueError) as error:
                result.update(complete=False, error=str(error))
            summary['comparisons'].append(result)
    summary['binary_unchanged'] = sha256(binary) == digest
    summary['evidence_complete'] = (summary['binary_unchanged']
                                    and all(run['complete'] for run in summary['runs'])
                                    and all(pair['complete'] for pair in summary['comparisons']))
    if summary['evidence_complete'] and args.capture_only:
        summary['contact_sheet'] = contact_sheet(out, cases, args.backends)
    write_json(out / 'summary.json', summary)
    print(out / 'summary.json')
    return 0 if summary['evidence_complete'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
