#!/usr/bin/env python3
"""Two-patch min-of-five occupancy fit with a shared face by construction.
Q_A=T(x,z)+(y-1)L_A; Q_B=T(x,z)+(y+1)L_B. 70 free parameters,
100 exported coefficients, no extra renderer data. All boundary equalities
are identities; this is not the earlier soft continuity penalty experiment.
"""
import os
for key in ('OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS'):os.environ[key]='1'
import argparse,itertools,json,subprocess,time
from pathlib import Path
import numpy as np
from PIL import Image
from fit import features,predict,sample_patch,write_json

# Maps one [T(6), LA(4), LB(4)] vector to the two monomial coefficient vectors.
MAP=np.zeros((2,10,14))
for side in range(2):
 m=MAP[side];j=6+side*4;s=-1 if side==0 else 1
 for row,col in [(0,0),(1,1),(3,2),(4,3),(8,4),(6,5)]:m[row,col]=1
 m[0,j]=s;m[1,j+1]=s;m[2,j]=1;m[2,j+2]=s;m[3,j+3]=s
 m[5,j+2]=1;m[7,j+1]=1;m[9,j+3]=1
PROJECT=np.linalg.pinv(np.vstack(MAP))

def bake(z):return np.array([z@m.T for m in MAP])

def face_coeff(c,side):
 return np.column_stack((c[:,0]+side*c[:,2]+c[:,5],c[:,1]+side*c[:,7],c[:,3]+side*c[:,9],c[:,4],c[:,8],c[:,6]))

def loss_gradient(z,data,anchor):
 coeff=bake(z);gradient=np.zeros_like(z);loss=0.;margin=.02
 for side,(a,positive) in enumerate(data):
  q=a@coeff[side].T;d=np.zeros_like(q);np_=max(1,int(positive.sum()));nn=max(1,len(a)-int(positive.sum()))
  residual=np.maximum(0.,margin-q[positive]);loss+=.5*np.sum(residual**2)/np_;d[positive]=-residual/np_
  negative=~positive;rows=np.flatnonzero(negative);owner=np.argmin(q[negative],axis=1);r=np.maximum(0.,margin+q[rows,owner]);loss+=.5*np.sum(r*r)/nn;d[rows,owner]=r/nn
  gradient+=(d.T@a)@MAP[side]
 loss+=1e-5*np.mean((z-anchor)**2);gradient+=2e-5*(z-anchor)/z.size
 return float(loss),gradient

def self_test():
 rng=np.random.default_rng(7);z=rng.normal(size=(5,14));c=bake(z)
 assert np.max(np.abs(face_coeff(c[0],1)-face_coeff(c[1],-1)))<1e-12
 p=rng.uniform(-1,1,(300,3));a=features(p);data=[(a,rng.random(len(a))>.5)]*2
 loss,grad=loss_gradient(z,data,z.copy());idx=np.unravel_index(np.argmax(np.abs(grad)),grad.shape);eps=1e-6;plus=z.copy();minus=z.copy();plus[idx]+=eps;minus[idx]-=eps
 numeric=(loss_gradient(plus,data,z)[0]-loss_gradient(minus,data,z)[0])/(2*eps)
 assert abs(numeric-grad[idx])<1e-6,(numeric,grad[idx])
 assert np.linalg.matrix_rank(np.vstack(MAP))==14
 return dict(shared_face_identity=True,gradient_check=True,free_parameters=70,exported_coefficients=100)

