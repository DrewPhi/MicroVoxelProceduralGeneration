"""Bounded local fitting ablation. Rust supplies FBM; no world model is replaced.
Surface samples + calibrated gradients/off-surface distances versus old hinge.
All previews are diagnostic 25 cm occupancy; validation samples native centers.
"""
import os
for key in ('OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS'):
    os.environ[key]='1'
import argparse,itertools,json,sys,time,subprocess
from pathlib import Path
import numpy as np
import scipy.linalg
from scipy.spatial import cKDTree
import torch
from model import ContinuousQuadratics
from train import balanced_loss
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from fit import features,iso
from quadratic_spline import TETS,piece_ids
from PIL import Image,ImageDraw

EDGE=8.;COUNTS=(2,2,2);VOXEL=.0625

def write(path,obj):
    path.write_text(json.dumps(obj,indent=2,allow_nan=False)+'\n')

def polynomial_design(p,gradients=True):
    cell=np.minimum(np.floor(p).astype(int),1).clip(0,1);u=2*(p-cell)-1
    pid=(cell[:,1]*2+cell[:,2])*2+cell[:,0];a=features(u)
    if not gradients:return pid,a,None
    x,y,z=u.T;zeros=np.zeros(len(p));ones=np.ones(len(p))
    d=2*np.array([np.column_stack((zeros,ones,zeros,zeros,2*x,zeros,zeros,y,z,zeros)),
                  np.column_stack((zeros,zeros,ones,zeros,zeros,2*y,zeros,x,zeros,z)),
                  np.column_stack((zeros,zeros,zeros,ones,zeros,zeros,2*z,zeros,x,y))]).transpose(1,0,2)
    return pid,a,d

def cube_basis(p,gradients=True):
    pid,a,d=polynomial_design(p,gradients);rows=np.arange(len(p));x=np.zeros((len(p),80));g=np.zeros((len(p),3,80)) if gradients else None
    for k in range(10):
        x[rows,pid*10+k]=a[:,k]
        if gradients:
            for axis in range(3):g[rows,axis,pid*10+k]=d[:,axis,k]
    return x,g

def shared_basis():
    m=ContinuousQuadratics(COUNTS,1,dtype=torch.float64)
    cells=torch.tensor([(x,y,z) for y in range(2) for z in range(2) for x in range(2)])
    columns=[]
    with torch.no_grad():
        for p in m.parameters():
            for index in np.ndindex(*p.shape[:-1]):
                p[index+(0,)]=1.;columns.append(m.coefficients(cells).numpy().ravel());p[index+(0,)]=0.
    mapping=np.column_stack(columns);u,s,_=np.linalg.svd(mapping,full_matrices=False)
    rank=int((s>1e-10).sum());assert rank==25
    return u[:,:rank]

