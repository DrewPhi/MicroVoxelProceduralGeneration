#!/usr/bin/env python3
"""Run independent 64/256 spp references and same-camera native GPU comparison.

Default output resolution is 1920x1080. This is a bounded native fixture quality
experiment; it does not certify hybrid ownership, streaming, or performance.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

from reference_compare import compare

CAMERAS = {'fixture': ([11,9,-15],[-1,3,1]),
           'cave': ([-3,3,-7],[-3,3,6]),
           'bridge': ([4.5,2.8,-2.5],[4.5,4.1,-1.5])}
ROOT = Path(__file__).resolve().parents[1]


def run(command):
    print('+ '+ ' '.join(map(str,command)), flush=True)
    subprocess.run(list(map(str,command)),cwd=ROOT,check=True)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--camera',choices=CAMERAS,default='fixture')
    p.add_argument('--width',type=int,default=1920)
    p.add_argument('--height',type=int,default=1080)
    p.add_argument('--seed',type=int,default=7)
    p.add_argument('--threads',type=int,default=min(8,os.cpu_count() or 1))
    p.add_argument('--skip-build',action='store_true')
    p.add_argument('--gpu-bin',type=Path,default=ROOT/'target/release/frontier')
    p.add_argument('--reference-bin',type=Path,default=ROOT/'target/release/frontier-reference')
    p.add_argument('--depth-tolerance',type=float,default=.002)
    a=p.parse_args()
    if not (1<=a.width<=8192 and 1<=a.height<=8192 and a.threads>0):
        p.error('dimensions must be in 1..8192 and threads positive')
    output=a.output.resolve();output.mkdir(parents=True,exist_ok=True)
    if not a.skip_build:
        run(['cargo','build','--release','-p','frontier-reference','--bin','frontier-reference'])
        run(['cargo','build','--release','-p','frontier-tools','--bin','frontier'])
    eye,target=CAMERAS[a.camera]
    camera=['--eye',','.join(map(str,eye)),'--target',','.join(map(str,target)), '--fov-y','60']
    common=['--width',a.width,'--height',a.height,'--seed',a.seed]+camera
    for spp,sample_seed in [(64,19),(256,83)]:
        run([a.reference_bin,'--output',output/f'reference{spp}', '--spp',spp,
             '--sample-seed',sample_seed,'--threads',a.threads]+common)
    gpu=output/'gpu.png'
    run([a.gpu_bin,'--scene','fixture','--renderer','mesh','--msaa','1',
         '--voxel-filter','off','--capture-linear','--capture',gpu]+common)
    report=compare(output/'reference256',gpu,output/'comparison',
                   output/'reference64',a.depth_tolerance)
    print(json.dumps(report,indent=2))
    (output/'run.json').write_text(json.dumps(dict(camera=a.camera,width=a.width,
        height=a.height,seed=a.seed,reference_spp=[64,256],sample_seeds=[19,83],
        gpu_binary=str(a.gpu_bin),reference_binary=str(a.reference_bin)),indent=2)+'\n')


if __name__=='__main__':
    main()
