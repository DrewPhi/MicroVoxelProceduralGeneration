#!/usr/bin/env python3
"""Orthographic whole-voxel preview of offline FVPHYS01 reference snapshots.

This draws the recorded solver state; it is not an in-game capture. Fraction
thresholding changes only this diagnostic image, never the saved fluid volume.
"""
import argparse
from pathlib import Path
import struct
import subprocess
import numpy as np
from PIL import Image, ImageDraw


def read_state(path):
    raw = path.read_bytes()
    if raw[:8] != b'FVPHYS01':
        raise ValueError('expected FVPHYS01 snapshot')
    integers = struct.unpack_from('<8Q', raw, 8)
    values = struct.unpack_from('<12d', raw, 72)
    _, nx, ny, nz, *_ = integers
    cells = nx * ny * nz
    faces = (nx+1)*ny*nz + nx*(ny+1)*nz + nx*ny*(nz+1)
    expected = 168 + (2*cells+faces)*8 + cells
    if len(raw) != expected:
        raise ValueError('snapshot length disagrees with dimensions')
    fraction = np.frombuffer(raw, '<f8', count=cells, offset=168).reshape(nz, ny, nx)
    solid = np.frombuffer(raw, 'u1', count=cells, offset=168+(2*cells+faces)*8).reshape(nz, ny, nx)
    return fraction, solid, values


def render(path, output, threshold):
    fraction, solid, state = read_state(path)
    nz, ny, nx = fraction.shape
    if max(nx, ny, nz) > 256:
        raise ValueError('preview limited to 256 cells per axis')
    wet = (fraction >= threshold) & ~solid.astype(bool)
    occupied = wet | solid.astype(bool)
    width, height = 1000, 720
    scale = min((width-80)/(nx+nz), (height-160)/(ny+(nx+nz)*.4))
    origin = np.array([width*.5+(nz-nx)*scale*.5,
                       height*.56-scale*(.4*(nx+nz)-ny)*.5])
    def project(p):
        x, y, z = p
        return tuple(origin+scale*np.array([x-z, .4*(x+z)-y]))
    faces = []
    offsets = [
        (1,0,0, [(1,0,0),(1,0,1),(1,1,1),(1,1,0)], .72),
        (0,1,0, [(0,1,0),(1,1,0),(1,1,1),(0,1,1)], 1.),
        (0,0,1, [(0,0,1),(1,0,1),(1,1,1),(0,1,1)], .86),
    ]
    for z,y,x in zip(*np.nonzero(occupied)):
        base = (95,99,106) if solid[z,y,x] else (45,143,181)
        for dx,dy,dz,corners,shade in offsets:
            xx,yy,zz = x+dx,y+dy,z+dz
            if xx < nx and yy < ny and zz < nz and occupied[zz,yy,xx]:
                continue
            polygon = [project((x+a,y+b,z+c)) for a,b,c in corners]
            depth = x+y+z+(dx+dy+dz)*.5
            faces.append((depth,polygon,tuple(round(v*shade) for v in base)))
    image = Image.new('RGB',(width,height),(27,32,39))
    draw = ImageDraw.Draw(image)
    for _,polygon,color in sorted(faces,key=lambda f:f[0]):
        draw.polygon(polygon,fill=color)
    draw.text((24,20),'Offline physical reference - whole voxel occupancy preview',fill=(230,235,240))
    draw.text((24,45),f't={state[8]:.3f}s | grid {nx} x {ny} x {nz} | cell {state[0]*100:.4g} cm',fill=(205,215,225))
    volume = float(fraction.sum())*state[0]**3
    draw.text((24,65),f'Physical volume {volume*1000:.6f} L | display threshold {threshold:g}',fill=(205,215,225))
    draw.text((24,height-25),'Rendering does not feed back into fluid mass; this is not the game renderer.',fill=(180,190,200))
    image.save(output)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('input',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--threshold',type=float,default=.5)
    parser.add_argument('--fps',type=int,default=24)
    args=parser.parse_args()
    if not 0 < args.threshold <= 1 or not 1 <= args.fps <= 120:
        parser.error('threshold must be (0,1], fps must be 1..120')
    if args.input.is_file():
        args.output.parent.mkdir(parents=True,exist_ok=True)
        render(args.input,args.output,args.threshold)
        return
    args.output.mkdir(parents=True,exist_ok=False)
    paths=sorted(args.input.glob('state-*.fvphys'))
    if not paths:
        parser.error('no state-*.fvphys snapshots')
    for i,path in enumerate(paths):
        render(path,args.output/f'frame-{i:05d}.png',args.threshold)
    subprocess.run(['ffmpeg','-y','-framerate',str(args.fps),'-i',str(args.output/'frame-%05d.png'),
                    '-an','-c:v','libx264','-pix_fmt','yuv420p',str(args.output/'water.mp4'),
                    '-loglevel','error'],check=True)


if __name__=='__main__':
    main()
