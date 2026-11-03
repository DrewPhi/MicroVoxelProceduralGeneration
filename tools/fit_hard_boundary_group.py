#!/usr/bin/env python3
"""Joint eight-patch occupancy fit in the nullspace of all 12 face constraints.
Consistent branch labels around grid cycles; no independently conflicting face
pairings. Five quadratics/cube, no subdivisions or additional renderer payload.
"""
import os
for name in ('OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS'):os.environ[name]='1'
import argparse,itertools,json,subprocess,time
from pathlib import Path
import numpy as np
from PIL import Image
from fit import features,predict,sample_patch,write_json

KEYS=list(np.ndindex(2,2,2)) # YZX
TERMS=[(0,0,0),(1,0,0),(0,1,0),(0,0,1),(2,0,0),(0,2,0),(0,0,2),(1,1,0),(1,0,1),(0,1,1)]

def trace_matrix(axis,side):
 tang=[a for a in range(3) if a!=axis];basis=[(0,0),(1,0),(0,1),(2,0),(1,1),(0,2)];m=np.zeros((6,10))
 for j,t in enumerate(TERMS):m[basis.index((t[tang[0]],t[tang[1]])),j]=side**t[axis]
 return m

def constraint_space():
 links=[];rows=[]
 for i,key in enumerate(KEYS):
  for axis,storage in [(0,2),(1,0),(2,1)]:
   other=list(key);other[storage]+=1
   if other[storage]>=2:continue
   j=KEYS.index(tuple(other));a=trace_matrix(axis,1);b=trace_matrix(axis,-1);m=np.zeros((6,80));m[:,i*10:i*10+10]=a;m[:,j*10:j*10+10]=-b;rows.extend(m);links.append((i,j,axis,a,b))
 matrix=np.array(rows);_,s,vh=np.linalg.svd(matrix,full_matrices=True);rank=int(np.count_nonzero(s>1e-10));return matrix,vh[rank:].T,links,rank

C,N,LINKS,RANK=constraint_space()

def bake(z):return (N@z.T).reshape(8,10,5).transpose(0,2,1)
def project(c):return (N.T@c.transpose(0,2,1).reshape(80,5)).T

def loss_gradient(z,data,anchor):
 coeff=bake(z);g=np.zeros_like(coeff);loss=0.;margin=.02
 for i,(a,positive) in enumerate(data):
  q=a@coeff[i].T;d=np.zeros_like(q);np_=max(1,int(positive.sum()));nn=max(1,len(a)-int(positive.sum()));r=np.maximum(0.,margin-q[positive]);loss+=.5*np.sum(r*r)/np_;d[positive]=-r/np_
  rows=np.flatnonzero(~positive);owner=np.argmin(q[rows],axis=1);r=np.maximum(0.,margin+q[rows,owner]);loss+=.5*np.sum(r*r)/nn;d[rows,owner]=r/nn;g[i]=d.T@a
 grad=(N.T@g.transpose(0,2,1).reshape(80,5)).T;loss+=1e-5*np.mean((z-anchor)**2);grad+=2e-5*(z-anchor)/z.size
 return float(loss),grad

def self_test():
 assert RANK==55 and len(LINKS)==12 and N.shape==(80,25)
 rng=np.random.default_rng(86);z=rng.normal(size=(5,25));c=bake(z);residual=max(float(np.max(np.abs(c[i]@a.T-c[j]@b.T))) for i,j,_,a,b in LINKS);assert residual<1e-12
 a=features(rng.uniform(-1,1,(100,3)));data=[(a,rng.random(100)>.5) for _ in KEYS];_,g=loss_gradient(z,data,z);idx=np.unravel_index(np.argmax(np.abs(g)),g.shape);eps=1e-6;p=z.copy();q=z.copy();p[idx]+=eps;q[idx]-=eps
 numeric=(loss_gradient(p,data,z)[0]-loss_gradient(q,data,z)[0])/(2*eps);assert abs(numeric-g[idx])<1e-6
 return dict(internal_faces=12,constraint_rank_per_branch=55,free_parameters=125,exported_coefficients=400,max_random_face_residual=residual,gradient_check=True)

