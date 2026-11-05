#!/usr/bin/env python3
"""Assemble a labelled, same-camera landscape still comparison without resizing."""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont
from reference_compare import same_camera


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference', type=Path)
    parser.add_argument('gpu', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    reference = json.loads((args.reference / 'manifest.json').read_text())
    gpu = json.loads(args.gpu.with_suffix('.capture.json').read_text())
    same_camera(reference, gpu)
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
    draw.text((width + 20, 20), 'Current real-time terrain | LOD + voxel appearance filter', font=font, fill='white')
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
    args.output.with_suffix('.json').write_text(json.dumps(report, indent=2) + '\n')
    print(args.output)


if __name__ == '__main__':
    main()
