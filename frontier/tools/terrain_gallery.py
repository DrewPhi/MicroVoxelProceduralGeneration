#!/usr/bin/env python3
"""Render terrain_gallery CSVs: python3 frontier/tools/terrain_gallery.py output/regional-terrain/cpu.
Requires numpy and matplotlib. Pure heightmap previews, not engine captures.
"""
import argparse
from pathlib import Path
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.colors import LightSource, Normalize
import numpy as np

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", nargs="?", type=Path, default=Path("output/regional-terrain/cpu"))
args = parser.parse_args()
recipes = ["legacy", "regional", "regional_unwarped"]
for seed in [7, 19, 83]:
    for span in [8000, 2000]:
        grids = [np.genfromtxt(args.directory / f"{r}_{seed}_{span}m.csv", delimiter=",", names=True) for r in recipes]
        n = int(np.sqrt(len(grids[0])))
        heights = [g["height_m"].reshape(n, n) for g in grids]
        # Shared elevation/color/axis scales within each matched comparison.
        lo = min(h.min() for h in heights)
        hi = max(h.max() for h in heights)
        norm = Normalize(lo, hi)
        fig = plt.figure(figsize=(15, 9), layout="constrained")
        for col, (recipe, g, h) in enumerate(zip(recipes, grids, heights)):
            rgb = LightSource(azdeg=315, altdeg=45).shade(h, cmap=plt.get_cmap("terrain"), norm=norm, vert_exag=1, dx=span/(n-1), dy=span/(n-1))
            top = fig.add_subplot(2, 3, col+1)
            top.imshow(rgb, extent=(-span/2, span/2, -span/2, span/2), origin="lower")
            top.set(title=recipe, xlabel="x (m)", ylabel="z (m)")
            ax = fig.add_subplot(2, 3, col+4, projection="3d")
            step = 4
            x = g["x_m"].reshape(n,n)[::step,::step]
            z = g["z_m"].reshape(n,n)[::step,::step]
            ax.plot_surface(x, z, h[::step,::step], facecolors=rgb[::step,::step], shade=False, linewidth=0, antialiased=True, rcount=n, ccount=n)
            ax.set(xlabel="x (m)", ylabel="z (m)", zlabel="height (m)", zlim=(lo,hi))
            ax.set_box_aspect((span,span,max(hi-lo,1)))
            ax.view_init(elev=28, azim=-55)
        fig.suptitle(f"Pure heightmap preview • seed {seed} • {span/1000:g} km square • true vertical scale\nShared elevation colors and axes; surface display decimated to 65×65; no engine materials/shadows")
        fig.savefig(args.directory / f"comparison_{seed}_{span}m.png", dpi=140)
        plt.close(fig)
