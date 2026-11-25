import json
import contextlib
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import numpy as np

from naadf_experiments import UnmatchedAdaptivePartition, compare, environment, main


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

    def test_timing_rejects_stage_profiling_before_launch_or_output(self):
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'config.json'
            output = Path(directory) / 'must-not-be-created'
            config.write_text(json.dumps({'candidate': {'FRONTIER_NAADF_GPU_STAGES': '1'}}))
            with patch('sys.argv', ['naadf_experiments', '--config', str(config),
                                    '--output', str(output)]):
                with contextlib.redirect_stderr(io.StringIO()):
                    with self.assertRaises(SystemExit) as error:
                        main()
            self.assertEqual(error.exception.code, 2)
            self.assertFalse(output.exists())

    def test_partition_mismatch_remains_failed_and_retains_exact_pixel_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            binary = directory / 'binary'
            binary.write_bytes(b'fake binary; subprocess mocked')
            config = directory / 'variants.json'
            config.write_text(json.dumps({'control': {}, 'candidate': {}}))
            output = directory / 'captures'
            def capture(command, **kwargs):
                frame = Path(command[command.index('--capture') + 1])
                candidate = frame.parent.name.endswith('candidate')
                meta = dict(width=1, height=1, eye=[0, 1, 0], target=[0, 1, 1],
                    fov_y_degrees=60, near=.025, far=100, terrain_recipe='alpine',
                    native_voxel_m=.0625, naadf={'world_bounds_m': [[-1]*3, [1]*3]},
                    adaptive={'visible_clusters': 3 if candidate else 2,
                              'triangles': 6 if candidate else 4,
                              'gpu_occluded_clusters': 0 if candidate else 1})
                frame.with_suffix('.capture.json').write_text(json.dumps(meta))
                np.array([1, 2, 3.01 if candidate else 3], '<f4').tofile(frame.with_suffix('.linear_rgb.f32'))
                np.array([.6 if candidate else .5], '<f4').tofile(frame.with_suffix('.depth.f32'))
                return type('Result', (), {'returncode': 0})()
            with patch('sys.argv', ['naadf_experiments', '--config', str(config),
                        '--output', str(output), '--binary', str(binary), '--mode', 'capture',
                        '--capture-size', '1,1', '--allow-color-error', '--run']), \
                 patch('naadf_experiments.subprocess.run', side_effect=capture):
                self.assertEqual(main(), 1)
            result = json.loads((output / 'summary.json').read_text())['runs'][1]
            self.assertEqual(result['comparison_classification'], 'unmatched_adaptive_partition')
            self.assertEqual(result['adaptive_partition_differences']['triangles'],
                             {'control': 4, 'candidate': 6})
            self.assertEqual(result['comparison_to_first']['linear_rgb']['different_values'], 1)
            self.assertEqual(result['comparison_to_first']['depth']['different_values'], 1)
            self.assertIn('unmatched adaptive partition', result['error'])
            # A geometry mismatch stays unmatched even when its image happens to
            # be equal; color-error allowances never waive geometry invariants.
            a, b = (output / ('0-' + name) / 'frame.png' for name in ('control', 'candidate'))
            for kind in ('linear_rgb', 'depth'):
                b.with_suffix(f'.{kind}.f32').write_bytes(a.with_suffix(f'.{kind}.f32').read_bytes())
            with self.assertRaises(UnmatchedAdaptivePartition) as error:
                compare(a, b)
            self.assertTrue(error.exception.comparison['depth']['equal'])

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
