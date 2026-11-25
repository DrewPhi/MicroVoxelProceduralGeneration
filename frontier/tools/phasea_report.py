#!/usr/bin/env python3
"""Compare explicitly matched Phase A captures; never infer visual acceptance."""
import argparse
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image
from compare_capture import compare as compare_linear, linear_rgb
from compare_volume_capture import compare as compare_bytes

MATCH_KEYS = ('source', 'seed', 'world_version', 'camera', 'far_m', 'width', 'height',
              'voxel_m', 'materials', 'lighting', 'sampling', 'route')
MEASUREMENT_KEYS = ('cpu_model', 'gpu_model', 'binary_sha256', 'command',
                    'p50_ms', 'p95_ms', 'p99_ms', 'cold_ready_ms',
                    'resident_bytes', 'pending_bytes', 'page_or_mesh_count')


def valid(value):
    if value is None or value == '' or value == [] or value == {}:
        return False
    if isinstance(value, (int, float)):
        return not isinstance(value, bool) and math.isfinite(value) and value >= 0
    return True


def capture_pair(a, b, root):
    # Capture-produced metadata is independent evidence; two absent fields do not match.
    sidecars = [json.loads((root / run['rgb']).with_suffix('.capture.json').read_text())
                for run in (a, b)]
    for run, side in zip((a, b), sidecars):
        if run.get('report'):
            benchmark = json.loads((root / run['report']).read_text())
            source = benchmark.get('source', {})
            extracted = {key: source.get(key) for key in
                         ('native_voxel_m', 'horizontal_scale_m', 'amplitude_m', 'octaves', 'roughness')}
            extracted.update(samples=benchmark.get('msaa_samples'), voxel_filter=benchmark.get('voxel_filter'),
                             visual=benchmark.get('visual'), render_appearance_version=benchmark.get('render_appearance_version'),
                             source_version=source.get('version'), seed=benchmark.get('seed'),
                             terrain_recipe=benchmark.get('terrain_recipe'),
                             appearance_version=benchmark.get('appearance_version'))
            for key, value in extracted.items():
                if value is not None:
                    if key in side and side[key] != value:
                        raise ValueError(f'capture/report provenance conflict: {key}')
                    side[key] = value
            if benchmark.get('visual', {}).get('settings_may_have_changed') is True:
                raise ValueError('visual settings changed during benchmark')
            if benchmark.get('benchmark_failed') or benchmark.get('benchmark_interrupted'):
                raise ValueError('benchmark failed or interrupted')
    for key in ('width', 'height', 'eye' , 'target', 'fov_y_degrees', 'near', 'far',
                'samples', 'voxel_filter', 'depth_encoding', 'seed',
                'source_version', 'terrain_recipe', 'native_voxel_m', 'appearance_version',
                'horizontal_scale_m', 'amplitude_m', 'octaves', 'roughness'):
        if any(key not in side or not valid(side[key]) for side in sidecars):
            # False is a valid filter setting, unlike absent/null metadata.
            if key != 'voxel_filter' or any(not isinstance(side.get(key), bool) for side in sidecars):
                raise ValueError(f'missing capture provenance: {key}')
        if sidecars[0][key] != sidecars[1][key]:
            raise ValueError(f'unmatched capture provenance: {key}')
    lighting_keys = ('atmosphere', 'exposure', 'fog_density', 'fog_height_falloff',
                     'sun_elevation', 'sun_azimuth', 'cloud_coverage', 'cloud_speed', 'overcast')
    for run, side in zip((a, b), sidecars):
        visual = side.get('visual', {})
        if any(key not in visual or visual[key] is None for key in (*lighting_keys, 'render_scale')):
            raise ValueError('missing actual lighting/render-scale provenance')
        if any(isinstance(value, float) and not math.isfinite(value) for value in visual.values()):
            raise ValueError('nonfinite actual visual provenance')
        for key in ('atmosphere', 'render_scale'):
            if key in side and side[key] != visual[key]:
                raise ValueError(f'capture/report provenance conflict: {key}')
        if side.get('postprocessed') is True:
            raise ValueError('postprocessed capture needs complete postprocess provenance')
        if 'render_appearance_version' not in side or side['render_appearance_version'] is None:
            raise ValueError('missing actual material/render appearance provenance')
        actual = {
            'lighting': {key: visual[key] for key in lighting_keys},
            'materials': {key: side[key] for key in ('appearance_version', 'render_appearance_version')},
            'sampling': {key: side.get(key) for key in ('samples', 'voxel_filter', 'color_encoding')},
        }
        actual['sampling']['render_scale'] = visual['render_scale']
        if actual['sampling']['color_encoding'] is None:
            raise ValueError('missing actual color encoding')
        for key, value in actual.items():
            if run['match'][key] != value:
                raise ValueError(f'declared metadata disagrees with actual: {key}')
        for declared, observed in (('width', 'width'), ('height', 'height'),
                                   ('far_m', 'far'), ('seed', 'seed'), ('source', 'terrain_recipe'),
                                   ('world_version', 'source_version'), ('voxel_m', 'native_voxel_m')):
            if run['match'][declared] != side[observed]:
                raise ValueError(f'declared metadata disagrees with capture: {declared}')
        if run['match']['camera'] != {key: side[key] for key in ('eye', 'target', 'fov_y_degrees', 'near')}:
            raise ValueError('declared camera disagrees with capture')
    if sidecars[0]['depth_encoding'] != 'ndc_0_1':
        raise ValueError('unsupported depth encoding')
    paths = [root / run[key] for run in (a, b) for key in ('rgb', 'depth')]
    for path in paths:
        if not path.is_file():
            raise ValueError(f'missing capture: {path}')
    with Image.open(paths[0]) as image:
        shape = (image.height, image.width)
    if shape != (a['match']['height'], a['match']['width']):
        raise ValueError('capture dimensions disagree with match metadata')
    for path in (paths[1], paths[3]):
        if path.stat().st_size != shape[0] * shape[1] * 4:
            raise ValueError(f'incorrect depth byte count: {path}')
        depth = np.fromfile(path, dtype='<f4')
        if depth.size != shape[0] * shape[1]:
            raise ValueError(f'incorrect depth byte count: {path}')
        if not np.isfinite(depth).all() or ((depth < 0) | (depth > 1)).any():
            raise ValueError(f'depth must be finite normalized [0,1]: {path}')
    result = compare_bytes(paths[0], paths[2], mesh_depth=paths[1], ray_depth=paths[3])
    result['linear_rgb'] = compare_linear(linear_rgb(paths[0]), linear_rgb(paths[2]))
    result['depth']['reference_hit_pixels'] = int((np.fromfile(paths[1], dtype='<f4') < 1).sum())
    result['depth']['candidate_hit_pixels'] = int((np.fromfile(paths[3], dtype='<f4') < 1).sum())
    if result['depth']['common_hit_pixels'] == 0:
        result['depth']['max_normalized_error'] = None
        result['depth']['p99_normalized_error'] = None
    return result


