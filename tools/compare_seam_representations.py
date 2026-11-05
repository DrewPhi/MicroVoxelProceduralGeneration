"""Small CPU experiment: cube-owned quadrics vs shared CSG vs conforming P2.

This writes diagnostic models, not viewer-format scenes or an active world.
Coordinates are in units of 8 metres; density > 0 means solid.
"""
import os
for variable in ('OPENBLAS_NUM_THREADS', 'OMP_NUM_THREADS', 'MKL_NUM_THREADS'):
    os.environ[variable] = '1'

import argparse
import itertools
import json
import time
from pathlib import Path

import numpy as np
import scipy.linalg
from scipy.optimize import minimize
from scipy.ndimage import label
from scipy.spatial import cKDTree
from PIL import Image, ImageDraw

from fit import features, iso, write_json
from fit_seam_targets import derivatives, query, roots, KEYS, SIZE

EXTENT = np.array([1., 2., 1.])
COUNTS = np.array([1, 2, 1])
PAIRS = list(itertools.combinations(range(4), 2))


def p2_design(points, gradients=False):
    """Face-consistent Kuhn split, with 45 globally shared P2 nodal values."""
    cells = np.minimum(np.floor(points).astype(int), COUNTS - 1).clip(0)
    t = points - cells
    order = np.argsort(-t, axis=1, kind='stable')
    r = np.arange(len(points))
    a, b, c = order.T
    lam = np.column_stack((1-t[r,a], t[r,a]-t[r,b], t[r,b]-t[r,c], t[r,c]))
    vertices = np.repeat((cells*2)[:,None,:], 4, axis=1)
    vertices[r,1,a] += 2
    vertices[r,2,a] += 2
    vertices[r,2,b] += 2
    vertices[:,3] += 2
    nodes = np.concatenate((vertices, np.stack([(vertices[:,i]+vertices[:,j])//2 for i,j in PAIRS], axis=1)), axis=1)
    indices = nodes[:,:,0] + 3*(nodes[:,:,2] + 3*nodes[:,:,1])
    weights = np.column_stack((lam*(2*lam-1), *[4*lam[:,i]*lam[:,j] for i,j in PAIRS]))
    design = np.zeros((len(points), 45))
    design[r[:,None],indices] = weights
    if not gradients:
        return design
    gl = np.zeros((len(points), 4, 3))
    gl[r,0,a]=-1; gl[r,1,a]=1; gl[r,1,b]=-1
    gl[r,2,b]=1; gl[r,2,c]=-1; gl[r,3,c]=1
    dw = np.concatenate(((4*lam-1)[:,:,None]*gl,
                         np.stack([4*(lam[:,i,None]*gl[:,j]+lam[:,j,None]*gl[:,i]) for i,j in PAIRS],axis=1)),axis=1)
    gradient = np.zeros((len(points),3,45))
    for axis in range(3):
        gradient[r[:,None],axis,indices] = dw[:,:,axis]
    return design, gradient


def tetrahedra():
    pieces=[]
    for y in range(2):
        for order in itertools.permutations(range(3)):
            v=np.tile([0.,float(y),0.],(4,1))
            for i,axis in enumerate(order):
                v[i+1: ,axis] += 1
            pieces.append(v)
    return np.array(pieces)


def bake_p2(nodes):
    coeff=[]
    for v in tetrahedra():
        p=np.vstack((v,[(v[i]+v[j])/2 for i,j in PAIRS]))
        coeff.append(np.linalg.solve(features(p),p2_design(p)@nodes))
    return np.array(coeff)


def evaluate(model, points):
    if model['kind']=='p2':
        return p2_design(points)@model['nodes']
    if model['kind']=='csg':
        q=features(points)@model['coeff'].reshape(-1,10).T
        return q.reshape(-1,2,5).min(2).max(1)
    side=(points[:,1]>=1).astype(int)
    local=points*2-1
    local[:,1]-=2*side
    return np.einsum('ni,nji->nj',features(local),model['coeff'][side]).min(1)


def transformed_original(coeff):
    """Convert original patch-local polynomials to one shared pair coordinate frame."""
    rng=np.random.default_rng(883)
    p=rng.uniform([0,0,0],EXTENT,(40,3))
    result=[]
    for side in range(2):
        local=p*2-1
        local[:,1]-=side*2
        result.append(np.linalg.lstsq(features(p),features(local)@coeff[side].T,rcond=None)[0].T)
    return np.array(result)


def fit_p2(train, targets, surface, normals):
    a=p2_design(train)
    s,g=p2_design(surface,True)
    # Distance/8m target: derivative in the pair coordinate frame is unit normal.
    design=np.vstack((a, s*3, g.reshape(-1,45)*.18))
    rhs=np.r_[targets,np.zeros(len(s)),normals.ravel()*.18]
    return scipy.linalg.lstsq(design,rhs,lapack_driver='gelsy')[0]


def refine_p2(nodes, train, targets, surface, normals, validation, truth):
    """Convex squared-hinge refinement: prioritize native occupancy over distances."""
    a=p2_design(train);s,g=p2_design(surface,True);g=g.reshape(-1,45)
    sign=np.sign(targets);vx=p2_design(validation)
    best=(int(((vx@nodes>0)!=truth).sum()),nodes)
    for strength in (10.,100.,1000.):
        def objective(c):
            residual=np.minimum(sign*(a@c)-.002,0)
            surface_value=s@c;gradient_value=g@c-normals.ravel()
            loss=strength*np.mean(residual**2)+np.mean(surface_value**2)+.0001*np.mean(gradient_value**2)
            gradient=(2*strength/len(a))*(a.T@(sign*residual))+(2/len(s))*(s.T@surface_value)+(.0002/len(g))*(g.T@gradient_value)
            return loss,gradient
        result=minimize(objective,nodes,jac=True,method='L-BFGS-B',options=dict(maxiter=300,ftol=1e-13,gtol=1e-9))
        errors=int(((vx@result.x>0)!=truth).sum())
        if errors<best[0]:best=(errors,result.x.copy())
    return best[1]


def fit_csg(initial, train, targets, surface, normals, validation, truth, seconds, out, classification=False):
    """Direct per-fixture fitting; no encoder or pretrained network.

    Shared union of two intersections, five general quadrics each. A smooth
    optimization curriculum is followed by hard Boolean fitting and evaluation.
    Soft functions are NEVER exported or used for reported occupancy.
    """
    import torch
    torch.set_num_threads(2)
    tx=torch.tensor(features(train),dtype=torch.float32)
    sy=torch.tensor(targets,dtype=torch.float32)
    sx=torch.tensor(features(surface),dtype=torch.float32)
    sg=torch.tensor(derivatives(surface),dtype=torch.float32)
    sn=torch.tensor(normals,dtype=torch.float32)
    vx=features(validation)
    d=derivatives(surface)
    coeff=initial.copy()
    for group in range(2):
        values=features(surface)@coeff[group].T
        owner=values.argmin(1)
        for branch in range(5):
            g=d[owner==branch]@coeff[group,branch]
            scale=np.median(np.linalg.norm(g,axis=1)) if len(g) else 1
            coeff[group,branch]/=max(scale,.05)
    initial_pred=(vx@coeff.reshape(-1,10).T).reshape(-1,2,5).min(2).max(1)>0
    best=(int((initial_pred!=truth).sum()),coeff.copy()); history=[]; began=time.monotonic()
    # Two deterministic starts: unperturbed and a small coefficient perturbation.
    for restart in range(2):
        c=coeff.copy()
        if restart:
            c+=np.random.default_rng(79).normal(0,.008,c.shape)
        parameter=torch.nn.Parameter(torch.tensor(c,dtype=torch.float32))
        optimizer=torch.optim.Adam([parameter],lr=.002)
        deadline=began+(restart+1)*seconds/2
        iteration=0
        while time.monotonic()<deadline:
            progress=(time.monotonic()-(deadline-seconds/2))/(seconds/2)
            raw=(tx@parameter.reshape(-1,10).T).reshape(-1,2,5)
            sv=(sx@parameter.reshape(-1,10).T).reshape(-1,2,5)
            if progress<.55:
                temperature=.015*(1-progress/.55)+.0005
                group=-temperature*torch.logsumexp(-raw/temperature,dim=2)
                field=temperature*torch.logsumexp(group/temperature,dim=1)
                surface_group=-temperature*torch.logsumexp(-sv/temperature,dim=2)
                surface_field=temperature*torch.logsumexp(surface_group/temperature,dim=1)
            else:
                field=raw.min(2).values.max(1).values
                surface_field=sv.min(2).values.max(1).values
            branch=sv.detach().min(2).indices
            group=sv.detach().min(2).values.max(1).indices
            rows=torch.arange(len(surface))
            active=parameter[group,branch[rows,group]]
            gradient=torch.einsum('nki,ni->nk',sg,active)
            # Balanced source labels and a signed-distance proxy from sampled roots.
            positive=sy>0
            loss=((field-sy).square().mean() if not classification else 0)+surface_field.square().mean()*(3 if not classification else .3)
            for mask in (positive,~positive):
                if mask.any():
                    margin=torch.minimum(sy[mask].abs(),torch.tensor(.008 if not classification else .002))
                    loss+=torch.relu(margin-torch.sign(sy[mask])*field[mask]).square().mean()*(2 if not classification else 100)
            loss+=(.015 if not classification else .0001)*(gradient-sn).square().mean()
            optimizer.zero_grad();loss.backward();optimizer.step();iteration+=1
            if iteration%40==0:
                hard=parameter.detach().numpy().copy()
                predicted=(vx@hard.reshape(-1,10).T).reshape(-1,2,5).min(2).max(1)>0
                errors=int((predicted!=truth).sum())
                history.append(dict(restart=restart,iteration=iteration,errors=errors,phase='soft' if progress<.55 else 'hard'))
                # Only hard-phase checkpoints are eligible for export.
                if progress>=.55 and (best is None or errors<best[0]):
                    best=(errors,hard)
        write_json(out/'training-status.json',dict(restart=restart,iterations=iteration,elapsed_seconds=time.monotonic()-began,best_errors=None if best is None else best[0]))
    if best is None:
        raise RuntimeError('Budget too small to reach a hard-phase checkpoint')
    write_json(out/'csg-history.json',history)
    return best[1],dict(seconds=time.monotonic()-began,iterations=history[-1]['iteration'],validation_errors=best[0],validation_samples=len(truth))


def native_metrics(model, blocks):
    result=dict(errors=0,cells=4194304,interior_errors=0,collar_errors=0,outer_collar_edge_errors=0,false_air=0,false_solid=0)
    for side,block in enumerate(blocks):
        for first in range(0,block.size,32768):
            ids=np.arange(first,min(first+32768,block.size))
            y,z,x=np.unravel_index(ids,block.shape)
            p=(np.column_stack((x,y,z))+.5)/128
            p[:,1]+=side
            truth=block[y,z,x]>0;pred=evaluate(model,p)>0;bad=pred!=truth
            collar=(y>=125) if side==0 else (y<3)
            edge=((y>=123)&(y<125)) if side==0 else ((y>=3)&(y<5))
            for name,mask in [('errors',bad),('interior_errors',bad&~collar),('collar_errors',bad&collar),('outer_collar_edge_errors',bad&edge),('false_air',bad&truth),('false_solid',bad&~truth)]:
                result[name]+=int(mask.sum())
    result['error_fraction']=result['errors']/result['cells']
    return result


def solid(model, p):
    return np.all((p>=0)&(p<=EXTENT),axis=1)&(evaluate(model,p)>0)


def quadratic_roots(coeff, origins, directions):
    """Vectorized stable solver, with linear and repeated-root fallbacks."""
    c=features(origins)@coeff.T
    b=np.einsum('nki,mi,nk->nm',derivatives(origins),coeff,directions)
    a=features(directions)@coeff.T-coeff[:,0]-directions@coeff[:,1:4].T
    linear=np.abs(a)<1e-12
    disc=b*b-4*a*c
    root=np.sqrt(np.maximum(disc,0))
    q=-.5*(b+np.where(b>=0,root,-root))
    with np.errstate(divide='ignore',invalid='ignore'):
        t1=q/a;t2=c/q
        repeated=-b/(2*a)
        t1=np.where(q==0,repeated,t1);t2=np.where(q==0,repeated,t2)
        t1=np.where(linear,-c/b,t1);t2=np.where(linear,np.inf,t2)
    t1=np.where((disc>=0)|linear,t1,np.inf)
    t2=np.where((disc>=0)|linear,t2,np.inf)
    return np.stack((t1,t2),axis=2).reshape(len(origins),-1)


def raycast(model, origins, directions):
    """Exact continuous primitive candidates; CPU reference, no GPU timing claim.

    Includes finite-fixture cut faces. There is no spatial acceleration in this
    tiny benchmark: all primitives are tested for every ray.
    """
    if model['kind']=='original':
        coeff=transformed_original(model['coeff']).reshape(-1,10)
    elif model['kind']=='p2':
        coeff=model['coeff']
    else:
        coeff=model['coeff'].reshape(-1,10)
    candidates=quadratic_roots(coeff,origins,directions)
    with np.errstate(divide='ignore',invalid='ignore'):
        planes=np.concatenate([(bound-origins[:,axis:axis+1])/directions[:,axis:axis+1]
                               for axis in range(3) for bound in (0.,EXTENT[axis])],axis=1)
        if model['kind']=='original':
            planes=np.c_[planes,(1-origins[:,1])/directions[:,1]]
    candidates=np.concatenate((candidates,planes),axis=1)
    candidates=np.where(np.isfinite(candidates)&(candidates>1e-7),candidates,np.inf)
    candidates.sort(axis=1)
    # Roots from inactive domains/Boolean primitives are rejected by full-model
    # occupancy on each side. No per-piece field is independently selected in CSG.
    result=np.full(len(origins),np.inf)
    valid=np.isfinite(candidates)
    row,col=np.nonzero(valid)
    t=candidates[row,col]
    points=origins[row]+directions[row]*t[:,None]
    eps=1e-7
    changed=solid(model,points-directions[row]*eps)!=solid(model,points+directions[row]*eps)
    np.minimum.at(result,row[changed],t[changed])
    return result,dict(primitive_count=len(coeff),quadratic_root_slots=2*len(coeff),candidate_slots=candidates.shape[1])


def self_test():
    rng=np.random.default_rng(742)
    p=rng.uniform([.01,.01,.01],EXTENT-.01,(500,3))
    y,z,x=np.meshgrid(np.arange(5)/2,np.arange(3)/2,np.arange(3)/2,indexing='ij')
    node_points=np.column_stack((x.ravel(),y.ravel(),z.ravel()))
    q=rng.normal(size=10);nodes=features(node_points)@q
    error=float(np.abs(p2_design(p)@nodes-features(p)@q).max())
    assert error<1e-12
    baked=bake_p2(nodes)
    assert np.abs(baked-q).max()<1e-10
    sphere=np.array([[1.,0,0,0,-1,-1,-1,0,0,0]])
    roots_=quadratic_roots(sphere,np.array([[0.,0.,-2.],[0.,0.,0.]]),np.array([[0.,0.,1.],[0.,0.,1.]]))
    assert np.allclose(np.sort(roots_[0]),[1,3]) and np.isclose(roots_[1].max(),1)
    return dict(p2_quadratic_reproduction_error=error)


def main(args):
    started=time.monotonic();out=Path(args.output);out.mkdir(parents=True,exist_ok=False)
    rng=np.random.default_rng(2304)
    def status(phase,**extra):
        data=dict(phase=phase,elapsed_seconds=time.monotonic()-started,**extra)
        write_json(out/'status.json',data);print(json.dumps(data),flush=True)
    tests=self_test()
    source=Path(args.source);header=json.loads((source/'terrain.json').read_text())
    coeff=np.array([np.array(header['coefficients'])[key] for key in KEYS])
    density=np.memmap(source/'density.f32',dtype='<f4',mode='r',shape=(512,512,512))
    blocks=[density[y*128:(y+1)*128,z*128:(z+1)*128,x*128:(x+1)*128] for y,z,x in KEYS]
    origin=(np.array(header['origin_cells'])+[384,0,0])*SIZE
    recipe=out/'recipe.json';write_json(recipe,header['recipe'])
    status('sampling_source',origin_m=origin.tolist(),extent_m=(EXTENT*8).tolist())
    y,z,x=np.meshgrid(np.arange(33)/16,np.arange(17)/16,np.arange(17)/16,indexing='ij')
    cloud=np.column_stack((x.ravel(),y.ravel(),z.ravel()))
    field=query(origin+cloud*8,recipe,out,'surface-lattice').reshape(33,17,17)
    low=[];high=[];sign=[]
    for axis in range(3):
        a=[slice(None)]*3;b=a.copy();a[axis]=slice(0,-1);b[axis]=slice(1,None)
        ids=np.argwhere((field[tuple(a)]>0)!=(field[tuple(b)]>0));other=ids.copy();other[:,axis]+=1
        low.append(ids[:,[2,0,1]]/16);high.append(other[:,[2,0,1]]/16);sign.append(field[tuple(ids.T)]>0)
    surface=(roots(origin+np.vstack(low)*8,origin+np.vstack(high)*8,np.concatenate(sign),recipe,out,'surface-roots')-origin)/8
    h=SIZE/4
    p=np.vstack([origin+surface*8+np.eye(3)[axis]*direction*h for axis in range(3) for direction in [-1,1]])
    values=query(p,recipe,out,'normals').reshape(3,2,len(surface))
    normal=(values[:,1]-values[:,0]).T
    normal/=np.maximum(np.linalg.norm(normal,axis=1,keepdims=True),1e-10)
    train=(rng.integers([0,0,0],[128,256,128],(16384,3))+.5)/128
    # Additional native centers around the old seam; not coefficient constraints.
    collar=rng.integers([0,125,0],[128,131,128],(8192,3))
    train=np.vstack((train,(collar+.5)/128))
    source_values=query(origin+train*8,recipe,out,'train')
    targets=np.where(source_values>0,1.,-1.)*np.minimum(cKDTree(surface).query(train)[0],.15)
    validation=(rng.integers([0,0,0],[128,256,128],(32768,3))+.5)/128
    truth=query(origin+validation*8,recipe,out,'validation')>0
    models={'original':dict(kind='original',coeff=coeff)}
    tick=time.monotonic();nodes=fit_p2(train,targets,surface,normal)
    if args.classification:
        nodes=refine_p2(nodes,train,targets,surface,normal,validation,truth)
    p2_seconds=time.monotonic()-tick
    models['p2']=dict(kind='p2',nodes=nodes,coeff=bake_p2(nodes))
    status('fitting_shared_csg',surface_points=len(surface),seconds_budget=args.seconds,p2_seconds=p2_seconds)
    initial=transformed_original(coeff)
    if args.warm_start:
        initial=np.array(json.loads(Path(args.warm_start).read_text())['coeff'])
    csg,training=fit_csg(initial,train,targets,surface,normal,validation,truth,args.seconds,out,args.classification)
    training.update(classification_refinement=args.classification,warm_start=args.warm_start)
    models['csg']=dict(kind='csg',coeff=csg)
    status('exhaustive_native_validation',training=training)
    metrics={name:native_metrics(model,blocks) for name,model in models.items()}
    y,z,x=np.meshgrid(np.arange(64)*2+.5,np.arange(64)*2+.5,np.arange(64)*2+.5,indexing='ij')
    base=np.column_stack((x.ravel(),y.ravel(),z.ravel()))/128
    face_x,face_z=np.meshgrid((np.arange(128)+.5)/128,(np.arange(128)+.5)/128,indexing='xy')
    face=np.column_stack((face_x.ravel(),np.ones(face_x.size),face_z.ravel()))
    target_face=query(origin+face*8,recipe,out,'face-truth')>0
    face_images=[]
    for name,model in models.items():
        left=face.copy();right=face.copy()
        if name=='original':
            left[:,1]-=1e-10;right[:,1]+=1e-10
        # P2 evaluates either polynomial trace at the exact same face via baked
        # quadrics below; CSG has one identical expression on both sides.
        lower=evaluate(model,left)>0;upper=evaluate(model,right)>0
        components=[]
        for occupied in (lower,upper):
            components.append(dict(solid=int(label(occupied.reshape(128,128))[1]),air=int(label(~occupied.reshape(128,128))[1])))
        metrics[name]['face']=dict(disagreements=int((lower!=upper).sum()),samples=len(face),lower_target_errors=int((lower!=target_face).sum()),upper_target_errors=int((upper!=target_face).sum()),components=components)
        for mask in (lower,upper,lower!=upper):
            rgb=np.where(mask.reshape(128,128,1),[180,180,185] if len(face_images)%3<2 else [245,60,50],[28,42,57]).astype('uint8')
            face_images.append(Image.fromarray(rgb).resize((256,256),Image.Resampling.NEAREST))
        volume=np.zeros((128,128,128),bool)
        for side in range(2):
            p=base.copy();p[:,1]+=side
            volume[side*64:(side+1)*64,:64,:64]=(evaluate(model,p)>0).reshape(64,64,64)
        iso.top=np.full((128,128),129)
        iso(np.packbits(volume.ravel(),bitorder='little'),128,448).save(out/f'{name}-isometric.png')
        write_json(out/f'{name}-model.json',dict(kind=model['kind'],origin_m=origin.tolist(),coordinate_unit_m=8.,extent_m=(EXTENT*8).tolist(),**{k:v.tolist() for k,v in model.items() if isinstance(v,np.ndarray)}))
        # Concrete payload files for storage accounting, without claiming these
        # experimental encodings are accepted by the production viewer.
        model['coeff'].astype('<f4').tofile(out/f'{name}-coefficients.f32')
        if name=='p2':
            model['nodes'].astype('<f4').tofile(out/'p2-nodes.f32')
        if name=='csg':
            np.array([0,5,*range(10)],dtype='<u4').tofile(out/'csg-groups.u32')
    # Independent continuity checks on all internal triangular faces, including
    # faces within each cube. Vertex/edge sharing gives exact traces in real math.
    tets=tetrahedra();baked=models['p2']['coeff'];max_jump=0.;face_checks=0
    for i,j in itertools.combinations(range(12),2):
        common=np.array([v for v in tets[i] if np.any(np.all(tets[j]==v,axis=1))])
        if len(common)!=3:continue
        weights=rng.dirichlet(np.ones(3),1000);p=weights@common
        max_jump=max(max_jump,float(np.abs(features(p)@(baked[i]-baked[j])).max()));face_checks+=len(p)
    assert max_jump<1e-9
    tests.update(p2_internal_face_samples=face_checks,p2_max_field_jump=max_jump)
    reference_rgb=np.where(target_face.reshape(128,128,1),[180,180,185],[28,42,57]).astype('uint8')
    reference_image=Image.fromarray(reference_rgb).resize((256,256),Image.Resampling.NEAREST)
    canvas=Image.new('RGB',(1024,3*280),'#18212b');draw=ImageDraw.Draw(canvas)
    for row,name in enumerate(models):
        canvas.paste(reference_image,(0,row*280+24))
        for col in range(3):canvas.paste(face_images[row*3+col],((col+1)*256,row*280+24))
        draw.text((8,row*280+5),name+': FBM / lower / upper / disagreement',fill='white')
    canvas.save(out/'comparison-faces.png')
    # Slice at the middle z plane reveals features hidden by the overview.
    x,y=np.meshgrid((np.arange(128)+.5)/128,(np.arange(256)+.5)/128)
    p=np.column_stack((x.ravel(),y.ravel(),np.full(x.size,.5)))
    truth_slice=query(origin+p*8,recipe,out,'slice-truth')>0
    canvas=Image.new('RGB',(4*256,536),'#18212b');draw=ImageDraw.Draw(canvas)
    for col,(name,mask) in enumerate([('FBM',truth_slice)]+[(n,evaluate(m,p)>0) for n,m in models.items()]):
        rgb=np.where(mask.reshape(256,128,1),[170,170,175],[28,42,57]).astype('uint8')[::-1]
        canvas.paste(Image.fromarray(rgb).resize((256,512),Image.Resampling.NEAREST),(col*256,24));draw.text((col*256+8,5),name,fill='white')
    canvas.save(out/'comparison-slice.png')
    status('ray_benchmark')
    origins=np.vstack((np.tile([.5,2.7,-1.],(512,1)),np.tile([-.6,1.02,.5],(512,1))))
    aim=rng.uniform([0,0,0],EXTENT,(1024,3))
    directions=aim-origins;directions/=np.linalg.norm(directions,axis=1,keepdims=True)
    for name,model in models.items():
        timings=[]
        for _ in range(4):
            tick=time.perf_counter();hits,counts=raycast(model,origins,directions);timings.append(time.perf_counter()-tick)
        metrics[name]['ray_reference']=dict(cpu_median_ms=float(np.median(timings[1:])*1000),rays=len(origins),hits=int(np.isfinite(hits).sum()),**counts)
        # Independent fine sampling/bisection check. This may miss features thinner
        # than its spacing, so it is a sanity test, not a proof of no missed roots.
        tested=0;disagreements=0;max_error=0.
        for index in range(0,1024,32):
            ts=np.linspace(0,6,4097);p=origins[index]+ts[:,None]*directions[index]
            flags=solid(model,p);edges=np.flatnonzero(flags[:-1]!=flags[1:])
            ref=np.inf
            if len(edges):
                lo,hi=ts[edges[0]],ts[edges[0]+1];flag=flags[edges[0]]
                for _ in range(25):
                    mid=(lo+hi)/2
                    if bool(solid(model,(origins[index]+directions[index]*mid)[None])[0])==flag:lo=mid
                    else:hi=mid
                ref=(lo+hi)/2
            hit=hits[index];tested+=1
            if np.isfinite(ref)!=np.isfinite(hit):disagreements+=1
            elif np.isfinite(ref):
                error=abs(ref-hit)*8;max_error=max(max_error,error)
                disagreements+=int(error>1e-4)
        metrics[name]['ray_reference'].update(sanity_rays=tested,sanity_disagreements=disagreements,sanity_max_error_m=max_error)
        assert disagreements==0,(name,metrics[name]['ray_reference'])
    metrics['original']['storage']=dict(coefficient_f32_bytes=400,gpu_padded_coefficient_bytes=480)
    metrics['csg']['storage']=dict(coefficient_f32_bytes=400,topology_u32_bytes=48,total_packed_bytes=448,note='Ten quadrics; two groups of five IDs plus two offsets. No spatial index. GPU backend not implemented.')
    metrics['p2']['storage']=dict(shared_node_f32_bytes=180,baked_coefficient_f32_bytes=480,gpu_padded_coefficient_bytes=576,note='Fixed regular topology implicit. Either nodal or baked representation; not both required. GPU backend not implemented.')
    result=dict(metrics=metrics,tests=tests,csg_training=training,p2_training_seconds=p2_seconds,elapsed_seconds=time.monotonic()-started,note='Exhaustive native occupancy on two original cubes. Face lattice is a diagnostic, not held-out accuracy. P2 C0, not C1. CSG globally shared max(min(Qgroup)); no gradient continuity guarantee. CPU ray timings include all primitives, Boolean tests and fixture caps; no acceleration, GPU or voxel ray performance claim. Models are experimental JSON, not viewer scene files.')
    write_json(out/'metrics.json',result)
    (out/'index.html').write_text('<html><body style="background:#18212b;color:white;font-family:sans-serif"><h1>Seam representation comparison</h1><img src="comparison-faces.png"><img src="comparison-slice.png">'+''.join('<h2>'+name+'</h2><img src="'+name+'-isometric.png">' for name in models)+'<pre>'+json.dumps(result,indent=2)+'</pre></body></html>')
    status('complete',result=result)


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source',default='output/fit-occupancy')
    parser.add_argument('--output',default='output/seam-representation-comparison')
    parser.add_argument('--seconds',type=float,default=40)
    parser.add_argument('--classification',action='store_true')
    parser.add_argument('--warm-start')
    main(parser.parse_args())
