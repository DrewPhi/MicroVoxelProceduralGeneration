#!/usr/bin/env python3
"""Compare matched MSAA1 fixed-camera native-depth captures inside NAADF's cube.

The independent GPU ray oracle remains the correctness check. This comparison
measures composed mesh/ray differences; depth32 quantization is explicitly
budgeted, and a 25cm boundary band is excluded and reported, never hidden.
"""
import argparse,json
from pathlib import Path
import numpy as np
from PIL import Image

def compare(mesh,hybrid):
    metadata=json.loads(mesh.with_suffix('.capture.json').read_text());other=json.loads(hybrid.with_suffix('.capture.json').read_text())
    for k in ['width','height','eye','target','fov_y_degrees','near','far','samples']:
        if metadata[k]!=other[k]:raise ValueError('unmatched '+k)
    if metadata['samples']!=1:raise ValueError('single-sample depth required')
    h,w=metadata['height'],metadata['width']
    a=np.fromfile(mesh.with_suffix('.depth.f32'),dtype='<f4').reshape(h,w)
    b=np.fromfile(hybrid.with_suffix('.depth.f32'),dtype='<f4').reshape(h,w)
    eye=np.array(metadata['eye']);forward=np.array(metadata['target'])-eye;forward/=np.linalg.norm(forward)
    right=np.cross(forward,[0,1,0]);right/=np.linalg.norm(right);up=np.cross(right,forward)
    y,x=np.nonzero(a<1.)
    tangent=np.tan(np.radians(metadata['fov_y_degrees'])/2)
    direction=forward+((x+.5)/w*2-1)[:,None]*tangent*w/h*right+(1-(y+.5)/h*2)[:,None]*tangent*up
    direction/=np.linalg.norm(direction,axis=1)[:,None]
    cosine=direction@forward;n,f=metadata['near'],metadata['far']
    distance=n*f/(f-a[y,x].astype(float)*(f-n))/cosine
    point=eye+direction*distance[:,None]
    naadf=other.get('naadf')
    if naadf is None:
        naadf=json.loads(hybrid.with_suffix('.json').read_text())['naadf']
    bounds=np.array(naadf['world_bounds_m'])
    core=np.all((point>=bounds[0]+.25)&(point<bounds[1]-.25),axis=1)
    y,x,cosine,distance=y[core],x[core],cosine[core],distance[core]
    actual=n*f/(f-b[y,x].astype(float)*(f-n))/cosine
    error=np.abs(actual-distance)
    tolerance=.001+2*np.spacing(a[y,x]).astype(float)*(distance*cosine)**2*(f-n)/(n*f)/cosine
    colors=np.asarray(Image.open(mesh).convert('RGB')).astype(np.int16)
    candidate=np.asarray(Image.open(hybrid).convert('RGB')).astype(np.int16)
    delta=np.max(np.abs(colors[y,x]-candidate[y,x]),axis=1)
    return {'core_pixels':len(y),'excluded_boundary_band_m':.25,'depth_quantization_allowance':'1mm plus two depth32 ULPs converted to ray distance','depth_outside_allowance_pixels':int(np.count_nonzero(error>tolerance)),'depth_error_m_p50_p95_p99_max':[float(v)for v in np.quantile(error,[.5,.95,.99,1])],'rgb_difference_over_one_code_pixels':int(np.count_nonzero(delta>1)),'rgb_code_difference_p50_p95_p99_max':[float(v)for v in np.quantile(delta,[.5,.95,.99,1])]}
def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('mesh',type=Path);p.add_argument('hybrid',type=Path);p.add_argument('--output',type=Path);a=p.parse_args();result=compare(a.mesh,a.hybrid);text=json.dumps(result,indent=2)+'\n';print(text)
    if a.output:a.output.write_text(text)
if __name__=='__main__':main()
