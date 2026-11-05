"""Scalable Hermite ALS in the existing shared quadratic space.
Scalar sparse design + five batched PCG solves; never a dense world normal matrix.
Rust provides continuous surface points/normals and native-center sign targets.
"""
import argparse,hashlib,json,os,signal,time
from pathlib import Path
import numpy as np
import scipy.sparse as sp
from scipy.spatial import cKDTree
import torch
from model import ContinuousQuadratics
from prepare import PARAMS
from train import write_json,load_dataset,evaluate,seam_check,export_model,preview

STOP=False
def stopped(*_):
    global STOP
    STOP=True

def design(points,counts,axis=None):
    p=np.asarray(points,dtype=float);nx,ny,nz=counts
    cell=np.minimum(np.floor(p).astype(int),np.array(counts)-1).clip(0)
    t=p-cell;ix,iy,iz=cell.T;u,v,w=t.T
    dimensions=[(nx+1)*(ny+1),(nx+1)*(nz+1),(ny+1)*(nz+1),nx,ny,nz]
    offsets=np.r_[0,np.cumsum(dimensions)];columns=[];values=[]
    for first,second,a,b,width,offset,ax1,ax2 in [(ix,iy,u,v,ny+1,offsets[0],0,1),(ix,iz,u,w,nz+1,offsets[1],0,2),(iy,iz,v,w,nz+1,offsets[2],1,2)]:
        base=first*width+second+offset
        columns.append(np.column_stack((base,base+width,base+1,base+width+1)))
        if axis is None:weights=np.column_stack(((1-a)*(1-b),a*(1-b),(1-a)*b,a*b))
        elif axis==ax1:weights=np.column_stack((-(1-b),1-b,-b,b))
        elif axis==ax2:weights=np.column_stack((-(1-a),-a,1-a,a))
        else:weights=np.zeros((len(p),4))
        values.append(weights)
    columns.append(np.column_stack((ix+offsets[3],iy+offsets[4],iz+offsets[5])))
    if axis is None:values.append(t*(1-t))
    else:
        g=np.zeros_like(t);g[:,axis]=1-2*t[:,axis];values.append(g)
    c=np.concatenate(columns,axis=1);v=np.concatenate(values,axis=1);rows=np.broadcast_to(np.arange(len(p))[:,None],c.shape)
    keep=v!=0
    return sp.coo_matrix((v[keep],(rows[keep],c[keep])),shape=(len(p),int(offsets[-1]))).tocsr()

def gpu_sparse(a,device):
    a=a.tocsr();return torch.sparse_csr_tensor(torch.from_numpy(a.indptr.astype('int64')),torch.from_numpy(a.indices.astype('int64')),torch.from_numpy(a.data.astype('float32')),size=a.shape,device=device)

def vector(model):return np.concatenate([p.detach().cpu().numpy().reshape(-1,model.branches) for p in model.parameters()])
@torch.no_grad()
def set_vector(model,x):
    start=0
    for p in model.parameters():
        count=p.numel()//model.branches;p.copy_(x[start:start+count].reshape(p.shape));start+=count

@torch.no_grad()
def pcg(a,at,a2t,w,targets,initial,iterations=120,ridge=1e-8):
    mm=torch.sparse.mm
    diagonal=mm(a2t,w)+ridge
    def normal(x):return mm(at,w*mm(a,x))+ridge*x
    b=mm(at,w*targets)+ridge*initial;x=initial.clone();r=b-normal(x);z=r/diagonal;p=z.clone();rz=(r*z).sum(0);norm0=torch.linalg.vector_norm(r,dim=0).clamp_min(1e-12)
    for step in range(iterations):
        ap=normal(p);denom=(p*ap).sum(0);active=torch.linalg.vector_norm(r,dim=0)>norm0*1e-4
        if not bool(active.any()) or STOP:break
        alpha=torch.where(active&(denom>1e-25),rz/denom.clamp_min(1e-25),torch.zeros_like(rz))
        x+=p*alpha;r-=ap*alpha;z=r/diagonal;next_rz=(r*z).sum(0)
        beta=torch.where(active,next_rz/rz.clamp_min(1e-30),torch.zeros_like(rz));p=z+p*beta;rz=next_rz
    relative=(torch.linalg.vector_norm(r,dim=0)/norm0).cpu().tolist()
    if not bool(torch.isfinite(x).all()):raise RuntimeError('Nonfinite PCG solution')
    return x,dict(iterations=step+1,relative_residual=relative)

