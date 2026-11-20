#!/usr/bin/env python3
"""Estimate conservative column rejection using a completed MSAA1 box capture."""
import argparse
import json
from pathlib import Path
import struct

import numpy as np


def depth_pyramid(depth):
    levels = [depth]
    while max(levels[-1].shape) > 1:
        source = levels[-1]
        height, width = source.shape
        source = np.pad(source, ((0, height % 2), (0, width % 2)), constant_values=1)
        levels.append(source.reshape(source.shape[0] // 2, 2, source.shape[1] // 2, 2).max(axis=(1, 3)))
    return levels


def records(path):
    with path.open('rb') as source:
        if source.read(8) != b'FRCVIS01':
            raise ValueError('unsupported coarse visibility capture version')
        while header := source.read(84):
            if len(header) != 84:
                raise ValueError('truncated tile camera')
            count, = struct.unpack_from('<I', header)
            camera = np.frombuffer(header, dtype='<f4', offset=4).astype(np.float64)
            payload = source.read(count * 52)
            if len(payload) != count * 52:
                raise ValueError('truncated column records')
            yield camera[:16].reshape(4, 4), camera[16:19], np.frombuffer(payload, dtype='<f4').reshape(count, 13)


def pyramid_rectangle_depth(levels, low, high):
    # At this mip each rectangle spans at most 2x2 blocks. Including whole
    # blocks can only increase maximum depth and make rejection more cautious.
    span = (high - low + 1).max(axis=1)
    mips = np.ceil(np.log2(span)).astype(np.int64)
    result = np.ones(len(low), dtype=np.float32)
    for mip in np.unique(mips):
        indices = mips == mip
        depth = levels[mip]
        x0, y0 = (low[indices] >> mip).T
        x1, y1 = (high[indices] >> mip).T
        result[indices] = np.maximum.reduce([depth[y0, x0], depth[y1, x0], depth[y0, x1], depth[y1, x1]])
    return result


CORNER = np.array([[bool(i & bit) for bit in (1, 2, 4)] for i in range(8)])


def classify(matrix, eye, columns, levels, bias, exact_rectangles=False):
    x0, z0, x1, z1, top, _ground, nx, px, nz, pz = columns.T[:10]
    faces_eye = ((eye[1] > top) | ((eye[0] < x0) & (nx < top))
                 | ((eye[0] > x1) & (px < top)) | ((eye[2] < z0) & (nz < top))
                 | ((eye[2] > z1) & (pz < top)))
    low = np.column_stack((x0, np.minimum.reduce((top - 0.0625, nx, px, nz, pz)), z0))[faces_eye]
    high = np.column_stack((x1, top, z1))[faces_eye]
    corner = np.where(CORNER[None, :, :], high[:, None, :], low[:, None, :])
    clip = corner @ matrix[:3] + matrix[3]
    x, y, z, w = np.moveaxis(clip, 2, 0)
    # Clip an AABB only when its entire volume lies beyond one clip plane.
    # Allow f32 projection error before declaring separation.
    allowance = 1e-5 * np.maximum(1, np.abs(w))
    outside = np.logical_or.reduce([(p < -allowance).all(axis=1) for p in
                                    (w + x, w - x, w + y, w - y, z, w - z)])
    safe = ~outside & (z > 0).all(axis=1) & (w > 0).all(axis=1)
    result = {
        'columns': len(columns),
        'backface_rejected': int((~faces_eye).sum()),
        'frustum_rejected': int(outside.sum()),
        'near_crossing_kept': int((~outside & ~safe).sum()),
    }

    height, width = levels[0].shape
    ndc = clip[safe, :, :3] / w[safe, :, None]
    screen = (ndc[:, :, :2] * [0.5, -0.5] + 0.5) * [width, height]
    low = np.maximum(np.floor(screen.min(axis=1) - 1), 0).astype(np.int64)
    high = np.minimum(np.ceil(screen.max(axis=1) + 1) - 1, [width - 1, height - 1]).astype(np.int64)
    on_screen = (low <= high).all(axis=1)
    result['frustum_rejected'] += int((~on_screen).sum())
    low, high = low[on_screen], high[on_screen]
    nearest = ndc[:, :, 2].min(axis=1)[on_screen]
    furthest_occluder = pyramid_rectangle_depth(levels, low, high)
    occluded = furthest_occluder.astype(np.float64) + bias < nearest
    rectangle_pixels = (high - low + 1).prod(axis=1)
    result.update(
        depth_rejected=int(occluded.sum()),
        visible_kept=int((~occluded).sum()),
        tested_rectangle_pixels=int(rectangle_pixels.sum()),
        depth_rejected_rectangle_pixels=int(rectangle_pixels[occluded].sum()),
    )
    if exact_rectangles:
        # This direct scan measures the extra rejection lost to mip footprints.
        # Every rejected mip rectangle is already proven by a superset query.
        undecided = np.flatnonzero(~occluded)
        depth = levels[0]
        hidden_pixels = np.array([
            np.count_nonzero(depth[y0:y1+1, x0:x1+1] < nearest[i] - bias)
            for i, (x0, y0), (x1, y1) in zip(undecided, low[undecided], high[undecided])
        ], dtype=np.int64)
        additionally_hidden = hidden_pixels == rectangle_pixels[undecided]
        result['additional_exact_depth_rejected'] = int(additionally_hidden.sum())
        result['additional_exact_depth_rejected_rectangle_pixels'] = int(rectangle_pixels[undecided[additionally_hidden]].sum())
        result['depth_covered_rectangle_pixels'] = int(rectangle_pixels[occluded].sum() + hidden_pixels.sum())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('capture', type=Path, help='Path to the rendered PNG')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--depth-bias', type=float, default=1e-6, help='NDC depth margin')
    parser.add_argument('--exact-rectangles', action='store_true', help='Also scan pixels for boxes retained by the pyramid')
    args = parser.parse_args()
    if not np.isfinite(args.depth_bias) or args.depth_bias < 0:
        parser.error('depth bias must be finite and nonnegative')

    capture = args.capture
    metadata = json.loads(capture.with_suffix('.capture.json').read_text())
    if metadata['samples'] != 1 or metadata['depth_encoding'] != 'ndc_0_1':
        parser.error('the probe requires a non-postprocessed MSAA1 depth capture')
    depth = np.fromfile(capture.with_suffix('.depth.f32'), dtype='<f4').reshape(metadata['height'], metadata['width'])
    if not np.isfinite(depth).all() or depth.min() < 0 or depth.max() > 1:
        parser.error('depth must contain finite values in [0, 1]')
    levels = depth_pyramid(depth)
    total = {}
    tiles = 0
    for matrix, eye, columns in records(capture.with_suffix('.coarse-visibility.bin')):
        for key, value in classify(matrix, eye, columns, levels, args.depth_bias, args.exact_rectangles).items():
            total[key] = total.get(key, 0) + value
        tiles += 1
    draw = metadata['adaptive']['coarse_projection']
    if tiles != draw['visible_tiles'] or total.get('columns') != draw['visible_columns']:
        raise ValueError('visibility input and capture metadata disagree')
    out = {'scope': 'Offline query against finished same-frame depth. Counts exclude the cost and incompleteness of building live occluder depth.',
           'capture': str(capture), 'tiles': tiles, 'depth_bias_ndc': args.depth_bias, **total}
    report = json.dumps(out, indent=2) + '\n'
    if args.output:
        args.output.write_text(report)
    print(report, end='')


if __name__ == '__main__':
    main()
