"""Issue23: independent five-quadric pair + common FBM crossing/collar targets.
A/B only. Residual exact corrections are counted/exported, not rendered.
"""
import os
for key in ['OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS']:os.environ[key]='1'
import argparse,itertools,json,subprocess,time
from pathlib import Path
import numpy as np
from PIL import Image,ImageDraw
from fit import features,predict,iso,write_json

KEYS=[(0,0,3),(1,0,3)];EDGE=128;SIZE=.0625

def derivatives(p):
    x,y,z=p.T;n=len(p);o=np.ones(n);v=np.zeros(n)
    return np.stack((np.column_stack((v,o,v,v,2*x,v,v,y,z,v)),np.column_stack((v,v,o,v,v,2*y,v,x,v,z)),np.column_stack((v,v,v,o,v,v,2*z,v,x,y))),1)

def query(p,recipe,out,name):
    write_json(out/f'{name}-points.json',np.asarray(p).tolist())
    subprocess.run(['target/release/fbm_points',str(recipe),str(out/f'{name}-points.json'),str(out/f'{name}.f64')],check=True)
    return np.fromfile(out/f'{name}.f64',dtype='<f8')

def roots(lo,hi,sign,recipe,out,name):
    for i in range(8):
        mid=(lo+hi)*.5;pred=query(mid,recipe,out,f'{name}-{i}')>0;same=pred==sign;lo[same]=mid[same];hi[~same]=mid[~same]
    return (lo+hi)*.5

def metrics(c,blocks,collar=3):
    result=[]
    for side,block in enumerate(blocks):
        counts=dict(errors=0,interior_errors=0,collar_errors=0,outer_collar_edge_errors=0,near_density_errors=0,near_density_cells=0,false_air=0,false_solid=0)
        for first in range(0,block.size,65536):
            ids=np.arange(first,min(first+65536,block.size));y,z,x=np.unravel_index(ids,block.shape);p=(np.column_stack((x,y,z))+.5)/64-1
            target=block[y,z,x];pred=predict(features(p),c[side])>0;wrong=pred!=(target>0);strip=y>=128-collar if side==0 else y<collar
            edge=(y>=128-collar-2)&(y<128-collar) if side==0 else (y>=collar)&(y<collar+2)
            counts['errors']+=int(wrong.sum());counts['interior_errors']+=int((wrong&~strip).sum());counts['collar_errors']+=int((wrong&strip).sum());counts['outer_collar_edge_errors']+=int((wrong&edge).sum())
            near=np.abs(target)<1.;counts['near_density_errors']+=int((wrong&near).sum());counts['near_density_cells']+=int(near.sum());counts['false_air']+=int(((target>0)&~pred).sum());counts['false_solid']+=int(((target<=0)&pred).sum())
        counts['cells']=block.size;result.append(counts)
    return dict(patches=result,errors=sum(r['errors'] for r in result),cells=sum(r['cells'] for r in result),error_fraction=sum(r['errors'] for r in result)/sum(r['cells'] for r in result),interior_errors=sum(r['interior_errors'] for r in result),collar_errors=sum(r['collar_errors'] for r in result),outer_collar_edge_errors=sum(r['outer_collar_edge_errors'] for r in result))