def cluster(points,k,rng):
    centers=points[rng.choice(len(points),k,replace=False)].copy()
    for _ in range(12):
        owner=((points[:,None]-centers)**2).sum(2).argmin(1)
        for j in range(k):
            if np.any(owner==j):centers[j]=points[owner==j].mean(0)
    return owner

def self_test(device):
    torch.manual_seed(17);rng=np.random.default_rng(19);m=ContinuousQuadratics((2,2,2),5,dtype=torch.float64)
    with torch.no_grad():
        for p in m.parameters():p.normal_()
    points=rng.random((512,3))*2;a=design(points,m.counts);theta=vector(m)
    assert np.max(np.abs(a@theta-m(torch.from_numpy(points)).detach().numpy()))<1e-12
    cells=np.floor(points);local=2*(points-cells)-1
    # Independent finite differences compare analytic parameter rows with forward().
    for axis in range(3):
        delta=np.eye(3)[axis]*1e-6
        finite=(m(torch.from_numpy(points+delta))-m(torch.from_numpy(points-delta))).detach().numpy()/2e-6
        assert np.max(np.abs(design(points,m.counts,axis)@theta-finite))<1e-7
    dense=rng.normal(size=(400,13));target=rng.normal(size=(400,5));weights=rng.uniform(.1,1.,target.shape);initial=np.zeros((13,5));s=sp.csr_matrix(dense)
    aa=gpu_sparse(s,device);at=gpu_sparse(s.T,device);square=s.copy();square.data**=2;a2t=gpu_sparse(square.T,device)
    solution,_=pcg(aa,at,a2t,torch.tensor(weights,dtype=torch.float32,device=device),torch.tensor(target,dtype=torch.float32,device=device),torch.tensor(initial,dtype=torch.float32,device=device),100,ridge=1e-5)
    exact=np.column_stack([np.linalg.solve(dense.T@(weights[:,j,None]*dense)+np.eye(13)*1e-5,dense.T@(weights[:,j]*target[:,j])) for j in range(5)])
    error=float(np.max(np.abs(solution.cpu().numpy()-exact)));assert error<1e-4,error
    return dict(device=str(device),design_forward_and_derivative_parity=True,pcg_dense_solution_max_error=error)

def prepare(a):
    import subprocess
    world=json.loads(Path(a.world).read_text());out=Path(a.output);out.mkdir(parents=True,exist_ok=True)
    config=dict(world=world,off_count=a.off,surface_count=a.surface,seed=a.seed,sampler_sha256=hashlib.sha256(Path(a.sampler).read_bytes()).hexdigest())
    if (out/'hermite.json').exists():
        if json.loads((out/'hermite.json').read_text())!=config:raise ValueError('Incompatible Hermite cache')
        if (out/'off.bin').stat().st_size!=a.off*32 or (out/'surface.bin').stat().st_size!=a.surface*32:raise ValueError('Truncated Hermite cache')
        return
    params=world['terrain'];values=[params[k] for k in PARAMS]+world['bounds_m']['min']+world['dimensions']+[world.get('proposed_patch_edge_m',8.)]
    (out/'sampler.params').write_text('\n'.join(map(str,values))+'\n')
    subprocess.run([a.sampler,str(out/'sampler.params'),str(out),str(a.off),str(a.surface),str(a.seed)],check=True)
    write_json(out/'hermite.json',config)

