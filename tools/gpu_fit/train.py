#!/usr/bin/env python3
"""GPU occupancy fitting with exact shared-face constraints, atomic checkpoints,
independent sampled validation and old quadratic coefficient export. No native
voxel-exact certification is claimed for the trillion-cell authoring region.
"""
import argparse,copy,hashlib,json,os,signal,struct,time,zlib
from pathlib import Path
import numpy as np
import torch
from model import ContinuousQuadratics,test_model

STOP=False
def stop_handler(*_):
    global STOP
    STOP=True

def write_json(path,data):
    path=Path(path);temp=path.with_suffix(path.suffix+'.tmp');temp.write_text(json.dumps(data,indent=2,allow_nan=False)+'\n');temp.replace(path)

def load_dataset(path,device,edge):
    meta=json.loads((Path(path)/'dataset.json').read_text());count=meta['samples'];raw=np.memmap(Path(path)/'samples.bin',dtype=np.dtype([('ijk','<u4',(3,)),('density','<f4')]),mode='r',shape=(count,))
    positions=torch.from_numpy(np.array(raw['ijk'],dtype=np.float32));positions.add_(.5).mul_(meta['world']['voxel_m']/edge)
    density=torch.from_numpy(np.array(raw['density'],dtype=np.float32))
    return positions.to(device),density.to(device),meta

def balanced_loss(q,density):
    positive=(density>0).to(q.dtype);negative=1-positive
    p=(torch.relu(.02-q).square()*positive[:,None]).sum()/positive.sum().clamp_min(1)
    a=(torch.relu(.02+q.amin(1)).square()*negative).sum()/negative.sum().clamp_min(1)
    return .5*(p+a)

@torch.no_grad()
def evaluate(model,positions,density,batch=131072):
    result={name:[0,0] for name in ['all','uniform','surface_sampling','density_band_1m']};false_air=false_solid=0
    for start in range(0,len(positions),batch):
        end=min(start+batch,len(positions));pred=model(positions[start:end]).amin(1)>0;target=density[start:end]>0;bad=pred!=target
        false_air+=int((target&~pred).sum());false_solid+=int((~target&pred).sum())
        indices=torch.arange(start,end,device=positions.device)
        for key,mask in [('all',torch.ones_like(bad)),('uniform',indices%3!=2),('surface_sampling',indices%3==2),('density_band_1m',density[start:end].abs()<1.)]:
            result[key][0]+=int((bad&mask).sum());result[key][1]+=int(mask.sum())
    return {**{k:dict(errors=v[0],samples=v[1],fraction=v[0]/max(v[1],1)) for k,v in result.items()},'false_air':false_air,'false_solid':false_solid,'certification':'independent sampled native centers; not exhaustive'}

@torch.no_grad()
def seam_check(model,count=12000):
    device=model.xy.device;dtype=model.xy.dtype;nx,ny,nz=model.counts;generator=torch.Generator(device=device).manual_seed(665)
    worst=0.;disagreements=0;parity=0.
    for axis,n in enumerate(model.counts):
        if n<=1:continue
        p=torch.rand((count,3),generator=generator,dtype=dtype,device=device)*torch.tensor(model.counts,device=device)
        p[:,axis]=torch.randint(1,n,(count,),generator=generator,device=device).to(dtype)
        right=torch.floor(p).long();left=right.clone();left[:,axis]-=1
        def values(cell):
            l=2*(p-cell)-1;x,y,z=l.unbind(1);basis=torch.stack((torch.ones_like(x),x,y,z,x*x,y*y,z*z,x*y,x*z,y*z),1);return (model.coefficients(cell)*basis[:,None,:]).sum(-1)
        a=values(left);b=values(right);worst=max(worst,float((a-b).abs().max()));disagreements+=int(((a.amin(1)>0)!=(b.amin(1)>0)).sum())
        parity=max(parity,float((model(p)-b).abs().max()))
    return dict(sampled_face_points_per_axis=count,max_field_jump=worst,occupancy_disagreements=disagreements,baked_direct_max_error=parity)

