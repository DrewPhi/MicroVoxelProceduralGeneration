"""GPU, source parity, constrained-basis and exported-model gate for the big job."""
import json,sys
from pathlib import Path
import numpy as np
import torch
from model import test_model
root=Path(sys.argv[1]);fixtures=Path(__file__).parent/'fixtures';checks=[test_model('cpu'),test_model('cuda')]
a=np.fromfile(root/'pilot-train/samples.bin',dtype='<u4',count=2048*4);b=np.fromfile(fixtures/'source-parity.bin',dtype='<u4')
if not np.array_equal(a,b):raise RuntimeError('Portable Rust source/record parity check failed')
metrics=json.loads((root/'pilot-result/metrics.json').read_text())
if metrics['validation']['all']['fraction']>.05:raise RuntimeError('Pilot sampled error exceeds 5%; investigate GPU implementation before large fit')
if metrics['seams']['occupancy_disagreements']!=0:raise RuntimeError('Pilot seam signs disagree')
if metrics['seams']['baked_direct_max_error']>1e-4:raise RuntimeError('Pilot baked coefficient mismatch')
(root/'preflight-passed.json').write_text(json.dumps(dict(checks=checks,pilot=metrics,source_parity_records=2048),indent=2))
print('GPU PREFLIGHT PASSED',flush=True)
