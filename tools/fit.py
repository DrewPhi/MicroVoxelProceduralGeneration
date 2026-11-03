#!/usr/bin/env python3
"""Offline hard-min quadratic regression. NumPy only; Rust owns target generation.
Coefficients: 1,x,y,z,x²,y²,z²,xy,xz,yz; local coordinates [-1,1].
"""
import os
for key in ('OPENBLAS_NUM_THREADS','OMP_NUM_THREADS','MKL_NUM_THREADS'): os.environ[key]='1'
import argparse, json, time, subprocess, sys
from pathlib import Path
import numpy as np
from PIL import Image


def write_json(path,data):
    tmp=path.with_suffix('.tmp');tmp.write_text(json.dumps(data,indent=2,allow_nan=False));tmp.replace(path)

def features(p):
    x,y,z=p.T
    return np.column_stack((np.ones(len(p)),x,y,z,x*x,y*y,z*z,x*y,x*z,y*z))

def predict(a,coeff): return np.min(a@coeff.T,axis=1)

def solve(a,b,previous,ridge):
    # QR/SVD least squares, with ridge toward the previous iterate.
    return np.linalg.lstsq(np.vstack((a,np.eye(10)*ridge**.5)),np.r_[b,previous*ridge**.5],rcond=None)[0]

def fit_patch(p,y,n,restarts,iterations,rng,initial=None,progress=None):
    a=features(p);scale=max(float(np.std(y)),0.1);target=y/scale
    best_loss=float('inf');best=None
    if initial is not None:
        best=initial/scale;best_loss=float(np.mean((predict(a,best)-target)**2))
    for restart in range(restarts):
        # Spatial initialization prevents every branch fitting the same samples.
        centers=p[rng.choice(len(p),n,replace=False)]
        owner=np.argmin(np.sum((p[:,None,:]-centers[None,:,:])**2,axis=2),axis=1)
        coeff=np.zeros((n,10))
        for k in range(n):
            mask=owner==k;coeff[k]=solve(a[mask],target[mask],coeff[k],1e-5)
        if restart == 0 and best is not None: coeff=best.copy()
        for iteration in range(iterations):
            values=a@coeff.T;owner=np.argmin(values,axis=1)
            loss=float(np.mean((values.min(axis=1)-target)**2))
            if loss<best_loss: best_loss=loss;best=coeff.copy()
            candidate=coeff.copy()
            for k in range(n):
                mask=owner==k
                if mask.sum()>=10:
                    candidate[k]=solve(a[mask],target[mask],coeff[k],1e-4)
                else:
                    # Revive near a bad overprediction; upward paraboloid limits influence.
                    residual=values.min(axis=1)-target
                    j=int(np.argmax(residual));c=p[j];curv=4.
                    candidate[k]=[target[j]+curv*np.dot(c,c),*(-2*curv*c),curv,curv,curv,0,0,0]
            accepted=False
            for alpha in (1.,.5,.25,.125,.0625,.015625):
                trial=coeff+alpha*(candidate-coeff)
                next_loss=float(np.mean((predict(a,trial)-target)**2))
                if next_loss<loss-1e-12:coeff=trial;accepted=True;break
            if not accepted:break
        final_loss=float(np.mean((predict(a,coeff)-target)**2))
        if final_loss<best_loss: best_loss=final_loss;best=coeff.copy()
        if progress is not None and (restart%8==0 or restart==restarts-1):
            progress(restart+1,best_loss*scale*scale)
    return best*scale, best_loss*scale*scale


def sample_patch(block,count,rng):
    edge=block.shape[0];total=block.size
    # Uniform samples + narrow-band samples, then remove duplicates.
    uniform=rng.choice(total,min(count,total),replace=False)
    near=np.flatnonzero(np.abs(block.reshape(-1))<1.0)
    if len(near): uniform=np.unique(np.r_[uniform,rng.choice(near,min(count,len(near)),replace=False)])
    iy,iz,ix=np.unravel_index(uniform,block.shape)
    p=(np.column_stack((ix,iy,iz))+.5)/edge*2-1
    return p,block.reshape(-1)[uniform].astype(float)


def palette_image(height,n):
    valid=height>=0;v=np.clip(height/max(n-1,1),0,1)
    rgb=np.stack((80+v*110,105+v*100,60+v*120),axis=-1).astype('uint8');rgb[~valid]=[155,190,215]
    return Image.fromarray(rgb)


