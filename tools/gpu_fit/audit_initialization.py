"""Cheap offline diagnostics; does not train or change the renderer model.
Requires a prepared probe dataset and an exported shared-parameters.npz.
"""
import argparse,json
from pathlib import Path
import numpy as np
import torch
from model import ContinuousQuadratics
from train import balanced_loss,load_dataset
p=argparse.ArgumentParser()
p.add_argument('--dataset',required=True);p.add_argument('--parameters',required=True)
p.add_argument('--output',required=True);p.add_argument('--no-caves-dataset');a=p.parse_args()
torch.set_num_threads(4)
meta=json.loads((Path(a.dataset)/'dataset.json').read_text());world=meta['world'];edge=world['proposed_patch_edge_m']
counts=np.rint(np.array(world['extent_m'])/edge).astype(int).tolist()
params=np.load(a.parameters);branches=params['xy'].shape[-1]
m=ContinuousQuadratics(counts,branches);m.load_state_dict({k:torch.from_numpy(params[k]) for k in params.files})
positions,density,_=load_dataset(a.dataset,'cpu',edge)
with torch.no_grad():
 q=m(positions);target=density>0
 metrics=[]
 for scale in [1.,.1,.01,.001,0.]:
  scaled=q*scale;pred=scaled.amin(1)>0
  metrics.append(dict(scale=scale,loss=float(balanced_loss(scaled,density)),error_fraction=float((pred!=target).float().mean()),false_air=int((target&~pred).sum()),false_solid=int((~target&pred).sum())))
 active=torch.bincount(q[~target].argmin(1),minlength=branches).tolist()
 # Every branch is a sum of terms each missing one coordinate. Its
 # alternating 8-corner difference is zero even across patch boundaries.
 low=torch.tensor([[13.2,5.3,18.4],[52.1,22.3,64.1],[90.7,35.5,93.2]])
 high=low+torch.tensor([3.1,2.7,4.3])
 mixed=torch.zeros((len(low),branches))
 for bits in range(8):
  b=torch.tensor([(bits>>i)&1 for i in range(3)])
  mixed+=((-1.)**(3-int(b.sum())))*m(torch.where(b.bool(),high,low))
 layer_points=torch.tensor([[13.5,2.,3.],[13.5,20.,80.]])
 before=m(layer_points).clone();m.qx[13,0]+=1.;after=m(layer_points);m.qx[13,0]-=1.
 slab_effect=(after-before)[:,0].tolist()
m.zero_grad();loss=balanced_loss(m(positions[:8192]),density[:8192]);loss.backward()
result=dict(samples=len(positions),counts=counts,branches=branches,parameters=sum(v.numel() for v in m.parameters()),independent_scalar_dimension_per_branch=sum((counts[i]+1)*(counts[j]+1) for i,j in [(0,1),(0,2),(1,2)])-2,
 scale_sweep=metrics,negative_active_branch_counts=active,initial_gradient_norm=float(torch.sqrt(sum(v.grad.square().sum() for v in m.parameters()))),mixed_corner_max_abs_per_branch=mixed.abs().amax(0).tolist(),one_qx_parameter_effect_at_two_distant_yz_points=slab_effect,
 note='Scaling by a positive constant preserves occupancy; low hinge loss alone does not establish a better boundary. Nonzero gradients exclude a completely disconnected graph, not other optimizer bugs. Zero mixed differences apply to individual branches, not generally their minimum.')
if a.no_caves_dataset:
    pp,dd,_=load_dataset(a.no_caves_dataset,'cpu',edge)
    assert torch.equal(pp,positions), 'Ablation must use identical sample coordinates'
    pred=q.amin(1)>0;removed=(dd>0)&~target;bad=pred!=target
    result['cave_ablation']=dict(no_cave_error_fraction=float((pred!=(dd>0)).float().mean()),
        cave_removed_points=int(removed.sum()),full_errors_in_removed_caves=int((bad&removed).sum()),
        full_errors_outside_removed_caves=int((bad&~removed).sum()))
Path(a.output).write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
