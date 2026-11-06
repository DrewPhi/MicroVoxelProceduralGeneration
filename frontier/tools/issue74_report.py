#!/usr/bin/env python3
"""Collect existing Issue 74 runs without executing benchmarks or claiming acceptance.

Usage: issue74_report.py --output report.json RUN_DIR [RUN_JSON ...]
Pillow is used only to hash decoded RGB pixels. Missing data stays unknown.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path

from adaptive_report import summarize

SCHEMA = 'issue74_phase0_v1'
PATH_FLAGS = {'--report', '--capture-on-exit', '--output'}


def read_json(path):
    return json.loads(path.read_text(), parse_constant=lambda s: None)


def options(command):
    result = {}
    i = 0
    while i < len(command):
        token = command[i]
        if token.startswith('--'):
            value = True
            if i + 1 < len(command) and not command[i + 1].startswith('--'):
                i += 1
                value = command[i]
            result[token] = value
        i += 1
    return result


def pick(data, keys):
    return {k: data.get(k) for k in keys}


def context(path):
    """Only associate metadata with an explicit matching report/output command."""
    found = {}
    for directory in (path.parent, *path.parents[1:]):
        for name in ('runs.json', 'sweep.json'):
            candidate = directory / name
            if not candidate.exists():
                continue
            try:
                rows = read_json(candidate)
            except (ValueError, OSError):
                continue
            if not isinstance(rows, list):
                continue
            for row in rows:
                if not isinstance(row, dict):
                    continue
                opts = options(row.get('command', []))
                report = opts.get('--report')
                output = opts.get('--output')
                matches = (report and Path(report).resolve() == path.resolve()) or (
                    output and Path(output).resolve() == path.parent.resolve())
                if matches:
                    found.setdefault(name, {'path': str(candidate), 'metadata': row})
        # Search only within this workspace ancestry, not unrelated filesystem metadata.
        if directory.name == 'frontier':
            break
    return found


def rgb_hash(path):
    if not path.exists():
        return {'path': str(path), 'rgb_sha256': None, 'error': 'missing capture'}
    try:
        from PIL import Image
        with Image.open(path) as image:
            rgb = image.convert('RGB')
            return {'path': str(path), 'rgb_sha256': hashlib.sha256(rgb.tobytes()).hexdigest(),
                    'width': rgb.width, 'height': rgb.height,
                    'convention': 'SHA256(decoded RGB bytes, row-major, no header)'}
    except (ImportError, OSError, ValueError) as exc:
        return {'path': str(path), 'rgb_sha256': None, 'error': str(exc)}


def collect_run(path, data=None):
    issues = []
    if data is None:
        try:
            data = read_json(path)
        except (OSError, ValueError) as exc:
            data = {}
            issues.append('missing or unreadable benchmark report: ' + str(exc))
    ctx = context(path)
    run = ctx.get('runs.json', {}).get('metadata', {})
    sweep = ctx.get('sweep.json', {}).get('metadata', {})
    command = run.get('command', data.get('command', []))
    opts = options(command)
    disk_state = sweep.get('disk_cache_initial_state')
    if disk_state is None and command and '--adaptive-disk-cache' not in opts:
        disk_state = 'disabled (recorded command; runner default)'
    capture_path = path.with_suffix('.capture.json')
    capture = {}
    if capture_path.exists():
        try:
            capture = read_json(capture_path)
        except (ValueError, OSError) as exc:
            issues.append('invalid capture metadata: ' + str(exc))
    adaptive = data.get('adaptive', {})
    visual = data.get('visual', {})
    projection = data.get('projection', {})
    atmosphere = visual.get('atmosphere', data.get('atmosphere', capture.get('atmosphere')))
    declared_profile = data.get('quality_profile', capture.get('quality_profile'))
    profile = declared_profile or ('strict_no_atmosphere_candidate' if atmosphere is False else
                                   'atmospheric_candidate' if atmosphere is True else 'unknown')
    preset = {
        'scene': data.get('scene'), 'seed': data.get('seed'),
        'source': data.get('source'), 'terrain_recipe': data.get('terrain_recipe'),
        'appearance_version': data.get('appearance_version'),
        'render_appearance_version': data.get('render_appearance_version'),
        'camera_start': data.get('camera_start'),
        'fov_y_degrees': projection.get('fov_y_degrees', capture.get('fov_y_degrees', opts.get('--fov-y'))),
        'near': projection.get('near_m', capture.get('near')),
        'far': projection.get('far_m', capture.get('far', opts.get('--capture-far'))),
        'output': data.get('output'), 'msaa_samples': data.get('msaa_samples'),
        'render_scale': visual.get('render_scale', data.get('render_scale', capture.get('render_scale'))),
        'visual_settings': visual or None,
        'atmosphere': atmosphere, 'postprocessed': data.get('postprocessed', capture.get('postprocessed')),
        'quality_profile': profile, 'voxel_filter': data.get('voxel_filter'),
        'pixel_error': adaptive.get('pixel_error'), 'refinement_reserve': adaptive.get('refinement_reserve'),
        'workers': adaptive.get('preparation_workers'),
        'refinements': adaptive.get('refinements_per_transaction'),
        'cpu_cache_budget_bytes': adaptive.get('cpu_cache_budget_bytes'),
        'gpu_pool_budget_bytes': adaptive.get('gpu_pool_budget_bytes'),
        'build_budget_bytes': adaptive.get('build_memory_model', {}).get('budget_bytes'),
        'upload_budget_bytes_per_frame': adaptive.get('upload_budget_bytes_per_frame'),
        'route': data.get('route'), 'route_clock': data.get('route_clock'),
        'route_speed_mps': data.get('route_speed_mps'),
        'requested_seconds': data.get('benchmark_requested_seconds'),
        'requested_frames': data.get('benchmark_requested_frames'),
        'warmup': data.get('warmup'), 'wait_quality': data.get('adaptive_wait_quality'),
        'adapter': data.get('adapter'), 'vsync': data.get('vsync'),
        'disk_cache_path': sweep.get('disk_cache_path', opts.get('--adaptive-disk-cache')),
        'disk_cache_initial_state': disk_state,
        'command_settings': {k: v for k, v in opts.items() if k not in PATH_FLAGS},
    }
    required = ['scene', 'seed', 'source', 'terrain_recipe', 'camera_start', 'fov_y_degrees',
                'near', 'far', 'output', 'msaa_samples', 'render_scale', 'atmosphere',
                'postprocessed', 'visual_settings', 'voxel_filter', 'pixel_error', 'refinement_reserve',
                'workers', 'refinements', 'cpu_cache_budget_bytes', 'gpu_pool_budget_bytes',
                'build_budget_bytes', 'upload_budget_bytes_per_frame', 'route', 'route_clock', 'route_speed_mps', 'warmup',
                'wait_quality', 'adapter', 'vsync', 'disk_cache_initial_state']
    missing = [key for key in required if preset[key] is None]
    if visual.get('settings_may_have_changed') or visual.get('menu_enabled'):
        missing.append('frozen settings: interactive settings may have changed')
    if not command:
        missing.append('command_settings')
    if preset['requested_seconds'] is None and preset['requested_frames'] is None:
        missing.append('requested duration/frames')
    provenance = data.get('capture_provenance', run.get('capture_provenance', {}))
    if not isinstance(provenance.get('experiments'), dict) or not provenance['experiments']:
        missing.append('experiment environment provenance')
    identity = {key: data.get(key, run.get(key, sweep.get(key, provenance.get(key))))
                for key in ('git_commit', 'git_dirty', 'binary_sha256', 'source_sha256')}
    if provenance:
        identity['binary_sha256'] = provenance.get('binary_sha256')
    identity['source_contract'] = {**(data.get('source') or {}), **pick(data, ('seed', 'terrain_recipe'))}
    screen = summarize(data)
    failures = list(screen['failures'])
    if data.get('benchmark_failed'):
        failures.append('benchmark_failed reported')
    for label, metadata in [('run', run), ('sweep', sweep)]:
        if metadata.get('exit_code', 0) != 0:
            failures.append(label + ' process failed')
    exclusive = adaptive.get('exclusive_cpu_profile')
    return {
        'path': str(path), 'identity': identity, 'metadata_context': ctx,
        'capture_provenance': provenance or None,
        'experiments': provenance.get('experiments'),
        'preset': preset, 'missing_preset_fields': missing,
        'missing_identity_fields': [k for k, v in identity.items() if v is None],
        'quality_scope': {'profile': profile, 'declared_profile': declared_profile,
                          'atmosphere_explicit': atmosphere,
                          'perceptual_acceptance': 'not evaluated',
                          'visual_temporal_acceptance': 'not evaluated'},
        'benchmark_numerical_screen': {**screen, 'numerical_screen_pass': not failures,
                                       'failures': failures},
        'acceptance': 'not established by this aggregation',
        'first_readiness_ms': {'coverage': adaptive.get('first_coverage_ms'),
                               'requested_quality': adaptive.get('first_quality_ms'),
                               'reserve': adaptive.get('first_refinement_ms')},
        'frames': data.get('frames'), 'gpu_render_pass': data.get('gpu_render_pass'),
        'gpu_timing_dropped': data.get('gpu_timing_dropped'),
        'gpu_timing_scope': visual.get('gpu_timing_scope', 'unknown; do not assume whole-frame GPU timing'),
        'draw_calls_final': adaptive.get('draw_calls'),
        'cpu': {'preparation_stage_sums_ms': adaptive.get('preparation_cpu_stage_ms'),
                'preparation_stage_note': adaptive.get('preparation_stage_note'),
                'exclusive_profile_raw': exclusive,
                'exclusive_profile_measured': bool(exclusive and exclusive.get('enabled')),
                'interpretation': 'Worker CPU sums overlap wall time; caller stages include source costs. '
                                  'Do not add inclusive stages, exclusive profile, affine time, or wall time. '
                                  'Disabled exclusive profiling does not measure zero source cost.',
                'total_build_ms': adaptive.get('total_build_ms'),
                'affine_ms': adaptive.get('affine_ms'), 'planning': adaptive.get('planning'),
                'presentation': adaptive.get('presentation_cpu_ms'),
                'draw_encode': data.get('draw_encode_cpu')},
        'cache_memory': {k: v for k, v in adaptive.items() if any(s in k for s in
                         ('bytes', 'cache', 'memory', 'upload', 'generated', 'builds'))},
        'process_memory': pick(data, ('used_memory_bytes', 'committed_memory_bytes')),
        'image': rgb_hash(path.with_suffix('.png')),
        'issues': issues,
        'raw_relevant': data,
    }


def aggregate(inputs, exclude=()):
    excluded = {Path(p).resolve() for p in exclude}
    paths = set()
    for item in inputs:
        path = Path(item).resolve()
        paths.update(path.rglob('*.json') if path.is_dir() else [path])
    runs, diagnostics = {}, []
    for path in sorted(paths - excluded):
        if path.name.endswith('.capture.json'):
            continue
        try:
            data = read_json(path)
        except (ValueError, OSError) as exc:
            diagnostics.append({'path': str(path), 'error': str(exc)})
            if path.name == 'static.json':
                runs[str(path)] = collect_run(path)
            continue
        if isinstance(data, dict) and data.get('schema') == SCHEMA:
            continue
        if isinstance(data, dict) and ('frames' in data and ('adaptive' in data or 'scene' in data)
                                       or path.name == 'static.json'):
            runs[str(path)] = collect_run(path, data)
        if path.name == 'sweep.json' and isinstance(data, list):
            for row in data:
                if isinstance(row, dict):
                    opts = options(row.get('command', []))
                    if opts.get('--static-only') and opts.get('--output'):
                        target = Path(opts['--output']).resolve() / 'static.json'
                        if not target.exists():
                            runs[str(target)] = collect_run(target)
        if path.name == 'runs.json' and isinstance(data, list):
            for row in data:
                if isinstance(row, dict):
                    target = options(row.get('command', [])).get('--report')
                    if target and not Path(target).exists():
                        target = Path(target).resolve()
                        runs[str(target)] = collect_run(target)
    groups = {}
    for index, run in enumerate(runs.values()):
        # Unknown == unknown is not proof of a matched preset.
        key = json.dumps(run['preset'], sort_keys=True) if not run['missing_preset_fields'] else 'unknown-' + str(index)
        groups.setdefault(key, []).append(run)
    output_groups = []
    for index, members in enumerate(groups.values(), 1):
        group_id = f'preset-{index}'
        for run in members:
            run['comparison_group'] = group_id
        output_groups.append({'id': group_id, 'runs': [r['path'] for r in members],
                              'matched_preset': not members[0]['missing_preset_fields'],
                              'comparison': 'descriptive only; no causal speedup or visual acceptance inferred'})
    return {'schema': SCHEMA, 'runs': list(runs.values()), 'groups': output_groups,
            'diagnostics': diagnostics}


def markdown(report):
    lines = ['# Issue 74 existing-run report', '',
             'Descriptive only. Numerical readiness and FPS do not establish visual or motion acceptance.', '',
             '| Run | Group | First coverage / quality / reserve ms | Frame p50 / p95 / p99 ms | GPU p95 ms / scope | Numerical screen | Missing preset fields |',
             '| --- | --- | --- | --- | --- | --- | --- |']
    def number(value):
        return f'{value:.3f}' if isinstance(value, (float, int)) and math.isfinite(value) else 'unknown'
    for run in report['runs']:
        latency = ' / '.join(number(v) for v in run['first_readiness_ms'].values())
        frames = ' / '.join(number((run['frames'] or {}).get(k)) for k in ('p50_ms', 'p95_ms', 'p99_ms'))
        status = 'passes numerical screen only' if run['benchmark_numerical_screen']['numerical_screen_pass'] else 'failed/incomplete'
        gpu = number((run['gpu_render_pass'] or {}).get('p95_ms')) + ' / ' + run['gpu_timing_scope']
        name = run['path'].replace('|', '\\|')
        lines.append(f"| {name} | {run['comparison_group']} | {latency} | {frames} | {gpu} | {status} | {', '.join(run['missing_preset_fields']) or 'none'} |")
    lines += ['', 'Full identity, command provenance, raw fields, RGB hashes, CPU overlap caveats and failures are in the JSON.', '']
    return '\n'.join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs', nargs='+', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--markdown', type=Path)
    args = parser.parse_args()
    md = args.markdown or args.output.with_suffix('.md')
    report = aggregate(args.inputs, exclude=(args.output, md))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
    md.parent.mkdir(parents=True, exist_ok=True)
    md.write_text(markdown(report))


if __name__ == '__main__':
    main()
