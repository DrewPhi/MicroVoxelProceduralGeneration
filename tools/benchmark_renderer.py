"""Repeatable same-camera comparisons; run from the repository root.
Timings are late-frame GPU samples, not whole-run percentiles.
"""
import argparse
import subprocess,json,statistics,pathlib
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--modes',nargs='+',choices=['shaded','snapped','voxels','voxel-reference'],default=['shaded','snapped','voxels','voxel-reference'])
parser.add_argument('--verify',action='store_true')
parser.add_argument('--size',default='320x240')
parser.add_argument('--frames',type=int,default=62)
parser.add_argument('--repeats',type=int,default=2)
parser.add_argument('--output',type=pathlib.Path,default=pathlib.Path('output/analytic/profile'))
args=parser.parse_args()
if args.frames<32 or args.repeats<1:parser.error('Use at least 32 frames and one repeat to exclude the startup timestamp')
root=args.output
root.mkdir(parents=True,exist_ok=True)
views={'overview':None,'near_oblique':'28,25,28,-135,-20','near_horizontal':'28,25,28,-135,0','overhead':'16,40,16,0,-89','underside':'16,-4,16,0,75'}
rows=[]
for view,cam in views.items():
 for mode in args.modes:
  samples=[]
  for repeat in range(args.repeats):
   path=root/f'{view}-{mode}-{repeat}.png'
   cmd=['./analytic','--headless','--size',args.size,'--frames',str(args.frames),'--mode',mode,'--capture',str(path)]
   if cam:cmd+=['--camera',cam]
   if args.verify:cmd+=['--verify']
   r=subprocess.run(cmd,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
   if r.returncode:raise RuntimeError(r.stdout+r.stderr)
   samples.append(json.loads(path.with_suffix('.json').read_text()))
  row={'view':view,'mode':mode,'gpu_ms':statistics.median(s['gpu_compute_ms'] for s in samples) if all(s['gpu_compute_ms'] is not None for s in samples) else None,'gpu_samples_ms':[s['gpu_compute_ms'] for s in samples],'cpu_encode_ms':statistics.median(s['cpu_encode_ms'] for s in samples),'visits':samples[-1]['average_traversal_visits'],'quadratic_tests':samples[-1]['average_quadratic_tests']}
  rows.append(row);print(json.dumps(row),flush=True)
(root/'summary.json').write_text(json.dumps({'adapter':samples[0]['adapter'],'size':args.size,'frames_per_run':args.frames,'repeats':args.repeats,'rows':rows},indent=2))
