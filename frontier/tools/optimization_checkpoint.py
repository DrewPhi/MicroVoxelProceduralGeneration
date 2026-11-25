#!/usr/bin/env python3
"""Repeatable 1080p optimization acceptance routes. Sequential, reversed A/B runs.
Three physical environments plus stop/resume. Primary detail, lighting, seed and
resolution remain matched; teleport is deliberately outside this checkpoint.
Defaults to a plan. --run measures without recording; record motion separately.
"""
import argparse
import json
from pathlib import Path
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[1]
CASES={
    'meadow': ('-41,-250','walk-circuit',45),
    'vegetation': ('-57,-47','walk-circuit',45),
    'mountain': ('0,3000','walk-circuit',45),
    'stop-resume': ('-41,-250','walk-stop-resume',12),
}
def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--binary',type=Path,default=ROOT/'target/release/frontier')
    p.add_argument('--config',type=Path,default=ROOT/'reference/naadf-next/exact.json')
    p.add_argument('--output',type=Path,required=True)
    p.add_argument('--cases',nargs='+',choices=CASES,default=list(CASES))
    p.add_argument('--radius',type=int,default=64)
    p.add_argument('--repeat',type=int,default=2)
    p.add_argument('--run',action='store_true')
    a=p.parse_args();a.output.mkdir(parents=True,exist_ok=False)
    manifest={'scope':__doc__,'cases':{}}
    for name in a.cases:
        xz,route,seconds=CASES[name]
        cmd=[sys.executable,str(ROOT/'tools/naadf_experiments.py'),'--binary',str(a.binary.resolve()),
            '--config',str(a.config.resolve()),'--output',str((a.output/name).resolve()),
            '--radius',str(a.radius),'--repeat',str(a.repeat),'--route',route,'--seconds',str(seconds),f'--start-xz={xz}']
        if a.run:cmd+=['--run']
        print(name,flush=True)
        result=subprocess.run(cmd,cwd=ROOT)
        manifest['cases'][name]={'command':cmd,'returncode':result.returncode}
        (a.output/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
        if result.returncode:raise SystemExit(result.returncode)
if __name__=='__main__':main()
