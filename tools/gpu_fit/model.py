"""Exact C0 total-degree-two cube field, represented without a dense nullspace.
For each branch: bilinear XY + XZ + YZ grid fields, plus one-dimensional
quadratic bubbles per axis. Every face restriction matches by construction.
For a 2³ group this spans exactly the prior rank-25 constrained scalar space.
No learned neural network, tensor-product triquadratic, or runtime blending.
"""
import numpy as np
import torch
from torch import nn

class ContinuousQuadratics(nn.Module):
    def __init__(self, counts, branches=5, dtype=torch.float32, device='cpu'):
        super().__init__();self.counts=tuple(map(int,counts));self.branches=branches
        nx,ny,nz=self.counts
        for name,shape in [('xy',(nx+1,ny+1)),('xz',(nx+1,nz+1)),('yz',(ny+1,nz+1)),('qx',(nx,)),('qy',(ny,)),('qz',(nz,))]:
            self.register_parameter(name,nn.Parameter(torch.zeros((*shape,branches),dtype=dtype,device=device)))

    @staticmethod
    def bilinear(grid,i,j,u,v):
        return ((1-u)*(1-v))[:,None]*grid[i,j]+(u*(1-v))[:,None]*grid[i+1,j]+((1-u)*v)[:,None]*grid[i,j+1]+(u*v)[:,None]*grid[i+1,j+1]

    def forward(self,positions):
        # Position units are patch edges, relative to the region's lower corner.
        maximum=positions.new_tensor(self.counts)-1
        cell=torch.minimum(torch.floor(positions).long(),maximum.long()).clamp_min(0)
        t=positions-cell;ix,iy,iz=cell.unbind(1);u,v,w=t.unbind(1)
        return self.bilinear(self.xy,ix,iy,u,v)+self.bilinear(self.xz,ix,iz,u,w)+self.bilinear(self.yz,iy,iz,v,w)+(u*(1-u))[:,None]*self.qx[ix]+(v*(1-v))[:,None]*self.qy[iy]+(w*(1-w))[:,None]*self.qz[iz]

    def coefficients(self,cells):
        """Bake cells XYZ to [batch,branch,10], native local [-1,1] monomials."""
        x,y,z=cells.long().unbind(1)
        a00=self.xy[x,y];a10=self.xy[x+1,y];a01=self.xy[x,y+1];a11=self.xy[x+1,y+1]
        b00=self.xz[x,z];b10=self.xz[x+1,z];b01=self.xz[x,z+1];b11=self.xz[x+1,z+1]
        c00=self.yz[y,z];c10=self.yz[y+1,z];c01=self.yz[y,z+1];c11=self.yz[y+1,z+1]
        c=a00+b00+c00;bu=a10-a00+b10-b00+self.qx[x];bv=a01-a00+c10-c00+self.qy[y];bw=b01-b00+c01-c00+self.qz[z]
        du=-self.qx[x];dv=-self.qy[y];dw=-self.qz[z];uv=a11-a10-a01+a00;uw=b11-b10-b01+b00;vw=c11-c10-c01+c00
        return torch.stack((c+.5*(bu+bv+bw)+.25*(du+dv+dw+uv+uw+vw),.5*(bu+du)+.25*(uv+uw),.5*(bv+dv)+.25*(uv+vw),.5*(bw+dw)+.25*(uw+vw),.25*du,.25*dv,.25*dw,.25*uv,.25*uw,.25*vw),dim=-1)

    @torch.no_grad()
    def initialize_surface(self,base_heights,origin_y,edge,seed=42):
        # Identical solid/air convention as Rust: density = surface height - Y.
        self.xz.copy_(((base_heights-origin_y)/edge)[:,:,None].expand_as(self.xz))
        y=torch.arange(self.counts[1]+1,dtype=self.xy.dtype,device=self.xy.device)
        self.xy.copy_((-y)[None,:,None].expand_as(self.xy));self.yz.zero_();self.qx.zero_();self.qy.zero_();self.qz.zero_()
        generator=torch.Generator(device=self.xy.device).manual_seed(seed)
        self.xy.add_(torch.randn(self.xy.shape,generator=generator,device=self.xy.device,dtype=self.xy.dtype)*.005)

    @torch.no_grad()
    def initialize_coefficients(self,coeff):
        """Small pilot only: project the already constrained local model to this basis."""
        nx,ny,nz=self.counts
        if nx*ny*nz>64:raise ValueError('Small pilot projection only')
        cells=torch.tensor([(x,y,z) for y in range(ny) for z in range(nz) for x in range(nx)],device=self.xy.device)
        # Each branch has the same linear mapping; build it with one active branch.
        params=list(self.parameters());columns=[]
        for p in params:p.zero_()
        for p in params:
            for index in np.ndindex(*p.shape[:-1]):
                p[index+(0,)]=1.;columns.append(self.coefficients(cells)[:,0,:].reshape(-1).cpu().numpy());p[index+(0,)]=0.
        mapping=np.column_stack(columns);target=np.asarray(coeff).reshape(len(cells),self.branches,10).transpose(0,2,1).reshape(-1,self.branches)
        solution=np.linalg.lstsq(mapping,target,rcond=None)[0];offset=0
        for p in params:
            n=int(np.prod(p.shape[:-1]));p.copy_(torch.tensor(solution[offset:offset+n].reshape(p.shape),dtype=p.dtype,device=p.device));offset+=n
        residual=float(np.max(np.abs(mapping@solution-target)))
        if residual>1e-8:raise ValueError(f'Pilot coefficients outside continuous space: {residual}')
        return residual


