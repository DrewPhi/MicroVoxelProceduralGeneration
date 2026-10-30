#!/usr/bin/env python3
"""Fetch a native 5 cm, 200 m square Volcan survey crop using COG HTTP ranges.

Offline import only: pip install rasterio numpy. Run from the repo root.
Data citation/CC BY 4.0: https://doi.org/10.5069/G99P2ZV5
The runtime reads the compact .mvearth file without GIS/Python dependencies.
"""
import hashlib
import json
import struct

import numpy as np
import rasterio
from rasterio.windows import Window
from pathlib import Path
base = 'https://opentopography.s3.sdsc.edu/raster/CA24_Volcan/'
elev = base + 'CA24_Volcan_hh/VolcanMt_20231025_DSM_05cm_elev-001_COG.tif'
photo = base + 'CA24_Volcan_op/VolcanMt_20231025_RGB_05cm_rgb_COG.tif'
out = Path('output/earth')
out.mkdir(parents=True, exist_ok=True)
with rasterio.Env(GDAL_DISABLE_READDIR_ON_OPEN='EMPTY_DIR', CPL_VSIL_CURL_ALLOWED_EXTENSIONS='.tif', GDAL_HTTP_TIMEOUT='60'):
    with rasterio.open(elev) as ds:
        col, row = ds.index(536975.0, 3665052.0)[::-1]
        win = Window(col, row, 4000, 4000)
        h = ds.read(1, window=win)
        t = ds.window_transform(win)
        bounds = rasterio.windows.bounds(win, ds.transform)
        print('height window', win, bounds, 'valid', np.mean(h != -9999.0), flush=True)
        assert h.shape == (4000, 4000) and np.all(np.isfinite(h) & (h > 0))
    with rasterio.open(photo) as ds:
        c, r = ds.index(t.c + 0.025, t.f - 0.025)[::-1]
        px, py = ds.xy(r, c)
        assert abs(px - (t.c + 0.025)) < 1e-05 and abs(py - (t.f - 0.025)) < 1e-05
        rgb = ds.read(window=Window(c, r, 4000, 4000))
        assert np.all(rgb[3] > 0)
    print('range', float(h.min()), float(h.max()), flush=True)
    offset = float(np.floor(h.min()) - 64)
    local = h - offset
    candidates = []
    for r in range(500, 3500, 100):
        for c in range(500, 3500, 100):
            a = local[r - 20:r + 21, c - 20:c + 21]
            slope = np.max(np.abs(np.diff(a, axis=0))) / 0.05 + np.max(np.abs(np.diff(a, axis=1))) / 0.05
            rr, gg, bb = rgb[:3, r, c].astype(float)
            score = slope + 0.0001 * ((r - 2300) ** 2 + (c - 1700) ** 2) ** 0.5
            if gg < 1.2 * rr and rr + gg + bb > 180:
                candidates.append((score, c, r))
    score, sx, sz = min(candidates)
    color = rgb[0].astype(np.uint16) >> 3 << 11 | rgb[1].astype(np.uint16) >> 2 << 5 | rgb[2].astype(np.uint16) >> 3
    header = struct.pack('<8sIIII3dII', b'MVEARTH1', 4000, 4000, 1, 20, t.c, t.f, offset, sx, sz)
    path = out / 'volcan.mvearth'
    temporary = path.with_suffix('.mvearth.tmp')
    with temporary.open('wb') as f:
        f.write(header)
        f.write(local.astype('<f4').tobytes())
        f.write(color.astype('<u2').tobytes())
    temporary.replace(path)
    # Only this preview is downsampled; the imported grid keeps original samples.
    with rasterio.open(str(out / 'crop_aerial.png'), 'w', driver='PNG', width=800, height=800, count=3, dtype='uint8') as dst:
        dst.write(rgb[:3, ::5, ::5])
    meta = {'dataset': 'CA24_Volcan', 'doi': '10.5069/G99P2ZV5', 'license': 'CC BY 4.0', 'height_url': elev, 'imagery_url': photo, 'crs': 'EPSG:32611', 'vertical_datum': 'WGS84 Ellipsoid', 'native_spacing_m': 0.05, 'source_bounds': bounds, 'source_elevation_min_m': float(h.min()), 'source_elevation_max_m': float(h.max()), 'vertical_offset_m': offset, 'spawn_native': [int(sx), int(sz)], 'spawn_local_m': [sx * 0.05, float(local[sz, sx]), sz * 0.05], 'width': 4000, 'height': 4000, 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
    (out / 'volcan.json').write_text(json.dumps(meta, indent=2))
    print(json.dumps(meta, indent=2), flush=True)