def tetra_basis(p,gradients=True):
    cell=np.minimum(np.floor(p).astype(int),1).clip(0,1);t=p-cell
    order=np.argsort(-t,axis=1,kind='stable');r=np.arange(len(p));a,b,c=order.T
    lam=np.column_stack((1-t[r,a],t[r,a]-t[r,b],t[r,b]-t[r,c],t[r,c]))
    gl=np.zeros((len(p),4,3));gl[r,0,a]=-1;gl[r,1,a]=1;gl[r,1,b]=-1;gl[r,2,b]=1;gl[r,2,c]=-1;gl[r,3,c]=1
    vertices=np.repeat((cell*2)[:,None,:],4,axis=1)
    vertices[r,1,a]+=2;vertices[r,2,a]+=2;vertices[r,2,b]+=2;vertices[:,3,:]+=2
    pairs=list(itertools.combinations(range(4),2))
    nodes=np.concatenate((vertices,np.stack([(vertices[:,i]+vertices[:,j])//2 for i,j in pairs],axis=1)),axis=1)
    ids=(nodes[:,:,1]*5+nodes[:,:,2])*5+nodes[:,:,0]
    weights=np.column_stack((lam*(2*lam-1),*[4*lam[:,i]*lam[:,j] for i,j in pairs]))
    x=np.zeros((len(p),125));x[r[:,None],ids]=weights
    if not gradients:return x,None
    dw=np.concatenate(((4*lam-1)[:,:,None]*gl,np.stack([4*(lam[:,i,None]*gl[:,j]+lam[:,j,None]*gl[:,i]) for i,j in pairs],axis=1)),axis=1)
    g=np.zeros((len(p),3,125))
    for axis in range(3):g[r[:,None],axis,ids]=dw[:,:,axis]
    return x,g

def linear_solve(x,y,weights,ridge=1e-6):
    w=np.sqrt(weights);a=x*w[:,None];b=y*w
    return scipy.linalg.solve(a.T@a+np.eye(a.shape[1])*ridge,a.T@b,assume_a='pos')

def query(points,recipe,out,tag):
    write(out/f'{tag}-points.json',np.asarray(points).tolist())
    subprocess.run(['target/release/fbm_points',str(recipe),str(out/f'{tag}-points.json'),str(out/f'{tag}.f64')],check=True)
    return np.fromfile(out/f'{tag}.f64',dtype='<f8')

def cluster(points,k,rng):
    centers=points[rng.choice(len(points),k,replace=False)].copy()
    for _ in range(12):
        owner=((points[:,None]-centers)**2).sum(2).argmin(1)
        for j in range(k):
            if np.any(owner==j):centers[j]=points[owner==j].mean(0)
    return owner

def metric(pred,target,caves):
    bad=pred!=target;removed=caves&~target
    return dict(errors=int(bad.sum()),samples=len(bad),error_fraction=float(bad.mean()),false_air=int((target&~pred).sum()),false_solid=int((~target&pred).sum()),cave_removed_points=int(removed.sum()),cave_errors=int((bad&removed).sum()))

def self_test():
    rng=np.random.default_rng(44);p=rng.uniform(.05,1.95,(128,3));q=rng.normal(size=10)
    # Global quadratics are reproduced exactly by the tetrahedral P2 basis.
    yy,zz,xx=np.meshgrid(np.arange(5)/2,np.arange(5)/2,np.arange(5)/2,indexing='ij')
    nodes=features(np.column_stack((xx.ravel(),yy.ravel(),zz.ravel())))@q
    z,g=tetra_basis(p);error=float(np.max(np.abs(z@nodes-features(p)@q)))
    assert error<1e-12
    eps=1e-6;worst=0.
    arbitrary=rng.normal(size=125)
    for axis in range(3):
        delta=np.eye(3)[axis]*eps
        finite=(tetra_basis(p+delta,False)[0]@arbitrary-tetra_basis(p-delta,False)[0]@arbitrary)/(2*eps)
        worst=max(worst,float(np.max(np.abs(finite-g[:,axis]@arbitrary))))
    assert worst<1e-7
    return dict(p2_quadratic_reproduction_max_error=error,p2_derivative_max_error=worst)

def main(a):
    tests=self_test()
    started=time.monotonic();out=Path(a.output);out.mkdir(parents=True,exist_ok=False);rng=np.random.default_rng(7123);torch.set_num_threads(2)
    def status(phase,**kw):
        d=dict(phase=phase,elapsed_seconds=time.monotonic()-started,**kw);write(out/'status.json',d);print(json.dumps(d),flush=True)
    world=json.loads(Path('output/world-1km/world.json').read_text());recipe=out/'recipe.json';write(recipe,dict(terrain=world['terrain']))
    no=out/'no-caves.json';nc=dict(world['terrain']);nc['cave_width']=0.;write(no,dict(terrain=nc))
    arrays=np.load('output/world-1km-initialized/shared-parameters.npz');base=arrays['xz'][:,:,0]*8+168
    candidates=[];probes=[];t=(np.arange(10)+.5)/10*2
    grid=np.array(list(itertools.product(t,t,t)))
    for sx,sz in itertools.product([58,62,66],[58,62,66]):
        for below in [1,2,4,8]:
            sy=int(np.floor((base[sx+1,sz+1]-168)/8))-below;sy=int(np.clip(sy,0,51))
            origin=np.array(world['bounds_m']['min'])+np.array([sx,sy,sz])*8
            candidates.append((sx,sy,sz,origin));probes.append(origin+grid*EDGE)
    values=query(np.vstack(probes),recipe,out,'search').reshape(len(candidates),-1)
    uncarved=query(np.vstack(probes),no,out,'search-uncarved').reshape(values.shape)
    scores=[]
    for f,u in zip(values,uncarved):
        solid=(f>0).mean();cave=((u>0)&(f<=0)).mean();air=(u<=0).mean()
        scores.append(cave-abs(solid-.5)*.2+min(air,.15))
    index=int(np.argmax(scores));sx,sy,sz,origin=candidates[index]
    status('selected_crop',origin_m=origin.tolist(),extent_m=16.,cave_fraction=float(((uncarved[index]>0)&(values[index]<=0)).mean()),solid_fraction=float((values[index]>0).mean()))
    # Surface crossings on a 0.5m lattice, then 8 authoritative bisections.
    coords=np.linspace(0,2,33);yy,zz,xx=np.meshgrid(coords,coords,coords,indexing='ij');cloud=np.column_stack((xx.ravel(),yy.ravel(),zz.ravel()))
    field=query(origin+cloud*EDGE,recipe,out,'surface-grid').reshape(33,33,33)
    starts=[];ends=[];start_values=[]
    for axis in range(3):
        low=[slice(None)]*3;high=low.copy();low[axis]=slice(0,-1);high[axis]=slice(1,None)
        indices=np.argwhere((field[tuple(low)]>0)!=(field[tuple(high)]>0));other=indices.copy();other[:,axis]+=1
        starts.append(indices[:,[2,0,1]]/16);ends.append(other[:,[2,0,1]]/16);start_values.append(field[tuple(indices.T)])
    lo=np.vstack(starts);hi=np.vstack(ends);positive=np.concatenate(start_values)>0
    if len(lo)>3500:
        take=rng.choice(len(lo),3500,replace=False);lo=lo[take];hi=hi[take];positive=positive[take]
    for i in range(8):
        middle=(lo+hi)/2;sign=query(origin+middle*EDGE,recipe,out,f'root-{i}')>0;same=sign==positive
        lo[same]=middle[same];hi[~same]=middle[~same]
    surface=(lo+hi)/2;h=.002
    points=np.vstack([surface+np.eye(3)[axis]*direction*h for axis in range(3) for direction in [-1,1]])
    v=query(origin+points*EDGE,recipe,out,'normal-samples').reshape(3,2,len(surface))
    normal=(v[:,1]-v[:,0]).T;length=np.linalg.norm(normal,axis=1);keep=length>1e-9
    surface=surface[keep];normal=normal[keep]/length[keep,None]
    assert len(surface)>50,'Insufficient surface data'
    # Current model scale is distance/8m, so gradients in patch coordinates have unit magnitude.
    ids=rng.integers(0,256,(8192,3));train=(ids+.5)/128
    td=query(origin+train*EDGE,recipe,out,'train');tree=cKDTree(surface)
    distance=np.minimum(tree.query(train)[0],.25);target=np.where(td>0,distance,-distance)
    val=(rng.integers(0,256,(65536,3))+.5)/128
    vd=query(origin+val*EDGE,recipe,out,'validation');vu=query(origin+val*EDGE,no,out,'validation-uncarved');truth=vd>0
    near=np.abs(vd)<.5
    # Same height initialization for both optimizers and both cube representations.
    initial=ContinuousQuadratics(COUNTS,5,dtype=torch.float64)
    initial.initialize_surface(torch.from_numpy(base[sx:sx+3,sz:sz+3].astype(float)),origin[1],EDGE,42)
    cells=torch.tensor([(x,y,z) for y in range(2) for z in range(2) for x in range(2)])
    with torch.no_grad():coeff0=initial.coefficients(cells).numpy().transpose(0,2,1).reshape(80,5)
    n=shared_basis();models={'height_initialization':dict(kind='cube',coeff=coeff0)}
    vx,vg=cube_basis(val,False);tx,tg=cube_basis(train,False);sp,sg=cube_basis(surface)
    # Surface initialization: uncarved top branch plus spatial cave segments.
    source_at_surface=query(origin+surface*EDGE,no,out,'surface-uncarved')
    ground=np.abs(source_at_surface)<.08
    owners=np.zeros(len(surface),int)
    if (~ground).sum()>=40:owners[~ground]=1+cluster(surface[~ground],4,rng)
    else:owners=cluster(np.column_stack((surface,.3*normal)),5,rng)
    durations={};steps={};selection={}
    status('training',surface_points=len(surface),training_points=len(train),validation_points=len(val),training_seconds_cap=a.seconds)
    # Known synthetic teacher: exact quadratic within the shared span, one linear solve.
    teacher=rng.normal(size=(25,1))*.1;sample_z=tx@n
    recovered=np.linalg.lstsq(sample_z,sample_z@teacher,rcond=None)[0]
    assert np.max(np.abs(sample_z@(recovered-teacher)))<1e-9
    training_start=time.monotonic();deadline=training_start+a.seconds
    # Baseline: unchanged occupancy hinge and height initialization, CPU Adam.
    m=ContinuousQuadratics(COUNTS,5,dtype=torch.float64);m.load_state_dict(initial.state_dict());opt=torch.optim.Adam(m.parameters(),lr=.003)
    pt=torch.from_numpy(train);den=torch.from_numpy(td)
    best=coeff0.copy();best_error=float((((vx@best).min(1)>0)!=truth).mean())
    stop=min(deadline,time.monotonic()+a.seconds/4);iteration=0;mark=time.monotonic()
    while time.monotonic()<stop and iteration<10000:
        iteration+=1;opt.zero_grad();loss=balanced_loss(m(pt),den);loss.backward();opt.step()
        if iteration%25==0:
            with torch.no_grad():cc=m.coefficients(cells).numpy().transpose(0,2,1).reshape(80,5)
            err=float((((vx@cc).min(1)>0)!=truth).mean())
            if err<best_error:best_error=err;best=cc.copy()
    models['shared_original_hinge']=dict(kind='cube',coeff=best);durations['shared_original_hinge']=time.monotonic()-mark;steps['shared_original_hinge']=iteration
    status('fit_complete',method='shared_original_hinge',steps=iteration,error=best_error)
    # Alternating active-branch assignments; each frozen assignment is a small ridge least-squares solve.
    for name,mapping in [('shared_hermite',n),('independent_hermite',np.eye(80))]:
        mark=time.monotonic();stop=min(deadline,mark+a.seconds/4);z=tx@mapping;zs=sp@mapping;gs=np.einsum('nai,ij->naj',sg,mapping)
        current=mapping.T@coeff0;best=current.copy();best_error=float((((vx@(mapping@best)).min(1)>0)!=truth).mean());history=[]
        for iteration in range(24):
            if time.monotonic()>stop:break
            owner=owners.copy() if iteration==0 else np.argmin(zs@current,axis=1)
            active=np.argmin(z@current,axis=1);candidate=current.copy()
            for j in range(5):
                mask=owner==j;owned=active==j
                if mask.sum()<4:continue
                # Keep every branch nonnegative in observed solid, even if it is not the minimum branch.
                positive=(td>0)&((z@current[:,j])<.01)
                matrices=[zs[mask],gs[mask].reshape(-1,z.shape[1]),z[owned],z[positive]]
                targets=[np.zeros(mask.sum()),normal[mask].reshape(-1),target[owned],np.full(positive.sum(),.01)]
                weights=np.r_[np.full(mask.sum(),1/max(mask.sum(),1)),np.full(mask.sum()*3,.08/max(mask.sum(),1)),np.full(owned.sum(),.25/max(owned.sum(),1)),np.full(positive.sum(),.5/max(positive.sum(),1))]
                candidate[:,j]=linear_solve(np.vstack(matrices),np.concatenate(targets),weights)
            # Evaluate candidates by actual sampled voxel error, retain best; no claim of global optimum.
            for alpha in [1.,.5,.25]:
                trial=current+alpha*(candidate-current);cc=mapping@trial
                err=float((((vx@cc).min(1)>0)!=truth).mean())
                if err<best_error:best_error=err;best=trial.copy()
            current=candidate;history.append(best_error)
        models[name]=dict(kind='cube',coeff=mapping@best);durations[name]=time.monotonic()-mark;steps[name]=iteration+1;selection[name]=history
        status('fit_complete',method=name,steps=steps[name],error=best_error)
    # Continuous local P2 field: 125 shared vertex/edge samples over 48 tetrahedra.
    mark=time.monotonic();zp,gp=tetra_basis(surface);zt,_=tetra_basis(train,False)
    x=np.vstack((zp,gp.reshape(-1,125),zt));y=np.r_[np.zeros(len(surface)),normal.reshape(-1),target]
    weights=np.r_[np.full(len(surface),1/len(surface)),np.full(len(surface)*3,.08/len(surface)),np.full(len(train),.25/len(train))]
    nodes=linear_solve(x,y,weights)
    models['local_p2_hermite']=dict(kind='tetra',nodes=nodes);durations['local_p2_hermite']=time.monotonic()-mark;steps['local_p2_hermite']=1
    train_seconds=time.monotonic()-training_start
    def evaluate(model,p):
        if model['kind']=='tetra':return tetra_basis(p,False)[0]@model['nodes']
        pid,b,_=polynomial_design(p,False);c=model['coeff'].reshape(8,10,5);return np.einsum('ni,nij->nj',b,c[pid]).min(1)
    metadata=dict(version=1,representation='min-of-implicit-quadrics',solid='min(Q)>0',feature_order=['1','x','y','z','xx','yy','zz','xy','xz','yz'],patch_order='y,z,x',patches_per_axis=[2]*3,quadratics_per_patch=5,patch_edge_m=8.,origin_cells=np.rint(origin/VOXEL).astype(int).tolist(),voxel_m=VOXEL,dimensions=[256]*3,normalization='local = 2*(world_m-patch_min_m)/patch_edge_m - 1')
    metrics={}
    for name,model in models.items():
        folder=out/name;folder.mkdir()
        f=np.concatenate([evaluate(model,p) for p in np.array_split(val,8)]);mtr=metric(f>0,truth,vu>0)
        mtr['density_band_error_fraction']=float(((f[near]>0)!=truth[near]).mean()) if near.any() else None
        mtr['train_seconds']=durations.get(name,0.);mtr['iterations']=steps.get(name,0)
        if model['kind']=='cube':
            coeff=model['coeff'].reshape(2,2,2,10,5).transpose(0,1,2,4,3);header=dict(metadata,coefficients=coeff.tolist());write(folder/'terrain.json',header)
            mtr['gpu_coefficient_bytes']=8*5*48;mtr['stored_parameter_count']=125 if name=='shared_hermite' else (165 if name!='independent_hermite' else 400)
        else:
            # The renderer's existing 24-tet partition refines these 6 Kuhn tetrahedra.
            # Verify containment by weak coordinate order, then exactly rebake quadratics.
            coefficients=[]
            for cell in cells.numpy():
                pieces=[]
                for vertices in TETS:
                    unit=vertices/4.;order=np.argsort(-unit.mean(0))
                    assert np.all(unit[:,order[0]]>=unit[:,order[1]]-1e-12) and np.all(unit[:,order[1]]>=unit[:,order[2]]-1e-12)
                    points=np.vstack((unit,np.array([(unit[i]+unit[j])/2 for i,j in itertools.combinations(range(4),2)])))
                    vals=evaluate(model,cell+points);q=np.linalg.solve(features(points*2-1),vals);pieces.append(q)
                coefficients.append(pieces)
            coeff=np.array(coefficients).reshape(2,2,2,24,10)
            write(folder/'terrain.json',dict(metadata,representation='quadratic-super-spline-24',solid='Q>0',quadratics_per_patch=24,coefficients=coeff.tolist()))
            write(folder/'compact-p2.json',dict(counts=[2]*3,nodes_yzx=model['nodes'].tolist(),construction='C0 P2 Lagrange, six Kuhn tetrahedra per cube'))
            mtr['gpu_coefficient_bytes']=8*24*48;mtr['stored_parameter_count']=125
        # Exact shared-face equality is assessed independently of renderer branch lookup.
        jumps=[];disagreements=0
        for axis in range(3):
            for tang in itertools.product(range(2),repeat=2):
                cell=np.zeros(3,int);other_axes=[k for k in range(3) if k!=axis];cell[other_axes]=tang
                local=rng.uniform(-1,1,(512,3));local[:,axis]=1;right=local.copy();right[:,axis]=-1
                left_id=(cell[1]*2+cell[2])*2+cell[0];next_cell=cell.copy();next_cell[axis]=1;right_id=(next_cell[1]*2+next_cell[2])*2+next_cell[0]
                if model['kind']=='cube':
                    c=model['coeff'].reshape(8,10,5);fl=(features(local)@c[left_id]).min(1);fr=(features(right)@c[right_id]).min(1)
                else:
                    cl=coeff.reshape(8,24,10)[left_id];cr=coeff.reshape(8,24,10)[right_id]
                    fl=np.einsum('ni,ni->n',features(local),cl[piece_ids(local)]);fr=np.einsum('ni,ni->n',features(right),cr[piece_ids(right)])
                jumps.extend(np.abs(fl-fr));disagreements+=int(((fl>0)!=(fr>0)).sum())
        mtr['max_shared_face_field_jump']=float(np.max(jumps));mtr['shared_face_sign_disagreements']=disagreements;metrics[name]=mtr
    # This set is generated after fitting and was never used to select iterations.
    test_rng=np.random.default_rng(11937)
    test=(test_rng.integers(0,256,(65536,3))+.5)/128
    test_density=query(origin+test*EDGE,recipe,out,'test')
    test_uncarved=query(origin+test*EDGE,no,out,'test-uncarved')
    for name,model in models.items():
        test_values=np.concatenate([evaluate(model,p) for p in np.array_split(test,8)])
        metrics[name]['untouched_test']=metric(test_values>0,test_density>0,test_uncarved>0)
    status('rendering_previews',training_seconds=train_seconds)
    coords=(np.arange(64)+.5)/32;yy,zz,xx=np.meshgrid(coords,coords,coords,indexing='ij');preview=np.column_stack((xx.ravel(),yy.ravel(),zz.ravel()))
    target_preview=query(origin+preview*EDGE,recipe,out,'preview')>0
    panels={};names=['target']+list(models)
    for name in names:
        bits=target_preview if name=='target' else np.concatenate([evaluate(models[name],p) for p in np.array_split(preview,16)])>0
        volume=bits.reshape(64,64,64);top=np.where(volume.any(0),63-volume[::-1].argmax(0),-1)
        packed=np.packbits(bits,bitorder='little');iso.top=np.full_like(top,1000000);im=iso(packed,64,320);im.save(out/f'{name}-isometric.png')
        slice_img=Image.fromarray(np.where(volume[:,32,:,None],np.array([140,145,150]),np.array([30,43,58])).astype('uint8')[::-1]).resize((320,320),Image.Resampling.NEAREST);slice_img.save(out/f'{name}-slice.png')
        panels[name]=(im,slice_img)
    for kind,idx,height in [('isometric',0,240),('slice',1,320)]:
        canvas=Image.new('RGB',(320*len(names),height+40),'#18212b');draw=ImageDraw.Draw(canvas)
        for i,name in enumerate(names):
            canvas.paste(panels[name][idx],(i*320,40));draw.text((i*320+8,8),name,fill='white')
        canvas.save(out/f'comparison-{kind}.png')
    report=dict(self_tests=tests,origin_m=origin.tolist(),extent_m=16.,native_voxel_m=VOXEL,surface_points=len(surface),training_seconds=train_seconds,elapsed_seconds=time.monotonic()-started,metrics=metrics,synthetic_linear_recovery_max_error=float(np.max(np.abs(sample_z@(recovered-teacher)))),note='Sampled native validation, not exhaustive. Validation set selects best iterations; a separate untouched_test set is evaluated afterward. P2 has a different representation and more baked GPU coefficients. Coarse unpainted-stone previews are 25cm, not native 6.25cm. Methods change initialization/assignments, objective and solver together; this does not isolate their individual contributions. No default world changed.')
    write(out/'metrics.json',report);write(out/'selection-history.json',selection)
    (out/'index.html').write_text('<html><body style="background:#18212b;color:white;font-family:sans-serif"><h1>Local fitting ablation</h1><p>Target, height initialization, original hinge, shared Hermite, independent Hermite, local P2 Hermite. Coarse previews: 25cm; metrics sampled at native 6.25cm centers.</p><img style="max-width:100%" src="comparison-isometric.png"><img style="max-width:100%" src="comparison-slice.png"><pre>'+json.dumps(report,indent=2)+'</pre></body></html>')
    status('complete',metrics=metrics,training_seconds=train_seconds)

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--output',default='output/local-hermite-experiment');p.add_argument('--seconds',type=float,default=100.);main(p.parse_args())
