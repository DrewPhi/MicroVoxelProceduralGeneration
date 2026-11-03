"""Prepare bounded authoritative Rust samples; no exhaustive voxel volume export."""
import argparse,hashlib,json,subprocess
from pathlib import Path

PARAMS=['seed','world_m','ceiling_m','voxel_cm','base_m','relief_m','wavelength_m','octaves','gain','lacunarity','overhang_m','rock_scale_m','cave_scale_m','cave_width','cave_roof_m']
def prepare(world_path,out,sampler,count,seed):
    world=json.loads(Path(world_path).read_text());out=Path(out);out.mkdir(parents=True,exist_ok=True)
    config=dict(world=world,samples=count,seed=seed,sampler_sha256=hashlib.sha256(Path(sampler).read_bytes()).hexdigest())
    signature=hashlib.sha256(json.dumps(config,sort_keys=True).encode()).hexdigest()
    if (out/'dataset.json').exists():
        old=json.loads((out/'dataset.json').read_text())
        if old.get('signature')==signature and (out/'samples.bin').stat().st_size==count*16:return old
        raise ValueError(f'Refusing incompatible cached dataset: {out}')
    p=world['terrain'];values=[p[k] for k in PARAMS]+world['bounds_m']['min']+world['dimensions']+[world.get('proposed_patch_edge_m',8.)]
    (out/'sampler.params').write_text('\n'.join(map(str,values))+'\n')
    subprocess.run([sampler,str(out/'sampler.params'),str(out),str(count),str(seed)],check=True)
    if (out/'samples.bin').stat().st_size!=count*16:raise ValueError('Truncated Rust samples')
    result=dict(config,signature=signature,record_bytes=16,record_layout='u32 X,Y,Z region-relative native indices; f32 authoritative density; little endian',sampling='2/3 uniform volume, 1/3 near base surface +/- (overhang+24)m; native centers',exact_native_certification=False)
    (out/'dataset.json').write_text(json.dumps(result,indent=2)+'\n');return result
if __name__=='__main__':
    a=argparse.ArgumentParser();a.add_argument('--world',required=True);a.add_argument('--output',required=True);a.add_argument('--sampler',required=True);a.add_argument('--samples',type=int,required=True);a.add_argument('--seed',type=int,required=True);p=a.parse_args();prepare(p.world,p.output,p.sampler,p.samples,p.seed)