def branch_initializations(original,rng):
 # Positive gauges do not change native occupancy. Labels are permuted once per
 # cube, so the same labels are used on all incident faces, including cycles.
 c=original/np.maximum(np.linalg.norm(original,axis=2),1e-12)[:,:,None];perms=np.array(list(itertools.permutations(range(5))));results=[]
 for seed in range(3):
  work=c.copy()
  if seed:
   for i in range(1,8):work[i]=work[i,rng.permutation(5)]
  for _ in range(8):
   changed=False
   for i in range(1,8):
    costs=np.zeros(len(perms))
    for a,b,axis,ta,tb in LINKS:
     if a==i:
      own=work[i]@ta.T;other=work[b]@tb.T
     elif b==i:
      own=work[i]@tb.T;other=work[a]@ta.T
     else:continue
     costs+=np.sum((own[perms]-other[None,:,:])**2,axis=(1,2))
    best=int(np.argmin(costs));changed|=best!=0;work[i]=work[i,perms[best]]
   if not changed:break
  results.append(project(work))
 return results

def main(args):
 tests=self_test()
 if args.self_test:print(tests);return
 out=Path(args.output);out.mkdir(parents=True,exist_ok=False);start=time.monotonic()
 def status(phase,**kw):
  d=dict(phase=phase,elapsed_seconds=time.monotonic()-start,**kw);write_json(out/'status.json',d)
  with (out/'run.log').open('a') as f:f.write(json.dumps(d)+'\n')
  print(json.dumps(d),flush=True)
 source=Path(args.source);model=json.loads((source/'terrain.json').read_text());all_coeff=np.array(model['coefficients']);base=np.array([0,0,2]);global_keys=[tuple(int(v) for v in base+key) for key in KEYS];original=np.array([all_coeff[k] for k in global_keys]);edge=128;voxel=.0625
 if model['dimensions']!=[512]*3 or model['patch_edge_m']!=8. or model['voxel_m']!=voxel or original.shape!=(8,5,10):raise ValueError('Requires the original 512-cubed fixture and five quadrics/patch')
 density=np.memmap(source/'density.f32',dtype='<f4',mode='r',shape=(512,512,512));blocks=[density[y*edge:(y+1)*edge,z*edge:(z+1)*edge,x*edge:(x+1)*edge] for y,z,x in global_keys];rng=np.random.default_rng(2208);train=[];hold=[]
 status('preparing',patches_yzx=global_keys,construction=tests)
 for block in blocks:
  p,y=sample_patch(block,3072,rng);train.append((features(p),y>0));ids=rng.integers(0,edge,(16384,3));hold.append((features((ids[:,[2,0,1]]+.5)/edge*2-1),block[ids[:,0],ids[:,1],ids[:,2]]>0))
 # Authoritative Rust density on every shared face, independent of adjacent voxels.
 u,v=np.meshgrid((np.arange(edge)+.5)/edge*2-1,(np.arange(edge)+.5)/edge*2-1);face_cache={};world=[]
 for axis in range(3):
  tang=[a for a in range(3) if a!=axis];p=np.empty((edge*edge,3));p[:,axis]=1;p[:,tang[0]]=u.ravel();p[:,tang[1]]=v.ravel();q=p.copy();q[:,axis]=-1;face_cache[axis]=(p,q,features(p),features(q))
 for i,j,axis,_,_ in LINKS:
  p=face_cache[axis][0];key=np.array(global_keys[i])[[2,0,1]];world.append((p+1)*4+(np.array(model['origin_cells'])+key*128)*voxel)
 write_json(out/'face-points.json',np.vstack(world).tolist());write_json(out/'recipe.json',model['recipe']);subprocess.run([args.exporter,str(out/'recipe.json'),str(out/'face-points.json'),str(out/'face-density.f64')],check=True)
 face_targets=np.fromfile(out/'face-density.f64',dtype='<f8').reshape(12,edge*edge)>0
 def assess(coeff,full=False):
  errors=[float(np.mean((predict(a.astype(coeff.dtype,copy=False),coeff[i])>0)!=labels)) for i,(a,labels) in enumerate(hold)];faces=[];accuracy=[];stride=1 if full else 32
  for k,(i,j,axis,_,_) in enumerate(LINKS):
   _,_,a,b=face_cache[axis];left=predict(a[::stride].astype(coeff.dtype,copy=False),coeff[i])>0;right=predict(b[::stride].astype(coeff.dtype,copy=False),coeff[j])>0;target=face_targets[k,::stride];faces.append(int(np.count_nonzero(left!=right)));accuracy.append(float((np.mean(left!=target)+np.mean(right!=target))/2))
  return dict(holdout_error_fraction=errors,internal_face_disagreements=faces,mean_face_target_error_fraction=float(np.mean(accuracy)))
 before=assess(original,True);initials=branch_initializations(original,rng);write_json(out/'run_config.json',dict(source=args.source,seconds=args.seconds,steps=args.steps,seed=2208,patches_yzx=global_keys,construction=tests));write_json(out/'before.json',before)
 hist=[];training_start=time.monotonic();best_global_score=float('inf')
 for candidate,initial in enumerate(initials):
  z=initial.copy();anchor=z.copy();m=np.zeros_like(z);v=m.copy();best=z.copy();best_score=float('inf');stale=0;candidate_start=time.monotonic();status('training',candidate=candidate+1,candidates=3,step=0,before=before)
  for step in range(1,args.steps+1):
   if step%500==0:
    coeff=bake(z)
    for i,block in enumerate(blocks):
     ids=rng.integers(0,edge,(8192,3));a=features((ids[:,[2,0,1]]+.5)/edge*2-1);labels=block[ids[:,0],ids[:,1],ids[:,2]]>0;bad=np.flatnonzero((predict(a,coeff[i])>0)!=labels)[:256];old_a,old_y=train[i]
     if len(bad) and len(old_a)<32768:train[i]=(np.vstack((old_a,a[bad])),np.r_[old_y,labels[bad]])
   loss,grad=loss_gradient(z,train,anchor);m=.9*m+.1*grad;v=.999*v+.001*grad*grad;z-=.001*(m/(1-.9**step))/(np.sqrt(v/(1-.999**step))+1e-8)
   if step%100==0 or step==args.steps:
    metrics=assess(bake(z));score=float(np.mean(metrics['holdout_error_fraction']))+.1*metrics['mean_face_target_error_fraction']
    if score<best_score-1e-7:best=z.copy();best_score=score;stale=0
    else:stale+=1
    if score<best_global_score:
     best_global_score=score;cc=all_coeff.copy()
     for i,key in enumerate(global_keys):cc[key]=bake(z)[i]
     write_json(out/'terrain-checkpoint.json',dict(model,coefficients=cc.tolist()))
    status('training',candidate=candidate+1,candidates=3,step=step,loss=loss,best_score=best_score,current=metrics,stale_checks=stale)
    if (stale>=20 and step>=2500) or time.monotonic()-candidate_start>=args.seconds/3 or time.monotonic()-training_start>=args.seconds:break
  hist.append(dict(candidate=candidate+1,steps=step,score=best_score,parameters=best.tolist()))
  if time.monotonic()-training_start>=args.seconds:break
 def native_metrics(coeff):
  result=[]
  for i,block in enumerate(blocks):
   errors=false_air=false_solid=band_errors=band_cells=0
   for first in range(0,block.size,65536):
    ids=np.arange(first,min(first+65536,block.size));yy,zz,xx=np.unravel_index(ids,block.shape);a=features((np.column_stack((xx,yy,zz))+.5)/edge*2-1);target=block[yy,zz,xx];pred=predict(a.astype(coeff.dtype,copy=False),coeff[i])>0;wrong=pred!=(target>0);band=np.abs(target)<1.
    errors+=int(wrong.sum());false_air+=int(np.count_nonzero((target>0)&~pred));false_solid+=int(np.count_nonzero((target<=0)&pred));band_errors+=int(np.count_nonzero(wrong&band));band_cells+=int(band.sum())
   result.append(dict(patch_yzx=global_keys[i],native_cells=block.size,errors=errors,false_air=false_air,false_solid=false_solid,surface_band_cells=band_cells,surface_band_errors=band_errors))
  return result
 status('validating_native_voxels');native_before=native_metrics(original);finalists=[np.array(h['parameters']) for h in hist]+initials;scored=[]
 for index,z in enumerate(finalists):
  metrics=native_metrics(bake(z));scored.append((sum(m['errors'] for m in metrics),z,metrics));status('validating_native_voxels',completed=index+1,total=len(finalists))
 scored.sort(key=lambda r:r[0]);_,best,native_after=scored[0];coeff=bake(best);native_after32=native_metrics(coeff.astype(np.float32));full=all_coeff.copy()
 for i,key in enumerate(global_keys):full[key]=coeff[i]
 write_json(out/'terrain.json',dict(model,coefficients=full.tolist()));cropped=dict(model,coefficients=full[0:2,0:2,2:4].tolist(),patches_per_axis=[2]*3,dimensions=[256]*3,origin_cells=[model['origin_cells'][0]+256,model['origin_cells'][1],model['origin_cells'][2]])
 write_json(out/'terrain-cropped.json',cropped);np.save(out/'nullspace-parameters.npy',best);np.save(out/'constraint-nullspace.npy',N)
 # Outer-face changes remain a separate diagnostic; only internal faces are constrained.
 incident=[]
 for key in np.ndindex(*all_coeff.shape[:3]):
  for axis,storage in [(0,2),(1,0),(2,1)]:
   other=list(key);other[storage]+=1;other=tuple(other)
   if other[storage]>=4 or (key not in global_keys and other not in global_keys):continue
   _,_,a,b=face_cache[axis];counts=[int(np.count_nonzero((predict(a,cc[key])>0)!=(predict(b,cc[other])>0))) for cc in (all_coeff,full)];incident.append(dict(a_yzx=key,b_yzx=other,axis_xyz=axis,internal=key in global_keys and other in global_keys,before=counts[0],after=counts[1]))
 for name,cc in [('before',original),('after',coeff)]:
  canvas=Image.new('RGB',(768,12*128))
  for row,(i,j,axis,_,_) in enumerate(LINKS):
   _,_,a,b=face_cache[axis];left=predict(a,cc[i])>0;right=predict(b,cc[j])>0;target=face_targets[row]
   for col,mask in enumerate((left,right,left!=right,left!=target)):
    rgb=np.where(mask[:,None],np.array([155,155,160]),np.array([35,48,63])).astype('uint8');canvas.paste(Image.fromarray(rgb.reshape(edge,edge,3)).resize((192,128)),(col*192,row*128))
  canvas.save(out/f'faces-{name}.png')
 residual=max(float(np.max(np.abs(coeff[i]@a.T-coeff[j]@b.T))) for i,j,_,a,b in LINKS)
 metrics=dict(before=before,after=assess(coeff,True),after_float32=assess(coeff.astype(np.float32),True),native_before=native_before,native_after=native_after,native_after_float32=native_after32,face_coefficient_max_residual=residual,construction=tests,gpu_coefficient_bytes_cropped=1920,incident_faces=incident,elapsed_seconds=time.monotonic()-start,note='All 12 internal faces constrained with consistent labels. Outer faces monitored only. Nonconvex occupancy fit; no default replacement.')
 write_json(out/'metrics.json',metrics);write_json(out/'candidate-training.json',hist);status('complete',metrics=metrics)

if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',default='output/fit-occupancy');p.add_argument('--output',default='output/fit-hard-boundary-group');p.add_argument('--seconds',type=float,default=180.);p.add_argument('--steps',type=int,default=20000);p.add_argument('--exporter',default='target/release/fbm_points');p.add_argument('--self-test',action='store_true');main(p.parse_args())
