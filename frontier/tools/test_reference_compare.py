"""Synthetic tests for independent reference/GPU comparison classification."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import numpy as np
from reference_compare import compare, same_camera, same_world


class ComparisonTests(unittest.TestCase):
    def fixture(self, root):
        ref = root/'reference'; ref.mkdir()
        gpu = root/'gpu.png'
        m = dict(width=3,height=2,eye=[0.,0.,0.],target=[0.,0.,1.],
                 fov_y_degrees=60.,near=.025,far=20000.,spp=64,seed=7,
                 source_version=1,edit_revision=0,native_min_cell=[-128,-32,-128],
                 native_size_cells=[256,224,256],sample_seed=17,sampling='stratified')
        c = dict(m,samples=1,scene='fixture',color_encoding='linear_rgb_float32_render_target')
        (ref/'manifest.json').write_text(json.dumps(m))
        gpu.with_suffix('.capture.json').write_text(json.dumps(c))
        np.zeros((2,3,3),'<f4').tofile(ref/'linear_rgb.f32')
        np.zeros((2,3,3),'<f4').tofile(gpu.with_suffix('.linear_rgb.f32'))
        coverage = np.array([[1,0,1],[1,.5,0]],'<f4')
        coverage.tofile(ref/'coverage.f32')
        depth = np.full((2,3,4),5,dtype='<f4'); depth[:,:,3]=0
        depth[coverage==0]=np.nan
        depth.tofile(ref/'depth.f32')
        # Invert ray-distance formula to encode known center-ray distances.
        t = np.tan(np.deg2rad(60)/2)
        x=((np.arange(3)+.5)*2/3-1)*t*3/2
        y=(1-(np.arange(2)+.5)*2/2)*t
        distance=np.array([[5.,5.,20000.],[10.,5.,20000.]])
        z=distance/np.sqrt(1+y[:,None]**2+x[None,:]**2)
        ndc=(20000.-.025*20000./z)/(20000.-.025)
        ndc[0,2]=1; ndc[1,2]=1
        ndc.astype('<f4').tofile(gpu.with_suffix('.depth.f32'))
        return ref,gpu,m,c

    def test_geometry_classification(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ref,gpu,_,_=self.fixture(root)
            report=compare(ref,gpu,root/'comparison')
            g=report['geometry']
            self.assertEqual(g['false_coverage_pixels'],1)
            self.assertEqual(g['missing_coverage_pixels'],1)
            self.assertEqual(g['reference_partial_coverage_pixels'],1)
            self.assertEqual(g['depth_outside_reference_range_pixels'],1)
            self.assertTrue((root/'comparison'/'side_by_side.png').exists())

    def test_pixel_center_geometry_and_stale_world(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ref,gpu,m,_=self.fixture(root)
            center=dict(m)
            (ref/'center_manifest.json').write_text(json.dumps(center))
            np.array([[1,0,1],[1,1,0]],'<f4').tofile(ref/'center_coverage.f32')
            np.full((2,3),5,dtype='<f4').tofile(ref/'center_depth.f32')
            report=compare(ref,gpu,root/'comparison')
            geometry=report['geometry']['pixel_center']
            self.assertEqual(geometry['false_coverage_pixels'],1)
            self.assertEqual(geometry['missing_coverage_pixels'],1)
            self.assertEqual(geometry['depth_error_above_tolerance_pixels'],1)
            center['seed']=9
            (ref/'center_manifest.json').write_text(json.dumps(center))
            with self.assertRaisesRegex(ValueError,'world mismatch: seed'):
                compare(ref,gpu,root/'comparison')

    def test_camera_and_world_rejection(self):
        with tempfile.TemporaryDirectory() as tmp:
            _,_,m,c=self.fixture(Path(tmp))
            for field,value in [('eye',[1,0,0]),('fov_y_degrees',45),('width',4)]:
                other=copy.deepcopy(c);other[field]=value
                with self.assertRaises(ValueError):same_camera(m,other)
            for field,value in [('seed',8),('source_version',2),('edit_revision',1),('native_size_cells',[1,1,1])]:
                other=copy.deepcopy(c);other[field]=value
                with self.assertRaises(ValueError):same_world(m,other)

    def test_convergence_rejects_different_world(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);ref,gpu,m,_=self.fixture(root)
            other=root/'other';other.mkdir();m['seed']=8
            (other/'manifest.json').write_text(json.dumps(m))
            with self.assertRaisesRegex(ValueError,'world mismatch: seed'):
                compare(ref,gpu,root/'comparison',other)

    def test_msaa_appearance_does_not_weaken_geometry_or_camera_validation(self):
        with tempfile.TemporaryDirectory() as tmp:
            _,_,m,c=self.fixture(Path(tmp))
            c['samples']=4
            with self.assertRaisesRegex(ValueError, 'requires --msaa 1'):
                same_camera(m,c)
            same_camera(m,c,require_single_sample=False)
            c['eye']=[1,0,0]
            with self.assertRaisesRegex(ValueError, 'camera mismatch'):
                same_camera(m,c,require_single_sample=False)


if __name__ == '__main__':
    unittest.main()
