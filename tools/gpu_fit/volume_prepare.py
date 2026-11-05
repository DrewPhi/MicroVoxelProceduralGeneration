"""Build the shared density-node cache with checksums and world provenance."""
import argparse,hashlib,json,subprocess
from pathlib import Path
from prepare import PARAMS
from train import write_json

def main(a):
    world=json.loads(Path(a.world).read_text());out=Path(a.output);out.mkdir(parents=True,exist_ok=True)
    path=out/'nodes.f32';metadata=path.with_suffix('.json')
    sha=hashlib.sha256(Path(a.sampler).read_bytes()).hexdigest()
    if metadata.exists():
        old=json.loads(metadata.read_text())
        if old['world']!=world or old['sampler_sha256']!=sha:raise ValueError('Incompatible node cache')
        if hashlib.sha256(path.read_bytes()).hexdigest()!=old['sha256']:raise ValueError('Node checksum mismatch')
        return
    p=world['terrain'];values=[p[key] for key in PARAMS]+world['bounds_m']['min']+world['dimensions']+[world['proposed_patch_edge_m']]
    params=out/'nodes.params';params.write_text('\n'.join(map(str,values))+'\n')
    subprocess.run([a.sampler,str(params),str(path)],check=True)
    counts=[round(v/world['proposed_patch_edge_m']) for v in world['extent_m']]
    expected=4
    for n in counts:expected*=2*n+1
    if path.stat().st_size!=expected:raise ValueError('Wrong node payload size')
    write_json(metadata,dict(world=world,sampler_sha256=sha,sha256=hashlib.sha256(path.read_bytes()).hexdigest(),bytes=expected,node_order='y,z,x',values='authoritative density / patch_edge_m at half-cell grid nodes'))

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    for key in ['world','output','sampler']:p.add_argument('--'+key,required=True)
    main(p.parse_args())