@torch.no_grad()
def export_model(model,world,out):
    """Expanded coefficients for the existing analytic kernel, plus compact planes.
    Binary export avoids gigabytes of decimal JSON. Large-scene viewer loader
    must read this format; do not silently pretend it is an old nested JSON.
    """
    out=Path(out);cpu=copy.deepcopy(model).to('cpu',dtype=torch.float64);nx,ny,nz=cpu.counts;branches=cpu.branches;edge=world.get('proposed_patch_edge_m',8.);voxel=world['voxel_m']
    arrays={k:v.detach().cpu().numpy() for k,v in model.state_dict().items()};np.savez_compressed(out/'shared-parameters.npz',**arrays)
    path=out/'terrain.coefficients.f32';temp=path.with_suffix('.tmp');sha=hashlib.sha256();count=nx*ny*nz
    with temp.open('wb') as f:
        for start in range(0,count,8192):
            ids=np.arange(start,min(start+8192,count));x=ids%nx;z=(ids//nx)%nz;y=ids//(nx*nz);cells=torch.from_numpy(np.column_stack((x,y,z)))
            coeff=cpu.coefficients(cells).numpy().astype(np.float32);packed=np.zeros((*coeff.shape[:-1],12),np.float32);packed[:,:,:10]=coeff
            q=coeff.astype(float);gx=np.abs(q[:,:,1])+2*np.abs(q[:,:,4])+np.abs(q[:,:,7])+np.abs(q[:,:,8]);gy=np.abs(q[:,:,2])+2*np.abs(q[:,:,5])+np.abs(q[:,:,7])+np.abs(q[:,:,9]);gz=np.abs(q[:,:,3])+2*np.abs(q[:,:,6])+np.abs(q[:,:,8])+np.abs(q[:,:,9]);packed[:,:,10]=((voxel/edge)*(gx+gy+gz)+.000012*(1+np.abs(q).sum(2)))*1.000001
            data=packed.astype('<f4',copy=False).tobytes();f.write(data);sha.update(data)
    temp.replace(path)
    header=dict(version=1,representation='min-of-implicit-quadrics',solid='min(Q)>0',feature_order=['1','x','y','z','xx','yy','zz','xy','xz','yz'],patch_order='y,z,x',patches_per_axis=list(cpu.counts),quadratics_per_patch=branches,origin_cells=world['origin_cells'],voxel_m=voxel,dimensions=world['dimensions'],normalization='local = 2*(world_m-patch_min_m)/patch_edge_m - 1',patch_edge_m=edge,coefficients_dtype='float32-padded12-le',coefficients_file=path.name,coefficients_sha256=sha.hexdigest(),coefficient_bytes=path.stat().st_size,shared_parameter_bytes=sum(v.nbytes for v in arrays.values()),terrain_recipe=world['terrain'],construction='exact C0 shared-plane P2 basis; same polynomial space as prior fixed-label face constraints')
    write_json(out/'terrain.json',header)
    # A small center crop also exports old-format JSON for immediate inspection.
    counts=np.minimum(np.array(cpu.counts),4);low=(np.array(cpu.counts)-counts)//2;cells=np.array([(x,y,z) for y in range(low[1],low[1]+counts[1]) for z in range(low[2],low[2]+counts[2]) for x in range(low[0],low[0]+counts[0])]);coeff=cpu.coefficients(torch.from_numpy(cells)).numpy().reshape(counts[1],counts[2],counts[0],branches,10)
    crop=dict(header,patches_per_axis=counts.tolist(),origin_cells=(np.array(world['origin_cells'])+np.rint(low*edge/voxel).astype(int)).tolist(),dimensions=np.rint(counts*edge/voxel).astype(int).tolist(),coefficients_dtype='float64',coefficients=coeff.tolist())
    for key in ['coefficients_file','coefficients_sha256','coefficient_bytes']:crop.pop(key,None)
    write_json(out/'center-crop.json',crop);return header

def save_png(path,rgb):
    h,w,_=rgb.shape
    def chunk(name,data):return struct.pack('>I',len(data))+name+data+struct.pack('>I',zlib.crc32(name+data)&0xffffffff)
    raw=b''.join(b'\0'+row.tobytes() for row in rgb)
    Path(path).write_bytes(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',w,h,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(raw,6))+chunk(b'IEND',b''))

@torch.no_grad()
def preview(model,world,out,width=512):
    """Diagnostic highest solid sample; coarse scan plus native vertical refinement.
    Not a certification of thin floating features or native XZ coverage.
    """
    device=model.xy.device;nx,ny,nz=model.counts;edge=world.get('proposed_patch_edge_m',8.);step=min(.5,ny/32);x=(torch.arange(width,device=device)+.5)*nx/width;z=(torch.arange(width,device=device)+.5)*nz/width;zz,xx=torch.meshgrid(z,x,indexing='ij');xx=xx.flatten();zz=zz.flatten();height=torch.full_like(xx,-1.);active=torch.ones_like(xx,dtype=torch.bool)
    for y in np.arange(ny-1e-5,-step,-step):
        indices=torch.nonzero(active).flatten()
        if len(indices)==0:break
        p=torch.stack((xx[indices],torch.full_like(xx[indices],max(y,0.)),zz[indices]),1);hit=model(p).amin(1)>0;found=indices[hit];height[found]=max(y,0.);active[found]=False
    native=world['voxel_m']/edge
    # Refine only the first occupied coarse interval, in parallel for all columns.
    valid=height>=0;indices=torch.nonzero(valid).flatten();base=height[indices].clone();refined=base.clone();pending=torch.ones_like(base,dtype=torch.bool)
    for delta in np.arange(step,0.,-native):
        sub=torch.nonzero(pending).flatten()
        if len(sub)==0:break
        yy=(base[sub]+delta).clamp_max(ny-1e-6);p=torch.stack((xx[indices[sub]],yy,zz[indices[sub]]),1);hit=model(p).amin(1)>0;found=sub[hit];refined[found]=yy[hit];pending[found]=False
    height[indices]=refined;array=(height.reshape(width,width).cpu().numpy()*edge+world['bounds_m']['min'][1]);np.save(Path(out)/'preview-heights.npy',array)
    good=(height.reshape(width,width).cpu().numpy()>=0);v=(array-world['surface_height_range_m'][0])/max(world['surface_height_range_m'][1]-world['surface_height_range_m'][0],1.)
    rgb=np.stack((60+100*v,100+65*v,45+100*v),2);dz,dx=np.gradient(array,world['extent_m'][0]/width);light=.45+.55*np.maximum(0.,(.45*dx+.82+.35*dz)/np.sqrt(1+dx*dx+dz*dz));rgb=np.clip(rgb*light[:,:,None],0,255).astype('uint8');rgb[~good]=[150,185,210];save_png(Path(out)/'preview-top-down.png',rgb)


def run(args):
    out=Path(args.output);out.mkdir(parents=True,exist_ok=True);start=time.monotonic()
    def status(phase,**kw):
        data=dict(phase=phase,elapsed_seconds=time.monotonic()-start,**kw);write_json(out/'status.json',data)
        with (out/'run.log').open('a') as f:f.write(json.dumps(data)+'\n')
        print(json.dumps(data),flush=True)
    device=torch.device(args.device)
    if device.type=='cuda' and not torch.cuda.is_available():raise RuntimeError('GPU requested but CUDA unavailable; refusing silent CPU fallback')
    torch.set_num_threads(min(int(os.environ.get('SLURM_CPUS_PER_TASK','4')),16));torch.manual_seed(args.seed)
    world=json.loads(Path(args.world).read_text());edge=world.get('proposed_patch_edge_m',8.);counts=np.rint(np.array(world['extent_m'])/edge).astype(int).tolist()
    if np.max(np.abs(np.array(counts)*edge-world['extent_m']))>1e-6:raise ValueError('Bounds must align to patch grid')
    model=ContinuousQuadratics(counts,args.branches,device=device);positions,density,train_meta=load_dataset(args.train,device,edge);vp,vd,val_meta=load_dataset(args.validation,device,edge)
    if train_meta['seed']==val_meta['seed']:raise ValueError('Training and validation seeds must differ')
    if train_meta['world']!=world or val_meta['world']!=world:raise ValueError('Dataset/world mismatch')
    base=np.fromfile(Path(args.train)/'base-heights.f32',dtype='<f4').reshape(counts[0]+1,counts[2]+1);model.initialize_surface(torch.tensor(base,device=device),world['bounds_m']['min'][1],edge,args.seed)
    if args.initialize:
        old=json.loads(Path(args.initialize).read_text());model.initialize_coefficients(old['coefficients'])
    optimizer=torch.optim.Adam(model.parameters(),lr=args.lr);generator=torch.Generator(device=device).manual_seed(args.seed+11);step=0;best_error=float('inf');stale=0;hard_pool=None
    if args.resume and (out/'latest.pt').exists():
        saved=torch.load(out/'latest.pt',map_location=device,weights_only=False)
        if saved['world']!=world:raise ValueError('Checkpoint/world mismatch')
        model.load_state_dict(saved['model']);optimizer.load_state_dict(saved['optimizer']);generator.set_state(saved['generator'].cpu());step=saved['step'];best_error=saved['best_error'];stale=saved['stale']
    initial=evaluate(model,vp,vd);write_json(out/'initial-metrics.json',initial)
    info=dict(torch=torch.__version__,device=str(device),gpu=torch.cuda.get_device_name(device) if device.type=='cuda' else 'CPU test',counts=counts,branches=args.branches,train_samples=len(positions),validation_samples=len(vp),parameters=sum(p.numel() for p in model.parameters()),world=world,seed=args.seed)
    write_json(out/'run-config.json',info);status('training',step=step,initial=initial,**{k:v for k,v in info.items() if k!='world'})
    signal.signal(signal.SIGUSR1,stop_handler);signal.signal(signal.SIGTERM,stop_handler)
    def checkpoint(path):
        state=dict(model=model.state_dict(),optimizer=optimizer.state_dict(),generator=generator.get_state(),step=step,best_error=best_error,stale=stale,world=world,config=info)
        temp=path.with_suffix('.tmp');torch.save(state,temp);temp.replace(path)
    best_error=min(best_error,initial['all']['fraction'])
    if not (out/'best.pt').exists():checkpoint(out/'best.pt')
    train_start=time.monotonic()
    for step in range(step+1,args.steps+1):
        ids=torch.randint(len(positions),(args.batch,),generator=generator,device=device)
        if hard_pool is not None and len(hard_pool):
            ids[:args.batch//4]=hard_pool[torch.randint(len(hard_pool),(args.batch//4,),generator=generator,device=device)]
        optimizer.zero_grad(set_to_none=True);q=model(positions[ids]);loss=balanced_loss(q,density[ids]);loss.backward();torch.nn.utils.clip_grad_norm_(model.parameters(),10.);optimizer.step()
        if step%args.evaluate==0 or step==args.steps or STOP:
            if not bool(torch.isfinite(loss)):raise RuntimeError('Nonfinite fitting loss')
            metrics=evaluate(model,vp,vd);error=metrics['all']['fraction']
            if error<best_error-1e-6:best_error=error;stale=0;checkpoint(out/'best.pt')
            else:stale+=1
            checkpoint(out/'latest.pt')
            with torch.no_grad():
                probe=torch.randint(len(positions),(262144,),generator=generator,device=device);wrong=(model(positions[probe]).amin(1)>0)!=(density[probe]>0);hard_pool=probe[wrong][:65536]
            status('training',step=step,loss=float(loss.detach()),best_error=best_error,validation=metrics,steps_per_second=(step)/(time.monotonic()-train_start),gpu_peak_bytes=torch.cuda.max_memory_allocated(device) if device.type=='cuda' else None,stale_checks=stale)
            if STOP:status('interrupted',step=step,checkpoint=str(out/'latest.pt'));return 75
            if (stale>=args.patience and step>=args.min_steps) or time.monotonic()-train_start>=args.seconds:break
    state=torch.load(out/'best.pt',map_location=device,weights_only=False);model.load_state_dict(state['model']);final=evaluate(model,vp,vd);seams=seam_check(model)
    status('exporting',validation=final,seams=seams);header=export_model(model,world,out);preview(model,world,out,args.preview_width)
    result=dict(initial=initial,validation=final,seams=seams,best_step=state['step'],last_step=step,elapsed_seconds=time.monotonic()-start,config=info,export=header,quality_note='Sampled validation only; global fixed-label constraints may limit detail. Inspect preview before replacing the old demo.')
    write_json(out/'metrics.json',result);status('complete',best_step=state['step'],validation=final,seams=seams,coefficient_bytes=header['coefficient_bytes']);return 0

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--world',required=True);p.add_argument('--train',required=True);p.add_argument('--validation',required=True);p.add_argument('--output',required=True);p.add_argument('--device',default='cuda');p.add_argument('--branches',type=int,default=5);p.add_argument('--steps',type=int,default=40000);p.add_argument('--min-steps',type=int,default=4000);p.add_argument('--batch',type=int,default=65536);p.add_argument('--evaluate',type=int,default=500);p.add_argument('--patience',type=int,default=30);p.add_argument('--seconds',type=float,default=4500.);p.add_argument('--lr',type=float,default=.003);p.add_argument('--seed',type=int,default=42);p.add_argument('--resume',action='store_true');p.add_argument('--initialize');p.add_argument('--preview-width',type=int,default=512)
    raise SystemExit(run(p.parse_args()))
