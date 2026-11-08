#!/usr/bin/env python3
"""Offline image-space diagnostics; color variation is not geometry correctness.

--metadata optionally supplies a JSON object with ``lod_encoding``::

  {"file": "lod.linear_rgb.f32", "dtype": "<f4", "channels": 3,
   "native_channel": 0, "native_value": 1.0,
   "step_channel": 1, "step_scale": 16,
   "width_channel": 2, "width_scale": 24, "tolerance": 0.0001}

Dimensions come from lod.png; float data must be tightly packed row-major RGB.
The encoding must describe actual framebuffer linear values, not PNG display RGB.
Only explicitly supplied encodings are interpreted. No camera/scene equivalence
is inferred when a reference is supplied; comparisons are image-space only.
"""

import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw


MODES = ('beauty', 'lod', 'normals', 'flat', 'paint', 'blend', 'raw-light', 'material', 'profile')


def components(mask):
    """4-connected component sizes/bounds, using row runs for bounded CPU cost."""
    parents, runs, previous = [], [], []

    def root(i):
        while parents[i] != i:
            parents[i] = parents[parents[i]]
            i = parents[i]
        return i

    for y, row in enumerate(mask):
        edges = np.diff(np.r_[False, row, False].astype(np.int8))
        current = []
        for x0, x1 in zip(np.flatnonzero(edges == 1), np.flatnonzero(edges == -1)):
            index = len(parents)
            parents.append(index)
            runs.append((int(x0), y, int(x1), y + 1))
            for old_x0, old_x1, old_index in previous:
                if old_x1 <= x0:
                    continue
                if old_x0 >= x1:
                    break
                parents[root(old_index)] = root(index)
            current.append((x0, x1, index))
        previous = current
    groups = {}
    for i, (x0, y0, x1, y1) in enumerate(runs):
        key = root(i)
        if key not in groups:
            groups[key] = dict(pixels=0, bounds=[x0, y0, x1, y1])
        group = groups[key]
        group['pixels'] += x1 - x0
        b = group['bounds']
        group['bounds'] = [min(b[0], x0), min(b[1], y0), max(b[2], x1), max(b[3], y1)]
    return dict(count=len(groups), largest=sorted(groups.values(), key=lambda g: -g['pixels'])[:10],
                bounds_coordinates='relative to ROI; x1/y1 exclusive')


def parse_roi(value):
    try:
        roi = tuple(int(x) for x in value.split(','))
    except ValueError as exc:
        raise argparse.ArgumentTypeError('ROI requires four integers') from exc
    if len(roi) != 4 or min(roi) < 0 or roi[2] <= roi[0] or roi[3] <= roi[1]:
        raise argparse.ArgumentTypeError('ROI must be x0,y0,x1,y1 with positive extent')
    return roi


def crop(array, roi):
    x0, y0, x1, y1 = roi
    return array[y0:y1, x0:x1]


