#!/usr/bin/env python3
"""JSON appearance diagnostics for matched Alpine stills; never a geometry pass.

Usage: alpine_tuning_metrics.py REFERENCE GPU.png [--provenance settings.json]
Extra provenance supplies missing capture fields (including nested visual fields).
Conflicting or unknown required settings are rejected, not guessed.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image

from reference_compare import read, same_camera


def merge_missing(actual, supplied):
    for key, value in supplied.items():
        if key not in actual:
            actual[key] = value
        elif isinstance(value, dict) and isinstance(actual[key], dict):
            merge_missing(actual[key], value)
        elif actual[key] != value:
            raise ValueError('conflicting explicit provenance: ' + key)


def require(data, key):
    if key not in data or data[key] is None:
        raise ValueError('unknown required provenance: ' + key)
    return data[key]


def equal(a, b, key):
    if a != b:
        raise ValueError('unmatched provenance: ' + key)


def metrics(reference, capture, provenance=None):
    ref = json.loads((reference / 'manifest.json').read_text())
    gpu = json.loads(capture.with_suffix('.capture.json').read_text())
    if provenance:
        merge_missing(gpu, json.loads(provenance.read_text()))
    cameras = []
    for data in (ref, gpu):
        camera = dict(data)
        for key in ('eye', 'target'):
            camera[key] = np.asarray(require(data, key), dtype=np.float64)
            if camera[key].shape != (3,) or not np.isfinite(camera[key]).all():
                raise ValueError('invalid camera vector: ' + key)
        if np.linalg.norm(camera['target'] - camera['eye']) == 0:
            raise ValueError('zero camera direction')
        cameras.append(camera)
    same_camera(*cameras, require_single_sample=True)
    equal(require(ref, 'pixel_crop'), [0, 0, ref['width'], ref['height']], 'full viewport')
    for key in ('seed', 'source_version', 'appearance_version', 'terrain_recipe'):
        equal(require(ref, key), require(gpu, key), key)
    equal(require(ref, 'terrain_recipe'), 'Alpine', 'Alpine source')
    equal(require(ref, 'voxel_size_m'), require(gpu, 'native_voxel_m'), 'voxel size')
    equal(require(gpu, 'edit_revision'), 0, 'unedited source')
    for a, b in [('horizontal_scale', 'horizontal_scale_m'), ('amplitude', 'amplitude_m'),
                 ('octaves', 'octaves'), ('roughness', 'roughness')]:
        equal(require(ref['params'], a), require(gpu, b), a)
    visual = require(gpu, 'visual')
    light = require(ref, 'lighting')
    equal(require(light, 'profile'), 'regional', 'lighting profile')
    for a, b in [('sun_elevation_degrees', 'sun_elevation'),
                 ('sun_azimuth_degrees', 'sun_azimuth'), ('overcast', 'overcast')]:
        equal(require(light, a), require(visual, b), a)
    for data in (ref, gpu, visual):
        equal(require(data, 'atmosphere'), False, 'atmosphere disabled')
    equal(require(ref, 'fog'), False, 'reference fog')
    # Fog is applied only by the atmosphere compositor, required off above.
    # Keep the configured value in diagnostics without treating it as active fog.
    fog_density = require(visual, 'fog_density')
    if not np.isfinite(fog_density) or fog_density < 0:
        raise ValueError('invalid configured fog density')
    equal(require(ref, 'shadows'), False, 'reference shadows')
    equal(require(visual, 'terrain_shadows_requested'), False, 'runtime shadows requested')
    equal(require(visual, 'terrain_shadows_active'), False, 'runtime shadows active')
    equal(require(gpu, 'render_scale'), 1, 'render scale')
    for data in (ref, visual):
        equal(require(data, 'exposure'), 1, 'exposure')
    equal(require(gpu, 'postprocessed'), False, 'postprocessing')
    equal(require(gpu, 'depth_encoding'), 'ndc_0_1', 'depth encoding')
    encoding = require(gpu, 'color_encoding')
    if encoding not in ('linear_rgb_float32_render_target', 'linear_rgb_decoded_from_8bit_target'):
        raise ValueError('unknown color encoding: ' + encoding)
    h, w = ref['height'], ref['width']
    equal(list(Image.open(capture).size), [w, h], 'PNG dimensions')
    a = read(reference / 'linear_rgb.f32', (h, w, 3))
    b = read(capture.with_suffix('.linear_rgb.f32'), (h, w, 3))
    coverage = read(reference / 'coverage.f32', (h, w))
    depth = read(capture.with_suffix('.depth.f32'), (h, w))
    if not np.isfinite(a).all() or not np.isfinite(b).all():
        raise ValueError('nonfinite RGB')
    hit = np.isfinite(depth) & (depth >= 0) & (depth < 1)
    terrain = coverage == 1
    common = terrain & hit
    interior = np.zeros_like(terrain)
    interior[1:-1, 1:-1] = np.logical_and.reduce([
        terrain[1+dy:h-1+dy, 1+dx:w-1+dx]
        for dy in (-1, 0, 1) for dx in (-1, 0, 1)])
    lower = np.indices((h, w))[0] >= int(.7 * h)
    strip = max(1, w // 20)
    edge = np.zeros_like(terrain)
    edge[:, :strip] = True
    edge[:, -strip:] = True
    delta = b - a

    def region(mask):
        values = delta[mask]
        return dict(pixels=int(mask.sum()), linear_rgb_rmse=float(np.sqrt(np.mean(values**2))) if values.size else None,
                    linear_rgb_mae=float(np.mean(np.abs(values))) if values.size else None)

    return dict(reference=str(reference), capture=str(capture), supplied_provenance=str(provenance) if provenance else None,
                color_encoding=encoding, width=w, height=h,
                runtime_fog=dict(active=False, configured_density=fog_density,
                                 reason='atmosphere disabled and capture not postprocessed'),
                regions=dict(common_full_terrain=region(common), lower_30_percent_image_roi=region(common & lower),
                             side_5_percent_image_strips=region(common & edge)),
                interior_uncovered_pixels=int((interior & ~hit).sum()),
                lower_roi_interior_uncovered_pixels=int((interior & lower & ~hit).sum()),
                reference_finite_domain_miss_samples=require(ref, 'finite_domain_miss_samples'),
                reference_depth_encoding=require(ref, 'depth'),
                geometry_pass=None,
                limitations=['Lower 30% is an image ROI, not an exact-native or distance mask.',
                             'Reference depth is mean hit distance, not pixel-center geometry; no depth error is inferred.',
                             'Uncovered counts compare runtime center coverage with a 3x3 interior of fully covered reference pixels.',
                             'Color error includes sampling, filtering, geometry and shading; finite-domain reference misses remain incomplete coverage.'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference', type=Path)
    parser.add_argument('capture', type=Path)
    parser.add_argument('--provenance', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = json.dumps(metrics(args.reference, args.capture, args.provenance), indent=2, allow_nan=False) + '\n'
    if args.output:
        args.output.write_text(result)
    else:
        print(result, end='')


if __name__ == '__main__':
    main()
