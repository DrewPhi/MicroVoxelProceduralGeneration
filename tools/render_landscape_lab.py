#!/usr/bin/env python3
"""Render the Rust landscape experiment, with no invented terrain or smoothing.

Binary format: little-endian f64 nodes [x,z,height,initial,uplift,area,soil,receiver],
and little-endian u32 triangle indices. All physical lengths are meters.
"""
import argparse
from pathlib import Path
import numpy as np
from PIL import Image, ImageDraw, ImageFont


def render(directory, crop=None, overview_only=False, painted=False):
    nodes = np.fromfile(directory / "nodes.bin", dtype="<f8").reshape(-1, 8)
    triangles = np.fromfile(directory / "triangles.bin", dtype="<u4").reshape(-1, 3)
    summary = dict(line.split("=", 1) for line in (directory / "summary.txt").read_text().splitlines())
    extent = float(summary["extent_m"])
    seed = summary["seed"]
    x, z, h = nodes[:, 0], nodes[:, 1], nodes[:, 2]
    sea = summary.get("geography") == "seeded_atlas_v1"
    # Sea display is a static level surface, not blue-painted bathymetry.
    # Match the compiler's boundary-connected ocean mask (inland pits stay lakes).
    ocean_path = directory / "ocean.bin"
    ocean = np.fromfile(ocean_path, dtype="u1").astype(bool) if sea else np.zeros(len(h), dtype=bool)
    if len(ocean) != len(h):
        raise ValueError("ocean/node count mismatch")
    budget_water = summary.get("climate_model") == "ocean_transport_v1"
    water = np.fromfile(directory / "water.bin", dtype="<f8") if budget_water else np.where(ocean, 0., h)
    if len(water) != len(h):
        raise ValueError("water/node count mismatch")
    water_mask = ocean | (budget_water & (water > h + 1e-6))
    display_h = np.where(water_mask, water, h)
    xyz = nodes[:, [0, 2, 1]]
    normal = np.cross(xyz[triangles[:, 2]] - xyz[triangles[:, 0]], xyz[triangles[:, 1]] - xyz[triangles[:, 0]])
    normal /= np.maximum(np.linalg.norm(normal, axis=1)[:, None], 1e-12)
    light = np.array([-0.5, 0.75, -0.43]); light /= np.linalg.norm(light)
    shade = np.clip(0.25 + 0.69 * np.maximum(normal @ light, 0), 0, 1)
    colors = (shade * 255).astype(np.uint8)
    paint = None
    if painted:
        paint = np.fromfile(directory / "paint.bin", dtype="<f4").reshape(-1, 10)
        if len(paint) != len(nodes):
            raise ValueError("paint/node count mismatch")
        rgb = np.clip(paint[triangles, :3].mean(axis=1) * shade[:, None], 0, 1)
        colors = np.round(np.where(rgb <= 0.0031308, 12.92*rgb, 1.055*rgb**(1/2.4)-0.055)*255).astype(np.uint8)
    label = "isometric" if crop is None else "range-closeup"
    if painted:
        label = "painted-" + label
    if crop is None:
        keep = np.ones(len(triangles), dtype=bool)
        edge = extent
    else:
        cx, cz, edge = crop
        center = nodes[triangles, :2].mean(axis=1)
        keep = (abs(center[:, 0]-cx)<edge/2) & (abs(center[:, 1]-cz)<edge/2)
    selected = triangles[keep]; colors = colors[keep]
    u = (x-z)/np.sqrt(2)
    v = (x+z)/np.sqrt(6)-display_h*np.sqrt(2/3)
    used = np.unique(selected)
    width, height = 3600, 2200
    scale = min((width-200)/np.ptp(u[used]), (height-300)/np.ptp(v[used]))
    px = (u-(u[used].min()+u[used].max())/2)*scale+width/2
    py = (v-v[used].min())*scale+170
    points = np.stack([px, py], axis=1)
    u_mid = (u[used].min()+u[used].max())/2
    v_min = v[used].min()
    image = Image.new("RGB", (width, height), (235, 237, 240))
    draw = ImageDraw.Draw(image)
    # Far to near: no transparency, fog, geometry enhancement or vertical scaling.
    order = np.argsort((x[selected]+z[selected]).mean(axis=1))
    for i in order:
        color = tuple(int(c) for c in colors[i]) if painted else (int(colors[i]),)*3
        triangle = selected[i]
        wet = water_mask[triangle]
        levels = water[triangle[wet]]
        # Never interpolate two different water bodies into an inclined surface.
        compatible = not len(levels) or np.ptp(levels) < 1e-6
        if not wet.any() or not compatible:
            draw.polygon([tuple(p) for p in points[triangle]], fill=color)
        elif wet.all():
            draw.polygon([tuple(p) for p in points[triangle]], fill=(48, 98, 123) if painted else (115, 130, 140))
        else:
            # Clip mixed coastal triangles at actual sea level, avoiding a jagged
            # all-or-nothing color classification across kilometre-sized faces.
            vertices = nodes[triangle, :3]
            level = float(levels[0])
            for is_land, fill in [(True,color),(False,(48,98,123) if painted else (115,130,140))]:
                polygon=[]
                for a,b in zip(vertices,np.roll(vertices,-1,axis=0)):
                    inside_a = (a[2]>=level) == is_land
                    inside_b = (b[2]>=level) == is_land
                    if inside_a: polygon.append(a)
                    if inside_a != inside_b:
                        polygon.append(a+(b-a)*((level-a[2])/(b[2]-a[2])))
                if len(polygon)<3: continue
                p=np.array(polygon)
                ph=p[:,2] if is_land else np.full(len(p),level)
                pu=(p[:,0]-p[:,1])/np.sqrt(2)
                pv=(p[:,0]+p[:,1])/np.sqrt(6)-ph*np.sqrt(2/3)
                projected=np.stack([(pu-u_mid)*scale+width/2,
                                    (pv-v_min)*scale+170],axis=1)
                draw.polygon([tuple(q) for q in projected],fill=fill)
    font_path = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
    font = ImageFont.truetype(font_path, 31)
    small = ImageFont.truetype(font_path, 23)
    model = "Current generator 6" if summary.get("model") == "generator6" else "Landscape evolution experiment"
    if sea:
        model = "Seed-generated geographic atlas"
    title = f"{model} | seed {seed} | {edge/1000:g} km x {edge/1000:g} km"
    draw.text((70,35), title, fill=(35,40,45), font=font)
    description = "Climate-driven ground cover" if painted else "Gray geometry"
    if budget_water and painted:
        description = "Ocean-fed climate, rainfall-weighted erosion and ground cover"
    draw.text((70,85), f"{description} | true-scale heights | {float(summary['min_m']):.0f}–{float(summary['max_m']):.0f} m across full domain", fill=(55,60,65), font=small)
    draw.text((70,height-65), f"Coarse landscape prototype; nominal node spacing {extent/(int(summary['side'])-1):.1f} m. Not native voxel detail.", fill=(55,60,65), font=small)
    image.save(directory / f"{label}.png")

    if crop is None and not overview_only:
        # Diagnostic views share exact geometry and triangles with the overview.
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        from matplotlib.collections import LineCollection
        fig, axes = plt.subplots(2, 2, figsize=(16, 15), layout="constrained")
        axes[0,0].tripcolor(x/1000,z/1000,triangles,facecolors=shade[keep],cmap="gray",vmin=0,vmax=1)
        receivers=nodes[:,7].astype(int)
        water_path=directory/"water.bin"
        water=np.fromfile(water_path,dtype="<f8") if water_path.exists() else h
        river=(nodes[:,5]>5e6)&(receivers!=np.arange(len(nodes)))&(water>=water[receivers]-1e-8)
        segments=np.stack([nodes[river,:2],nodes[receivers[river],:2]],axis=1)/1000
        axes[0,0].add_collection(LineCollection(segments,colors="#266692",linewidths=0.45))
        wet=(water>h+1e-6)&~ocean
        axes[0,0].scatter(x[wet]/1000,z[wet]/1000,s=1,c="#43b8d4",linewidths=0)
        axes[0,0].set_title("Drainage (>5 km² catchment); cyan = spill-level lakes")
        for ax,field,title in [(axes[0,1],h,"Elevation (m)"),(axes[1,0],nodes[:,4]*1000,"Initial uplift (mm/year)"),(axes[1,1],nodes[:,6],"Retained mobile sediment (m)")]:
            p=ax.tripcolor(x/1000,z/1000,triangles,field,shading="gouraud",cmap="terrain" if ax is axes[0,1] else "viridis")
            fig.colorbar(p,ax=ax,shrink=0.8);ax.set_title(title)
        for ax in axes.flat:
            ax.set_aspect("equal");ax.set_xlabel("x (km)");ax.set_ylabel("z (km)")
        fig.suptitle(f"{extent/1000:g} km landscape experiment — seed {seed}",fontsize=18)
        fig.savefig(directory/("painted-drainage.png" if painted else "diagnostics.png"),dpi=160);plt.close(fig)
        if painted:
            from matplotlib.collections import PolyCollection
            fig, axes = plt.subplots(2,2,figsize=(15,14),layout="constrained")
            poly=PolyCollection(nodes[triangles,:2]/1000,facecolors=colors/255,edgecolors="none",rasterized=True)
            axes[0,0].add_collection(poly);axes[0,0].autoscale_view();axes[0,0].set_title("Ground-cover albedo + slope shading (no trees)")
            for ax,field,title in [(axes[0,1],paint[:,3],"Mean temperature (°C)"),(axes[1,0],paint[:,4],"Moisture suitability"),(axes[1,1],paint[:,8],"Snow coverage")]:
                mesh=ax.tripcolor(x/1000,z/1000,triangles,field,shading="gouraud",cmap="viridis")
                fig.colorbar(mesh,ax=ax,shrink=0.8);ax.set_title(title)
            for ax in axes.flat:
                ax.set_aspect("equal");ax.set_xlabel("x (km)");ax.set_ylabel("z (km)")
            fig.savefig(directory/"painted-diagnostics.png",dpi=140);plt.close(fig)
    print(directory / f"{label}.png", flush=True)


if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("directory",type=Path)
    parser.add_argument("--crop",nargs=3,type=float,metavar=("X_M","Z_M","EDGE_M"))
    parser.add_argument("--overview-only",action="store_true")
    parser.add_argument("--painted",action="store_true")
    args=parser.parse_args()
    render(args.directory,args.crop,args.overview_only,args.painted)
