#!/usr/bin/env python3
"""Small warm-start continuity experiment; never replaces the baseline fit.
Shared-face hard-min value loss + native occupancy margins + fixed neighbor anchors.
Continuity is sampled, not guaranteed between samples. Density is not an SDF.
"""
import os
for k in ('OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS'):os.environ[k]='1'
import argparse,json,time
from pathlib import Path
import numpy as np
from PIL import Image
from fit import features,predict,sample_patch,write_json
from occupancy import native_check

def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--source',default='output/fit-occupancy');p.add_argument('--output',default='output/fit-boundaries')
 p.add_argument('--steps',type=int,default=20000);p.add_argument('--seconds',type=float,default=240.)
 args=p.parse_args();out=Path(args.output);out.mkdir(parents=True,exist_ok=True);start=time.monotonic()
 def status(phase,**kw):
  d=dict(phase=phase,elapsed_seconds=time.monotonic()-start,**kw);write_json(out/'status.json',d);print(json.dumps(d),flush=True);
  with (out/'run.log').open('a') as log:log.write(json.dumps(d)+'\n')
 source=Path(args.source);model=json.loads((source/'terrain.json').read_text());c=np.array(model['coefficients']);baseline=c.copy();g=model['patches_per_axis'];n=model['dimensions'][0];edge=n//g[0]
 density=np.memmap(source/'density.f32',dtype='<f4',mode='r',shape=(n,n,n));rng=np.random.default_rng(817)
 def block(key):
  y,z,x=key;return density[y*edge:(y+1)*edge,z*edge:(z+1)*edge,x*edge:(x+1)*edge]
 def faces():
  for key in np.ndindex(*c.shape[:3]):
   for axis,storage in [(0,2),(1,0),(2,1)]:
    other=list(key);other[storage]+=1
    if other[storage]<c.shape[storage]:yield key,tuple(other),axis
 def face_points(axis,count):
  uv=(np.arange(count)+.5)/count*2-1;u,v=np.meshgrid(uv,uv);a=np.empty((count*count,3));a[:,axis]=1.;remaining=[k for k in range(3) if k!=axis];a[:,remaining[0]]=u.ravel();a[:,remaining[1]]=v.ravel();b=a.copy();b[:,axis]=-1.;return a,b
 cache={a:tuple(features(p) for p in face_points(a,edge)) for a in range(3)}
 scores=[]
 for a,b,axis in faces():
  fa,fb=cache[axis];va=predict(fa,c[a]);vb=predict(fb,c[b]);scores.append((int(np.count_nonzero((va>0)!=(vb>0))),a,b,axis))
 scores.sort(reverse=True);worst=scores[0];selected=[worst[1],worst[2]];lookup={k:i for i,k in enumerate(selected)}
 write_json(out/'selection.json',dict(selected_yzx=selected,worst_face_axis_xyz=worst[3],baseline_face_disagreements=worst[0],face_samples=edge**2,all_faces=[dict(disagreements=s[0],a_yzx=s[1],b_yzx=s[2],axis_xyz=s[3]) for s in scores]))
 status('preparing',selected_yzx=selected,boundary_disagreements=worst[0])
 # Put independent occupancy fits into comparable value gauges; positive scaling
 # preserves their entire surfaces and all native labels.
 for key in np.ndindex(*c.shape[:3]):
  pts,y=sample_patch(block(key),1024,rng);a=features(pts);values=a@c[key].T;ids=np.argmin(values,axis=1);q=c[key][ids];x,yy,z=pts.T
  grad=np.column_stack((q[:,1]+2*q[:,4]*x+q[:,7]*yy+q[:,8]*z,q[:,2]+2*q[:,5]*yy+q[:,7]*x+q[:,9]*z,q[:,3]+2*q[:,6]*z+q[:,8]*x+q[:,9]*yy))
  near=np.argsort(np.abs(values.min(axis=1)))[:min(512,len(pts))];scale=max(float(np.median(np.linalg.norm(grad[near],axis=1))),.05);c[key]/=scale
 initial=c.copy();weights=np.array([c[k] for k in selected]);anchor=weights.copy();train=[];hold=[]
 for key in selected:
  pts,y=sample_patch(block(key),4096,rng);train.append((features(pts),np.where(y>0,1.,-1.)))
  ids=rng.integers(0,edge,size=(32768,3));pts=(ids[:,[2,0,1]]+.5)/edge*2-1;labels=block(key)[ids[:,0],ids[:,1],ids[:,2]]>0;hold.append((features(pts),labels))
 links=[]
 for a,b,axis in faces():
  if a not in lookup and b not in lookup:continue
  fa,fb=cache[axis];va=predict(fa,c[a]);vb=predict(fb,c[b]);importance=np.abs(va-vb)/(0.02+np.minimum(np.abs(va),np.abs(vb)))
  ids=np.unique(np.r_[rng.choice(len(fa),min(1024,len(fa)),replace=False),np.argsort(importance)[-1024:]])
  links.append((a,b,fa[ids],fb[ids],10. if a in lookup and b in lookup else 2.))
 def objective(w):
  grad=np.zeros_like(w);loss=0.
  for i,(a,sign) in enumerate(train):
   q=a@w[i].T;owner=np.argmin(q,axis=1);minimum=q[np.arange(len(a)),owner];res=np.maximum(0.,.02-sign*minimum);loss+=20.*np.mean(res**2)
   d=-40.*sign*res/len(a)
   for j in range(len(w[i])):mask=owner==j;grad[i,j]+=a[mask].T@d[mask]
  for ka,kb,a,b,strength in links:
   ia=lookup.get(ka);ib=lookup.get(kb);wa=c[ka] if ia is None else w[ia];wb=c[kb] if ib is None else w[ib]
   qa=a@wa.T;qb=b@wb.T;oa=np.argmin(qa,axis=1);ob=np.argmin(qb,axis=1);res=qa[np.arange(len(a)),oa]-qb[np.arange(len(b)),ob];loss+=strength*np.mean(res**2);d=2*strength*res/len(a)
   for index,f,owners,derivative in [(ia,a,oa,d),(ib,b,ob,-d)]:
    if index is not None:
     for j in range(len(w[index])):mask=owners==j;grad[index,j]+=f[mask].T@derivative[mask]
  loss+=.01*np.mean((w-anchor)**2);grad+=.02*(w-anchor)/w.size
  return float(loss),grad
 def measure(w):
  cc=c.copy()
  for key,i in lookup.items():cc[key]=w[i]
  fa,fb=cache[worst[3]];a=predict(fa,cc[worst[1]]);b=predict(fb,cc[worst[2]])
  errors=[float(np.mean((predict(f,cc[key])>0)!=labels)) for key,(f,labels) in zip(selected,hold)]
  return dict(shared_face_disagreements=int(np.count_nonzero((a>0)!=(b>0))),face_samples=len(a),shared_face_value_rmse=float(np.mean((a-b)**2)**.5),holdout_native_error_fraction=errors)
 before=measure(weights);write_json(out/'before.json',before)
 best=weights.copy();best_metrics=before;best_score=before['shared_face_disagreements']/before['face_samples']+.02*before['shared_face_value_rmse'];m=np.zeros_like(weights);v=m.copy();stale=0
 status('training',step=0,steps=args.steps,before=before)
 for step in range(1,args.steps+1):
  loss,grad=objective(weights);norm=np.linalg.norm(grad);grad*=min(1.,10./max(norm,1e-12));m=.9*m+.1*grad;v=.999*v+.001*grad**2
  weights-=.00001*(m/(1-.9**step))/(np.sqrt(v/(1-.999**step))+1e-8)
  if step%25==0 or step==args.steps:
   metrics=measure(weights);degradation=max(0.,sum(metrics['holdout_native_error_fraction'])-sum(before['holdout_native_error_fraction']))
   score=metrics['shared_face_disagreements']/metrics['face_samples']+8.*degradation+.02*metrics['shared_face_value_rmse']
   if score<best_score-1e-6 and degradation<=.01:best=weights.copy();best_metrics=metrics;best_score=score;stale=0
   else:stale+=1
   cc=c.copy()
   for key,i in lookup.items():cc[key]=best[i]
   checkpoint=dict(model,coefficients=cc.tolist());write_json(out/'terrain-checkpoint.json',checkpoint)
   status('training',step=step,steps=args.steps,loss=loss,best=best_metrics,stale_checks=stale)
   if (stale>=160 and step>=5000) or time.monotonic()-start>args.seconds:break
 # Exhaustive checks only at the end, on the two changed native blocks.
 status('validating',step=step);native=[]
 final=baseline.copy()
 # Keep every unselected patch byte-for-byte unchanged. Undo selected gauges.
 for key,i in lookup.items():
  scale=np.linalg.norm(baseline[key])/max(np.linalg.norm(initial[key]),1e-30);final[key]=best[i]*scale
  before_errors,_=native_check(block(key),baseline[key]);after_errors,_=native_check(block(key),final[key]);native.append(dict(patch_yzx=key,native_cells=edge**3,before_errors=before_errors,after_errors=after_errors))
 model['coefficients']=final.tolist();write_json(out/'terrain.json',model)
 fa,fb=cache[worst[3]]
 for name,cc in [('before',baseline),('after',final)]:
  a=predict(fa,cc[worst[1]])>0;b=predict(fb,cc[worst[2]])>0;images=[]
  for mask in [a,b,a!=b]:
   rgb=np.where(mask[:,None],np.array([155,155,160]),np.array([35,48,63])).astype('uint8');images.append(Image.fromarray(rgb.reshape(edge,edge,3)).resize((384,384)))
  image=Image.new('RGB',(1152,384))
  for i,im in enumerate(images):image.paste(im,(i*384,0))
  image.save(out/f'boundary-{name}.png')
 metrics=dict(before=before,after=best_metrics,native_validation=native,selected_yzx=selected,steps=step,elapsed_seconds=time.monotonic()-start,note='Sampled continuity penalty, not guaranteed C0 or C1 continuity. Face images: patch A, patch B, disagreement. Only two patches changed; inspect neighbor effects.')
 # Report all incident faces, including effects on fixed neighbors.
 metrics['incident_faces']=[]
 for a,b,axis in faces():
  if a in lookup or b in lookup:
   fa,fb=cache[axis];counts=[int(np.count_nonzero((predict(fa,cc[a])>0)!=(predict(fb,cc[b])>0))) for cc in [baseline,final]]
   metrics['incident_faces'].append(dict(a_yzx=a,b_yzx=b,axis_xyz=axis,before=counts[0],after=counts[1]))
 write_json(out/'metrics.json',metrics);status('complete',metrics=metrics)
if __name__=='__main__':main()