def color_stats(rgb):
    """Summarize exact stored 8-bit values, without claiming linear radiometry."""
    values = rgb.reshape(-1, 3)
    if not len(values):
        raise ValueError('empty image region')
    colors, counts = np.unique(values, axis=0, return_counts=True)
    probability = counts / len(values)
    rgb_float = rgb.astype(np.float64) / 255
    adjacent = []
    for axis in (0, 1):
        differences = np.diff(rgb_float, axis=axis)
        adjacent.append(dict(axis='vertical' if axis == 0 else 'horizontal',
                             pairs=int(differences.size // 3),
                             mean_absolute_rgb_difference=float(np.mean(np.abs(differences))) if differences.size else None,
                             changed_pair_fraction=float(np.mean(np.any(differences != 0, axis=-1))) if differences.size else None))
    order = np.argsort(-counts, kind='stable')[:12]
    return dict(pixels=len(values), unique_rgb_colors=len(colors),
                color_entropy_bits=float(-np.sum(probability * np.log2(probability))),
                rgb_mean_8bit=values.mean(axis=0).tolist(),
                rgb_stddev_8bit=values.std(axis=0).tolist(),
                most_common_rgb=[dict(rgb=colors[i].tolist(), pixels=int(counts[i])) for i in order],
                adjacent_variation=adjacent)


def decode_lod(directory, image_shape, encoding, roi, output_dir=None):
    """Decode only validated, explicitly declared linear diagnostic channels."""
    channels = encoding.get('channels', 3)
    if channels != 3 or encoding.get('dtype', '<f4') != '<f4':
        raise ValueError('LOD float format must be tightly packed little-endian float32 RGB')
    relative = Path(encoding['file'])
    if relative.is_absolute() or '..' in relative.parts:
        raise ValueError('LOD float file must be relative to input directory')
    data = np.fromfile(directory / relative, dtype='<f4')
    if data.size != int(np.prod(image_shape)):
        raise ValueError('LOD float file size does not match lod.png dimensions')
    data = data.reshape(image_shape)
    if not np.isfinite(data).all():
        raise ValueError('LOD float data contains nonfinite values')
    tolerance = float(encoding.get('tolerance', 1e-4))
    if not 0 < tolerance < 0.25:
        raise ValueError('LOD tolerance must be between zero and 0.25')

    def channel(name):
        index = encoding[name + '_channel']
        if not isinstance(index, int) or index not in range(3):
            raise ValueError('invalid channel: ' + name)
        return data[..., index]

    native = np.abs(channel('native') - float(encoding['native_value'])) <= tolerance
    proxy = np.abs(channel('native') - float(encoding.get('proxy_value', 0))) <= tolerance
    depth_path = directory / 'lod.depth.f32'
    if not depth_path.exists():
        raise ValueError('LOD classification requires lod.depth.f32 to exclude background')
    depth = np.fromfile(depth_path, dtype='<f4')
    if depth.size != data.shape[0] * data.shape[1]:
        raise ValueError('LOD depth dimensions do not match lod.png')
    depth = depth.reshape(data.shape[:2])
    hit = np.isfinite(depth) & (depth >= 0) & (depth < 1)
    decoded = {}
    valid = hit & (native | proxy)
    for name in ('step', 'width'):
        scale = float(encoding[name + '_scale'])
        if not np.isfinite(scale) or scale <= 0:
            raise ValueError('LOD index scales must be finite and positive')
        values = channel(name) * scale
        nearest = np.rint(values)
        # Residual is measured in linear channel units, not index units.
        matches = (np.abs(values - nearest) <= tolerance * scale) & (nearest >= 0)
        valid &= matches
        decoded[name] = nearest

    def summarize(region):
        accepted = crop(valid, region)
        native_region = crop(native, region)
        hits = crop(hit, region)
        result = dict(pixels=int(accepted.size), hit_pixels=int(hits.sum()),
                      valid_index_pixels=int(accepted.sum()),
                      invalid_index_pixels=int((hits & ~accepted).sum()),
                      native_value_pixels=int((native_region & hits).sum()),
                      proxy_value_pixels=int(crop(proxy & hit, region).sum()),
                      invalid_native_class_pixels=int(crop(hit & ~native & ~proxy, region).sum()))
        for name, values in decoded.items():
            ids, counts = np.unique(crop(values, region)[accepted], return_counts=True)
            result[name + '_index_histogram'] = {str(int(i)): int(n) for i, n in zip(ids, counts)}
        return result

    height, width = data.shape[:2]
    x0, y0, x1, y1 = roi
    regions = {name: components(crop(mask, roi)) for name, mask in
               [('native', native & hit), ('proxy', proxy & hit)]}
    row_bands = [dict(y0=y, y1=min(y + 20, y1), **summarize((x0, y, x1, min(y + 20, y1))))
                 for y in range(y0, y1, 20)]
    if output_dir:
        # Fixed native color; proxy colors index a discrete, deterministic palette.
        palette = np.array([[246, 191, 79], [224, 104, 64], [188, 75, 151], [121, 93, 194],
                            [59, 149, 203], [76, 189, 164], [163, 194, 78]], dtype=np.uint8)
        false_color = np.zeros_like(data, dtype=np.uint8)
        accepted = valid & ~native
        false_color[accepted] = palette[decoded['step'][accepted].astype(int) % len(palette)]
        false_color[native & hit] = [65, 225, 100]
        false_color[hit & ~valid] = [255, 0, 255]
        panel({'FALSE COLOR: native green; proxy step palette; background black': false_color},
              output_dir / 'lod-classification.png')
        panel({'FALSE COLOR: native green; proxy step palette': false_color},
              output_dir / 'lod-classification-roi.png', roi, 2)
    return dict(encoding=encoding, full=summarize((0, 0, width, height)), roi=summarize(roi),
                roi_components=regions, roi_row_bands=row_bands,
                indices='step_index=log2(step_cells); width_index=log2(tilewidth_cells)',
                false_color_legend=dict(native=[65, 225, 100], background=[0, 0, 0], invalid=[255, 0, 255],
                                        proxy='step index modulo 7; repeated colors do not imply equal LOD'))


def panel(images, path, roi=None, zoom=1):
    tiles = []
    for label, rgb in images.items():
        im = Image.fromarray(crop(rgb, roi) if roi else rgb)
        if zoom != 1:
            im = im.resize((im.width * zoom, im.height * zoom), Image.Resampling.NEAREST)
        tile = Image.new('RGB', (im.width, im.height + 30), '#20252b')
        tile.paste(im, (0, 30))
        ImageDraw.Draw(tile).text((8, 8), label + (f' ROI {roi}' if roi else ''), fill='white')
        tiles.append(tile)
    columns = min(3, len(tiles))
    rows = (len(tiles) + columns - 1) // columns
    width = max(tile.width for tile in tiles)
    height = max(tile.height for tile in tiles)
    result = Image.new('RGB', (width * columns, height * rows), '#101418')
    for i, tile in enumerate(tiles):
        result.paste(tile, ((i % columns) * width, (i // columns) * height))
    result.save(path)


def analyze(input_dir, output_dir, roi, reference=None, metadata=None, zoom=2):
    images = {name: np.asarray(Image.open(input_dir / (name + '.png')).convert('RGB'))
              for name in MODES if (input_dir / (name + '.png')).exists()}
    if not images:
        raise ValueError('no recognized diagnostic PNGs found')
    shapes = {image.shape for image in images.values()}
    if len(shapes) != 1:
        raise ValueError('diagnostic PNG dimensions differ')
    height, width, _ = next(iter(shapes))
    if roi[2] > width or roi[3] > height:
        raise ValueError(f'ROI {roi} exceeds image dimensions {width}x{height}')
    output_dir.mkdir(parents=True, exist_ok=True)
    sidecar = input_dir / 'lod.capture.json'
    if metadata is None and sidecar.exists():
        provenance = json.loads(sidecar.read_text())
        if (provenance.get('diagnostic_mode', provenance.get('debug_view')) == 'lod'
                or provenance.get('adaptive_appearance', {}).get('debug_view') == 1):
            metadata = dict(lod_encoding=dict(file='lod.linear_rgb.f32', native_channel=0,
                            native_value=1, step_channel=1, step_scale=16, width_channel=2, width_scale=24))
    result = dict(input=str(input_dir), width=width, height=height, roi=list(roi),
                  modes={}, limitations=[
                      'PNG variation metrics use stored 8-bit display RGB and include sky/background.',
                      'Image-space ROI does not imply distance, native ownership or geometry accuracy.',
                      'A still image cannot certify temporal transitions or performance.'])
    for mode, rgb in images.items():
        result['modes'][mode] = dict(full=color_stats(rgb), roi=color_stats(crop(rgb, roi)))
    if metadata:
        result['metadata'] = metadata
        if 'lod_encoding' in metadata:
            if 'lod' not in images:
                raise ValueError('LOD encoding supplied without lod.png')
            result['lod_decoded'] = decode_lod(input_dir, images['lod'].shape, metadata['lod_encoding'], roi, output_dir)
    if reference:
        reference_path = reference / 'color.png' if reference.is_dir() else reference
        reference_rgb = np.asarray(Image.open(reference_path).convert('RGB'))
        if reference_rgb.shape != (height, width, 3):
            raise ValueError('reference dimensions differ from diagnostic images')
        result['reference'] = dict(file=str(reference_path),
                                   roi=color_stats(crop(reference_rgb, roi)),
                                   provenance_matched=False)
        result['limitations'].append('Reference may use sampled far terrain; reference mean/color metrics are appearance observations only.')
        if 'beauty' in images:
            delta = crop(images['beauty'], roi).astype(float) - crop(reference_rgb, roi).astype(float)
            result['reference']['beauty_roi_display_rgb_mae_8bit'] = float(np.mean(np.abs(delta)))
            result['reference']['beauty_roi_display_rgb_rmse_8bit'] = float(np.sqrt(np.mean(delta ** 2)))
        images = {'reference': reference_rgb, **images}
    output_dir.mkdir(parents=True, exist_ok=True)
    panel(images, output_dir / 'panels.png')
    panel(images, output_dir / 'roi-panels.png', roi, zoom)
    (output_dir / 'metrics.json').write_text(json.dumps(result, indent=2, allow_nan=False) + '\n')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--input', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--reference', type=Path)
    parser.add_argument('--metadata', type=Path)
    parser.add_argument('--roi', type=parse_roi, default=(0, 420, 640, 620))
    parser.add_argument('--zoom', type=int, choices=range(1, 9), default=2)
    args = parser.parse_args()
    metadata = json.loads(args.metadata.read_text()) if args.metadata else None
    try:
        result = analyze(args.input, args.output, args.roi, args.reference, metadata, args.zoom)
    except (ValueError, KeyError, OSError) as exc:
        parser.error(str(exc))
    print(json.dumps(dict(output=str(args.output), modes=list(result['modes']), roi=result['roi'])))


if __name__ == '__main__':
    main()
