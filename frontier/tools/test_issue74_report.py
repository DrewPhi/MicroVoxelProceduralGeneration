"""Phase 0 provenance and comparison safeguards; never execute a renderer."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from PIL import Image

from issue74_report import aggregate, markdown


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def write(self, path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value))

    def run_fixture(self, name='a', atmosphere=False):
        path = self.root / name / 'static.json'
        data = {
            'scene': 'adaptive', 'seed': 7, 'terrain_recipe': 'Legacy',
            'capture_provenance': {'binary_sha256': 'frozen-binary',
                                   'experiments': {'FRONTIER_NATIVE_PROFILE_REUSE': None}},
            'source': {'version': 1, 'native_voxel_m': .0625},
            'appearance_version': 0, 'render_appearance_version': 1,
            'visual': {'atmosphere': atmosphere, 'render_scale': 1, 'exposure': 1,
                       'menu_enabled': False, 'settings_may_have_changed': False},
            'camera_start': {'position': [0, 1, 0], 'yaw': 0, 'pitch': 0},
            'output': {'width': 2, 'height': 1}, 'msaa_samples': 4,
            'voxel_filter': True, 'route': 'Stationary', 'route_clock': 'realtime',
            'route_speed_mps': 3, 'benchmark_requested_seconds': 5,
            'route_elapsed_seconds': 5, 'warmup': {'frames': 60},
            'adaptive_wait_quality': True, 'adapter': 'test integrated GPU', 'vsync': False,
            'frames': {'sample_count': 10, 'accepted_frames': 10, 'p50_ms': 2, 'p95_ms': 3, 'p99_ms': 4},
            'adaptive': {'pixel_error': 5, 'refinement_reserve': .8,
                         'preparation_workers': 4, 'refinements_per_transaction': 8,
                         'cpu_cache_budget_bytes': 512, 'gpu_pool_budget_bytes': 512,
                         'build_memory_model': {'budget_bytes': 512},
                         'coverage_ready': True, 'quality_ready': True,
                         'upload_budget_bytes_per_frame': 1000,
                         'first_coverage_ms': 1, 'first_quality_ms': 2, 'first_refinement_ms': 3,
                         'exclusive_cpu_profile': {'enabled': False, 'source_generate_ms': 0}},
            'adaptive_route_quality': {'measured_frames': 10, 'coverage_ready_frames': 10,
                                       'quality_ready_frames': 10, 'max_finite_visible_error_px': 4},
        }
        self.write(path, data)
        self.write(path.with_suffix('.capture.json'), {'atmosphere': atmosphere, 'render_scale': 1,
                   'postprocessed': False, 'near': .025, 'far': 300, 'fov_y_degrees': 60})
        command = ['frontier', '--report', str(path), '--scene', 'adaptive']
        self.write(path.parent / 'runs.json', [{'command': command, 'exit_code': 0}])
        return path, data

    def sweep(self, paths):
        self.write(self.root / 'sweep.json', [{'command': ['python', '--output', str(p.parent)],
                   'binary_sha256': 'frozen-binary', 'disk_cache_path': None,
                   'disk_cache_initial_state': 'unknown/not asserted', 'exit_code': 0}
                  for p in paths])

    def test_matching_presets_and_rgb_bytes_with_metadata_provenance(self):
        a, _ = self.run_fixture('a')
        b, _ = self.run_fixture('b')
        self.sweep([a, b])
        pixels = bytes([255, 0, 0, 0, 12, 255])
        Image.frombytes('RGB', (2, 1), pixels).save(a.with_suffix('.png'))
        result = aggregate([self.root, a])
        self.assertEqual(len(result['runs']), 2)
        self.assertEqual(len(result['groups']), 1)
        run = result['runs'][0]
        self.assertTrue(result['groups'][0]['matched_preset'])
        self.assertEqual(run['identity']['binary_sha256'], 'frozen-binary')
        self.assertIsNone(run['identity']['git_commit'])
        self.assertEqual(run['image']['rgb_sha256'], hashlib.sha256(pixels).hexdigest())
        self.assertFalse(run['cpu']['exclusive_profile_measured'])
        self.assertEqual(run['first_readiness_ms']['reserve'], 3)
        self.assertEqual(run['acceptance'], 'not established by this aggregation')

    def test_atmosphere_and_unknown_presets_do_not_compare(self):
        a, _ = self.run_fixture('a')
        b, _ = self.run_fixture('b', atmosphere=True)
        self.sweep([a, b])
        self.assertEqual(len(aggregate([self.root])['groups']), 2)
        a.with_suffix('.capture.json').unlink()
        b.with_suffix('.capture.json').unlink()
        for path in (a, b):
            data = json.loads(path.read_text())
            data.pop('visual')
            self.write(path, data)
        result = aggregate([self.root])
        self.assertEqual(len(result['groups']), 2)
        self.assertTrue(all(not g['matched_preset'] for g in result['groups']))
        self.assertEqual(result['runs'][0]['quality_scope']['profile'], 'unknown')

    def test_failed_and_missing_reports_survive_derived_report_scan(self):
        a, data = self.run_fixture()
        data['benchmark_failed'] = True
        self.write(a, data)
        self.sweep([a])
        absent = self.root / 'failed' / 'static.json'
        self.write(absent.parent / 'runs.json', [{'command': ['frontier', '--report', str(absent)], 'exit_code': 137}])
        self.write(self.root / 'old_summary.json', {'schema': 'issue74_phase0_v1', 'runs': []})
        result = aggregate([self.root])
        self.assertEqual(len(result['runs']), 2)
        self.assertTrue(all(not r['benchmark_numerical_screen']['numerical_screen_pass'] for r in result['runs']))
        self.assertIn('failed/incomplete', markdown(result))
        self.assertTrue(result['runs'][1]['issues'])

    def test_fast_frames_do_not_excuse_unready_route(self):
        a, data = self.run_fixture()
        data['adaptive_route_quality']['quality_ready_frames'] = 9
        self.write(a, data)
        run = aggregate([a])['runs'][0]
        self.assertFalse(run['benchmark_numerical_screen']['numerical_screen_pass'])
        self.assertEqual(run['quality_scope']['visual_temporal_acceptance'], 'not evaluated')

    def test_raw_visual_projection_override_sidecar_and_interaction_is_unfrozen(self):
        a, data = self.run_fixture()
        self.sweep([a])
        data['visual'] = {'atmosphere': True, 'render_scale': .75,
                          'exposure': 2, 'menu_enabled': True,
                          'settings_may_have_changed': True,
                          'gpu_timing_scope': 'terrain_pass_only'}
        data['projection'] = {'fov_y_degrees': 80, 'near_m': .1, 'far_m': 1000}
        data['adaptive']['upload_budget_bytes_per_frame'] = 1234
        self.write(a, data)
        run = aggregate([a])['runs'][0]
        self.assertTrue(run['preset']['atmosphere'])
        self.assertEqual(run['preset']['far'], 1000)
        self.assertEqual(run['preset']['render_scale'], .75)
        self.assertEqual(run['preset']['upload_budget_bytes_per_frame'], 1234)
        self.assertEqual(run['gpu_timing_scope'], 'terrain_pass_only')
        self.assertIn('frozen settings: interactive settings may have changed', run['missing_preset_fields'])

    def test_sweep_failed_before_child_runs_file_still_reported(self):
        self.write(self.root / 'sweep.json', [{'command': ['python', '--output',
                   str(self.root / 'never_started'), '--static-only'], 'exit_code': 1}])
        result = aggregate([self.root])
        self.assertEqual(len(result['runs']), 1)
        self.assertIn('sweep process failed', result['runs'][0]['benchmark_numerical_screen']['failures'])

    def test_experiment_provenance_preserved_but_not_preset_grouped(self):
        a, da = self.run_fixture('a')
        b, db = self.run_fixture('b')
        self.sweep([a, b])
        for path, data, value in ((a, da, '0'), (b, db, '1')):
            data['capture_provenance'] = {'binary_sha256': 'actual-' + value,
                'observed_checkout_commit': 'observed-only',
                'experiments': {'FRONTIER_NATIVE_PROFILE_REUSE': value}}
            self.write(path, data)
        result = aggregate([self.root])
        self.assertEqual(len(result['groups']), 1)
        self.assertEqual(result['runs'][0]['identity']['binary_sha256'], 'actual-0')
        self.assertIsNone(result['runs'][0]['identity']['git_commit'])
        self.assertEqual(result['runs'][1]['experiments']['FRONTIER_NATIVE_PROFILE_REUSE'], '1')

    def test_command_proves_disabled_disk_cache_but_not_enabled_initial_state(self):
        a, _ = self.run_fixture('a')
        run = aggregate([a])['runs'][0]
        self.assertTrue(run['preset']['disk_cache_initial_state'].startswith('disabled'))
        commands = [{'command': ['frontier', '--report', str(a), '--adaptive-disk-cache', '/cache']}]
        self.write(a.parent / 'runs.json', commands)
        run = aggregate([a])['runs'][0]
        self.assertIsNone(run['preset']['disk_cache_initial_state'])
        self.assertIn('disk_cache_initial_state', run['missing_preset_fields'])
        (a.parent / 'runs.json').unlink()
        run = aggregate([a])['runs'][0]
        self.assertIsNone(run['preset']['disk_cache_initial_state'])

    def test_missing_upload_budget_and_experiments_prevent_comparison(self):
        paths = []
        for name in ('a', 'b'):
            path, data = self.run_fixture(name)
            data['adaptive'].pop('upload_budget_bytes_per_frame')
            data.pop('capture_provenance')
            self.write(path, data)
            paths.append(path)
        self.sweep(paths)
        result = aggregate([self.root])
        self.assertEqual(len(result['groups']), 2)
        for run in result['runs']:
            self.assertIn('upload_budget_bytes_per_frame', run['missing_preset_fields'])
            self.assertIn('experiment environment provenance', run['missing_preset_fields'])

    def test_unreadable_static_report_is_not_dropped(self):
        path = self.root / 'static.json'
        path.write_text('{broken')
        result = aggregate([self.root])
        self.assertEqual(len(result['runs']), 1)
        self.assertTrue(result['diagnostics'])
        self.assertTrue(result['runs'][0]['issues'])


if __name__ == '__main__':
    unittest.main()
