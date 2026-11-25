#!/usr/bin/env python3
"""Plot seeded_world_gallery's actual CPU samples. Requires numpy and matplotlib.
Usage: python3 tools/seeded_world_gallery.py output/seeded-world/cpu
No engine captures: top-down canonical material colors and true-scale isometric terrain.
"""
import argparse
import json
import os
from pathlib import Path
os.environ.setdefault("MPLCONFIGDIR", "/tmp/frontier-matplotlib")
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.colors import LightSource
import numpy as np

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("directory", nargs="?", type=Path, default=Path("output/seeded-world/cpu"))
args = parser.parse_args()
for path in sorted(args.directory.glob("seeded_world_*.csv")):
    meta = json.loads(path.with_suffix(".json").read_text())
    g = np.genfromtxt(path, delimiter=",", names=True, dtype=None, encoding="utf-8")
    n = meta["resolution"]
    shape = (n, n)
    ground = g["ground_m"].reshape(shape)
    h = g["visible_m"].reshape(shape)
    spacing = meta["grid_spacing_m"]
    rgb = np.stack([g[c].reshape(shape) for c in ("r", "g", "b")], axis=-1)
    # Material colors are linear; shade the sampled canonical ground, then encode
    # a simple sRGB display approximation. Water keeps its level surface.
    light = LightSource(azdeg=315, altdeg=40).hillshade(h, vert_exag=1, dx=spacing, dy=spacing)
    rgb = np.clip(rgb * (0.40 + 0.85 * light[..., None]), 0, 1) ** (1 / 2.2)
    span = meta["span_m"]
    x = g["x_m"].reshape(shape)
    z = g["z_m"].reshape(shape)
    extent = (x.min(), x.max(), z.min(), z.max())
    fig = plt.figure(figsize=(15, 10), layout="constrained")
    ax = fig.add_subplot(2, 2, 1)
    ax.imshow(rgb, origin="lower", extent=extent)
    ax.set(title="Canonical materials + sampled relief shading", xlabel="x (m)", ylabel="z (m)")
    ax = fig.add_subplot(2, 2, 2, projection="3d")
    step = max(1, (n - 1) // 128)
    sl = (slice(None, None, step), slice(None, None, step))
    ax.plot_surface(x[sl], z[sl], h[sl], facecolors=rgb[sl], shade=False, linewidth=0, antialiased=True, rcount=129, ccount=129)
    ax.set(xlabel="x (m)", ylabel="z (m)", zlabel="height (m)", title="Isometric · true vertical scale")
    ax.set_box_aspect((span, span, max(float(np.ptp(h)), 1)))
    ax.view_init(elev=30, azim=-55)
    biome_names, biome_ids = np.unique(g["biome"], return_inverse=True)
    ax = fig.add_subplot(2, 2, 3)
    cmap = plt.get_cmap("tab10", len(biome_names))
    im = ax.imshow(biome_ids.reshape(shape), origin="lower", extent=extent, cmap=cmap, vmin=-.5, vmax=len(biome_names)-.5, interpolation="nearest")
    cb = fig.colorbar(im, ax=ax, ticks=range(len(biome_names)))
    cb.ax.set_yticklabels(biome_names)
    ax.set(title="Canonical biome classification", xlabel="x (m)", ylabel="z (m)")
    ax = fig.add_subplot(2, 2, 4)
    wet = g["water_depth_m"].reshape(shape)
    im = ax.imshow(np.ma.masked_less_equal(wet, 0), origin="lower", extent=extent, cmap="Blues", interpolation="nearest")
    fig.colorbar(im, ax=ax, label="static water depth (m)")
    ax.set_facecolor("#ddd6b8")
    ax.set(title=f"Sampled water {np.mean(wet > 0)*100:.2f}% · sparse samples may miss narrow rivers", xlabel="x (m)", ylabel="z (m)")
    fig.suptitle(f"Seed {meta['seed']} · {span/1000:g} km square · {n}×{n} CPU samples · source v{meta['source_version']} / appearance v{meta['appearance_version']}\nGround {ground.min():.1f}…{ground.max():.1f} m · CPU queries {meta['eval_ms']:.1f} ms (not FPS) · no engine atmosphere or vegetation")
    fig.savefig(path.with_suffix(".png"), dpi=140)
    plt.close(fig)
    print(path.with_suffix(".png"))
