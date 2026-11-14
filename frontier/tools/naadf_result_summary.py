#!/usr/bin/env python3
"""Curate radius-sweep reports without aggregating unmatched runs or GPU spans.

Accept sweep directories, parent directories, or manifest.json files. Missing or
failed cases remain visible. Publication percentiles use nearest rank and include
all reported stream stages, which can include incremental startup growth.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def select(data, names):
    return {name: data[name] for name in names if name in data}


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def summarize(manifest_path):
    manifest = json.loads(manifest_path.read_text())
    rows = []
    for name, case in manifest.get('cases', {}).items():
        report_path = manifest_path.parent / (name + '.json')
        report = case.get('measurements') or {}
        origin = 'manifest_case_measurements'
        report_error = None
        report_hash = None
        if report_path.exists():
            try:
                raw = report_path.read_bytes()
                report = json.loads(raw)
                report_hash = hashlib.sha256(raw).hexdigest()
                origin = 'raw_report'
            except (OSError, ValueError) as error:
                report_error = str(error)
        n = report.get('naadf') or {}
        stages = [row for row in n.get('stream_stage_ms', [])
                  if isinstance(row, list) and len(row) == 4 and all(finite(v) for v in row)]
        names = ['prepare_ready', 'upload_ready', 'fallback_ready', 'request_to_publication']
        stage_percentiles = {
            stage: {'p50_ms': percentile([row[i] for row in stages], .5),
                    'p95_ms': percentile([row[i] for row in stages], .95),
                    'p99_ms': percentile([row[i] for row in stages], .99)}
            for i, stage in enumerate(names)
        }
        publications = n.get('startup_publications_radius_seconds', [])
        valid_publications = [row for row in publications
                              if isinstance(row, list) and len(row) == 2 and all(finite(v) for v in row)]
        radius = case.get('radius_m', n.get('requested_radius_m'))
        final_seconds = next((seconds for published, seconds in valid_publications
                              if radius is not None and published >= radius), None)
        adaptive = report.get('adaptive') or {}
        vegetation = report.get('vegetation') or {}
        rows.append({
            'provenance': {
                'manifest': str(manifest_path), 'case': name,
                'report': str(report_path), 'data_origin': origin,
                'report_sha256': report_hash, 'report_error': report_error,
                'binary_sha256': manifest.get('binary_sha256'),
                'experiment_environment': manifest.get('experiment_environment'),
                'conditions': manifest.get('conditions'), 'command': case.get('command'),
                'report_metadata': select(report, [
                    'schema', 'scene', 'seed', 'source', 'terrain_recipe', 'representation',
                    'appearance_version', 'render_appearance_version', 'route', 'route_clock',
                    'route_speed_mps', 'route_height_policy', 'route_elapsed_seconds',
                    'camera_start', 'projection', 'output', 'msaa_samples', 'adapter',
                    'experiments', 'visual', 'beauty', 'naadf_ray_profile',
                    'development_resources', 'adaptive_preparation_policy']),
            },
            'radius_m': radius, 'status': case.get('status'),
            'returncode': case.get('returncode'), 'wall_seconds': case.get('wall_seconds'),
            'benchmark_failed': report.get('benchmark_failed'),
            'benchmark_interrupted': report.get('benchmark_interrupted'),
            'frames': report.get('frames'),
            'gpu_full_render': report.get('gpu_full_render'),
            'naadf_gpu_pass': report.get('naadf_gpu_pass'),
            'process_memory': report.get('process_memory'),
            'startup': {
                'final_requested_radius_publication_seconds': final_seconds,
                'publications_radius_seconds': publications,
                **select(n, ['startup_expanding', 'incremental_startup', 'resident_radius_m',
                             'startup_generated_roots', 'arena_resizes', 'arena_copy_bytes']),
            },
            'publication_stages': {
                'scope': 'All reported stream_stage_ms samples; may include incremental startup growth',
                'percentile_method': 'nearest_rank', 'valid_sample_count': len(stages),
                'reported_sample_count': len(n.get('stream_stage_ms', [])),
                'stages': stage_percentiles,
            },
            'naadf_counters': select(n, [
                'build_ms', 'upload_cpu_ms', 'mixed_chunks', 'unique_blocks', 'sampled_columns',
                'plant_cells', 'palette_entries', 'gpu_payload_bytes', 'gpu_leaf_bits',
                'stream_shifts', 'stream_build_ms', 'stream_sampled_columns',
                'stream_shared_leaves_last_shift', 'stream_outside_frames', 'min_camera_margin_m',
                'native_column_cache', 'stream_upload', 'cpu_root_refresh', 'cpu_root_refresh_ms',
                'cpu_root_occupancy_bytes', 'cpu_profile_enabled', 'cpu_profile']),
            'ground_cover': report.get('ground_cover'),
            'adaptive_prepare': select(adaptive, [
                'elapsed_preparation_ms', 'first_coverage_ms', 'first_quality_ms',
                'quality_cycle_latency', 'preparation_cpu_stage_ms', 'preparation_stage_note',
                'terrain_prepare_stages_cpu_ms', 'terrain_prepare_detail_cpu_ms',
                'deferred_ground_cover', 'prepared_view_cache', 'cache_hits', 'builds',
                'generated_columns', 'source_cache_hits', 'source_cache_bytes', 'planning']),
            'vegetation_prepare': select(vegetation, [
                'pending_jobs', 'missing_tiles', 'missing_templates', 'resident_templates',
                'ground_assembly_cache', 'jobs', 'local_edit_cache']),
            'coverage': report.get('adaptive_route_quality'),
            'motion': case.get('motion'),
        })
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs', type=Path, nargs='+')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    manifests = set()
    for path in args.inputs:
        if path.is_file():
            manifests.add(path.resolve())
        elif path.is_dir():
            manifests.update(p.resolve() for p in path.rglob('manifest.json'))
        else:
            parser.error(f'input does not exist: {path}')
    runs = []
    for path in sorted(manifests):
        runs.extend(summarize(path))
    if not runs:
        parser.error('no radius-sweep cases found')
    result = {'schema': 'naadf_result_summary_v1',
              'note': 'Individual runs only; no paired performance conclusion or additive GPU accounting.',
              'runs': runs}
    text = json.dumps(result, indent=2, allow_nan=False) + '\n'
    if args.output:
        args.output.write_text(text)
    else:
        print(text, end='')


if __name__ == '__main__':
    main()
