#!/usr/bin/env python3
"""Offline, orthographic whole-cell preview of recorded discrete-water states."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).resolve().parent
DEFAULT_OUTPUT = HERE.parents[1] / "output" / "discrete-water-lab"
BG = (15, 24, 37)
INK = (231, 240, 250)
MUTED = (145, 166, 190)


def font(size, bold=False):
    name = "DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"
    return ImageFont.truetype(f"/usr/share/fonts/truetype/dejavu/{name}", size)


def load_frames(path):
    with path.open() as stream:
        return [json.loads(line) for line in stream if line.strip()]


def cells(ids, nx, ny):
    return {(i % nx, (i // nx) % ny, i // (nx * ny)) for i in ids}


def render(frame, title, size=(1280, 800)):
    """Draw only actual exposed cell faces. Front exterior walls are cut away."""
    width, height = size
    im = Image.new("RGB", size, BG)
    draw = ImageDraw.Draw(im)
    nx, ny, nz = (frame[k] for k in ("nx", "ny", "nz"))
    water = cells(frame["water"], nx, ny)
    solids = cells(frame["solids"], nx, ny)
    # Camera looks toward the origin from +x,+z. Removing front perimeter
    # walls is exclusively a display cutaway; the simulation solids persist.
    visible_solids = {c for c in solids if c[1] == 0 or (c[0] != nx-1 and c[2] != nz-1 and c[1] != ny-1)}
    scale = min((width - 170) / (nx+nz), (height - 280) / (.46*(nx+nz) + ny))
    cx = width/2 - (nx-nz)*scale/2
    cy = 168 + ny*scale

    def point(x, y, z):
        return (cx+(x-z)*scale, cy+(.46*(x+z)-y)*scale)

    # A quiet ground plane and retained low rim make the sealed domain legible.
    floor = [point(0, 0, 0), point(nx, 0, 0), point(nx, 0, nz), point(0, 0, nz)]
    draw.polygon(floor, fill=(25, 40, 55), outline=(80, 100, 119), width=2)
    faces = []
    for occupied, material in ((visible_solids, "stone"), (water, "water")):
        for x, y, z in occupied:
            for direction, verts, shade in (
                ((0, 1, 0), ((0,1,0),(1,1,0),(1,1,1),(0,1,1)), 0),
                ((1, 0, 0), ((1,0,0),(1,0,1),(1,1,1),(1,1,0)), 1),
                ((0, 0, 1), ((0,0,1),(0,1,1),(1,1,1),(1,0,1)), 2),
            ):
                neighbor = (x+direction[0], y+direction[1], z+direction[2])
                if neighbor in occupied or (material == "water" and neighbor in visible_solids):
                    continue
                coords = [(x+a,y+b,z+c) for a,b,c in verts]
                depth = sum(a+c+.92*b for a,b,c in coords)/4
                palette = ((70,179,235),(24,111,179),(35,137,201)) if material == "water" else ((118,132,143),(65,81,96),(83,100,115))
                outline = (25,99,149) if material == "water" else (51,67,82)
                faces.append((depth, [point(*c) for c in coords], palette[shade], outline))
    for _, polygon, color, outline in sorted(faces, key=lambda f: f[0]):
        draw.polygon(polygon, fill=color, outline=outline, width=1)

    draw.text((40, 25), "DISCRETE WATER / OFFLINE LAB", font=font(15, True), fill=(81,186,238))
    draw.text((40, 52), title, font=font(30, True), fill=INK)
    draw.text((40, 96), "Binary occupancy · equal-size cubes · exposed planar faces", font=font(17), fill=MUTED)
    t = frame.get("time", frame.get("time_s", 0))
    count = len(water)
    draw.rounded_rectangle((width-267, 28, width-34, 105), radius=10, fill=(27,43,61))
    draw.text((width-248, 39), f"t = {t:.3f} s", font=font(22, True), fill=INK)
    draw.text((width-248, 72), f"{count:,} water cells", font=font(16), fill=(110,201,242))
    cell_size = frame.get("cell_size_m", 1)
    draw.text((40,height-77), f"{nx} × {ny} × {nz} domain  |  cell edge {cell_size:g} m  |  frame {frame.get('frame', '?')}", font=font(16), fill=INK)
    draw.text((40,height-49), "Front exterior walls / ceiling cut away. Internal barriers and every water cell retained.", font=font(15), fill=MUTED)
    draw.text((40,height-26), "Experimental CPU model / software preview — not in-game rendering or measured engine performance", font=font(13), fill=MUTED)
    return im


def make_sheet(frames, title, destination):
    choices = [round((len(frames)-1)*i/5) for i in range(6)]
    sheet = Image.new("RGB", (1536, 750), BG)
    d = ImageDraw.Draw(sheet)
    d.text((24,16), title + " / state comparison", fill=INK, font=font(25,True))
    for i, index in enumerate(choices):
        tile = render(frames[index], title)
        tile = tile.resize((512, 320), Image.Resampling.LANCZOS)
        px, py = (i%3)*512, 62+(i//3)*344
        sheet.paste(tile, (px, py))
        time = frames[index].get("time", frames[index].get("time_s", 0))
        d.text((px+18, py+320), f"t = {time:.3f} s · {len(frames[index]['water']):,} cells", fill=INK, font=font(15))
    sheet.save(destination)


def preview(source, output, fps, speed):
    frames = load_frames(source)
    if not frames:
        raise ValueError(f"No states in {source}")
    name = source.stem
    title = {"dam": "Dam release", "vessels": "Communicating vessels"}.get(name, name.replace("_", " ").replace("-", " ").title())
    folder = output/name
    folder.mkdir(parents=True, exist_ok=True)
    # Render each sampled state once; playback uses actual simulation timestamps.
    for i, frame in enumerate(frames):
        render(frame, title).save(folder/f"frame_{i:05d}.png")
    make_sheet(frames, title, output/f"{name}_comparison.png")
    shutil.copyfile(folder/f"frame_{len(frames)-1:05d}.png", output/f"{name}_final.png")
    times = [f.get("time", f.get("time_s",0)) for f in frames]
    manifest = []
    for i in range(len(frames)):
        manifest.append(f"file 'frame_{i:05d}.png'")
        interval = times[i+1]-times[i] if i+1 < len(frames) else 1/fps*speed
        manifest.append(f"duration {max(interval/speed, 0.000001):.9f}")
    manifest.append(f"file 'frame_{len(frames)-1:05d}.png'")
    (folder/"playback.txt").write_text("\n".join(manifest)+"\n")
    if shutil.which("ffmpeg"):
        subprocess.run(["ffmpeg","-hide_banner","-loglevel","error","-y","-f","concat","-safe","0","-i",str(folder/"playback.txt"),"-vf",f"fps={fps}","-c:v","libx264","-crf","19","-pix_fmt","yuv420p","-movflags","+faststart",str(output/f"{name}.mp4")],check=True)
    metadata = {"source":str(source),"sampled_states":len(frames),"simulation_start_s":times[0],"simulation_end_s":times[-1],"playback_speed":speed,"video_fps":fps,"terminal_hold_note":"approximately two video frames after final simulation timestamp","projection":"orthographic equal-size cubes","display_cutaway":"front exterior solid walls and ceiling only","initial_water_cells":len(frames[0]["water"]),"final_water_cells":len(frames[-1]["water"])}
    (output/f"{name}_preview.json").write_text(json.dumps(metadata,indent=2)+"\n")
    print(json.dumps(metadata))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sources", nargs="*", type=Path)
    parser.add_argument("--output",type=Path,default=DEFAULT_OUTPUT)
    parser.add_argument("--fps",type=int,default=30)
    parser.add_argument("--speed",type=float,default=1,help="simulation seconds per video second")
    args = parser.parse_args()
    if args.speed <= 0 or args.fps <= 0:
        parser.error("--speed and --fps must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    for source in args.sources or sorted((args.output/"data").glob("*.jsonl")):
        preview(source.resolve(), args.output.resolve(), args.fps, args.speed)


if __name__ == "__main__":
    main()
