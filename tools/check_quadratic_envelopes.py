"""Small adversarial scene: compare grid/air/envelopes and independent CPU hits.
Run: python3 tools/check_quadratic_envelopes.py [--binary target/release/analytic]
Generated fixtures, captures and summary remain under ignored output/envelopes.
"""
import argparse
import json
import math
import pathlib
import subprocess
from PIL import Image, ImageChops

parser = argparse.ArgumentParser()
parser.add_argument('--binary', default='target/release/analytic')
args = parser.parse_args()
out = pathlib.Path('output/envelopes/verification')
out.mkdir(parents=True, exist_ok=True)
grid = [3, 5, 2]
coefficients = []
for y in range(grid[1]):
    layer = []
    for z in range(grid[2]):
        row = []
        for x in range(grid[0]):
            # Sloped curved ground, air above, and spherical cavity branch.
            ground = [0.9-y*.5+math.sin(x+z)*.2, .2, -1., .15, .08, -.02, .1, .03, -.1, .02]
            cave = [-.25, 0., 0., 0., 1., 1., 1., 0., 0., 0.]
            row.append([ground, cave])
        layer.append(row)
    coefficients.append(layer)
scene = dict(version=1, representation='min-of-implicit-quadrics', solid='min(Q)>0',
             feature_order=['1','x','y','z','xx','yy','zz','xy','xz','yz'],
             patch_order='y,z,x', patches_per_axis=grid, quadratics_per_patch=2,
             patch_edge_m=2., origin_cells=[0,0,0], voxel_m=.125,
             dimensions=[v*16 for v in grid],
             normalization='local = 2*(world_m-patch_min_m)/patch_edge_m - 1',
             coefficients=coefficients)
scene_path = out/'scene.json'
scene_path.write_text(json.dumps(scene))
cameras = ['-2,4,2,0,-15', '8,3,2,180,-10', '3,12,2,0,-89',
           '3,0.1,2,45,20', '-1,3.5,-1,45,0', '2,2,2,-135,1']
reports = []
for mode in ['shaded', 'voxels']:
    for i, camera in enumerate(cameras):
        images = []
        for traversal in ['grid', 'hierarchy', 'envelopes']:
            path = out/f'{mode}-{i}-{traversal}.png'
            command = [args.binary, '--scene', str(scene_path), '--headless',
                       '--camera', camera, '--mode', mode, '--size', '320x240',
                       '--frames', '2', '--verify', '--traversal', traversal,
                       '--capture', str(path)]
            result = subprocess.run(command, capture_output=True, text=True)
            if result.returncode:
                raise RuntimeError(' '.join(command)+'\n'+result.stdout+result.stderr)
            report = json.loads(path.with_suffix('.json').read_text())
            assert report['verification']['mismatches'] == 0
            reports.append(report)
            images.append(Image.open(path).convert('RGB'))
        for image in images[1:]:
            difference = ImageChops.difference(images[0], image)
            maximum = max(high for low, high in difference.getextrema())
            assert maximum <= 2, (mode, i, maximum)
        print(mode, i, 'CPU hits and full-image comparisons passed', flush=True)
(out/'summary.json').write_text(json.dumps(reports, indent=2))
print('All checks passed; reports:', out/'summary.json', flush=True)