def test_model(device='cpu'):
    dtype=torch.float64;torch.manual_seed(23);m=ContinuousQuadratics((2,2,2),2,dtype,device)
    with torch.no_grad():
        for p in m.parameters():p.normal_()
    p=torch.rand((2048,3),device=device,dtype=dtype)*2;cells=torch.floor(p).long();local=2*(p-cells)-1;x,y,z=local.unbind(1)
    basis=torch.stack((torch.ones_like(x),x,y,z,x*x,y*y,z*z,x*y,x*z,y*z),1)
    direct=m(p);baked=(m.coefficients(cells)*basis[:,None,:]).sum(-1);error=float((direct-baked).detach().abs().max())
    assert error<1e-10,error
    jump=0.
    for axis in range(3):
        p=torch.rand((1000,3),device=device,dtype=dtype)*2;p[:,axis]=1.;left=torch.floor(p).long();left[:,axis]=0;right=left.clone();right[:,axis]=1
        for idx in [left,right]:idx.clamp_max_(1)
        ll=2*(p-left)-1;rr=2*(p-right)-1
        def evaluate(c,q):
            x,y,z=q.unbind(1);a=torch.stack((torch.ones_like(x),x,y,z,x*x,y*y,z*z,x*y,x*z,y*z),1);return (c*a[:,None,:]).sum(-1)
        j=float((evaluate(m.coefficients(left),ll)-evaluate(m.coefficients(right),rr)).detach().abs().max());jump=max(jump,j)
    assert jump<1e-10,jump
    # Certify equivalence to the old rank-25 nullspace, not just one example.
    single=ContinuousQuadratics((2,2,2),1,dtype,'cpu');cells=torch.tensor([(x,y,z) for y in range(2) for z in range(2) for x in range(2)])
    cols=[]
    with torch.no_grad():
        for p in single.parameters():
            for index in np.ndindex(*p.shape[:-1]):
                p[index+(0,)]=1.;cols.append(single.coefficients(cells).numpy().reshape(-1));p[index+(0,)]=0.
    mapping=np.column_stack(cols);assert np.linalg.matrix_rank(mapping)==25
    # A numerical directional gradient check for the actual interpolation graph.
    sample=torch.rand((40,3),device=device,dtype=dtype)*2;m.zero_grad();loss=m(sample).square().mean();loss.backward();parameter=m.xy;idx=(0,0,0);analytic=parameter.grad[idx].item();eps=1e-6
    with torch.no_grad():
        parameter[idx]+=eps;a=m(sample).square().mean().item();parameter[idx]-=2*eps;b=m(sample).square().mean().item();parameter[idx]+=eps
    assert abs((a-b)/(2*eps)-analytic)<1e-6
    return dict(device=str(device),coefficient_evaluation_max_error=error,face_max_jump=jump,scalar_space_rank=25,gradient_check=True)

if __name__=='__main__':
    print(test_model())
    if torch.cuda.is_available():print(test_model('cuda'))
