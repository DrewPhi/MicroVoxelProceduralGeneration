"""Compare optional envelope traversal with unchanged grid/air baselines.
Requires local 1 km initialization and real GPU access. Raw reports/captures are
ignored output data. GPU compute_ms is a sampled warmed timestamp, not a p95.
"""
import argparse
import json
import pathlib
import subprocess
from PIL import Image, ImageChops
parser=argparse.ArgumentParser()
parser.add_argument('--scene',default='output/world-1km-initialized/terrain.json')
parser.add_argument('--binary',default='target/release/analytic')
parser.add_argument('--frames',type=int,default=62)
a=parser.parse_args()
out=pathlib.Path('output/envelopes/benchmark');out.mkdir(parents=True,exist_ok=True)
reports=[]
for mode,poses,samples in [('voxels',[-25,0,25],[1,2,4]),('shaded',[25],[1])]:
    for pitch in poses:
        for count in samples:
            images=[]
            for traversal in ['grid','hierarchy','envelopes']:
                stem=f'{mode}-{pitch}-{count}-{traversal}'
                path=out/f'{stem}.png'
                cmd=[a.binary,'--scene',a.scene,'--headless','--camera',f'750,270,750,-145,{pitch}',
                     '--walk','--mode',mode,'--size','960x540','--frames',str(a.frames),
                     '--samples',str(count),'--traversal',traversal,'--capture',str(path)]
                result=subprocess.run(cmd,text=True,capture_output=True)
                if result.returncode:raise RuntimeError(' '.join(cmd)+'\n'+result.stdout+result.stderr)
                r=json.loads(path.with_suffix('.json').read_text());r['pitch']=pitch
                reports.append(r);images.append(Image.open(path).convert('RGB'))
                print(stem,round(r['gpu_compute_ms'],3),'ms',
                      round(r['average_quadratic_tests'],2),'source tests',
                      round(r['average_envelope_tests'],2),'envelope tests',flush=True)
                (out/'summary.json').write_text(json.dumps(reports,indent=2))
            for image in images[1:]:
                diff=ImageChops.difference(images[0],image)
                high=max(upper for lower,upper in diff.getextrema())
                assert high<=2,(mode,pitch,count,'image mismatch',high)
print('Benchmarks and image comparisons complete:',out/'summary.json',flush=True)
