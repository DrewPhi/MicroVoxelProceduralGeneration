import json,pathlib,subprocess
out=pathlib.Path('output/analytic/fixtures');out.mkdir(parents=True,exist_ok=True)
base=json.loads(pathlib.Path('output/fit-occupancy/terrain.json').read_text())
cases={'sphere':[[.47,0,0,0,-1,-1,-1,0,0,0]],'saddle':[[.017,.23,-1,.17,.3,-.07,-.2,.14,.07,-.09]],'intersection':[[.52,.24,.1,0,-1,-1,-1,0,0,0],[.41,-.3,.14,.2,-1,-1,-1,0,0,0]],'patch_jump':[[1,0,0,0,0,0,0,0,0,0]]}
for name,qs in cases.items():
 d=base.copy();d.update(patches_per_axis=[1,1,1],origin_cells=[0,0,0],dimensions=[32,32,32],patch_edge_m=2.,quadratics_per_patch=len(qs),coefficients=[[[qs]]])
 if name=='patch_jump':d.update(patches_per_axis=[2,1,1],dimensions=[64,32,32],coefficients=[[[[[-1,0,0,0,0,0,0,0,0,0]],qs]]])
 p=out/f'{name}.json';p.write_text(json.dumps(d))
 for index,cam in enumerate([None,'-1,1,1,0,0']):
  cmd=['./analytic','--scene',str(p),'--headless','--size','320x240','--frames','2','--mode','voxels','--verify','--capture',str(out/f'{name}-{index}.png')]
  if cam:cmd+=['--camera',cam]
  r=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
  if r.returncode:raise RuntimeError(name+r.stdout+r.stderr)
  report=json.loads((out/f'{name}-{index}.json').read_text());print(name,index,report['verification'],flush=True)
