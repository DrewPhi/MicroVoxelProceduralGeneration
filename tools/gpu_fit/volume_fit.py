"""Independent full-ROI P2/overlapping-CSG GPU experiment, not a viewer rewrite.

Reuses authoritative native sample files, initializes from Rust density nodes,
fits on GPU, and exports best validation weights plus untouched test metrics.
No full native voxel array or dense global least-squares matrix is materialized.
"""
import argparse
import hashlib
import json
import signal
import time
from pathlib import Path

import numpy as np
import torch

from train import load_dataset,write_json,save_png
from volume_models import P2Volume,CSGVolume,p2_rows,self_test

STOP=False
def stop(*_):
    global STOP
    STOP=True


@torch.no_grad()
def evaluate(model,positions,density,batch=8192):
    counts={name:[0,0] for name in ['all','uniform','surface_sampling','density_band_1m']}
    false_air=0;false_solid=0
    for first in range(0,len(positions),batch):
        p=positions[first:first+batch];d=density[first:first+batch]
        pred=model(p)>0;truth=d>0;wrong=pred!=truth
        ids=torch.arange(first,first+len(p),device=p.device)
        masks=[torch.ones_like(wrong),ids%3!=2,ids%3==2,d.abs()<1]
        for name,mask in zip(counts,masks):
            counts[name][0]+=int((wrong&mask).sum());counts[name][1]+=int(mask.sum())
        false_air+=int((truth&~pred).sum());false_solid+=int((~truth&pred).sum())
    return {**{k:dict(errors=v[0],samples=v[1],fraction=v[0]/max(v[1],1)) for k,v in counts.items()},'false_air':false_air,'false_solid':false_solid,'certification':'sampled native centers, not exhaustive voxel certification'}


@torch.no_grad()
def seam_diagnostics(model):
    device=next(model.parameters()).device
    if isinstance(model,CSGVolume):
        return dict(construction='one global union of overlapping box-bounded quadric groups',tile_specific_geometry_selection=False,warning='Box planes are real analytic CSG boundaries; artificial planar features remain possible. No claim of matching source topology.')
    generator=torch.Generator(device=device).manual_seed(665)
    worst=0.;bad=0;near_zero=0
    for axis,count in enumerate(model.counts):
        if count<=1:continue
        p=torch.rand((12000,3),generator=generator,device=device)*torch.tensor(model.counts,device=device)
        p[:,axis]=torch.randint(1,count,(len(p),),generator=generator,device=device).float()
        right=p.floor().long();left=right.clone();left[:,axis]-=1
        li,lw=p2_rows(p,model.counts,cell_override=left)
        ri,rw=p2_rows(p,model.counts,cell_override=right)
        a=(model.nodes[li]*lw).sum(1);b=(model.nodes[ri]*rw).sum(1)
        worst=max(worst,float((a-b).abs().max()));bad+=int(((a>0)!=(b>0)).sum())
        near_zero+=int((torch.minimum(a.abs(),b.abs())<1e-6).sum())
    return dict(points_per_axis=12000,max_field_jump=worst,occupancy_disagreements=bad,near_zero_points=near_zero,construction='shared C0 P2 values, no C1 guarantee')


@torch.no_grad()
def previews(model,world,out,width=256):
    device=next(model.parameters()).device;edge=world['proposed_patch_edge_m'];nx,ny,nz=model.counts
    z,x=torch.meshgrid((torch.arange(width,device=device)+.5)*nz/width,(torch.arange(width,device=device)+.5)*nx/width,indexing='ij')
    x=x.flatten();z=z.flatten();height=torch.full_like(x,-1.)
    for y in np.arange(ny-1e-5,-.5,-.5):
        ids=torch.nonzero(height<0).flatten()
        if not len(ids):break
        p=torch.stack((x[ids],torch.full_like(x[ids],max(y,0)),z[ids]),1)
        height[ids[model(p)>0]]=max(y,0)
    h=(height.reshape(width,width).cpu().numpy()*edge+world['bounds_m']['min'][1])
    np.save(out/'preview-heights.npy',h)
    normalized=np.clip((h-world['bounds_m']['min'][1])/(ny*edge),0,1)
    gy,gx=np.gradient(h,world['extent_m'][2]/width,world['extent_m'][0]/width)
    light=np.clip((.9-gx*.35-gy*.25)/np.sqrt(1+gx*gx+gy*gy),.25,1)
    color=np.stack((80+normalized*95,110+normalized*65,65+normalized*115),2)
    save_png(out/'top-down.png',(color*light[:,:,None]).clip(0,255).astype('uint8'))
    rows=max(64,round(width*ny/nx))
    y,x=torch.meshgrid((torch.arange(rows,device=device)+.5)*ny/rows,(torch.arange(width,device=device)+.5)*nx/width,indexing='ij')
    p=torch.stack((x.flatten(),y.flatten(),torch.full_like(x.flatten(),nz/2)),1)
    f=torch.cat([model(p[i:i+8192]) for i in range(0,len(p),8192)])
    occupancy=(f>0).reshape(rows,width).cpu().numpy()[::-1]
    rgb=np.where(occupancy[:,:,None],np.array([170,170,175]),np.array([28,42,57])).astype('uint8')
    save_png(out/'center-slice.png',rgb)


