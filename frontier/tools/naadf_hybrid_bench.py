#!/usr/bin/env python3
"""Matched fixed-region visibility comparison. Prints plans unless --run.

White Lambert shading excludes material/lighting differences. Both backends
request native terrain and plant detail to twice the NAADF cube half-extent,
so its corners cannot receive a cheaper mesh LOD. Each run has a fresh derived
cache; measurement starts only after requested quality/publication is ready.
RAM capacities are soft independent ceilings; actual memory pressure still
stops background work. Do not interpret their sum as committed RAM.
"""
import argparse, hashlib, json, os, shlex, subprocess, time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def available_mib():
    return next(int(line.split()[1])//1024 for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))
def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=ROOT/'target/release/frontier')
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--radii',type=int,nargs='+',default=[8,16,32])
    p.add_argument('--seconds',type=float,default=8.)
    p.add_argument('--run',action='store_true')
    a=p.parse_args()
    if any(r<1 or r>64 for r in a.radii):p.error('matched native-radius comparison supports1..64m')
    output=a.output.resolve();output.mkdir(parents=True,exist_ok=True)
    if (output/'manifest.json').exists():p.error('use a new output directory to preserve measurements')
    ram=available_mib();capacity=max(256,ram-max(1024,ram//5));plant_gpu=max(64,min(512,capacity//3))
    workers=max(1,min(16,(os.cpu_count() or 2)-2))
    results={};manifest={'binary_sha256':hashlib.sha256(a.binary.read_bytes()).hexdigest(),'available_ram_mib_at_plan':ram,'soft_cpu_capacity_mib':capacity,'plant_gpu_capacity_mib':plant_gpu,'workers':workers,'scope':__doc__,'cases':results}
    for radius in a.radii:
        for backend in ['mesh','naadf']:
            name=f'{backend}-r{radius}';report=output/(name+'.json');png=output/(name+'.png')
            command=[str(a.binary.resolve()),'--preset','alpine','--seed','7','--capture-far','300','--pixel-error','5','--msaa','1','--width','1920','--height','1080','--visibility-benchmark','--beauty','off','--atmosphere','off','--terrain-shadows','off','--walk','off','--naadf-radius',str(radius),'--naadf-follow','off','--naadf','on' if backend=='naadf' else 'off','--native-radius',str(radius*2),'--vegetation-near',str(radius*2),'--resource-profile','custom','--terrain-cache-mib',str(capacity),'--build-memory-mib',str(capacity),'--terrain-gpu-mib',str(capacity),'--plant-cpu-mib',str(capacity),'--plant-gpu-mib',str(plant_gpu),'--triangle-budget','16000000','--adaptive-workers',str(workers),'--adaptive-refinements','32','--upload-mib','16','--adaptive-disk-cache',str(output/(name+'-cache')),'--adaptive-disk-mib','256','--adaptive-wait-quality','--benchmark-input-lock','--uncapped','--warmup','30','--seconds',str(a.seconds),'--route','static','--report',str(report),'--capture-on-exit',str(png)]
            print(shlex.join(command),flush=True)
            entry={'command':command,'status':'planned'};results[name]=entry
            if a.run:
                start=time.monotonic()
                with (output/(name+'.log')).open('w') as log:
                    try:code=subprocess.run(command,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT,timeout=240).returncode
                    except subprocess.TimeoutExpired:code=None
                entry.update(status='ok' if code==0 else 'failed',returncode=code,wall_seconds=time.monotonic()-start)
                if report.exists():
                    d=json.loads(report.read_text())
                    entry['measurements']={k:d.get(k) for k in ['frames','gpu_full_render','naadf_gpu_pass','process_memory','naadf','vegetation','adaptive_route_quality','camera_start','projection','source','development_resources']}
                    entry['terrain']={k:d.get('adaptive',{}).get(k) for k in ['quality_ready','coverage_ready','naadf_owned_tiles','naadf_ownership_id','triangles','resident_clusters','gpu_resident_bytes','gpu_allocated_bytes','cpu_cache_bytes','total_build_ms','generated_columns']}
                (output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
                if code!=0:
                    print(f'{name} failed; inspect its log. No lower-quality result is accepted.',flush=True)
                    return
    (output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
if __name__=='__main__':main()
