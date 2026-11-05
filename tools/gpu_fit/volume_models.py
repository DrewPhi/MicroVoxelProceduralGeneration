"""Local volumetric P2 and overlapping analytic CSG models for the 1km trial.

All coordinates are relative to the ROI, in 8m-cell units. Positive means solid.
The CSG grid is an acceleration index, not ownership of separate fitted worlds.
"""
import itertools
import numpy as np
import torch
from torch import nn

PAIRS=list(itertools.combinations(range(4),2))
CORNERS=list(itertools.product((0,1),repeat=3))


def features(p):
    x,y,z=p.unbind(-1)
    return torch.stack((torch.ones_like(x),x,y,z,x*x,y*y,z*z,x*y,x*z,y*z),-1)


def polynomial_gradient(p,c):
    x,y,z=p.unbind(-1)
    return torch.stack((c[...,1]+2*c[...,4]*x+c[...,7]*y+c[...,8]*z,
                        c[...,2]+2*c[...,5]*y+c[...,7]*x+c[...,9]*z,
                        c[...,3]+2*c[...,6]*z+c[...,8]*x+c[...,9]*y),-1)


def p2_rows(points,counts,gradient=False,cell_override=None):
    maximum=points.new_tensor(counts,dtype=torch.long)-1
    cell=torch.minimum(points.floor().long(),maximum).clamp_min(0) if cell_override is None else cell_override
    t=points-cell
    order=torch.argsort(t,descending=True,stable=True,dim=1)
    sorted_t=t.gather(1,order)
    lam=torch.stack((1-sorted_t[:,0],sorted_t[:,0]-sorted_t[:,1],sorted_t[:,1]-sorted_t[:,2],sorted_t[:,2]),1)
    increments=torch.nn.functional.one_hot(order,3).to(cell.dtype)
    vertices=torch.cat((cell[:,None,:],cell[:,None,:]+increments.cumsum(1)),1)*2
    mids=torch.stack([(vertices[:,i]+vertices[:,j])//2 for i,j in PAIRS],1)
    nodes=torch.cat((vertices,mids),1)
    nx,_,nz=2*np.array(counts)+1
    indices=nodes[:,:,0]+int(nx)*(nodes[:,:,2]+int(nz)*nodes[:,:,1])
    weights=torch.cat((lam*(2*lam-1),torch.stack([4*lam[:,i]*lam[:,j] for i,j in PAIRS],1)),1)
    if not gradient:return indices,weights
    axis=increments.to(points.dtype)
    gl=torch.stack((-axis[:,0],axis[:,0]-axis[:,1],axis[:,1]-axis[:,2],axis[:,2]),1)
    dw=torch.cat(((4*lam-1)[:,:,None]*gl,torch.stack([4*(lam[:,i,None]*gl[:,j]+lam[:,j,None]*gl[:,i]) for i,j in PAIRS],1)),1)
    return indices,weights,dw


class P2Volume(nn.Module):
    def __init__(self,counts,device='cpu',dtype=torch.float32):
        super().__init__();self.counts=tuple(map(int,counts))
        self.nodes=nn.Parameter(torch.zeros(int(np.prod(2*np.array(counts)+1)),device=device,dtype=dtype))

    def forward(self,points,gradient=False):
        rows=p2_rows(points,self.counts,gradient)
        values=self.nodes[rows[0]]
        field=(rows[1]*values).sum(1)
        return (field,(rows[2]*values[:,:,None]).sum(1)) if gradient else field

    @torch.no_grad()
    def initialize(self,nodes):self.nodes.copy_(nodes)


class CSGVolume(nn.Module):
    """Union of box-bounded intersections of five general implicit quadrics.

    One group is centered on each original cube. Its support extends a full cell
    in every direction (16m edge), so neighboring groups overlap. A point queries
    at most eight groups. All groups belong to ONE global Boolean solid.
    Box planes are explicit CSG primitives and must be included in future ray
    candidate generation. Planar artifacts are possible, not hidden by clipping.
    """
    def __init__(self,counts,device='cpu',dtype=torch.float32):
        super().__init__();self.counts=tuple(map(int,counts))
        self.coeff=nn.Parameter(torch.zeros((int(np.prod(counts)),5,10),device=device,dtype=dtype))

    def candidates(self,points):
        cells=(points-.5).floor().long()[:,None,:]+torch.tensor(CORNERS,device=points.device)
        limits=torch.tensor(self.counts,device=points.device)
        valid=((cells>=0)&(cells<limits)).all(2)
        cells=torch.minimum(cells.clamp_min(0),limits-1)
        nx,_,nz=self.counts
        ids=cells[:,:,0]+nx*(cells[:,:,2]+nz*cells[:,:,1])
        local=points[:,None,:]-(cells+.5)
        return ids,local,valid

    def forward(self,points,gradient=False):
        ids,local,valid=self.candidates(points)
        coeff=self.coeff[ids]
        q=(coeff*features(local)[:,:,None,:]).sum(-1)
        polynomial,branch=q.min(2)
        plane,axis=(1-local.abs()).min(2)
        bounded=torch.minimum(polynomial,plane).masked_fill(~valid,-1e20)
        field,group=bounded.max(1)
        if not gradient:return field
        r=torch.arange(len(points),device=points.device)
        chosen=coeff[r,group,branch[r,group]]
        u=local[r,group]
        g=polynomial_gradient(u,chosen)
        plane_g=torch.zeros_like(g)
        chosen_axis=axis[r,group]
        plane_g[r,chosen_axis]=-torch.sign(u[r,chosen_axis])
        use_plane=plane[r,group]<polynomial[r,group]
        return field,torch.where(use_plane[:,None],plane_g,g)

    @torch.no_grad()
    def initialize(self,nodes,seed=7723):
        """Least-squares quadric initialization from authoritative nodal densities.

        Repeated local sample layout means the 10x27 fitting matrix is constant.
        This is initialization, not evidence that fitting improved the world.
        """
        nx,ny,nz=self.counts;nnx,_,nnz=2*np.array(self.counts)+1
        offsets=torch.tensor(list(itertools.product(range(3),repeat=3)),device=nodes.device)
        local=(offsets.to(nodes.dtype)-1)/2
        inverse=torch.linalg.pinv(features(local)).T
        generator=torch.Generator(device=nodes.device).manual_seed(seed)
        for first in range(0,len(self.coeff),8192):
            ids=torch.arange(first,min(first+8192,len(self.coeff)),device=nodes.device)
            cells=torch.stack((ids%nx,ids//(nx*nz),(ids//nx)%nz),1)
            ijk=2*cells[:,None,:]+offsets
            indices=ijk[:,:,0]+int(nnx)*(ijk[:,:,2]+int(nnz)*ijk[:,:,1])
            fit=nodes[indices]@inverse
            scale=torch.linalg.vector_norm(fit[:,1:4],dim=1).clamp_min(.5)
            fit=fit/scale[:,None]
            noise=torch.randn((len(ids),5,10),device=nodes.device,dtype=nodes.dtype,generator=generator)*.002
            self.coeff[first:first+len(ids)].copy_(fit[:,None,:]+noise)


def self_test(device='cpu'):
    """Independent reproduction, gradient and global CSG occupancy checks."""
    torch.manual_seed(44);dtype=torch.float64;counts=(2,2,2)
    model=P2Volume(counts,device,dtype)
    y,z,x=torch.meshgrid(*[torch.arange(5,device=device,dtype=dtype)/2]*3,indexing='ij')
    nodes=torch.stack((x.flatten(),y.flatten(),z.flatten()),1)
    q=torch.randn(10,device=device,dtype=dtype)
    model.initialize(features(nodes)@q)
    p=torch.rand((250,3),device=device,dtype=dtype)*1.98+.01
    error=float((model(p)-features(p)@q).detach().abs().max())
    assert error<1e-11,error
    csg=CSGVolume(counts,device,dtype)
    with torch.no_grad():csg.coeff.normal_()
    for candidate in [model,csg]:
        _,gradient=candidate(p,True)
        for axis in range(3):
            delta=torch.eye(3,device=device,dtype=dtype)[axis]*1e-6
            finite=(candidate(p+delta)-candidate(p-delta))/2e-6
            assert float((finite-gradient[:,axis]).detach().abs().max())<1e-7
    # Brute-force the complete union, independent of neighborhood lookup.
    cells=torch.tensor([(x,y,z) for y in range(2) for z in range(2) for x in range(2)],device=device,dtype=dtype)
    local=p[:,None,:]-(cells[None]+.5)
    values=(features(local)[:,:,None,:]*csg.coeff[None]).sum(-1).min(2).values
    brute=torch.minimum(values,(1-local.abs()).min(2).values).max(1).values
    assert bool(((csg(p)>0)==(brute>0)).all())
    return dict(quadratic_reproduction_max_error=error,analytic_gradients=True,csg_full_union_lookup_occupancy_parity=True,device=device)


if __name__=='__main__':
    print(self_test())
