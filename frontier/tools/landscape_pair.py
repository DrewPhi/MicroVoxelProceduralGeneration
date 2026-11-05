#!/usr/bin/env python3
"""Assemble a labelled, same-camera landscape still comparison without resizing."""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from reference_compare import same_camera, ray_distance, stats, srgb


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference', type=Path)
    parser.add_argument('gpu', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--report', type=Path,
                        help='Explicit same-run window report supplying source metadata absent from exit captures')
    args = parser.parse_args()
    reference = json.loads((args.reference / 'manifest.json').read_text())
    gpu = json.loads(args.gpu.with_suffix('.capture.json').read_text())
    if args.report:
        run = json.loads(args.report.read_text())
        assert (run['output']['width'], run['output']['height']) == (gpu['width'], gpu['height']), 'report dimensions differ'
        source = run['source']
        supplied = {
            'seed': run['seed'], 'source_version': source['version'],
            'terrain_recipe': run['terrain_recipe'], 'scene': run['scene'],
            'native_voxel_m': source['native_voxel_m'],
            'horizontal_scale_m': source['horizontal_scale_m'],
            'amplitude_m': source['amplitude_m'], 'octaves': source['octaves'],
            'roughness': source['roughness'],
            'edit_revision': run.get('adaptive', {}).get('displayed_world_revision', run['edits']['world_revision']),
        }
        for key, value in supplied.items():
            assert key not in gpu or gpu[key] == value, f'report/capture mismatch: {key}'
            gpu[key] = value
        if 'adaptive' in run:
            gpu['adaptive'] = run['adaptive']
        gpu['source_metadata_from_explicit_run_report'] = str(args.report)
    same_camera(reference, gpu, require_single_sample=False)
    assert reference['seed'] == gpu['seed'], 'seed mismatch'
    assert reference['source_version'] == gpu['source_version'], 'source mismatch'
    assert gpu['terrain_recipe'] == reference.get('terrain_recipe', 'Regional'), 'recipe mismatch'
    assert gpu['edit_revision'] == 0, 'reference does not contain edits'
    for a, b in [('horizontal_scale', 'horizontal_scale_m'),
                 ('amplitude', 'amplitude_m'), ('octaves', 'octaves'),
                 ('roughness', 'roughness')]:
        assert reference['params'][a] == gpu[b], f'parameter mismatch: {a}'
    if reference['miss_fraction'] != 0:
        # Sky is valid only when the entire view frustum stays within the crop.
        half = np.tan(np.deg2rad(reference['fov_y_degrees']) / 2)
        radius = reference['far'] * np.sqrt(1 + half**2 * (1 + (reference['width']/reference['height'])**2))
        lo = np.array(reference['native_min_xz']) * reference['voxel_size_m']
        hi = lo + reference['native_size_xz'] * reference['voxel_size_m']
        eye = np.array(reference['eye'])[[0,2]]
        assert np.all(eye-radius >= lo) and np.all(eye+radius <= hi), 'sky/crop ambiguity: frustum extends beyond reference'
    width, height = reference['width'], reference['height']
    left = Image.open(args.reference / 'color.png').convert('RGB')
    right = Image.open(args.gpu).convert('RGB')
    assert left.size == right.size == (width, height), 'image size mismatch'
    image = Image.new('RGB', (2 * width, height + 64), '#18212a')
    image.paste(left, (0, 64))
    image.paste(right, (width, 64))
    draw = ImageDraw.Draw(image)
    try:
        font = ImageFont.truetype('DejaVuSans.ttf', 24)
    except OSError:
        font = ImageFont.load_default()
    draw.text((20, 20), f"Exact 6.25 cm voxels | offline {reference['spp']} samples/pixel", font=font, fill='white')
    label = ('Adaptive terrain | projected error + voxel-derived appearance'
             if gpu.get('scene', '').startswith('adaptive')
             else 'Current real-time terrain | LOD + voxel appearance filter')
    if 'adaptive' in gpu:
        label += f" | {gpu['adaptive']['pixel_error']:g}px, MSAA{gpu.get('samples', 1)}"
    draw.text((width + 20, 20), label, font=font, fill='white')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    image.save(args.output)
    shape = (height, width, 3)
    a = np.fromfile(args.reference / 'linear_rgb.f32', dtype='<f4').reshape(shape)
    b = np.fromfile(args.gpu.with_suffix('.linear_rgb.f32'), dtype='<f4').reshape(shape)
    delta = a.astype(np.float64) - b
    report = {'reference': reference, 'gpu': gpu,
              'linear_rgb_rmse': float(np.sqrt(np.mean(delta ** 2))),
              'linear_rgb_mae': float(np.mean(np.abs(delta))),
              'interpretation': 'Appearance comparison, not a geometry error bound: exact sampled microfaces versus filtered LOD geometry/shading.'}
    coverage = np.fromfile(args.reference / 'coverage.f32', dtype='<f4').reshape(height, width)
    full, empty = coverage == 1, coverage == 0
    ref_depth = np.fromfile(args.reference / 'depth.f32', dtype='<f4').reshape(height, width)
    # MSAA captures do not export a resolved depth/coverage attachment. Never
    # read a stale single-sample file left by a previous capture with this name.
    if gpu.get('samples', 1) == 1 and args.gpu.with_suffix('.depth.f32').exists():
        ndc = np.fromfile(args.gpu.with_suffix('.depth.f32'), dtype='<f4').reshape(height, width)
        hit = np.isfinite(ndc) & (ndc >= 0) & (ndc < 1)
        shared = full & hit
        report['geometry_diagnostics'] = {
            'available': True,
            'false_coverage_pixels': int((hit & empty).sum()),
            'missing_coverage_pixels': int((~hit & full).sum()),
            'partial_reference_pixels': int(((coverage > 0) & (coverage < 1)).sum()),
            'coverage_absolute_error': stats(hit.astype(float) - coverage),
            'center_vs_subpixel_mean_depth_difference_m': stats((ray_distance(ndc, gpu) - ref_depth)[shared]),
            'limitations': 'Finite sampled coverage cannot prove thin-feature absence. Depth is candidate pixel-center versus reference subpixel mean, not a strict geometry-error test.',
        }
        diagnostic = np.zeros((height, width, 3), dtype=np.uint8)
        diagnostic[hit & empty] = [255, 0, 255]
        diagnostic[~hit & full] = [0, 255, 255]
        diagnostic[(coverage > 0) & (coverage < 1)] = [90, 90, 90]
        Image.fromarray(diagnostic).save(args.output.with_name(args.output.stem + '-coverage.png'))
    else:
        shared = full
        report['geometry_diagnostics'] = {
            'available': False,
            'limitations': 'This capture has no single-sample depth/coverage export; color is not used to infer geometry.',
        }
        args.output.with_name(args.output.stem + '-coverage.png').unlink(missing_ok=True)
    report['reference_full_terrain_linear_rgb_error'] = stats(delta[full])
    report['fully_covered_terrain_mask'] = ('reference_full_and_candidate_hit'
                                           if report['geometry_diagnostics']['available']
                                           else 'reference_full_only')
    report['fully_covered_terrain_linear_rgb_error'] = stats(delta[shared])
    report['appearance_by_reference_distance_m'] = [
        {'range': [lo, hi],
         'pixels': int((shared & (ref_depth >= lo) & (ref_depth < hi)).sum()),
         'linear_rgb_error': stats(delta[shared & (ref_depth >= lo) & (ref_depth < hi)])}
        for lo, hi in [(0, 10), (10, 25), (25, 50), (50, 100), (100, 200), (200, 500)]
    ]
    Image.fromarray(srgb(np.abs(delta) * 4)).save(args.output.with_name(args.output.stem + '-difference-x4.png'))
    args.output.with_suffix('.json').write_text(json.dumps(report, indent=2) + '\n')
    print(args.output)


if __name__ == '__main__':
    main()