def main(args):
 tests=self_test()
 if args.self_test:print(tests);return
 start=time.monotonic();out=Path(args.output);out.mkdir(parents=True,exist_ok=False);source=Path(args.source)
 def status(phase,**kw):
  record=dict(phase=phase,elapsed_seconds=time.monotonic()-start,**kw);write_json(out/'status.json',record)
  with (out/'run.log').open('a') as f:f.write(json.dumps(record)+'\n')
  print(json.dumps(record),flush=True)
 model=json.loads((source/'terrain.json').read_text());all_coeff=np.array(model['coefficients']);keys=[(0,0,3),(1,0,3)];original=np.array([all_coeff[k] for k in keys]);edge=128;voxel=.0625
 if model['dimensions']!=[512,512,512] or model['patch_edge_m']!=8. or model['voxel_m']!=voxel or original.shape!=(2,5,10):raise ValueError('This experiment requires the original two selected patches and model')
 rng=np.random.default_rng(1807);density=np.memmap(source/'density.f32',dtype='<f4',mode='r',shape=(512,512,512));blocks=[density[y*edge:(y+1)*edge,z*edge:(z+1)*edge,x*edge:(x+1)*edge] for y,z,x in keys]
 train=[];hold=[]
 for side,block in enumerate(blocks):
  p,y=sample_patch(block,4096,rng)
  # Explicit narrow band of native centers on both sides of the shared face.
  ids=rng.integers(0,edge,(2048,3));ids[:,0]=rng.integers(edge-4,edge,2048) if side==0 else rng.integers(0,4,2048)
  points=(ids[:,[2,0,1]]+.5)/edge*2-1;p=np.vstack((p,points));y=np.r_[y,block[ids[:,0],ids[:,1],ids[:,2]]];train.append((features(p),y>0))
  ids=rng.integers(0,edge,(16384,3));points=(ids[:,[2,0,1]]+.5)/edge*2-1;hold.append((features(points),block[ids[:,0],ids[:,1],ids[:,2]]>0))
 grid_u,grid_v=np.meshgrid((np.arange(edge)+.5)/edge*2-1,(np.arange(edge)+.5)/edge*2-1);pa=np.column_stack((grid_u.ravel(),np.ones(edge*edge),grid_v.ravel()));pb=pa.copy();pb[:,1]=-1.;fa,fb=features(pa),features(pb)
 points=(pa+1)*4+(np.array(model['origin_cells'])+np.array([384,0,0]))*voxel
 write_json(out/'boundary-points.json',points.tolist());write_json(out/'recipe.json',model['recipe'])
 subprocess.run([args.exporter,str(out/'recipe.json'),str(out/'boundary-points.json'),str(out/'boundary-density.f64')],check=True)
 face_target=np.fromfile(out/'boundary-density.f64',dtype='<f8')>0
 write_json(out/'run_config.json',dict(source=args.source,seconds=args.seconds,max_steps=args.steps,selected_yzx=keys,seed=1807,construction=tests))
 def assess(coeff,full_face=False):
  errors=[float(np.mean((predict(a.astype(coeff.dtype,copy=False),coeff[i])>0)!=labels)) for i,(a,labels) in enumerate(hold)]
  stride=1 if full_face else 8;sa=predict(fa[::stride].astype(coeff.dtype,copy=False),coeff[0])>0;sb=predict(fb[::stride].astype(coeff.dtype,copy=False),coeff[1])>0;target=face_target[::stride]
  return dict(holdout_error_fraction=errors,face_disagreements=int(np.count_nonzero(sa!=sb)),face_target_error_fraction=float((np.mean(sa!=target)+np.mean(sb!=target))/2))
 before=assess(original,True);write_json(out/'before.json',before)
 # Positive per-branch gauges preserve every native occupancy label.
 gauge=original.copy()
 for side in range(2):
  norm=np.linalg.norm(face_coeff(gauge[side],1 if side==0 else -1),axis=1);norm=np.maximum(norm,1e-4*np.linalg.norm(gauge[side],axis=1));gauge[side]/=np.maximum(norm,1e-12)[:,None]
 status('ranking_pairings',permutations=120)
 ranked=[]
 for permutation in itertools.permutations(range(5)):
  pair=np.vstack((gauge[0].T,gauge[1,list(permutation)].T)).T
  z=pair@PROJECT.T;metrics=assess(bake(z));score=sum(metrics['holdout_error_fraction'])+.1*metrics['face_target_error_fraction'];ranked.append((score,permutation,z,metrics))
 ranked.sort(key=lambda r:r[0]);write_json(out/'pairing-ranking.json',[dict(score=r[0],pairing=list(r[1]),initial=r[3]) for r in ranked])
 training_start=time.monotonic();global_best=None;global_score=float('inf');hist=[]
 for candidate,(_,permutation,initial,_) in enumerate(ranked[:3]):
  z=initial.copy();anchor=z.copy();m=np.zeros_like(z);v=m.copy();best_score=float('inf');best=z.copy();stale=0;candidate_start=time.monotonic()
  status('training',candidate=candidate+1,candidates=3,step=0,pairing=list(permutation),before=before)
  for step in range(1,args.steps+1):
   if step%500==0:
    # Fresh counterexamples are separate from the fixed validation holdout.
    current=bake(z)
    for side,block in enumerate(blocks):
     ids=rng.integers(0,edge,(8192,3));a=features((ids[:,[2,0,1]]+.5)/edge*2-1);labels=block[ids[:,0],ids[:,1],ids[:,2]]>0;wrong=np.flatnonzero((predict(a,current[side])>0)!=labels)[:256]
     old_a,old_labels=train[side]
     if len(wrong) and len(old_a)<32768:train[side]=(np.vstack((old_a,a[wrong])),np.r_[old_labels,labels[wrong]])
   loss,grad=loss_gradient(z,train,anchor);m=.9*m+.1*grad;v=.999*v+.001*grad*grad;z-=.001*(m/(1-.9**step))/(np.sqrt(v/(1-.999**step))+1e-8)
   if step%100==0 or step==args.steps:
    metrics=assess(bake(z));score=sum(metrics['holdout_error_fraction'])+.1*metrics['face_target_error_fraction']
    if score<best_score-1e-7:best=z.copy();best_score=score;stale=0
    else:stale+=1
    if score<global_score:
     global_score=score;global_best=z.copy();cc=all_coeff.copy()
     for i,k in enumerate(keys):cc[k]=bake(z)[i]
     write_json(out/'terrain-checkpoint.json',dict(model,coefficients=cc.tolist()))
    status('training',candidate=candidate+1,candidates=3,step=step,loss=loss,best_score=best_score,current=metrics,stale_checks=stale)
    # Up to a third of the total optimization budget per initial pairing.
    if (stale>=20 and step>=2500) or time.monotonic()-candidate_start>=args.seconds/3 or time.monotonic()-training_start>=args.seconds:break
  hist.append(dict(pairing=list(permutation),steps=step,score=best_score,parameters=best.tolist()))
  if time.monotonic()-training_start>=args.seconds:break
 # Exhaustively compare finalists; initialization alone is also a valid constrained candidate.
 status('validating_native_voxels')
 finalists=[np.array(h['parameters']) for h in hist]+[r[2] for r in ranked[:3]]
 def native_metrics(coeff):
  metrics=[]
  for side,block in enumerate(blocks):
   errors=false_air=false_solid=band_errors=band_cells=0
   for start_index in range(0,block.size,65536):
    ids=np.arange(start_index,min(start_index+65536,block.size));yy,zz,xx=np.unravel_index(ids,block.shape);p=(np.column_stack((xx,yy,zz))+.5)/edge*2-1;a=features(p);values=block[yy,zz,xx];target=values>0;pred=predict(a.astype(coeff.dtype,copy=False),coeff[side])>0;wrong=pred!=target;band=np.abs(values)<1.
    errors+=int(wrong.sum());false_air+=int(np.count_nonzero(target&~pred));false_solid+=int(np.count_nonzero(~target&pred));band_errors+=int(np.count_nonzero(wrong&band));band_cells+=int(band.sum())
   metrics.append(dict(patch_yzx=keys[side],native_cells=block.size,errors=errors,false_air=false_air,false_solid=false_solid,surface_band_cells=band_cells,surface_band_errors=band_errors))
  return metrics
 native_before=native_metrics(original);scored=[]
 for z in finalists:
  c=bake(z);native=native_metrics(c);scored.append((sum(m['errors'] for m in native),z,native))
 scored.sort(key=lambda r:r[0]);_,best,native_after=scored[0];coeff=bake(best);full=all_coeff.copy()
 for i,k in enumerate(keys):full[k]=coeff[i]
 write_json(out/'terrain.json',dict(model,coefficients=full.tolist()))
 cropped=dict(model,coefficients=full[0:2,0:1,3:4].tolist(),patches_per_axis=[1,2,1],dimensions=[128,256,128],origin_cells=[model['origin_cells'][0]+384,model['origin_cells'][1],model['origin_cells'][2]])
 write_json(out/'terrain-cropped.json',cropped);np.save(out/'shared-parameters.npy',best)
 after=assess(coeff,True);after32=assess(coeff.astype(np.float32),True);native_after32=native_metrics(coeff.astype(np.float32));residual=face_coeff(coeff[0],1)-face_coeff(coeff[1],-1)
 # Neighbor faces are monitored, not claimed fixed by a two-patch experiment.
 incident=[]
 for key in np.ndindex(*all_coeff.shape[:3]):
  for axis,storage in [(0,2),(1,0),(2,1)]:
   other=list(key);other[storage]+=1;other=tuple(other)
   if other[storage]>=all_coeff.shape[storage] or (key not in keys and other not in keys):continue
   tang=[a for a in range(3) if a!=axis];p=np.empty_like(pa);p[:,axis]=1;p[:,tang[0]]=grid_u.ravel();p[:,tang[1]]=grid_v.ravel();q=p.copy();q[:,axis]=-1;a,b=features(p),features(q)
   counts=[int(np.count_nonzero((predict(a,cc[key])>0)!=(predict(b,cc[other])>0))) for cc in (all_coeff,full)];incident.append(dict(a_yzx=key,b_yzx=other,axis_xyz=axis,before=counts[0],after=counts[1]))
 for name,cc in [('before',original),('after',coeff)]:
  a=predict(fa,cc[0])>0;b=predict(fb,cc[1])>0;canvas=Image.new('RGB',(1152,384))
  for i,mask in enumerate((a,b,a!=b)):
   rgb=np.where(mask[:,None],np.array([155,155,160]),np.array([35,48,63])).astype('uint8');canvas.paste(Image.fromarray(rgb.reshape(edge,edge,3)).resize((384,384)),(i*384,0))
  canvas.save(out/f'boundary-{name}.png')
 result=dict(before=before,after=after,after_float32=after32,native_before=native_before,native_after=native_after,native_after_float32=native_after32,face_coefficient_max_residual=float(np.max(np.abs(residual))),coefficient_count=100,gpu_coefficient_bytes_cropped=480,free_parameters=70,incident_faces=incident,elapsed_seconds=time.monotonic()-start,training_seconds=time.monotonic()-training_start,finalists=len(finalists),note='Only central face constrained. Occupancy fitting remains nonconvex. No default world replacement.')
 write_json(out/'metrics.json',result);write_json(out/'candidate-training.json',hist);status('complete',metrics=result)

if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',default='output/fit-occupancy');p.add_argument('--output',default='output/fit-hard-boundary');p.add_argument('--seconds',type=float,default=180.);p.add_argument('--steps',type=int,default=20000);p.add_argument('--exporter',default='target/release/fbm_points');p.add_argument('--self-test',action='store_true');main(p.parse_args())
