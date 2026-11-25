#!/usr/bin/env python3
"""Sequential, frozen-binary Alpine appearance experiments; no automatic promotion.

All four 720p stills precede all four 1080p motion runs. Existing arm directories
are preserved on resume; choose a new output directory to repeat an arm. Missing
or failed captures stay invalid. Geometry differences are diagnostics, never an
appearance quality pass. --dry-run performs no writes or renderer launches.
--refresh-metadata --output DIR revalidates saved evidence without GPU runs.
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

from adaptive_motion_check import EXPERIMENT_ENV_KEYS, capture_provenance

ROOT = Path(__file__).resolve().parents[1]
METHODS = ('baseline', 'hierarchy', 'visibility', 'map')
RESET_KEYS = tuple(dict.fromkeys((*EXPERIMENT_ENV_KEYS,
                                'FRONTIER_INDEXED_PROXY', 'FRONTIER_GPU_PROFILES')))


def write_json(path, data):
    path.write_text(json.dumps(data, indent=2, allow_nan=False) + '\n')


def read_json(path):
    return json.loads(path.read_text(), parse_constant=lambda _: None)


def sha256(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def freeze(source, target):
    """Snapshot both binaries before any run; refuse mixed-binary resumed evidence."""
    before = sha256(source)
    if not target.exists():
        shutil.copy2(source, target)
    digest = sha256(target)
    if before != digest or sha256(source) != before:
        raise ValueError(f'binary changed or differs from preserved snapshot: {source}')
    return dict(source=str(source), frozen=str(target), sha256=digest)


def environment(method):
    env = os.environ.copy()
    for key in RESET_KEYS:
        env.pop(key, None)
    env['FRONTIER_LOD_APPEARANCE'] = method
    return env


def command(binary, directory, stage, seconds):
    cmd = [str(binary), '--preset', 'alpine', '--scene', 'adaptive',
           '--pixel-error', '3', '--width', '1280' if stage == 'still' else '1920',
           '--height', '720' if stage == 'still' else '1080', '--msaa', '1',
           '--voxel-filter', 'on', '--render-scale', '1', '--atmosphere', 'off',
           '--terrain-shadows', 'off', '--adaptive-workers', '4',
           '--adaptive-refinements', '8']
    capture = str(directory / 'capture.png')
    if stage == 'still':
        cmd += ['--capture-linear', '--capture', capture]
    else:
        cmd += ['--uncapped', '--warmup', '60', '--benchmark-input-lock',
                '--route', 'fly', '--route-clock', 'realtime', '--route-speed', '3',
                '--seconds', str(seconds), '--adaptive-wait-quality',
                '--capture-on-exit', capture, '--report', str(directory / 'report.json')]
    return cmd


def capture_check(directory, width, height, require_float, report_provenance=None):
    """Check independent artifacts before treating an image as usable evidence."""
    import numpy as np
    from PIL import Image
    issues = []
    metadata = {}
    supplement = None
    try:
        metadata = read_json(directory / 'capture.capture.json')
        if report_provenance is not None:
            # Same-invocation report may supply only this missing source label.
            # Compare independently recorded identity before accepting the link.
            report = report_provenance
            identity_issues = []
            if report.get('terrain_recipe') != 'Alpine':
                identity_issues.append('report terrain_recipe mismatch or unavailable: Alpine required')
            output = report.get('output', {})
            for key in ('width', 'height'):
                if output.get(key) is None or output.get(key) != metadata.get(key):
                    identity_issues.append('report/capture output mismatch or unavailable: ' + key)
            for key in ('scene', 'seed', 'adapter'):
                if report.get(key) is None or report.get(key) != metadata.get(key):
                    identity_issues.append('report/capture identity mismatch or unavailable: ' + key)
            for key, value in metadata.get('visual', {}).items():
                if report.get('visual', {}).get(key) != value:
                    identity_issues.append('report/capture visual mismatch or unavailable: ' + key)
            if ('terrain_recipe' in metadata and report.get('terrain_recipe') != metadata['terrain_recipe']):
                identity_issues.append('report/capture terrain_recipe conflict')
            if ('source_version' in metadata and report.get('source', {}).get('version') != metadata['source_version']):
                identity_issues.append('report/capture source_version conflict')
            issues.extend(identity_issues)
            if 'terrain_recipe' not in metadata and not identity_issues:
                if report.get('terrain_recipe') is not None:
                    metadata['terrain_recipe'] = report['terrain_recipe']
                    supplement = dict(field='terrain_recipe', value=report['terrain_recipe'],
                                      source=str(directory / 'report.json'),
                                      report_source_version=report.get('source', {}).get('version'),
                                      note='Missing field supplied in memory from same-invocation report after output, scene, seed, adapter and visual identity checks; capture sidecar unchanged.')
        with Image.open(directory / 'capture.png') as image:
            image.load()
            if image.size != (width, height):
                issues.append('PNG dimensions disagree with requested viewport')
        if (metadata.get('width'), metadata.get('height')) != (width, height):
            issues.append('capture metadata dimensions disagree')
        depth_path = directory / 'capture.depth.f32'
        depth = np.fromfile(depth_path, dtype='<f4')
        if depth.size != width * height or not np.isfinite(depth).all() or ((depth < 0) | (depth > 1)).any():
            issues.append('invalid normalized depth buffer')
        coverage = np.fromfile(directory / 'capture.coverage.f32', dtype='<f4')
        if coverage.size != width * height or not np.isfinite(coverage).all() or ((coverage < 0) | (coverage > 1)).any():
            issues.append('invalid coverage buffer')
        if require_float:
            rgb = np.fromfile(directory / 'capture.linear_rgb.f32', dtype='<f4')
            if rgb.size != width * height * 3 or not np.isfinite(rgb).all():
                issues.append('invalid float32 RGB buffer')
            if metadata.get('color_encoding') != 'linear_rgb_float32_render_target':
                issues.append('capture is not actual float32 RGB render target')
        for key, wanted in {'samples': 1, 'atmosphere': False, 'render_scale': 1,
                            'postprocessed': False, 'terrain_recipe': 'Alpine'}.items():
            if metadata.get(key) != wanted:
                issues.append(f'capture setting mismatch or unavailable: {key}')
        visual = metadata.get('visual', {})
        for key in ('terrain_shadows_requested', 'terrain_shadows_active'):
            if visual.get(key) is not False:
                issues.append(f'capture setting mismatch or unavailable: {key}')
    except (OSError, ValueError) as exc:
        issues.append(str(exc))
    return dict(artifacts_valid=not issues, issues=issues, metadata=metadata,
                provenance_supplement=supplement)


def report_summary(report):
    adaptive = report.get('adaptive', {})
    quality = report.get('adaptive_route_quality', {})
    memory = {k: v for k, v in adaptive.items()
              if 'bytes' in k or k in ('build_memory_model', 'appearance_cache',
                                      'cold_source_residency', 'indexed_proxy', 'gpu_profiles')}
    frames = report.get('frames', {})
    mismatches = []
    measured = quality.get('measured_frames')
    accepted = frames.get('accepted_frames', frames.get('sample_count'))
    if measured is not None and accepted is not None and measured != accepted:
        mismatches.append('quality measured_frames differs from timing accepted_frames')
    if frames.get('sample_count') != accepted:
        mismatches.append('timing distribution is not the complete accepted route')
    for key in ('coverage_ready_frames', 'quality_ready_frames'):
        if measured is not None and quality.get(key) != measured:
            mismatches.append(key + ' differs from measured_frames')
    return dict(frame_ms={k: report.get('frames', {}).get(k)
                         for k in ('p50_ms', 'p95_ms', 'p99_ms', 'sample_count', 'accepted_frames')},
                gpu_render_pass=report.get('gpu_render_pass'),
                gpu_timing_scope=report.get('visual', {}).get('gpu_timing_scope'),
                measurement_mismatches=mismatches,
                memory=memory,
                readiness={k: adaptive.get(k) for k in (
                    'first_coverage_ms', 'first_quality_ms', 'first_refinement_ms',
                    'coverage_ready', 'quality_ready', 'refinement_ready',
                    'cpu_quality_ready', 'cpu_refinement_ready', 'elapsed_preparation_ms')},
                route_quality=quality or None, triangles=adaptive.get('triangles'),
                route_elapsed_seconds=report.get('route_elapsed_seconds'),
                adapter=report.get('adapter'), quality_pass=None)


def run_arm(stage, method, binary, out, seconds, timeout, reference):
    directory = out / stage / method
    if directory.exists():
        record = directory / 'run.json'
        if record.is_file():
            return read_json(record)
        return dict(stage=stage, method=method, valid=False,
                    errors=['preserved incomplete arm directory; choose new output to rerun'])
    directory.mkdir(parents=True)
    cmd, env = command(binary, directory, stage, seconds), environment(method)
    row = dict(stage=stage, method=method, command=cmd, command_shell=shlex.join(cmd),
               capture=str(directory / 'capture.png'), report=str(directory / 'report.json'),
               environment={k: env.get(k) for k in RESET_KEYS},
               capture_provenance=capture_provenance(binary, environ=env), errors=[])
    write_json(directory / 'command.json', row)
    print('+', shlex.join(cmd), flush=True)
    start = time.monotonic()
    try:
        with (directory / 'run.log').open('w') as log:
            result = subprocess.run(cmd, cwd=ROOT, env=env, stdout=log,
                                    stderr=subprocess.STDOUT, timeout=timeout)
        row['exit_code'] = result.returncode
        if result.returncode:
            row['errors'].append(f'renderer exited {result.returncode}')
    except (OSError, subprocess.TimeoutExpired) as exc:
        row['exit_code'] = None
        row['errors'].append(str(exc))
    row['wall_seconds'] = time.monotonic() - start
    row['process_errors'] = list(row['errors'])
    return analyze_arm(row, directory, seconds, reference, annotate_report=True)


def analyze_arm(row, directory, seconds, reference, annotate_report=False):
    """Revalidate saved artifacts without launching or altering capture sidecars."""
    stage = row['stage']
    previous_issues = row.get('capture_validation', {}).get('issues', [])
    process_errors = row.get('process_errors')
    if process_errors is None:
        # Legacy run records predate process_errors. Remove only errors whose
        # artifact/timing checks are repeated below; retain process failures.
        repeated = ('missing or invalid measured timing:', 'no measured timing samples',
                    'requested realtime route duration not completed',
                    'missing or unreadable timing report:')
        process_errors = [e for e in row.get('errors', [])
                          if e not in previous_issues and not e.startswith(repeated)]
    row['process_errors'] = list(process_errors)
    row['errors'] = list(process_errors)
    report = None
    if stage == 'motion':
        try:
            report = read_json(directory / 'report.json')
            frames = report.get('frames', {})
            for key in ('p50_ms', 'p95_ms', 'p99_ms'):
                value = frames.get(key)
                if not isinstance(value, (float, int)) or not math.isfinite(value) or value <= 0:
                    row['errors'].append('missing or invalid measured timing: ' + key)
            if frames.get('sample_count', 0) <= 0:
                row['errors'].append('no measured timing samples')
            if report.get('route_elapsed_seconds', 0) + 0.1 < seconds:
                row['errors'].append('requested realtime route duration not completed')
            if report.get('benchmark_interrupted') or report.get('benchmark_failed'):
                row['errors'].append('benchmark report declares interruption or failure')
            if annotate_report:
                report['benchmark_requested_seconds'] = seconds
                report['benchmark_run_exit_code'] = row['exit_code']
                report['capture_provenance'] = row['capture_provenance']
                write_json(directory / 'report.json', report)
        except (OSError, ValueError) as exc:
            row['errors'].append('missing or unreadable timing report: ' + str(exc))
    check = capture_check(directory, 1280 if stage == 'still' else 1920,
                          720 if stage == 'still' else 1080, stage == 'still', report)
    row['capture_validation'] = {k: v for k, v in check.items() if k != 'metadata'}
    row['errors'].extend(check['issues'])
    row['actual'] = report_summary(report if report is not None else check['metadata'])
    row['valid'] = not row['errors']
    if stage == 'still' and row['valid']:
        if (reference / 'manifest.json').is_file():
            try:
                from alpine_tuning_metrics import metrics
                row['reference_metrics'] = metrics(reference, directory / 'capture.png')
                write_json(directory / 'reference-metrics.json', row['reference_metrics'])
            except (OSError, ValueError, KeyError) as exc:
                row['reference_metrics'] = dict(unavailable=str(exc), quality_pass=None)
        else:
            row['reference_metrics'] = dict(unavailable='reference manifest does not exist', quality_pass=None)
    write_json(directory / 'run.json', row)
    return row


def geometry_diagnostics(out, baseline, candidate):
    """Exact still-buffer comparison; no tolerance silently excuses changes."""
    import numpy as np
    if not baseline or not candidate or not baseline.get('valid') or not candidate.get('valid'):
        return dict(available=False, reason='baseline or candidate still failed/missing')
    try:
        paths = [out / 'still' / row['method'] for row in (baseline, candidate)]
        sides = [read_json(p / 'capture.capture.json') for p in paths]
        keys = ('width', 'height', 'eye', 'target', 'near', 'far', 'fov_y_degrees',
                'samples', 'seed', 'source_version', 'edit_revision', 'terrain_recipe')
        mismatches = [k for k in keys if k not in sides[0] or k not in sides[1] or sides[0][k] != sides[1][k]]
        depth = [np.fromfile(p / 'capture.depth.f32', dtype='<f4') for p in paths]
        if depth[0].shape != depth[1].shape:
            raise ValueError('depth dimensions differ')
        hit = [d < 1 for d in depth]
        common = hit[0] & hit[1]
        delta = np.abs(depth[0][common] - depth[1][common])
        triangles = [side.get('adaptive', {}).get('triangles') for side in sides]
        depth_changed = int(np.count_nonzero(depth[0] != depth[1]))
        coverage = [np.fromfile(p / 'capture.coverage.f32', dtype='<f4') for p in paths]
        if coverage[0].shape != depth[0].shape or coverage[1].shape != depth[1].shape:
            raise ValueError('coverage dimensions differ from depth')
        coverage_changed = int(np.count_nonzero(coverage[0] != coverage[1]))
        depth_hit_changed = int(np.count_nonzero(hit[0] != hit[1]))
        same = not mismatches and not depth_changed and not coverage_changed and triangles[0] is not None and triangles[0] == triangles[1]
        return dict(available=True, metadata_mismatches=mismatches,
                    depth_differing_pixels=depth_changed, coverage_differing_pixels=coverage_changed,
                    depth_hit_mask_differing_pixels=depth_hit_changed,
                    common_hit_pixels=int(common.sum()),
                    common_hit_max_normalized_depth_error=float(delta.max()) if delta.size else None,
                    triangles=dict(baseline=triangles[0], candidate=triangles[1]),
                    geometry_unchanged=same, quality_pass=None,
                    note='Any difference remains explicit; minor readiness/selection differences may explain it but are not waived.')
    except (OSError, ValueError) as exc:
        return dict(available=False, reason=str(exc))


def summary(out, runs, frozen):
    by_key = {(r['stage'], r['method']): r for r in runs}
    ratios = {}
    baseline = by_key.get(('motion', 'baseline'), {})
    for method in METHODS[1:]:
        candidate = by_key.get(('motion', method), {})
        values = {}
        if baseline.get('valid') and candidate.get('valid'):
            for key in ('p50_ms', 'p95_ms', 'p99_ms'):
                a = baseline['actual']['frame_ms'].get(key)
                b = candidate['actual']['frame_ms'].get(key)
                values[key] = b / a if isinstance(a, (int, float)) and a > 0 and isinstance(b, (int, float)) else None
        ratios[method] = dict(candidate_over_baseline=values or None,
                              lower_is_faster=True, quality_pass=None)
    return dict(schema='frontier-lod-method-compare-v1', frozen_binaries=frozen, runs=runs,
                geometry={m: geometry_diagnostics(out, by_key.get(('still', 'baseline')),
                                                 by_key.get(('still', m))) for m in METHODS[1:]},
                motion_endpoint_comparison='Unavailable: realtime routes end at different trajectory frames.',
                performance_ratios=ratios, quality_pass=None, default_promoted=False,
                limitations=['Still wall time includes source preparation and readback; it is not frame timing.',
                             'Timing ratios do not imply matching route quality or visual acceptance.',
                             'Inspect route quality counts, memory, readiness and failed artifacts independently.'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline-binary', type=Path)
    parser.add_argument('--candidate-binary', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--reference-dir', type=Path, default=ROOT / 'output/alpine-tuning/reference')
    parser.add_argument('--seconds', type=float, default=6.)
    parser.add_argument('--timeout', type=float, default=1200., help='Per arm wall-time limit in seconds')
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--stills-only', action='store_true')
    group.add_argument('--motion-only', action='store_true')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--refresh-metadata', action='store_true',
                        help='Only revalidate saved arms and regenerate summary; never launch a renderer')
    args = parser.parse_args()
    if any(not math.isfinite(n) or n <= 0 for n in (args.seconds, args.timeout)):
        parser.error('--seconds and --timeout must be finite positive numbers')
    out = args.output.resolve()
    if args.refresh_metadata:
        if args.dry_run or args.stills_only or args.motion_only:
            parser.error('--refresh-metadata is separate from run selection and dry-run')
        manifest = read_json(out / 'manifest.json')
        frozen = manifest['sources']
        reference = Path(manifest['reference_dir'])
        runs = []
        for stage in ('still', 'motion'):
            for method in METHODS:
                directory = out / stage / method
                if (directory / 'run.json').is_file():
                    runs.append(analyze_arm(read_json(directory / 'run.json'), directory,
                                            manifest['seconds'], reference))
        write_json(out / 'summary.json', summary(out, runs, frozen))
        if any(not row.get('valid') for row in runs):
            raise SystemExit(1)
        return
    if args.baseline_binary is None or args.candidate_binary is None:
        parser.error('--baseline-binary and --candidate-binary are required for renderer runs')
    sources = dict(baseline=args.baseline_binary.resolve(), candidate=args.candidate_binary.resolve())
    stages = ['still'] if args.stills_only else ['motion'] if args.motion_only else ['still', 'motion']
    if args.dry_run:
        for stage in stages:
            for method in METHODS:
                binary = out / 'bin' / ('baseline' if method == 'baseline' else 'candidate')
                print(json.dumps(dict(stage=stage, method=method,
                                      command=command(binary, out / stage / method, stage, args.seconds),
                                      environment={k: environment(method).get(k) for k in RESET_KEYS})))
        return
    (out / 'bin').mkdir(parents=True, exist_ok=True)
    try:
        frozen = {name: freeze(source, out / 'bin' / name) for name, source in sources.items()}
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
    manifest = out / 'manifest.json'
    settings = dict(seconds=args.seconds, sources=frozen, reference_dir=str(args.reference_dir.resolve()))
    if manifest.exists() and read_json(manifest) != settings:
        parser.error('resume settings differ from preserved manifest; choose a new output')
    write_json(manifest, settings)
    runs = []
    for stage in ('still', 'motion'):
        for method in METHODS:
            record = out / stage / method / 'run.json'
            if stage in stages:
                binary = out / 'bin' / ('baseline' if method == 'baseline' else 'candidate')
                runs.append(run_arm(stage, method, binary, out, args.seconds, args.timeout,
                                    args.reference_dir.resolve()))
                write_json(out / 'summary.json', summary(out, runs, frozen))
            elif record.exists():
                runs.append(read_json(record))
    write_json(out / 'summary.json', summary(out, runs, frozen))
    if any(not run.get('valid') for run in runs):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