def main(a):
    out=Path(a.output);out.mkdir(parents=True,exist_ok=False);began=time.monotonic();rng=np.random.default_rng(2304)
    def status(phase,**kw):
        d=dict(phase=phase,elapsed_seconds=time.monotonic()-began,**kw);write_json(out/'status.json',d);print(json.dumps(d),flush=True)
    model=json.loads(Path(a.source,'terrain.json').read_text());all_c=np.array(model['coefficients']);original=np.array([all_c[k] for k in KEYS]);assert original.shape==(2,5,10)
    density=np.memmap(Path(a.source,'density.f32'),dtype='<f4',mode='r',shape=(512,512,512));blocks=[density[y*128:(y+1)*128,z*128:(z+1)*128,x*128:(x+1)*128] for y,z,x in KEYS]
    origins=[(np.array(model['origin_cells'])+np.array(k)[[2,0,1]]*128)*SIZE for k in KEYS]
    recipe=out/'recipe.json';write_json(recipe,model['recipe']);before=metrics(original,blocks,a.collar)
    # Native face centers are the original1027-disagreement diagnostic grid.
    zz,xx=np.meshgrid((np.arange(128)+.5)/64-1,(np.arange(128)+.5)/64-1,indexing='ij');face=np.column_stack((xx.ravel(),np.ones(xx.size),zz.ravel()));right=face.copy();right[:,1]=-1
    fa,fb=features(face),features(right);face_world=origins[0]+(face+1)*4
    face_truth=query(face_world,recipe,out,'face-native-centers')>0
    def face_metric(c):
        left=predict(fa,c[0])>0;right_=predict(fb,c[1])>0
        return dict(disagreements=int((left!=right_).sum()),samples=len(left),left_target_errors=int((left!=face_truth).sum()),right_target_errors=int((right_!=face_truth).sum()))
    before['face']=face_metric(original);status('baseline',metrics=before)
    # Authoritative129x129shared face lattice; both fits receive the same roots/normals.
    z,x=np.meshgrid(np.linspace(0,8,129),np.linspace(0,8,129),indexing='ij');fw=origins[0]+np.column_stack((x.ravel(),np.full(x.size,8.),z.ravel()))
    field=query(fw,recipe,out,'face-lattice').reshape(129,129);lo=[];hi=[];sign=[]
    for axis in range(2):
        low=[slice(None)]*2;high=low.copy();low[axis]=slice(0,-1);high[axis]=slice(1,None)
        ids=np.argwhere((field[tuple(low)]>0)!=(field[tuple(high)]>0));other=ids.copy();other[:,axis]+=1
        indices=np.ravel_multi_index(ids.T,field.shape);next_indices=np.ravel_multi_index(other.T,field.shape)
        lo.append(fw[indices]);hi.append(fw[next_indices]);sign.append(field[tuple(ids.T)]>0)
    common=roots(np.vstack(lo),np.vstack(hi),np.concatenate(sign),recipe,out,'face-root')
    def normals(points,name):
        h=SIZE/4;offset=np.concatenate([points+np.eye(3)[axis]*direction*h for axis in range(3) for direction in [-1,1]])
        values=query(offset,recipe,out,name).reshape(3,2,len(points));g=(values[:,1]-values[:,0]).T
        return g/np.maximum(np.linalg.norm(g,axis=1,keepdims=True),1e-9)
    common_norm=normals(common,'face-normals')*.5
    samples=[]
    for side,block in enumerate(blocks):
        local=[];target=[]
        ids=rng.integers(0,128,(8192,3));local.append((ids[:,[2,0,1]]+.5)/64-1);target.append(block[ids[:,0],ids[:,1],ids[:,2]]>0)
        z,x=np.meshgrid(np.arange(128),np.arange(128),indexing='ij');strip=[]
        for y in (range(128-a.collar,128) if side==0 else range(a.collar)):
            ids=np.column_stack((np.full(x.size,y),z.ravel(),x.ravel()));strip.append(ids)
        ids=np.vstack(strip);collar_points=(ids[:,[2,0,1]]+.5)/64-1;collar_labels=block[ids[:,0],ids[:,1],ids[:,2]]>0
        # Independent interior source crossings protect existing geometry.
        starts=[];ends=[];sgn=[]
        for axis in range(3):
            low=[slice(None)]*3;high=low.copy();low[axis]=slice(0,-1);high[axis]=slice(1,None)
            hit=np.argwhere((block[tuple(low)]>0)!=(block[tuple(high)]>0))
            if len(hit)>800:hit=hit[rng.choice(len(hit),800,False)]
            nxt=hit.copy();nxt[:,axis]+=1
            starts.append(origins[side]+(hit[:,[2,0,1]]+.5)*SIZE);ends.append(origins[side]+(nxt[:,[2,0,1]]+.5)*SIZE);sgn.append(block[tuple(hit.T)]>0)
        surface=roots(np.vstack(starts),np.vstack(ends),np.concatenate(sgn),recipe,out,f'interior-root-{side}');grad=normals(surface,f'interior-normals-{side}')*.5
        ps=(surface-origins[side])/4-1;boundary=(common-origins[side])/4-1
        x0=features(local[0]);xc=features(collar_points);xs=features(ps);xb=features(boundary);ds=derivatives(ps);db=derivatives(boundary)
        coeff=original[side].copy();owner=(xs@coeff.T).argmin(1);calibration=[]
        for j in range(5):
            mask=owner==j;g=np.einsum('nai,i->na',ds[mask],coeff[j])
            norm=np.median(np.linalg.norm(g,axis=1)) if len(g) else np.linalg.norm(coeff[j,1:4]);scale=.5/max(norm,1e-5);coeff[j]*=scale;calibration.append(scale)
        samples.append(dict(x=x0,labels=target[0],xc=xc,collar_labels=collar_labels,xs=xs,ds=ds,grad=grad,xb=xb,db=db,bgrad=common_norm,initial=coeff))
    # Fixed independent tuning points, distinct from exhaustive final evaluation.
    hold=[]
    for block in blocks:
        ids=rng.integers(0,128,(32768,3));hold.append((features((ids[:,[2,0,1]]+.5)/64-1),block[ids[:,0],ids[:,1],ids[:,2]]>0,ids[:,0]))
    def rank(c):
        interior=[]
        for side,(x,truth,y) in enumerate(hold):
            wrong=(predict(x,c[side])>0)!=truth;mask=y<128-a.collar if side==0 else y>=a.collar;interior.append(float(wrong[mask].mean()))
        f=face_metric(c);return float(np.mean(interior)),(f['left_target_errors']+f['right_target_errors'])/(2*f['samples']),f['disagreements']/f['samples']
    baseline_rank=rank(original);candidates=[dict(coeff=original.copy(),weight=0.,iteration=0,rank=baseline_rank)]
    train_start=time.monotonic();deadline=train_start+a.seconds
    for strength in [.5,2.,8.,32.]:
        current=np.array([d['initial'] for d in samples]);anchor=current.copy()
        for iteration in range(20):
            if time.monotonic()>deadline:break
            for side,d in enumerate(samples):
                c=current[side];x=d['x'];xc=d['xc'];xs=d['xs'];xb=d['xb'];q=x@c.T;qc=xc@c.T
                so=(xs@c.T).argmin(1);bo=(xb@c.T).argmin(1);no=q.argmin(1);nco=qc.argmin(1)
                proposed=c.copy()
                for j in range(5):
                    sm=so==j;bm=bo==j
                    if not sm.any() and not bm.any():continue
                    # Squared hinge active sets are linear rows for this frozen assignment.
                    positive=d['labels']&(q[:,j]<.01);negative=~d['labels']&(no==j)&(q[:,j]>-.01)
                    cp=d['collar_labels']&(qc[:,j]<.01);cn=~d['collar_labels']&(nco==j)&(qc[:,j]>-.01)
                    matrices=[];targets=[];weights=[]
                    def add(mat,y,weight):
                        if len(mat):matrices.append(mat);targets.append(np.asarray(y));weights.append(np.full(len(mat),weight/max(len(mat),1)))
                    add(xs[sm],np.zeros(sm.sum()),1.)
                    add(d['ds'][sm].reshape(-1,10),d['grad'][sm].ravel(),.12)
                    add(xb[bm],np.zeros(bm.sum()),strength)
                    add(d['db'][bm].reshape(-1,10),d['bgrad'][bm].ravel(),strength*.05)
                    add(x[positive],np.full(positive.sum(),.01),2.)
                    add(x[negative],np.full(negative.sum(),-.01),2.)
                    add(xc[cp],np.full(cp.sum(),.01),strength*2.)
                    add(xc[cn],np.full(cn.sum(),-.01),strength*2.)
                    # Preserve the old interior branch shape away from the supervised seam.
                    add(x,x@anchor[side,j],.2)
                    xx=np.vstack(matrices);yy=np.concatenate(targets);w=np.concatenate(weights)
                    normal=xx.T@(w[:,None]*xx)+np.eye(10)*1e-4;rhs=xx.T@(w*yy)+anchor[side,j]*1e-4
                    proposed[j]=np.linalg.solve(normal,rhs)
                current[side]=c+.5*(proposed-c)
            r=rank(current);candidates.append(dict(coeff=current.copy(),weight=strength,iteration=iteration+1,rank=r))
        status('candidate',weight=strength,iterations=iteration+1,rank=rank(current))
        if time.monotonic()>deadline:break
    training_elapsed=time.monotonic()-train_start
    # Small trust-region steps preserve the already accurate interior. Polynomial
    # gauges are positive rescalings, so the anchor has exactly A's occupancy.
    anchor=np.array([d['initial'] for d in samples])
    promising=sorted(candidates,key=lambda c:c['rank'][1]+.25*c['rank'][2])[:12]
    for c in promising:
        for alpha in [.1,.25,.5,.75]:
            trial=anchor+alpha*(c['coeff']-anchor)
            candidates.append(dict(coeff=trial,weight=c['weight'],iteration=c['iteration'],alpha=alpha,rank=rank(trial)))
    eligible=[c for c in candidates if c['rank'][0]<=baseline_rank[0]*1.20]
    eligible.sort(key=lambda c:c['rank'][1]+.25*c['rank'][2])
    best=candidates[0];after=before.copy();checked=[]
    for c in eligible:
        native=metrics(c['coeff'],blocks,a.collar)
        feasible=native['interior_errors']<=before['interior_errors']*1.10 and native['errors']<=before['errors']*1.05
        checked.append(dict(weight=c['weight'],iteration=c['iteration'],alpha=c.get('alpha',1.),feasible=feasible,errors=native['errors'],interior_errors=native['interior_errors']))
        if feasible:
            best=c;after=native;break
    after['face']=face_metric(best['coeff'])
    status('native_validation',before=before,after=after)
    # These are only residual correction records; no changes to ray tracing are made.
    corrections=[]
    for side,block in enumerate(blocks):
        z,x=np.meshgrid(np.arange(128),np.arange(128),indexing='ij')
        for y in (range(128-a.collar,128) if side==0 else range(a.collar)):
            p=(np.column_stack((x.ravel(),np.full(x.size,y),z.ravel()))+.5)/64-1;target=block[y].ravel()>0;bad=(predict(features(p),best['coeff'][side])>0)!=target
            index=x.ravel()+128*(z.ravel()+128*(y+side*128));corrections.extend(((index[bad].astype('uint32')<<1)|target[bad].astype('uint32')).tolist())
    np.save(out/'residual-collar-corrections.u32.npy',np.array(corrections,dtype='<u4'))
    for name,c in [('A',original),('B',best['coeff'])]:
        full=all_c.copy()
        for side,key in enumerate(KEYS):full[key]=c[side]
        write_json(out/f'{name}-full-fixture.json',dict(model,coefficients=full.tolist()))
        cropped=dict(model,coefficients=c[:,None,None,:,:].tolist(),patches_per_axis=[1,2,1],dimensions=[128,256,128],origin_cells=(np.array(model['origin_cells'])+[384,0,0]).tolist())
        write_json(out/f'{name}-pair.json',cropped)
    # Two rows: authoritative face and left/right fits, disagreement highlighted.
    canvas=Image.new('RGB',(1024,552),'#18212b');draw=ImageDraw.Draw(canvas)
    for row,(name,c) in enumerate([('A original',original),('B shared targets',best['coeff'])]):
        masks=[face_truth,predict(fa,c[0])>0,predict(fb,c[1])>0,(predict(fa,c[0])>0)!=(predict(fb,c[1])>0)]
        for col,mask in enumerate(masks):
            rgb=np.where(mask.reshape(128,128,1),np.array([170,170,175]) if col<3 else np.array([240,65,55]),np.array([28,42,57])).astype('uint8');im=Image.fromarray(rgb).resize((256,256),Image.Resampling.NEAREST)
            canvas.paste(im,(col*256,row*276+20));draw.text((col*256+5,row*276+4),name+' '+['FBM','lower','upper','disagreement'][col],fill='white')
    canvas.save(out/'comparison-faces.png')
    # Matching cutaway voxel previews; deliberately125mm, not native-resolution certification.
    n=128;images=[]
    for name,c in [('target',None),('A',original),('B',best['coeff'])]:
        volume=np.zeros((n,n,n),bool)
        for side in range(2):
            if c is None:volume[side*64:(side+1)*64,:64,:64]=blocks[side][::2,::2,::2]>0
            else:
                y,z,x=np.meshgrid((np.arange(64)*2+.5)/64-1,(np.arange(64)*2+.5)/64-1,(np.arange(64)*2+.5)/64-1,indexing='ij');p=np.column_stack((x.ravel(),y.ravel(),z.ravel()));volume[side*64:(side+1)*64,:64,:64]=(predict(features(p),c[side])>0).reshape(64,64,64)
        iso.top=np.full((n,n),n+1);im=iso(np.packbits(volume.ravel(),bitorder='little'),n,320);images.append(im);im.save(out/f'{name}-isometric.png')
    canvas=Image.new('RGB',(960,268),'#18212b');draw=ImageDraw.Draw(canvas)
    for i,(name,im) in enumerate(zip(['FBM','A original','B shared boundary targets'],images)):
        canvas.paste(im,(i*320,28));draw.text((i*320+6,8),name,fill='white')
    canvas.save(out/'comparison-isometric.png')
    # Count missed/extra sign-changing edges of the authoritative native-center face lattice.
    def crossing_errors(c):
        total=dict(missed=0,extra=0)
        for side,xx in enumerate([fa,fb]):
            pred=(predict(xx,c[side])>0).reshape(128,128);target=face_truth.reshape(128,128)
            for axis in range(2):
                lo=[slice(None)]*2;hi=lo.copy();lo[axis]=slice(0,-1);hi[axis]=slice(1,None)
                t=target[tuple(lo)]!=target[tuple(hi)];f=pred[tuple(lo)]!=pred[tuple(hi)];total['missed']+=int((t&~f).sum());total['extra']+=int((f&~t).sum())
        return total
    # All other incident faces remain unconstrained and are explicitly audited.
    incident=[]
    changed=all_c.copy()
    for side,key in enumerate(KEYS):changed[key]=best['coeff'][side]
    for key in np.ndindex(*all_c.shape[:3]):
        for axis,storage in [(0,2),(1,0),(2,1)]:
            other=list(key);other[storage]+=1;other=tuple(other)
            if other[storage]>=4 or (key not in KEYS and other not in KEYS):continue
            p=rng.uniform(-1,1,(4096,3));p[:,axis]=1.;q=p.copy();q[:,axis]=-1.
            counts=[int(((predict(features(p),cc[key])>0)!=(predict(features(q),cc[other])>0)).sum()) for cc in [all_c,changed]]
            incident.append(dict(a=key,b=other,internal=key in KEYS and other in KEYS,before=counts[0],after=counts[1]))
    result=dict(before=before,after=after,selected=dict(weight=best['weight'],iteration=best['iteration'],alpha=best.get('alpha',1.)),exhaustive_candidate_checks=checked,baseline_tuning=baseline_rank,selected_tuning=best['rank'],crossing_errors_before=crossing_errors(original),crossing_errors_after=crossing_errors(best['coeff']),collar_depth_voxels=a.collar,correction_count=len(corrections),packed_correction_payload_bytes=4*len(corrections),gpu_base_coefficient_bytes=480,incident_faces=incident,shared_crossing_points=len(common),training_seconds=training_elapsed,elapsed_seconds=time.monotonic()-began,note='A/B independent fitting; no coefficient equality or overrides applied in renderer. Native pair metrics exhaustive; face lattice is supervised. Correction payload excludes lookup/index metadata and runtime. Continuous zero-set continuity not guaranteed. Candidates ranked on tuning samples, then accepted using exhaustive native fidelity limits: <=10% relative interior-error increase and <=5% total-error increase. This is fitting to a finite fixture, not a generalization estimate. Preview downsamples native centers at125mm spacing.')
    write_json(out/'metrics.json',result);write_json(out/'candidate-history.json',[{k:v for k,v in c.items() if k!='coeff'} for c in candidates])
    (out/'index.html').write_text('<html><body style="background:#18212b;color:white;font-family:sans-serif"><h1>Independent two-patch seam targets</h1><img style="max-width:100%" src="comparison-faces.png"><img style="max-width:100%" src="comparison-isometric.png"><pre>'+json.dumps(result,indent=2)+'</pre></body></html>')
    status('complete',metrics=result)

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--source',default='output/fit-occupancy');p.add_argument('--output',default='output/seam-target-pair');p.add_argument('--seconds',type=float,default=60.);p.add_argument('--collar',type=int,default=3);main(p.parse_args())