def report(manifest, root):
    runs = manifest.get('runs', [])
    ids = [r['id'] for r in runs]
    if len(ids) != len(set(ids)):
        raise ValueError('duplicate run IDs')
    indexed = dict(zip(ids, runs))
    results = []
    for run in runs:
        if 'compare_to' not in run:
            continue
        failures = []
        other = indexed.get(run['compare_to'])
        result = {'id': run['id'], 'compare_to': run['compare_to'],
                  'quality_pass': None, 'measurements': run.get('measurements', {}),
                  'failures': failures}
        if other is None:
            failures.append('missing comparison run')
        else:
            for key in MATCH_KEYS:
                if not valid(run.get('match', {}).get(key)) or not valid(other.get('match', {}).get(key)):
                    failures.append(f'missing matching metadata: {key}')
                elif run['match'][key] != other['match'][key]:
                    failures.append(f'unmatched metadata: {key}')
            if not failures:
                try:
                    result['comparison'] = capture_pair(other, run, root)
                except (OSError, ValueError, KeyError) as exc:
                    failures.append(str(exc))
                    # Retain useful color evidence while leaving depth/provenance incomplete.
                    try:
                        result['rgb_only_unverified'] = compare_bytes(root / other['rgb'], root / run['rgb'])
                    except (OSError, ValueError, KeyError):
                        pass
        for item in (run, other):
            if item is not None:
                for key in MEASUREMENT_KEYS:
                    if not valid(item.get('measurements', {}).get(key)):
                        failures.append(f"{item['id']}: unavailable measurement: {key}")
        result['evidence_complete'] = not failures
        results.append(result)
    return {'schema': 'phasea_report_v1', 'comparisons': results,
            'evidence_complete': bool(results) and all(r['evidence_complete'] for r in results),
            'quality_pass': None,
            'note': 'Image agreement does not certify geometry, temporal stability, readiness or performance.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    data = json.loads(args.manifest.read_text())
    result = report(data, args.manifest.resolve().parent)
    args.output.write_text(json.dumps(result, indent=2, allow_nan=False) + '\n')
    raise SystemExit(0 if result['evidence_complete'] else 1)


if __name__ == '__main__':
    main()
