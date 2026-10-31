#!/usr/bin/env python3
"""Diagnostic maps of saved atlas climate/runoff. No procedural data invented.

Raster panels use the logical node lattice (the irregular jitter is approximated).
River polylines use actual node coordinates; widths are exaggerated for legibility.
"""
import argparse
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from matplotlib.colors import LogNorm
from matplotlib.collections import LineCollection


def render(path):
    meta = dict(line.split('=', 1) for line in (path/'summary.txt').read_text().splitlines())
    side = int(meta['side']); extent = float(meta['extent_m'])/1000
    n = np.fromfile(path/'nodes.bin', '<f8').reshape(-1, 8)
    c = np.fromfile(path/'climate.bin', '<f4').reshape(-1, 8)
    q = np.fromfile(path/'flow.bin', '<f8').reshape(-1, 2)
    ocean = np.fromfile(path/'ocean.bin', 'u1').astype(bool)
    water = np.fromfile(path/'water.bin', '<f8')
    paint = np.fromfile(path/'paint.bin', '<f4').reshape(-1, 10)
    assert len(n) == len(c) == len(q) == len(ocean) == len(water) == len(paint) == side*side
    fig, axes = plt.subplots(2, 3, figsize=(19, 12), layout='constrained')
    ext = [0, extent, 0, extent]
    rgb = np.clip(paint[:, :3], 0, 1)
    rgb = np.where(rgb <= .0031308, 12.92*rgb, 1.055*rgb**(1/2.4)-.055)
    rgb[ocean | (water > n[:,2]+1e-6)] = [48/255,98/255,123/255]
    axes[0,0].imshow(rgb.reshape(side,side,3), origin='lower', extent=ext)
    axes[0,0].set_title('Ground cover + permanent water (no trees)')
    fields = [
        (axes[0,1], c[:,3], 'Annual precipitation (mm/year)', 'YlGnBu', LogNorm(10,4000)),
        (axes[0,2], c[:,0], 'Mean temperature (°C)', 'coolwarm', None),
        (axes[1,0], c[:,6], 'Moisture suitability: precipitation / (precipitation + PET)', 'BrBG', None),
        (axes[1,1], c[:,2]-c[:,1], 'Seasonal temperature range (°C)', 'magma', None),
    ]
    for ax, field, title, cmap, norm in fields:
        data = np.ma.array(field.reshape(side,side), mask=ocean.reshape(side,side))
        ax.set_facecolor('#30627b')
        im = ax.imshow(data, origin='lower', extent=ext, cmap=cmap, norm=norm)
        fig.colorbar(im, ax=ax, shrink=.75)
        ax.set_title(title)
    ax = axes[1,2]
    ax.imshow(n[:,2].reshape(side,side), origin='lower', extent=ext, cmap='gist_earth',vmin=-500,vmax=4000)
    receiver = n[:,7].astype(int)
    mask = (q[:,0]>10) & (receiver != np.arange(len(n))) & ~ocean
    segments = np.stack([n[mask,:2], n[receiver[mask],:2]],axis=1)/1000
    ax.add_collection(LineCollection(segments, colors='#167db8', linewidths=np.clip(.25+q[mask,1]/100,.25,1.6)))
    ax.set_title('Routed rivers >10 m³/s; line widths exaggerated')
    for ax in axes.flat:
        ax.set_xlabel('x (km)');ax.set_ylabel('z (km)');ax.set_aspect('equal')
    fig.suptitle(f"Atlas climate and drainage | seed {meta['seed']} | {extent:g} × {extent:g} km\n"
        f"Latitude at z=0: {meta['latitude_at_z0']}°; prevailing wind toward {meta['wind_degrees']}° from +x",fontsize=16)
    fig.savefig(path/'climate-and-drainage.png',dpi=150)
    plt.close(fig)
    print(path/'climate-and-drainage.png')


if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('directory',type=Path)
    render(parser.parse_args().directory)
