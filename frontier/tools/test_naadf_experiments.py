import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

from naadf_experiments import compare, environment


class ExperimentTests(unittest.TestCase):
    def test_environment_removes_inherited_experiment_flags(self):
        with patch.dict(os.environ, {'FRONTIER_NAADF_CAPTURE_LAYER': 'steps',
                                    'FRONTIER_NAADF_CACHE_NODES': '1', 'DISPLAY': ':0'}):
            env = environment({'FRONTIER_NAADF_CACHE_NODES': '0'})
        self.assertNotIn('FRONTIER_NAADF_CAPTURE_LAYER', env)
        self.assertEqual(env['FRONTIER_NAADF_CACHE_NODES'], '0')
        self.assertEqual(env['DISPLAY'], ':0')

    def test_rejects_non_experiment_configuration(self):
        with self.assertRaises(ValueError):
            environment({'HOME': '/tmp'})

    def test_comparison_rejects_camera_mismatch_and_detects_depth_change(self):
        with tempfile.TemporaryDirectory() as directory:
            a, b = (Path(directory) / n for n in ('a.png', 'b.png'))
            meta = dict(width=1, height=1, eye=[0, 1, 0], target=[0, 1, 1],
                        fov_y_degrees=60, near=.025, far=100, terrain_recipe='alpine',
                        native_voxel_m=.0625, naadf={'world_bounds_m': [[-1]*3, [1]*3]})
            for stem in (a, b):
                stem.with_suffix('.capture.json').write_text(json.dumps(meta))
                np.array([1, 2, 3], '<f4').tofile(stem.with_suffix('.linear_rgb.f32'))
                np.array([.5], '<f4').tofile(stem.with_suffix('.depth.f32'))
            self.assertTrue(compare(a, b)['depth']['equal'])
            np.array([.6], '<f4').tofile(b.with_suffix('.depth.f32'))
            self.assertFalse(compare(a, b)['depth']['equal'])
            meta['vegetation'] = {'triangles': 42}
            b.with_suffix('.capture.json').write_text(json.dumps(meta))
            with self.assertRaisesRegex(ValueError, 'unmatched vegetation triangles'):
                compare(a, b)
            del meta['vegetation']
            meta['eye'] = [1, 1, 0]
            b.with_suffix('.capture.json').write_text(json.dumps(meta))
            with self.assertRaises(ValueError):
                compare(a, b)


if __name__ == '__main__':
    unittest.main()