def run(a):
    global STOP
    device=torch.device(a.device)
    if device.type=='cuda' and not torch.cuda.is_available():raise RuntimeError('CUDA requested but unavailable')
    torch.set_num_threads(4);torch.manual_seed(a.seed);rng=np.random.default_rng(a.seed);out=Path(a.output);out.mkdir(parents=True,exist_ok=True);started=time.monotonic()
    def status(phase,**kw):
        d=dict(phase=phase,elapsed_seconds=time.monotonic()-started,**kw);write_json(out/'status.json',d)
        with (out/'run.log').open('a') as f:f.write(json.dumps(d)+'\n')
        print(json.dumps(d),flush=True)
    world=json.loads(Path(a.world).read_text());metadata=json.loads((Path(a.data)/'hermite.json').read_text());assert world==metadata['world']
    edge=world.get('proposed_patch_edge_m',8.);counts=np.rint(np.array(world['extent_m'])/edge).astype(int).tolist()
    raw=np.fromfile(Path(a.data)/'off.bin',dtype='<f4').reshape(-1,8);surface=np.fromfile(Path(a.data)/'surface.bin',dtype='<f4').reshape(-1,8)
    if len(raw)!=metadata['off_count'] or len(surface)!=metadata['surface_count']:raise ValueError('Record count mismatch')
    if not np.isfinite(raw).all() or not np.isfinite(surface).all():raise ValueError('Nonfinite geometric data')
    if not np.allclose(np.linalg.norm(surface[:,4:7],axis=1),1.,atol=1e-4):raise ValueError('Invalid surface normals')
    s=surface[:,:3];p=raw[:,:3];distance=np.minimum(cKDTree(s).query(p)[0],.25);off_target=np.where(raw[:,3]>0,distance,-distance)
    model=ContinuousQuadratics(counts,5,device=device)
    # Sample authoritative base-height nodes from an already prepared occupancy dataset.
    vpos,vd,vm=load_dataset(a.validation,device,edge);assert vm['world']==world
    base=np.fromfile(Path(a.validation)/'base-heights.f32',dtype='<f4').reshape(counts[0]+1,counts[2]+1)
    model.initialize_surface(torch.tensor(base,device=device),world['bounds_m']['min'][1],edge,a.seed)
    theta=torch.tensor(vector(model),device=device);initial=evaluate(model,vpos,vd)
    scalar=[design(s,counts)]+[design(s,counts,i) for i in range(3)]+[design(p,counts)]
    matrix=sp.vstack(scalar,format='csr').astype('float32');aa=gpu_sparse(matrix,device);at=gpu_sparse(matrix.T,device);squared=matrix.copy();squared.data**=2;a2t=gpu_sparse(squared.T,device)
    ns=len(s);nr=len(p);sd=torch.tensor(surface[:,4:7],device=device);off_y=torch.tensor(off_target,dtype=torch.float32,device=device);positive=torch.tensor(raw[:,3]>0,device=device)
    cave=surface[:,7]>1.5;owners=np.zeros(ns,int)
    if cave.sum()>=4:owners[cave]=1+cluster(s[cave].astype(float),4,rng)
    else:owners=cluster(np.column_stack((s,.3*surface[:,4:7])).astype(float),5,rng)
    initial_owners=torch.tensor(owners,device=device);best=theta.clone();best_error=initial['all']['fraction'];best_step=0;first=0
    if a.resume and (out/'latest.pt').exists():
        checkpoint=torch.load(out/'latest.pt',map_location=device,weights_only=False)
        if checkpoint['world']!=world or checkpoint['metadata']!=metadata:raise ValueError('Incompatible checkpoint')
        theta=checkpoint['theta'];best=checkpoint['best'];best_error=checkpoint['best_error'];best_step=checkpoint['best_step'];first=checkpoint['step']
    signal.signal(signal.SIGUSR1,stopped);signal.signal(signal.SIGTERM,stopped)
    config=dict(method='shared Hermite ALS; sparse batched preconditioned CG',counts=counts,branches=5,parameters=theta.numel(),surface_points=ns,off_points=nr,sparse_rows=matrix.shape[0],sparse_columns=matrix.shape[1],sparse_nnz=matrix.nnz,device=str(device),gpu=torch.cuda.get_device_name(device) if device.type=='cuda' else 'CPU',initial=initial,world=world)
    write_json(out/'run-config.json',config);status('training',initial=initial,configuration={k:v for k,v in config.items() if k not in ['world','initial']})
    training_start=time.monotonic()
    for step in range(first,a.outer):
        q=torch.sparse.mm(aa,theta);sv=q[:ns];off=q[4*ns:]
        owner=initial_owners if step==0 else sv.argmin(1);off_owner=off.argmin(1)
        w=torch.zeros_like(q);target=torch.zeros_like(q)
        for j in range(5):
            mask=owner==j;owned=off_owner==j;violated=positive&(off[:,j]<.01)
            if int(mask.sum())<4:continue
            w[:ns,j]=mask/max(int(mask.sum()),1)
            for axis in range(3):
                w[(axis+1)*ns:(axis+2)*ns,j]=mask*(.08/max(int(mask.sum()),1));target[(axis+1)*ns:(axis+2)*ns,j]=sd[:,axis]
            # Combine least-squares distance targets and violated inside constraints.
            v1=owned*(.25/max(int(owned.sum()),1));v2=violated*(.5/max(int(violated.sum()),1))
            w[4*ns:,j]=v1+v2;target[4*ns:,j]=(v1*off_y+v2*.01)/(v1+v2).clamp_min(1e-30)
        candidate,solver=pcg(aa,at,a2t,w,target,theta,a.cg)
        for alpha in [1.,.5,.25]:
            trial=theta+alpha*(candidate-theta);set_vector(model,trial);metrics=evaluate(model,vpos,vd);error=metrics['all']['fraction']
            if error<best_error:best=trial.clone();best_error=error;best_step=step+1
        theta=candidate
        checkpoint=dict(theta=theta,best=best,best_error=best_error,best_step=best_step,step=step+1,world=world,metadata=metadata)
        temp=out/'latest.tmp';torch.save(checkpoint,temp);temp.replace(out/'latest.pt')
        status('training',step=step+1,best_step=best_step,best_error=best_error,solver=solver,active_branches=torch.bincount(owner,minlength=5).cpu().tolist(),cuda_peak_bytes=torch.cuda.max_memory_allocated(device) if device.type=='cuda' else None)
        if STOP:status('interrupted');return 75
        if time.monotonic()-training_start>a.seconds:break
    set_vector(model,best);validation=evaluate(model,vpos,vd);seams=seam_check(model)
    if a.gate and (best_error>.08 or best_error>initial['all']['fraction']*.5 or seams['occupancy_disagreements']):raise RuntimeError('Hermite learning gate failed; world job must not start')
    test=None
    if a.test:
        tp,td,tm=load_dataset(a.test,device,edge);assert tm['world']==world and tm['seed']!=vm['seed'];test=evaluate(model,tp,td)
    status('exporting',validation=validation,seams=seams);header=export_model(model,world,out);preview(model,world,out,a.preview_width)
    write_json(out/'metrics.json',dict(initial=initial,validation=validation,test=test,best_step=best_step,elapsed_seconds=time.monotonic()-started,seams=seams,config=config,export=header,note='Sampled geometry fitting in globally coupled shared space; not exhaustive native overfit. Off distances approximate nearest sampled surface. Surface points generated by bounded Newton projection; rejected failures do not imply exhaustive topology.'))
    status('complete',best_step=best_step,validation=validation,test=test);return 0

def main():
    p=argparse.ArgumentParser(description=__doc__);sub=p.add_subparsers(dest='command',required=True)
    t=sub.add_parser('test');t.add_argument('--device',default='cpu')
    s=sub.add_parser('prepare');s.add_argument('--world',required=True);s.add_argument('--output',required=True);s.add_argument('--sampler',required=True);s.add_argument('--off',type=int,default=262144);s.add_argument('--surface',type=int,default=131072);s.add_argument('--seed',type=int,default=77123)
    r=sub.add_parser('fit');r.add_argument('--world',required=True);r.add_argument('--data',required=True);r.add_argument('--validation',required=True);r.add_argument('--test');r.add_argument('--output',required=True);r.add_argument('--device',default='cuda');r.add_argument('--outer',type=int,default=24);r.add_argument('--cg',type=int,default=160);r.add_argument('--seconds',type=float,default=4500.);r.add_argument('--seed',type=int,default=42);r.add_argument('--preview-width',type=int,default=512);r.add_argument('--gate',action='store_true');r.add_argument('--resume',action='store_true')
    a=p.parse_args()
    if a.command=='test':print(self_test(a.device));return
    if a.command=='prepare':prepare(a);return
    raise SystemExit(run(a))
if __name__=='__main__':main()