def main(a):
    started=time.monotonic();device=torch.device(a.device)
    if device.type=='cuda' and not torch.cuda.is_available():raise RuntimeError('CUDA required, unavailable')
    torch.set_num_threads(4);torch.manual_seed(a.seed)
    out=Path(a.output);out.mkdir(parents=True,exist_ok=True)
    def status(phase,**kw):
        row=dict(phase=phase,elapsed_seconds=time.monotonic()-started,**kw)
        write_json(out/'status.json',row)
        with (out/'run.log').open('a') as file:file.write(json.dumps(row)+'\n')
        print(json.dumps(row),flush=True)
    tests=self_test(str(device))
    world=json.loads(Path(a.world).read_text());edge=world.get('proposed_patch_edge_m',8.)
    counts=np.rint(np.array(world['extent_m'])/edge).astype(int).tolist()
    if not np.allclose(np.array(counts)*edge,world['extent_m']):raise ValueError('ROI must align with cells')
    model=(P2Volume if a.kind=='p2' else CSGVolume)(counts,device)
    node_array=np.fromfile(a.nodes,dtype='<f4')
    expected=int(np.prod(2*np.array(counts)+1))
    if len(node_array)!=expected or not np.isfinite(node_array).all():raise ValueError('Invalid node cache')
    node_metadata=json.loads(Path(a.nodes).with_suffix('.json').read_text())
    if node_metadata['world']!=world:raise ValueError('Wrong source world for node cache')
    if hashlib.sha256(Path(a.nodes).read_bytes()).hexdigest()!=node_metadata['sha256']:raise ValueError('Node cache checksum mismatch')
    model.initialize(torch.tensor(node_array,device=device));del node_array
    train_p,train_d,train_meta=load_dataset(a.data,device,edge)
    val_p,val_d,val_meta=load_dataset(a.validation,device,edge)
    if train_meta['world']!=world or val_meta['world']!=world:raise ValueError('Wrong world in sample cache')
    surface_meta=json.loads((Path(a.surface)/'hermite.json').read_text())
    if surface_meta['world']!=world:raise ValueError('Wrong source world for surface samples')
    surface=np.fromfile(Path(a.surface)/'surface.bin',dtype='<f4').reshape(-1,8)
    if len(surface)!=surface_meta['surface_count'] or not np.isfinite(surface).all():raise ValueError('Invalid surface records')
    surface=torch.tensor(surface,device=device)
    initial=evaluate(model,val_p,val_d);initial_seams=seam_diagnostics(model)
    parameters=sum(p.numel() for p in model.parameters())
    config=dict(kind=a.kind,world=world,counts=counts,parameters=parameters,parameter_f32_bytes=parameters*4,training_samples=len(train_p),surface_samples=len(surface),validation_samples=len(val_p),train_dataset_signature=train_meta['signature'],validation_dataset_signature=val_meta['signature'],surface_seed=surface_meta['seed'],node_metadata=node_metadata,device=str(device),gpu=torch.cuda.get_device_name(device) if device.type=='cuda' else None,tests=tests,initial=initial,initial_seams=initial_seams,method='classification hinge + roots + analytic normal constraints; bounded Adam; best sampled validation checkpoint',max_steps=a.steps,seconds_budget=a.seconds)
    write_json(out/'run-config.json',config)
    optimizer=torch.optim.Adam(model.parameters(),lr=.005 if a.kind=='p2' else .0015)
    best={k:v.detach().cpu().clone() for k,v in model.state_dict().items()}
    best_error=initial['all']['fraction'];best_step=0;first=0
    if a.resume and (out/'latest.pt').exists():
        saved=torch.load(out/'latest.pt',map_location=device,weights_only=False)
        if saved['world']!=world or saved['kind']!=a.kind:raise ValueError('Incompatible checkpoint')
        model.load_state_dict(saved['model']);optimizer.load_state_dict(saved['optimizer'])
        best={k:v.cpu() for k,v in saved['best'].items()};best_error=saved['best_error'];best_step=saved['best_step'];first=saved['step']
    def checkpoint(step):
        temp=out/'latest.tmp'
        torch.save(dict(world=world,kind=a.kind,model=model.state_dict(),optimizer=optimizer.state_dict(),best=best,best_error=best_error,best_step=best_step,step=step),temp)
        temp.replace(out/'latest.pt')
    signal.signal(signal.SIGUSR1,stop);signal.signal(signal.SIGTERM,stop)
    status('training',initial=initial,parameters=parameters,steps=a.steps)
    training_start=time.monotonic();last_step=first
    for step in range(first,a.steps):
        ids=torch.randint(len(train_p),(a.batch,),device=device)
        sids=torch.randint(len(surface),(a.surface_batch,),device=device)
        field=model(train_p[ids]);sign=torch.sign(train_d[ids])
        sf,gradient=model(surface[sids,:3],True)
        loss=.3*sf.square().mean()+.0001*(gradient-surface[sids,4:7]).square().mean()
        for positive in [True,False]:
            mask=(sign>0)==positive
            if bool(mask.any()):loss+=100*torch.relu(.002-sign[mask]*field[mask]).square().mean()
        if not bool(torch.isfinite(loss)):raise RuntimeError('Nonfinite loss')
        optimizer.zero_grad(set_to_none=True);loss.backward()
        torch.nn.utils.clip_grad_norm_(model.parameters(),10.);optimizer.step();last_step=step+1
        if last_step%a.validate_every==0 or last_step==a.steps or STOP:
            validation=evaluate(model,val_p,val_d);error=validation['all']['fraction']
            if error<best_error:
                best={k:v.detach().cpu().clone() for k,v in model.state_dict().items()};best_error=error;best_step=last_step
                temp=out/'best.tmp';torch.save(best,temp);temp.replace(out/'best.pt')
            status('training',step=last_step,loss=float(loss.detach()),validation=validation,best_step=best_step,best_error=best_error,cuda_peak_bytes=torch.cuda.max_memory_allocated(device) if device.type=='cuda' else None)
        if last_step%1000==0 or STOP:checkpoint(last_step)
        if STOP or time.monotonic()-training_start>a.seconds:break
    checkpoint(last_step)
    model.load_state_dict(best);validation=evaluate(model,val_p,val_d)
    test_p,test_d,test_meta=load_dataset(a.test,device,edge)
    if test_meta['world']!=world or test_meta['signature']==val_meta['signature']:raise ValueError('Independent test cache required')
    test=evaluate(model,test_p,test_d);seams=seam_diagnostics(model)
    arrays={k:v.detach().cpu().numpy() for k,v in model.state_dict().items()}
    for name,data in arrays.items():np.save(out/f'{name}.npy',data)
    header=dict(version=1,representation='conforming-P2-Kuhn6' if a.kind=='p2' else 'global-union-overlapping-quadric-CSG',world=world,counts=counts,coordinate_unit_m=edge,positive_is_solid=True,arrays={k:dict(file=k+'.npy',shape=list(v.shape),dtype=str(v.dtype)) for k,v in arrays.items()},parameter_bytes=sum(v.nbytes for v in arrays.values()),viewer_compatible=False,notes='Experimental full-ROI representation. Existing viewer is unchanged; a new loader/evaluator is required.')
    if a.kind=='p2':header.update(node_order='y,z,x; shared nodes at half-cell spacing',tetrahedra_per_cube=6)
    else:header.update(group_order='y,z,x',quadrics_per_group=5,group_centers='(cube_xyz + 0.5)*cell_edge relative to ROI origin',support_half_edge_cells=1.,solid='union over groups of intersection(Q0>0,...,Q4>0,1-abs(local_x)>0,1-abs(local_y)>0,1-abs(local_z)>0)',lookup='up to eight overlapping groups; all support box planes are real CSG primitives',warning='Box boundaries may introduce planar geometry; smoothness/source topology not guaranteed')
    write_json(out/'model.json',header)
    previews(model,world,out)
    result=dict(initial=initial,validation=validation,test=test,best_step=best_step,steps=last_step,seams=seams,training_seconds=time.monotonic()-training_start,elapsed_seconds=time.monotonic()-started,export=header,tests=tests,test_signature=test_meta['signature'],note='No exhaustive native certification or GPU renderer timing. Best step 0 means source-based initialization was not improved. CSG is overlapping bounded groups, not ten global quadrics or independently clipped tiles.')
    write_json(out/'metrics.json',result);status('interrupted' if STOP else 'complete',best_step=best_step,test=test)
    return 75 if STOP else 0


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for key in ['world','nodes','data','surface','validation','test','output']:p.add_argument('--'+key,required=True)
    p.add_argument('--kind',choices=['p2','csg'],required=True);p.add_argument('--device',default='cuda')
    p.add_argument('--steps',type=int,default=6000);p.add_argument('--seconds',type=float,default=1800)
    p.add_argument('--batch',type=int,default=8192);p.add_argument('--surface-batch',type=int,default=2048)
    p.add_argument('--validate-every',type=int,default=250);p.add_argument('--seed',type=int,default=77123);p.add_argument('--resume',action='store_true')
    raise SystemExit(main(p.parse_args()))
