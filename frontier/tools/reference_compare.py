#!/usr/bin/env python3
"""Compare independent canonical DDA reference with a matched GPU capture.

Usage: reference_compare.py REFERENCE_DIR GPU.png --output comparison-dir
Requires NumPy and Pillow. Geometry and shading are reported separately. Use
GPU --capture-linear for unclipped float32 radiance; ordinary captures decode
the quantized 8-bit target. Reference colors are unclipped linear radiance. Photometric disagreement includes the
renderer lighting approximation and is not an estimator of geometry error.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image


def read(path, shape):
    a = np.fromfile(path, dtype='<f4')
    if a.size != int(np.prod(shape)):
        raise ValueError(f'{path}: expected {shape}, got {a.size} floats')
    return a.reshape(shape).astype(np.float64)


def same_camera(a, b, require_single_sample=True):
    for field in ('width', 'height'):
        if a[field] != b[field]:
            raise ValueError(f'camera mismatch: {field}')
    if not np.allclose(a['eye'], b['eye'], atol=1e-6, rtol=0):
        raise ValueError('camera mismatch: eye')
    da = np.subtract(a['target'], a['eye']); da /= np.linalg.norm(da)
    db = np.subtract(b['target'], b['eye']); db /= np.linalg.norm(db)
    if not np.allclose(da, db, atol=2e-6, rtol=0):
        raise ValueError('camera mismatch: viewing direction')
    for field in ('fov_y_degrees', 'near', 'far'):
        if not np.isclose(a[field], b[field], atol=1e-5, rtol=1e-7):
            raise ValueError(f'camera mismatch: {field}')
    if require_single_sample and b['samples'] != 1:
        raise ValueError('GPU geometry comparison requires --msaa 1')


def same_world(a, b):
    for field in ('seed', 'source_version', 'edit_revision', 'native_min_cell', 'native_size_cells'):
        if field not in a or field not in b or a[field] != b[field]:
            raise ValueError(f'world mismatch: {field}')
    field = 'canonical_source_fnv1a64'
    if field in a and field in b and a[field] != b[field]:
        raise ValueError(f'world mismatch: {field}')


def ray_distance(depth, camera):
    """Convert RH zero-to-one perspective depth to center-ray metres."""
    h, w = depth.shape
    t = np.tan(np.deg2rad(camera['fov_y_degrees']) / 2)
    x = ((np.arange(w) + .5) * 2 / w - 1) * t * w / h
    y = (1 - (np.arange(h) + .5) * 2 / h) * t
    n, f = camera['near'], camera['far']
    return (n * f / (f - depth * (f - n))) * np.sqrt(1 + y[:, None]**2 + x[None, :]**2)


def stats(a):
    a = a[np.isfinite(a)]
    return {'samples': int(a.size), 'mae': float(np.mean(np.abs(a))) if a.size else None,
            'rmse': float(np.sqrt(np.mean(a*a))) if a.size else None,
            'p95_absolute': float(np.quantile(np.abs(a), .95)) if a.size else None,
            'max_absolute': float(np.max(np.abs(a))) if a.size else None}


def srgb(a):
    a = np.maximum(a, 0)
    a = np.where(a <= .0031308, 12.92*a, 1.055*a**(1/2.4)-.055)
    return (np.clip(a, 0, 1)*255+.5).astype(np.uint8)


def compare(reference, gpu, output, convergence=None, depth_tolerance=.002):
    if not np.isfinite(depth_tolerance) or depth_tolerance < 0:
        raise ValueError('depth tolerance must be finite and nonnegative')
    manifest = json.loads((reference/'manifest.json').read_text())
    camera = json.loads(gpu.with_suffix('.capture.json').read_text())
    same_camera(manifest, camera)
    same_world(manifest, camera)
    if camera.get('scene') != 'fixture':
        raise ValueError('comparison currently requires the native fixture')
    h, w = camera['height'], camera['width']
    ref = read(reference/'linear_rgb.f32', (h,w,3))
    coverage = read(reference/'coverage.f32', (h,w))
    depth = read(reference/'depth.f32', (h,w,4))
    candidate = read(gpu.with_suffix('.linear_rgb.f32'), (h,w,3))
    ndc = read(gpu.with_suffix('.depth.f32'), (h,w))
    hit = np.isfinite(ndc) & (ndc >= 0) & (ndc < 1)
    distance = ray_distance(ndc, camera)
    empty, full = coverage == 0, coverage == 1
    partial = ~(empty | full)
    false = hit & empty
    missing = ~hit & full
    shared = hit & full
    # Compare against the entire subpixel reference hit range. The mean alone
    # would incorrectly mark a pixel containing several surfaces as wrong.
    residual = np.maximum(np.maximum(depth[:,:,0]-distance,
                                     distance-depth[:,:,1]), 0)
    bad_depth = shared & (residual > depth_tolerance)
    stable = shared & ~bad_depth
    report = {
        'reference': str(reference), 'gpu': str(gpu), 'width': w, 'height': h,
        'reference_spp': manifest['spp'],
        'world': {k: manifest[k] for k in ('seed','source_version','edit_revision','native_min_cell','native_size_cells')},
        'reference_provenance': {k: manifest.get(k) for k in ('git_revision','git_dirty','source_fnv1a64','canonical_source_fnv1a64')},
        'geometry': {'false_coverage_pixels': int(false.sum()),
                     'missing_coverage_pixels': int(missing.sum()),
                     'reference_partial_coverage_pixels': int(partial.sum()),
                     'coverage_absolute_error': stats(hit.astype(float)-coverage),
                     'depth_tolerance_m': depth_tolerance,
                     'depth_outside_reference_range_pixels': int(bad_depth.sum()),
                     'depth_range_residual_m_on_fully_covered_pixels': stats(residual[shared]),
                     'depth_mean_difference_m_diagnostic': stats((distance-depth[:,:,2])[shared])},
        'appearance': {'all_pixels_linear_rgb_error': stats(candidate-ref),
                       'matched_full_coverage_depth_linear_rgb_error': stats((candidate-ref)[stable]),
                       'gpu_encoding': camera['color_encoding'],
                       'reference_radiance_above_one_fraction': float((ref>1).mean()),
                       'interpretation': 'Lighting, shadows, indirect transport and sampling differ; ordinary 8-bit captures also include quantization. Color error is not geometry error.'},
        'limitations': ['Finite reference samples cannot prove absence of thin subpixel geometry.',
                        'Partial-coverage pixels are excluded from strict false/missing counts.',
                        'Depth range acceptance may hide a wrong surface between reference min/max.',
                        'Normals and materials are reference histograms only; GPU MRT parity is not measured.']}
    center_manifest = reference/'center_manifest.json'
    if center_manifest.exists():
        centers = json.loads(center_manifest.read_text())
        same_camera(centers, camera)
        same_world(centers, manifest)
        center_hit = read(reference/'center_coverage.f32', (h,w)) > .5
        center_depth = read(reference/'center_depth.f32', (h,w))
        center_shared = center_hit & hit
        center_error = distance-center_depth
        center_bad = center_shared & (np.abs(center_error)>depth_tolerance)
        report['center_provenance'] = {k: centers.get(k) for k in ('git_revision','git_dirty','source_fnv1a64','canonical_source_fnv1a64')}
        report['geometry']['pixel_center'] = {
            'false_coverage_pixels': int((hit & ~center_hit).sum()),
            'missing_coverage_pixels': int((~hit & center_hit).sum()),
            'depth_error_above_tolerance_pixels': int(center_bad.sum()),
            'depth_error_m_on_shared_hits': stats(center_error[center_shared]),
            'interpretation': 'Matched pixel-center rays; raster edge fill conventions and GPU float projection can differ at exact voxel edges.'}
        center_image = np.zeros((h,w,3),dtype=np.uint8)
        center_image[hit & ~center_hit] = [255,0,255]
        center_image[~hit & center_hit] = [0,255,255]
        center_image[center_bad] = [255,80,0]
    else:
        center_image = None
    if convergence is not None:
        other = json.loads((convergence/'manifest.json').read_text())
        same_camera(other, camera)
        same_world(other, manifest)
        report['reference_convergence'] = {
            'other_spp': other['spp'],
            'reference_sample_seed': manifest.get('sample_seed'),
            'other_sample_seed': other.get('sample_seed'),
            'reference_sampling': manifest.get('sampling'),
            'other_sampling': other.get('sampling'),
            'linear_rgb_difference': stats(read(convergence/'linear_rgb.f32',(h,w,3))-ref),
            'coverage_difference': stats(read(convergence/'coverage.f32',(h,w))-coverage)}
    output.mkdir(parents=True, exist_ok=True)
    Image.fromarray(np.concatenate((srgb(ref),srgb(candidate)),axis=1)).save(output/'side_by_side.png')
    Image.fromarray(srgb(np.abs(candidate-ref)*4)).save(output/'color_difference_x4.png')
    diagnostic = np.zeros((h,w,3), dtype=np.uint8)
    diagnostic[partial] = [90,90,90]
    diagnostic[false] = [255,0,255]
    diagnostic[missing] = [0,255,255]
    diagnostic[bad_depth] = [255,80,0]
    Image.fromarray(diagnostic).save(output/'geometry_difference.png')
    if center_image is not None:
        Image.fromarray(center_image).save(output/'geometry_pixel_center_difference.png')
    report['geometry_image_legend'] = {'magenta':'false coverage','cyan':'missing coverage',
                                       'orange':'depth outside range','gray':'partial reference coverage'}
    (output/'metrics.json').write_text(json.dumps(report, indent=2)+'\n')
    return report


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('reference', type=Path)
    p.add_argument('gpu', type=Path)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--convergence', type=Path, help='second reference directory with different spp')
    p.add_argument('--depth-tolerance', type=float, default=.002)
    a = p.parse_args()
    if not np.isfinite(a.depth_tolerance) or a.depth_tolerance < 0:
        p.error('--depth-tolerance must be finite and nonnegative')
    print(json.dumps(compare(a.reference,a.gpu,a.output,a.convergence,a.depth_tolerance),indent=2))


if __name__ == '__main__':
    main()
