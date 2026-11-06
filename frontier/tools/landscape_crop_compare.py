#!/usr/bin/env python3
"""Compare a finite native reference pixel crop with a full GPU color capture.

Appearance diagnostics only. Requires NumPy/Pillow; never infers geometry from
color or treats finite reference samples as a strict coverage/depth oracle.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

from reference_compare import read, same_camera, srgb, stats


def validate_reference(reference, gpu):
    full = dict(reference, width=reference['viewport_width'],
                height=reference['viewport_height'])
    same_camera(full, gpu, require_single_sample=False)
    x, y, w, h = reference['pixel_crop']
    if (min(x, y) < 0 or min(w, h) <= 0 or x + w > full['width']
            or y + h > full['height'] or (w, h) != (reference['width'], reference['height'])):
        raise ValueError('invalid reference pixel crop')
    if reference.get('finite_domain_miss_samples') != 0:
        raise ValueError('reference has incomplete or unreported finite-domain misses')
    for a, b in [('seed', 'seed'), ('source_version', 'source_version'),
                 ('terrain_recipe', 'terrain_recipe'), ('voxel_size_m', 'native_voxel_m'),
                 ('appearance_version', 'appearance_version')]:
        if reference[a] != gpu[b]:
            raise ValueError(f'world mismatch: {a}')
    for a, b in [('horizontal_scale', 'horizontal_scale_m'),
                 ('amplitude', 'amplitude_m'), ('octaves', 'octaves'),
                 ('roughness', 'roughness')]:
        if reference['params'][a] != gpu[b]:
            raise ValueError(f'parameter mismatch: {a}')
    if gpu['edit_revision'] != 0 or gpu['scene'] != 'adaptive':
        raise ValueError('crop comparison requires unedited adaptive surface source')
    return x, y, w, h


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference', type=Path)
    parser.add_argument('gpu', type=Path)
    parser.add_argument('--report', type=Path, required=True,
                        help='Explicit same-run benchmark report supplying source metadata')
    parser.add_argument('--convergence', type=Path,
                        help='Same crop at another sample count, e.g. ridge16 against ridge64')
    parser.add_argument('--output', type=Path, required=True, help='Comparison PNG; JSON shares stem')
    args = parser.parse_args()
    reference = json.loads((args.reference / 'manifest.json').read_text())
    gpu = json.loads(args.gpu.with_suffix('.capture.json').read_text())
    run = json.loads(args.report.read_text())
    if (run['output']['width'], run['output']['height']) != (gpu['width'], gpu['height']):
        raise ValueError('report/capture output dimensions differ')
    source = run['source']
    supplied = {
        'seed': run['seed'], 'source_version': source['version'],
        'terrain_recipe': run['terrain_recipe'], 'scene': run['scene'],
        'native_voxel_m': source['native_voxel_m'],
        'appearance_version': run['appearance_version'],
        'horizontal_scale_m': source['horizontal_scale_m'],
        'amplitude_m': source['amplitude_m'], 'octaves': source['octaves'],
        'roughness': source['roughness'],
        'edit_revision': run['adaptive']['displayed_world_revision'],
    }
    for key, value in supplied.items():
        if key in gpu and gpu[key] != value:
            raise ValueError(f'report/capture mismatch: {key}')
        gpu[key] = value
    x, y, w, h = validate_reference(reference, gpu)
    shape = (h, w, 3)
    a = read(args.reference / 'linear_rgb.f32', shape)
    full = read(args.gpu.with_suffix('.linear_rgb.f32'), (gpu['height'], gpu['width'], 3))
    b = full[y:y+h, x:x+w]
    if not np.isfinite(a).all() or not np.isfinite(b).all():
        raise ValueError('nonfinite linear RGB payload')
    coverage = read(args.reference / 'coverage.f32', (h, w))
    depth = read(args.reference / 'depth.f32', (h, w))
    delta = b - a
    result = {
        'reference': str(args.reference), 'gpu': str(args.gpu), 'report': str(args.report),
        'pixel_crop': [x, y, w, h], 'viewport': [gpu['width'], gpu['height']],
        'reference_spp': reference['spp'], 'gpu_samples': gpu['samples'],
        'gpu_color_encoding': gpu['color_encoding'],
        'reference_manifest': reference,
        'candidate_quality': {
            'coverage_ready': gpu.get('adaptive', {}).get('coverage_ready'),
            'quality_ready': gpu.get('adaptive', {}).get('quality_ready'),
            'max_visible_error_px': gpu.get('adaptive', {}).get('max_visible_error_px'),
            'benchmark_failed': run.get('benchmark_failed'),
            'benchmark_interrupted': run.get('benchmark_interrupted'),
        },
        'linear_rgb_error': stats(delta),
        'reference_full_terrain_linear_rgb_error': stats(delta[coverage == 1]),
        'reference_partial_pixels': int(((coverage > 0) & (coverage < 1)).sum()),
        'reference_mean_hit_depth_m': stats(depth),
        'appearance_by_reference_mean_hit_distance_m': [
            {'range': [lo, hi], 'pixels': int(((depth >= lo) & (depth < hi)).sum()),
             'linear_rgb_error': stats(delta[(depth >= lo) & (depth < hi)])}
            for lo, hi in [(0, 300), (300, 600), (600, 800), (800, 1200)]],
        'geometry': {'evaluated': False},
        'limitations': [
            'Color differences include geometry, filtering, lighting, sampling and target quantization.',
            'Finite sampled reference coverage/depth means are not exact pixel-center geometry.',
            'No geometry pass or full-view quality acceptance is inferred.',
        ],
    }
    if args.convergence:
        other = json.loads((args.convergence / 'manifest.json').read_text())
        validate_reference(other, gpu)
        if (other['pixel_crop'] != reference['pixel_crop']
                or other['cache_identity_fnv1a64'] != reference['cache_identity_fnv1a64']):
            raise ValueError('reference convergence crop/source mismatch')
        c = read(args.convergence / 'linear_rgb.f32', shape)
        cov = read(args.convergence / 'coverage.f32', (h, w))
        result['reference_sampling_difference'] = {
            'other': str(args.convergence), 'other_spp': other['spp'],
            'linear_rgb_error': stats(c - a), 'coverage_error': stats(cov - coverage),
            'interpretation': 'Finite-sample sensitivity diagnostic, not a bound or correction to GPU error.',
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    image = Image.new('RGB', (w * 2, h + 24), '#18212a')
    image.paste(Image.open(args.reference / 'color.png').convert('RGB'), (0, 24))
    candidate = Image.open(args.gpu).convert('RGB')
    if candidate.size != (gpu['width'], gpu['height']):
        raise ValueError('GPU PNG dimensions differ from capture metadata')
    image.paste(candidate.crop((x, y, x+w, y+h)), (w, 24))
    draw = ImageDraw.Draw(image)
    draw.text((2, 4), f"Ref {reference['spp']}", fill='white')
    draw.text((w+2, 4), f"GPU {gpu['samples']}", fill='white')
    image.save(args.output)
    Image.fromarray(srgb(np.abs(delta) * 4)).save(
        args.output.with_name(args.output.stem + '-difference-x4.png'))
    args.output.with_suffix('.json').write_text(json.dumps(result, indent=2) + '\n')
    print(args.output)


if __name__ == '__main__':
    main()
