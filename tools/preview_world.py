#!/usr/bin/env python3
"""Readable top-down diagnostic from native-column heights; never modifies terrain.
Broad hillshade reveals landforms; a subtle fine hillshade retains local detail.
Colors indicate elevation and are not a biome/material simulation.
"""
import argparse,json,hashlib
from pathlib import Path
import numpy as np
from PIL import Image,ImageDraw
p=argparse.ArgumentParser(description=__doc__);p.add_argument('directory',nargs='?',default='output/world-1km');args=p.parse_args();out=Path(args.directory)
m=json.loads((out/'world.json').read_text());w=m['preview']['width'];h=m['preview']['height'];pixel=m['preview']['meters_per_pixel'];height=np.fromfile(out/'surface-heights.f32',dtype='<f4').reshape(h,w).astype(float)
sigma=8./pixel;radius=int(np.ceil(3*sigma));x=np.arange(-radius,radius+1);kernel=np.exp(-.5*(x/sigma)**2);kernel/=kernel.sum()
broad=np.apply_along_axis(lambda row:np.convolve(row,kernel,mode='valid'),1,np.pad(height,((0,0),(radius,radius)),mode='edge'))
broad=np.apply_along_axis(lambda row:np.convolve(row,kernel,mode='valid'),0,np.pad(broad,((radius,radius),(0,0)),mode='edge'))
def shade(values):
 dz,dx=np.gradient(values,pixel);return np.maximum(0.,(.45*dx+.82+.35*dz)/np.sqrt(dx*dx+dz*dz+1.))
t=np.clip((height-height.min())/max(float(np.ptp(height)),1.),0,1);stops=[0.,.45,.75,1.];palette=np.array([[48,85,57],[95,128,65],[145,150,100],[194,188,163]])
color=np.stack([np.interp(t,stops,palette[:,a]) for a in range(3)],axis=-1);light=.38+.62*shade(broad);fine=.92+.08*shade(height)
rgb=np.clip(color*light[:,:,None]*fine[:,:,None],0,255).astype('uint8');Image.fromarray(rgb).save(out/'top-down.png')
poster=Image.new('RGB',(w,h+108),(23,31,42));poster.paste(Image.fromarray(rgb),(0,52));d=ImageDraw.Draw(poster)
d.text((16,12),'FBM AUTHORING REGION | 1 km x 1 km | Seed '+str(m['terrain']['seed']),fill='white');d.text((16,30),'Height tint + hillshade | 6.25 cm native voxels | unfitted 3D terrain',fill=(195,205,215))
y=h+70;d.text((16,y),f"Surface {height.min():.1f} to {height.max():.1f} m | Vertical region {m['bounds_m']['min'][1]:g} to {m['bounds_m']['max'][1]:g} m",fill='white');length=round(100/pixel);d.line((w-length-20,y+8,w-20,y+8),fill='white',width=3);d.text((w-length-20,y+18),'100 m',fill='white');poster.save(out/'overview.png')
m['preview']['color']='Hypsometric elevation tint, 8m-smoothed broad hillshade and subtle native-column hillshade. Diagnostic only; terrain unchanged.'
m['authoring_sources_sha256']={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in [Path('src/terrain.rs'),Path('src/bin/fbm_world.rs')]}
(out/'world.json').write_text(json.dumps(m,indent=2)+'\n')
print(out/'top-down.png')