def iso(bits,n,width=384):
    # Same exact voxel-box reference for both volumes, vectorized across pixel rays.
    h=width*3//4;py,px=np.mgrid[:h,:width]
    f=np.array([1.,-1.,-1.]);f/=np.linalg.norm(f)
    right=np.cross(f,[0.,1.,0.]);right/=np.linalg.norm(right);up=np.cross(right,f)
    eye=np.ones(3)*n/2-f*n*1.5
    origins=eye+(2*(px.ravel()+.5)/width-1)[:,None]*right*n*.9*width/h+(1-2*(py.ravel()+.5)/h)[:,None]*up*n*.9
    t0=(0-origins)/f;t1=(n-origins)/f
    enter=np.maximum(np.minimum(t0,t1).max(axis=1),0)
    exit=np.maximum(t0,t1).min(axis=1);active=np.flatnonzero(enter<exit)
    cell=np.floor(origins+f*(enter[:,None]+1e-7)).astype(int)
    step=np.sign(f).astype(int);delta=1/np.abs(f)
    next_t=((cell+(step>0))-origins)/f
    normals=np.zeros_like(cell,dtype=float)
    axis=np.argmax(np.minimum(t0,t1),axis=1);normals[np.arange(len(cell)),axis]=-step[axis]
    rgb=np.tile([155,190,215],(len(cell),1)).astype('uint8')
    for _ in range(n*3+3):
        if not len(active):break
        c=cell[active];inbounds=np.all((c>=0)&(c<n),axis=1);active=active[inbounds]
        if not len(active):break
        c=cell[active];idx=c[:,0]+n*(c[:,2]+n*c[:,1])
        solid=(bits[idx//8]>>(idx%8))&1;hits=active[solid!=0]
        if len(hits):
            light=.38+.62*np.maximum(normals[hits]@np.array([-.424,.848,-.318]),0)
            base=np.tile([140.,140.,145.],(len(hits),1))
            # Exterior top skin: no higher solid in this column (computed separately).
            ch=iso.top[cell[hits,2],cell[hits,0]]
            grass=cell[hits,1]>=ch-1;base[grass]=[110.,140.,78.]
            rgb[hits]=(base*light[:,None]).clip(0,255).astype('uint8')
        active=active[solid==0]
        if not len(active):break
        boundary=next_t[active].min(axis=1);keep=boundary<exit[active];active=active[keep];boundary=boundary[keep]
        axes=np.isclose(next_t[active],boundary[:,None],atol=1e-9,rtol=0)
        cell[active]+=axes*step;next_t[active]+=axes*delta
        normals[active]=0;ax=np.argmax(axes,axis=1);normals[active,ax]=-step[ax]
    return Image.fromarray(rgb.reshape(h,width,3))


def run(args):
    out=Path(args.output).resolve();out.mkdir(parents=True,exist_ok=True);start=time.time()
    def status(phase,**extra):
        write_json(out/'status.json',dict(phase=phase,elapsed_seconds=time.time()-start,**extra));print(phase,extra,flush=True)
    write_json(out/'run_config.json',vars(args))
    recipe=json.loads(Path(args.recipe).read_text());recipe['inspection']['span']=args.span
    write_json(out/'recipe.json',recipe)
    if args.target_dir:
        source=Path(args.target_dir).resolve()
        saved=json.loads((source/'target.json').read_text())
        if saved['recipe']!=recipe:raise ValueError('Cached target recipe mismatch')
        for name in ['target.json','density.f32']:
            destination=out/name
            if not destination.exists():os.link(source/name,destination)
        status('reusing_exact_rust_target')
    else:
        status('exporting_rust_target')
        subprocess.run([str(Path(args.exporter).resolve()),str(out/'recipe.json'),str(out)],check=True)
    meta=json.loads((out/'target.json').read_text());n=meta['dimensions'][0];g=args.patches
    if n%g:raise ValueError('Patch count per axis must divide voxel grid dimension')
    if args.n<1 or args.n>128 or g<1:raise ValueError('Invalid model dimensions')
    if n>512:raise ValueError('First fitter limits fixtures to 512 cells per axis to bound memory')
    edge=n//g;density=np.memmap(out/'density.f32',dtype='<f4',mode='r',shape=(n,n,n))
    coeff=np.zeros((g,g,g,args.n,10));rng=np.random.default_rng(args.seed);patch_metrics=[]
    model=dict(version=1,representation='min-of-implicit-quadrics',solid='min(Q)>0',
        feature_order=['1','x','y','z','xx','yy','zz','xy','xz','yz'],
        patch_order='y,z,x',patches_per_axis=[g,g,g],quadratics_per_patch=args.n,
        origin_cells=meta['origin_cells'],voxel_m=meta['voxel_m'],dimensions=meta['dimensions'],
        normalization='local = 2*(world_m-patch_min_m)/patch_edge_m - 1',
        patch_edge_m=edge*meta['voxel_m'],coefficients_dtype='float64',recipe=recipe)
    baseline=None
    if args.baseline:
        baseline_dir=Path(args.baseline)
        old_config=json.loads((baseline_dir/'run_config.json').read_text())
        if any(old_config[k]!=getattr(args,k) for k in ['n','patches','span','samples','seed']):
            raise ValueError('Baseline model and sampling configuration must match')
        if json.loads((baseline_dir/'recipe.json').read_text())!=recipe:
            raise ValueError('Baseline terrain recipe must match')
        baseline=np.load(baseline_dir/'coefficients.npy')
    warm=None;warm_count=0
    if args.warm_checkpoint:
        warm_dir=Path(args.warm_checkpoint)
        if json.loads((warm_dir/'recipe.json').read_text())!=recipe:raise ValueError('Warm terrain mismatch')
        warm=np.load(warm_dir/'coefficients-checkpoint.npy')
        if warm.shape!=coeff.shape:raise ValueError('Warm coefficient shape mismatch')
        warm_count=len(json.loads((warm_dir/'patch_metrics.json').read_text()))
    status('training',completed=0,total=g**3)
    for iy in range(g):
      for iz in range(g):
       for ix in range(g):
        block=np.asarray(density[iy*edge:(iy+1)*edge,iz*edge:(iz+1)*edge,ix*edge:(ix+1)*edge])
        p,y=sample_patch(block,args.samples,rng)
        fit_rng=rng
        initial=None
        if baseline is not None:
            import copy
            fit_rng=np.random.default_rng();fit_rng.bit_generator.state=copy.deepcopy(rng.bit_generator.state)
            # Reproduce the baseline's samples/validation exactly, independent of extra restarts.
            for _ in range(old_config['restarts']):rng.choice(len(p),args.n,replace=False)
            initial=baseline[iy,iz,ix]
        def progress(restart,loss,**extra):
            status('training',completed=len(patch_metrics),total=g**3,patch=[ix,iy,iz],
                restart=restart,restarts=args.restarts,best_training_mse=loss,**extra)
        extra_metrics={}
        if args.occupancy:
            from occupancy import fit_occupancy
            extra=warm[iy,iz,ix] if warm is not None and len(patch_metrics)<warm_count else None
            c,loss=fit_occupancy(p,y,args.n,args.restarts,args.iterations,fit_rng,initial,progress,block,args.patience,args.min_restarts,extra)
            extra_metrics=fit_occupancy.last_stats
        else:c,loss=fit_patch(p,y,args.n,args.restarts,args.iterations,fit_rng,initial,progress)
        coeff[iy,iz,ix]=c
        # Random native-center validation; exhaustive voxel validation follows.
        indices=rng.integers(0,edge,size=(8192,3));vp=(indices[:,[2,0,1]]+.5)/edge*2-1
        vy=block[indices[:,0],indices[:,1],indices[:,2]];pred=predict(features(vp),c)
        patch_metrics.append(dict(index=[ix,iy,iz],training_mse=loss,validation_mse=float(np.mean((pred-vy)**2)),
            validation_disagreement=float(np.mean((pred>0)!=(vy>0))),**extra_metrics))
        np.save(out/'coefficients-checkpoint.npy',coeff)
        write_json(out/'patch_metrics.json',patch_metrics)
        status('training',completed=len(patch_metrics),total=g**3,last=patch_metrics[-1])
    model['coefficients']=coeff.tolist();write_json(out/'terrain.json',model)
    np.save(out/'coefficients.npy',coeff)
    status('validating_native_voxels',completed=0,total=g**3)
    actual=np.memmap(out/'target-occupancy.u8',dtype='uint8',mode='w+',shape=(n,n,n))
    fitted=np.memmap(out/'fit-occupancy.u8',dtype='uint8',mode='w+',shape=(n,n,n))
    mismatches=0;solid_count=0;false_air=0;false_solid=0;max_error=0.;sse=0.
    for iy in range(g):
      for iz in range(g):
       for ix in range(g):
        c=coeff[iy,iz,ix];a=0
        while a<edge**3:
            ids=np.arange(a,min(a+65536,edge**3));yy,zz,xx=np.unravel_index(ids,(edge,edge,edge))
            p=(np.column_stack((xx,yy,zz))+.5)/edge*2-1
            pred=predict(features(p),c);target=density[iy*edge+yy,iz*edge+zz,ix*edge+xx]
            t=target>0;f=pred>0
            actual[iy*edge+yy,iz*edge+zz,ix*edge+xx]=t;fitted[iy*edge+yy,iz*edge+zz,ix*edge+xx]=f
            mismatches+=int(np.count_nonzero(t!=f));solid_count+=int(t.sum())
            false_air+=int(np.count_nonzero(t&~f));false_solid+=int(np.count_nonzero(~t&f))
            error=pred-target;sse+=float(error@error);max_error=max(max_error,float(np.max(np.abs(error))))
            a+=len(ids)
        status('validating_native_voxels',completed=iy*g*g+iz*g+ix+1,total=g**3)
    actual.flush();fitted.flush()
    metrics=dict(native_cells=n**3,disagreements=mismatches,disagreement_fraction=mismatches/n**3,
        false_air=false_air,false_solid=false_solid,target_solid_cells=solid_count,
        density_rmse=(sse/n**3)**.5,max_density_error=max_error,coefficient_bytes=coeff.nbytes,
        elapsed_seconds=time.time()-start,exact_native_match=mismatches==0,
        note='Density is not a signed distance; density error is not geometric error. No global-optimum guarantee.')
    write_json(out/'metrics.json',metrics);status('rendering_previews')
    heights=[]
    for name,volume in [('fbm',actual),('fit',fitted)]:
        top=np.full((n,n),-1,dtype=np.int32)
        for y in range(n):top[volume[y]!=0]=y
        heights.append(top);palette_image(top,n).save(out/f'{name}-surface.png')
        slice_rgb=np.where(volume[:,n//2,:,None]!=0,np.array([140,140,145]),np.array([22,31,43])).astype('uint8')
        Image.fromarray(slice_rgb[::-1]).save(out/f'{name}-slice.png')
        bits=np.packbits(np.asarray(volume).reshape(-1),bitorder='little');bits.tofile(out/f'{name}.bits')
        iso.top=top;iso(bits,n,args.preview_width).save(out/f'{name}-isometric.png')
    for kind in ['surface','slice','isometric']:
        left=Image.open(out/f'fbm-{kind}.png');right=Image.open(out/f'fit-{kind}.png')
        pair=Image.new('RGB',(left.width*2,left.height));pair.paste(left,(0,0));pair.paste(right,(left.width,0));pair.save(out/f'comparison-{kind}.png')
    valid=(heights[0]>=0)&(heights[1]>=0)
    metrics['surface_height_max_error_m']=float(np.max(np.abs(heights[0][valid]-heights[1][valid]))*meta['voxel_m']) if valid.any() else None
    metrics['surface_column_presence_disagreements']=int(np.count_nonzero((heights[0]>=0)!=(heights[1]>=0)))
    metrics['elapsed_seconds']=time.time()-start;write_json(out/'metrics.json',metrics)
    (out/'index.html').write_text('<html><body style="background:#18212b;color:white;font-family:sans-serif"><h1>FBM → fitted quadrics</h1><p>Original left, fitted right. Native occupancy; same camera. Surfaces are highest occupied cell in this cropped cube.</p>'+''.join(f'<h2>{k}</h2><img style="max-width:100%" src="comparison-{k}.png">' for k in ['surface','slice','isometric'])+'<h2>Metrics</h2><pre>'+json.dumps(metrics,indent=2)+'</pre></body></html>')
    status('complete',metrics=metrics)

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--recipe',required=True);p.add_argument('--output',required=True)
    p.add_argument('--exporter',default='target/release/fbm_samples');p.add_argument('--n',type=int,default=5)
    p.add_argument('--patches',type=int,default=4,help='Uniform patches per axis; total is patches³')
    p.add_argument('--span',type=float,default=32);p.add_argument('--samples',type=int,default=4096)
    p.add_argument('--restarts',type=int,default=4);p.add_argument('--iterations',type=int,default=60)
    p.add_argument('--occupancy',action='store_true')
    p.add_argument('--target-dir',help='Reuse an identical Rust target fixture')
    p.add_argument('--warm-checkpoint',help='Additional completed patch candidates')
    p.add_argument('--patience',type=int,default=24)
    p.add_argument('--min-restarts',type=int,default=16)
    p.add_argument('--baseline',help='Warm start and retain original sampling from a completed run')
    p.add_argument('--seed',type=int,default=42);p.add_argument('--preview-width',type=int,default=384)
    args=p.parse_args()
    try:run(args)
    except Exception as e:
        Path(args.output).mkdir(parents=True,exist_ok=True);write_json(Path(args.output)/'status.json',{'phase':'failed','error':str(e)})
        raise
