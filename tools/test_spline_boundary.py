#!/usr/bin/env python3
"""Reconstruct the two previously selected seam patches with the published QSS rules.
Export independent analytic models; exhaustively compare native cells to Rust FBM.
"""
import os
for k in ('OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS'):os.environ[k]='1'
import argparse,itertools,json,subprocess,time
from pathlib import Path
import numpy as np
from PIL import Image
from fit import features,predict,write_json
from quadratic_spline import construct,evaluate,piece_ids,self_test

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',default='output/fit-occupancy');p.add_argument('--output',default='output/fit-spline-boundary');args=p.parse_args()
 out=Path(args.output);out.mkdir(parents=True,exist_ok=True);source=Path(args.source);start=time.monotonic()
 def status(phase,**kw):
  d=dict(phase=phase,elapsed_seconds=time.monotonic()-start,**kw);write_json(out/'status.json',d);print(json.dumps(d),flush=True)
 baseline=json.loads((source/'terrain.json').read_text());voxel=baseline['voxel_m'];original=np.array(baseline['coefficients']);selection=json.loads(Path('output/fit-boundaries-targeted/selection.json').read_text())
 selected=np.array(selection['selected_yzx']);lo=selected.min(axis=0);hi=selected.max(axis=0)+1;offset=lo[[2,0,1]]*128;dims=(hi-lo)[[2,0,1]]*128
 if baseline['dimensions']!=[512,512,512] or baseline['patch_edge_m']!=8. or voxel!=.0625 or selected.tolist()!=[[0,0,3],[1,0,3]]:raise ValueError('This targeted fixture expects the original 512-cubed scene and selected Y seam; use a separate experiment for other inputs.')
 origin=np.array(baseline['origin_cells'])+offset;physical=dims*voxel;global_origin=origin*voxel
 density=np.memmap(source/'density.f32',dtype='<f4',mode='r',shape=(512,512,512));target=np.array(density[lo[0]*128:hi[0]*128,lo[1]*128:hi[1]*128,lo[2]*128:hi[2]*128])>0
 write_json(out/'recipe.json',baseline['recipe']);tests=self_test();results=[]
 # Baseline is scored on precisely the same native cells as each reconstruction.
 old_errors=0
 for key in map(tuple,selected):
  from occupancy import native_check
  y,z,x=key;b=density[y*128:(y+1)*128,z*128:(z+1)*128,x*128:(x+1)*128];old_errors+=native_check(b,original[key])[0]
 for spacing in (8.,4.,2.,1.):
  folder=out/f'{spacing:g}m';folder.mkdir(exist_ok=True);counts=np.rint(physical/spacing).astype(int)
  keys=list(itertools.product(*(range(-1,int(g)+1) for g in counts)))
  points=global_origin+(np.array(keys)+.5)*spacing
  write_json(folder/'points.json',points.tolist());subprocess.run(['target/release/fbm_points',str(out/'recipe.json'),str(folder/'points.json'),str(folder/'samples.f64')],check=True)
  values=np.fromfile(folder/'samples.f64',dtype='<f8');samples=dict(zip(keys,values));c=np.empty((counts[1],counts[2],counts[0],24,10))
  status('constructing',spacing_m=spacing,cubes=int(np.prod(counts)),samples=len(keys))
  for y,z,x in np.ndindex(*c.shape[:3]):c[y,z,x]=construct((x,y,z),samples.__getitem__)
  model=dict(baseline,representation='quadratic-super-spline-24',solid='Q>0',patches_per_axis=counts.tolist(),quadratics_per_patch=24,patch_edge_m=spacing,origin_cells=origin.tolist(),dimensions=dims.tolist(),coefficients=c.tolist(),construction='Roessl et al. VIS 2003 sections 2-3; 65 BB controls; 27-sample halo',source_original_patches_yzx=selected.tolist())
  write_json(folder/'terrain.json',model)
  shape=target.shape;fitted=np.empty(shape,dtype=bool);errors=0
  for first in range(0,target.size,65536):
   ids=np.arange(first,min(first+65536,target.size));yy,zz,xx=np.unravel_index(ids,shape);p=(np.column_stack((xx,yy,zz))+.5)*voxel;pc=np.floor(p/spacing).astype(int);local=2*(p/spacing-pc)-1;t=piece_ids(local);coeff=c[pc[:,1],pc[:,2],pc[:,0],t];v=np.einsum('ij,ij->i',features(local),coeff)>0
   fitted[yy,zz,xx]=v;errors+=int(np.count_nonzero(v!=target[yy,zz,xx]))
  # Original seam y=8m; independently evaluate the two face restrictions.
  zz,xx=np.mgrid[:shape[1],:shape[2]];world=np.column_stack(((xx.ravel()+.5)*voxel,np.full(xx.size,8.),(zz.ravel()+.5)*voxel));pc=np.floor(world/spacing).astype(int);local=2*(world/spacing-pc)-1
  a_local=local.copy();a_local[:,1]=1.;a_pc=pc.copy();a_pc[:,1]-=1
  def face_values(coords,cells,cast):
   ids=piece_ids(coords);q=c[cells[:,1],cells[:,2],cells[:,0],ids].astype(cast);return np.einsum('ij,ij->i',features(coords).astype(cast),q)
  a=face_values(a_local,a_pc,np.float64);b=face_values(local,pc,np.float64);a32=face_values(a_local,a_pc,np.float32);b32=face_values(local,pc,np.float32)
  va=predict(features(a_local),original[tuple(selected[0])])>0;vb=predict(features(local),original[tuple(selected[1])])>0 # only valid baseline at original 8m spacing
  if spacing==8.:before_signs=(va,vb)
  for name,left,right in [('before',*before_signs),('after',a>0,b>0)]:
   image=Image.new('RGB',(1152,384))
   for i,mask in enumerate((left,right,left!=right)):
    colors=np.where(mask[:,None],np.array([155,155,160]),np.array([35,48,63])).astype('uint8');image.paste(Image.fromarray(colors.reshape(shape[1],shape[2],3)).resize((384,384)),(i*384,0))
   image.save(folder/f'boundary-{name}.png')
  # The isometric comparison can be rendered by the analytic viewer; these
  # orthogonal density slices additionally reveal lost cave/overhang detail.
  for name,volume in [('fbm',target),('spline',fitted)]:
   rgb=np.where(volume[:,shape[1]//2,:,None],np.array([155,155,160]),np.array([35,48,63])).astype('uint8');Image.fromarray(rgb[::-1]).resize((384,768)).save(folder/f'{name}-slice.png')
  result=dict(spacing_m=spacing,cubes=int(np.prod(counts)),quadratic_pieces=int(np.prod(counts))*24,coefficient_bytes_f32=int(c.size)*4,gpu_expanded_bytes=int(c.size//10)*48,shared_sample_bytes_f32=len(keys)*4,native_cells=int(target.size),baseline_native_errors=old_errors,spline_native_errors=errors,baseline_error_fraction=old_errors/target.size,spline_error_fraction=errors/target.size,boundary_samples=len(a),baseline_boundary_disagreements=int(np.count_nonzero(before_signs[0]!=before_signs[1])),spline_boundary_disagreements=int(np.count_nonzero((a>0)!=(b>0))),boundary_max_value_jump=float(np.max(np.abs(a-b))),f32_boundary_disagreements=int(np.count_nonzero((a32>0)!=(b32>0))),f32_boundary_max_value_jump=float(np.max(np.abs(a32-b32))),note='Published direct quasi-interpolation, no training. Same two native blocks; original fixed baseline preserved.')
  write_json(folder/'metrics.json',result);results.append(result);status('validated',result=result)
 write_json(out/'metrics.json',dict(construction_tests=tests,results=results,elapsed_seconds=time.monotonic()-start));status('complete',results=results)
if __name__=='__main__':main()
