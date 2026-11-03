"""Rössl et al. 2003, sections 2–3: type-6 quadratic super-spline construction.
65 Bernstein–Bézier controls per cube; shared 27-cell-center input stencil.
Returns total-degree-two coefficients in cube-local [-1,1] coordinates.
Primary source: https://doi.org/10.1109/VISUAL.2003.1250399
No independent per-tetrahedron fit or runtime blending.
"""
import itertools
import numpy as np
from fit import features
CORNERS=list(itertools.product((0,4),repeat=3))
EDGES=((0,1),(1,2),(2,3),(3,0))
SQUARE=((0,0),(4,0),(4,4),(0,4))
TETS=[]
for axis in range(3):
 for side in (0,4):
  axes=[a for a in range(3) if a!=axis];center=np.array([2,2,2]);face=center.copy();face[axis]=side
  corners=[]
  for uv in SQUARE:
   v=face.copy();v[axes]=uv;corners.append(v)
  for a,b in EDGES:TETS.append(np.array([center,face,corners[a],corners[b]]))
TETS=np.array(TETS)
# lambda = [1,x,y,z] @ BARY.T
BARY=np.array([np.linalg.inv(np.column_stack((np.ones(4),v/2-1))).T for v in TETS])

def piece_ids(p):
 p=np.asarray(p);axis=np.argmax(np.abs(p),axis=-1);ids=np.empty(len(p),dtype=int)
 for a in range(3):
  mask=axis==a;u,v=[j for j in range(3) if j!=a];q=p[mask]
  edge=np.where(np.abs(q[:,u])>=np.abs(q[:,v]),np.where(q[:,u]>=0,1,3),np.where(q[:,v]>=0,2,0))
  ids[mask]=a*8+(q[:,a]>=0)*4+edge
 return ids

def construct(key,sample):
 """key XYZ integer cube index; sample returns global cell-center density."""
 key=np.asarray(key,dtype=int);bb={}
 def avg_at(v):
  # For an edge midpoint only the two fixed axes have adjacent cells;
  # a corner has eight. v coordinates are in quarter-cube units.
  indices=[[-1,0] if x==0 else [0,1] if x==4 else [0] for x in v]
  return float(np.mean([sample(tuple(key+np.array(d))) for d in itertools.product(*indices)]))
 for corner in CORNERS:bb[corner]=avg_at(corner) # red, Eq.4
 for axis in range(3):
  for bits in itertools.product((0,4),repeat=2):
   v=np.array([2,2,2]);v[[a for a in range(3) if a!=axis]]=bits
   bb[tuple(v)]=avg_at(v) # blue, Eq.3
 for axis in range(3):
  for side in (0,4):
   face=np.array([2,2,2]);face[axis]=side;axes=[a for a in range(3) if a!=axis];greens=[]
   for bits in itertools.product((0,4),repeat=2):
    vertex=face.copy();vertex[axes]=bits;edges=[]
    for a in axes:
     edge=vertex.copy();edge[a]=2;edges.append(bb[tuple(edge)])
    m=tuple((vertex+face)//2);bb[m]=sum(edges)/2;greens.append(bb[m]) # green, Eq.6
   bb[tuple(face)]=sum(greens)/4 # yellow; either opposite pair has this average
 for corner in CORNERS:
  v=np.array(corner);f0=v.copy();f0[[1,2]]=2;f1=v.copy();f1[[0,2]]=2;e=v.copy();e[2]=2
  bb[tuple((v+2)//2)]=bb[tuple((v+f0)//2)]+bb[tuple((v+f1)//2)]-.5*(bb[corner]+bb[tuple(e)]) # white
 white=sum(bb[tuple((np.array(v)+2)//2)] for v in CORNERS)
 black=0.
 for axis in range(3):
  for side in (0,4):
   face=np.array([2,2,2]);face[axis]=side
   value=sum(bb[tuple((np.array(v)+2)//2)] for v in CORNERS if v[axis]==side)/4
   bb[tuple((face+2)//2)]=value;black+=value # black
 bb[(2,2,2)]=black/3-white/8 # magenta, average of twelve smoothness conditions
 assert len(bb)==65
 coeff=[]
 for vertices,matrix in zip(TETS,BARY):
  b=np.array([[bb[tuple((v+w)//2)] for w in vertices] for v in vertices]);c=matrix.T@b@matrix
  coeff.append([c[0,0],2*c[0,1],2*c[0,2],2*c[0,3],c[1,1],c[2,2],c[3,3],2*c[1,2],2*c[1,3],2*c[2,3]])
 return np.array(coeff)

def evaluate(coeff,p):
 p=np.asarray(p);return np.einsum('ij,ij->i',features(p),coeff[piece_ids(p)])

def gradient(coeff,p):
 p=np.asarray(p);q=coeff[piece_ids(p)];x,y,z=p.T
 return np.column_stack((q[:,1]+2*q[:,4]*x+q[:,7]*y+q[:,8]*z,q[:,2]+2*q[:,5]*y+q[:,7]*x+q[:,9]*z,q[:,3]+2*q[:,6]*z+q[:,8]*x+q[:,9]*y))

def self_test():
 rng=np.random.default_rng(5)
 # Original quasi-interpolant reproduces affine and mixed quadratic terms;
 # it is not an interpolant/reproducer of every pure square term.
 f=lambda p:1+.3*p[0]-.7*p[1]+.2*p[2]+.12*p[0]*p[1]-.2*p[1]*p[2]
 c=construct((0,0,0),lambda k:f(np.array(k)+.5));p=rng.uniform(-1,1,(1000,3))
 assert np.max(np.abs(evaluate(c,p)-np.array([f((v+1)/2) for v in p])))<1e-12
 # All shared internal triangular faces have equal restrictions.
 values={k:float(rng.normal()) for k in itertools.product(range(-1,3),repeat=3)}
 c=construct((0,0,0),values.__getitem__);faces={};worst=0.
 for i,vs in enumerate(TETS):
  for face in itertools.combinations(range(4),3):
   triangle=tuple(sorted(tuple(vs[j]) for j in face))
   if triangle in faces:
    other=faces[triangle];w=rng.dirichlet([1,1,1],100);pts=w@(np.array(triangle)/2-1);a=features(pts)
    worst=max(worst,float(np.max(np.abs(a@(c[i]-c[other])))))
   else:faces[triangle]=i
 assert worst<1e-12
 for axis in range(3):
  key=np.eye(3,dtype=int)[axis];neighbor=construct(key,values.__getitem__);p=rng.uniform(-1,1,(1000,3));p[:,axis]=1;q=p.copy();q[:,axis]=-1
  assert np.max(np.abs(evaluate(c,p)-evaluate(neighbor,q)))<1e-12
  assert np.max(np.abs(gradient(c,p)-gradient(neighbor,q)))<1e-12
 # Every point chosen by the O(1) locator belongs to that tetrahedron.
 p=rng.uniform(-1,1,(10000,3));lam=np.einsum('nij,nj->ni',BARY[piece_ids(p)],np.column_stack((np.ones(len(p)),p)))
 assert np.min(lam)>-1e-12
 return dict(affine_and_mixed_reproduction=True,internal_face_max_jump=worst,cube_face_C0_and_C1=True,locator_points=10000)
if __name__=='__main__':print(self_test())
