from pathlib import Path
import json
import numpy as np
from PIL import Image
import subprocess
import sys
import tempfile
import unittest

from adaptive_motion_check import EXPERIMENT_ENV_KEYS
from phasea_iteration_check import (ITERATION_ENV_KEYS, environment, match_metadata,
                                    artifact_diagnostics, float32_rgb_diagnostics)


class IterationCheckTests(unittest.TestCase):
    def test_actual_float32_rgb_buffers_and_unavailable_or_invalid_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertFalse(float32_rgb_diagnostics(root, 2, 1)['available'])
            left = np.array([0, 0, 0, 1, 1, 1], dtype='<f4')
            left.tofile(root / 'baseline.linear_rgb.f32')
            right = left.copy()
            right[3] = .5
            path = root / 'candidate.linear_rgb.f32'
            right.tofile(path)
            compared = float32_rgb_diagnostics(root, 2, 1)['comparison']
            self.assertEqual(compared['max_abs_error'], .5)
            self.assertAlmostEqual(compared['mae'], .5 / 6)
            self.assertAlmostEqual(compared['rmse'], (.25 / 6) ** .5)
            self.assertEqual(compared['differing_pixels'], 1)
            self.assertFalse(compared['byte_equal'])
            left.tofile(path)
            self.assertTrue(float32_rgb_diagnostics(root, 2, 1)['comparison']['byte_equal'])
            path.write_bytes(b'bad')
            with self.assertRaisesRegex(ValueError, 'unexpected float32 RGB size'):
                float32_rgb_diagnostics(root, 2, 1)
            right[0] = float('nan')
            right.tofile(path)
            with self.assertRaisesRegex(ValueError, 'must be finite'):
                float32_rgb_diagnostics(root, 2, 1)

    def test_environment_rejects_untracked_and_duplicate_values(self):
        key = EXPERIMENT_ENV_KEYS[0]
        self.assertEqual(environment([key + '=0']), {key: '0'})
        for entries in (['SECRET_TOKEN=hidden'], [key + '=0', key + '=1'], [key]):
            with self.assertRaises(ValueError):
                environment(entries)

    def test_gpu_profiles_is_an_explicit_tracked_arm_setting(self):
        self.assertIn('FRONTIER_GPU_PROFILES', ITERATION_ENV_KEYS)
        self.assertEqual(environment(['FRONTIER_GPU_PROFILES=1']), {'FRONTIER_GPU_PROFILES': '1'})

    def test_cold_source_codec_is_an_explicit_tracked_arm_setting(self):
        self.assertIn('FRONTIER_SOURCE_COLD_CODEC', EXPERIMENT_ENV_KEYS)
        self.assertEqual(environment(['FRONTIER_SOURCE_COLD_CODEC=lz4']),
                         {'FRONTIER_SOURCE_COLD_CODEC': 'lz4'})

    def test_dry_run_does_not_create_output_or_need_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'untouched'
            process = subprocess.run([sys.executable, str(Path(__file__).with_name('phasea_iteration_check.py')),
                                      '--dry-run', '--output', str(output),
                                      '--baseline-binary', '/absent/baseline',
                                      '--candidate-binary', '/absent/candidate'],
                                     capture_output=True, text=True)
            self.assertEqual(process.returncode, 0, process.stderr)
            self.assertFalse(output.exists())
            self.assertEqual(process.stdout.count('--adaptive-wait-quality'), 2)
            self.assertEqual(process.stdout.count('--msaa 1'), 2)
            self.assertEqual(process.stdout.count('--capture-far 300'), 2)
            headless = subprocess.run([sys.executable, str(Path(__file__).with_name('phasea_iteration_check.py')),
                                       '--dry-run', '--headless', '--output', str(output),
                                       '--baseline-env', 'FRONTIER_INDEXED_PROXY=0',
                                       '--candidate-env', 'FRONTIER_INDEXED_PROXY=1',
                                       '--candidate-env', 'FRONTIER_GPU_PROFILES=1'], capture_output=True, text=True)
            self.assertEqual(headless.returncode, 0, headless.stderr)
            self.assertNotIn('--adaptive-wait-quality', headless.stdout)
            self.assertEqual(headless.stdout.count('--capture-linear'), 2)
            self.assertEqual(headless.stdout.count('FRONTIER_GPU_PROFILES=1'), 1)
            self.assertFalse(output.exists())
            invalid = subprocess.run([sys.executable, str(Path(__file__).with_name('phasea_iteration_check.py')),
                                      '--dry-run', '--headless', '--route', 'fly', '--output', str(output)],
                                     capture_output=True, text=True)
            self.assertNotEqual(invalid.returncode, 0)
            self.assertIn('--headless requires --route static', invalid.stderr)

    def test_partial_headless_provenance_retains_depth_and_pixel_diagnostics(self):
        reference = dict(width=2, height=1, seed=7, eye=[0, 1, 0], target=[0, 0, 0],
                         near=.025, far=300, fov_y_degrees=60,
                         params=dict(horizontal_scale=1800, amplitude=1000, octaves=8, roughness=.5))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('baseline', 'candidate'):
                Image.new('RGB', (2, 1)).save(root / (name + '.png'))
                np.array([.5, 1], dtype='<f4').tofile(root / (name + '.depth.f32'))
                (root / (name + '.capture.json')).write_text(json.dumps(dict(width=2, height=1, eye=[0, 1, 0],
                                                                 target=[1e-7, 0, 0])))
            result = artifact_diagnostics(root, reference)
            self.assertEqual(result['comparison']['different_pixels'], 0)
            self.assertEqual(result['comparison']['depth']['coverage_disagreement_pixels'], 0)
            self.assertTrue(result['unavailable_provenance'])
            self.assertFalse(result['observed_provenance_failures'])
            target = [item for item in result['camera_roundtrip'] if item['field'] == 'target'][0]
            self.assertEqual(target['max_component_difference_m'], 1e-7)
            self.assertEqual(target['tolerance_m'], 1e-6)
            side_path = root / 'candidate.capture.json'
            changed = json.loads(side_path.read_text())
            changed['target'] = [2e-7, 0, 0]
            side_path.write_text(json.dumps(changed))
            motion = artifact_diagnostics(root, reference, motion=True)
            self.assertIsNone(motion['comparison'])
            self.assertFalse(motion['observed_provenance_failures'])
            self.assertIn('unmatched', motion['comparison_unavailable'])
            self.assertIn('capture arms disagree: target',
                          artifact_diagnostics(root, reference)['observed_provenance_failures'])
            changed['target'] = [1e-5, 0, 0]
            side_path.write_text(json.dumps(changed))
            self.assertIn('candidate: capture disagrees with frozen settings: target',
                          artifact_diagnostics(root, reference)['observed_provenance_failures'])
            np.array([float('nan'), 1], dtype='<f4').tofile(root / 'candidate.depth.f32')
            with self.assertRaises(ValueError):
                artifact_diagnostics(root, reference)
        self.assertEqual(match_metadata({}, {}, headless=True)['route']['warmup_frames'], 0)
        self.assertEqual(match_metadata({}, {}, headless=True)['route']['clock'], 'headless_still')

    def test_actual_sidecar_wins_over_conflicting_benchmark(self):
        # The downstream report must see the conflict, not silently use report settings.
        matched = match_metadata({'seed': 7, 'visual': {'exposure': 1}},
                                 {'seed': 8, 'visual': {'exposure': 2}})
        self.assertEqual(matched['seed'], 7)
        self.assertEqual(matched['lighting']['exposure'], 1)
        self.assertIsNone(matched['world_version'])


if __name__ == '__main__':
    unittest.main()
