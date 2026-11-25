import json
from pathlib import Path
import tempfile
import unittest

import numpy as np
from PIL import Image
from phasea_report import capture_pair, report, MEASUREMENT_KEYS, MATCH_KEYS


class PhaseAReportTests(unittest.TestCase):
    def test_missing_runs_and_unknowns_never_complete(self):
        self.assertFalse(report({}, Path('.'))['evidence_complete'])
        result = report({'runs': [{'id': 'base'}, {'id': 'page', 'compare_to': 'base'}]}, Path('.'))
        self.assertFalse(result['evidence_complete'])
        self.assertIsNone(result['quality_pass'])

    def test_depth_coverage_nonfinite_and_provenance(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            camera = {'eye': [0, 1, 0], 'target': [0, 0, 1], 'near': .025, 'fov_y_degrees': 60}
            side = dict(camera, width=2, height=2, far=300, samples=1, voxel_filter=False,
                        depth_encoding='ndc_0_1', seed=7, source_version=1, terrain_recipe='Legacy',
                        native_voxel_m=.0625, appearance_version=1, horizontal_scale_m=1800,
                        amplitude_m=1000, octaves=8, roughness=.5)
            visual = dict(atmosphere=False, exposure=1, fog_density=0, fog_height_falloff=0,
                          sun_elevation=45, sun_azimuth=90, cloud_coverage=0, cloud_speed=0,
                          overcast=0, render_scale=1)
            side.update(visual=visual, render_appearance_version=1, color_encoding='linear_rgb_8bit')
            match = dict.fromkeys(MATCH_KEYS, 'fixed')
            match.update(source='Legacy', width=2, height=2, far_m=300, seed=7, world_version=1, voxel_m=.0625, camera=camera)
            match.update(lighting={k: v for k, v in visual.items() if k != 'render_scale'},
                         materials=dict(appearance_version=1, render_appearance_version=1),
                         sampling=dict(samples=1, voxel_filter=False, color_encoding='linear_rgb_8bit', render_scale=1))
            runs = []
            for name in ('a', 'b'):
                Image.new('RGB', (2, 2)).save(root / f'{name}.png')
                (root / f'{name}.capture.json').write_text(json.dumps(side))
                np.array([.5, 1, 1, 1], dtype='<f4').tofile(root / f'{name}.f32')
                runs.append(dict(id=name, rgb=f'{name}.png', depth=f'{name}.f32', match=match,
                                 measurements=dict.fromkeys(MEASUREMENT_KEYS, 1)))
            runs[1]['compare_to'] = 'a'
            self.assertTrue(report({'runs': runs}, root)['evidence_complete'])
            for field, value in (('visual', dict(visual, sun_elevation=20)),
                                 ('render_appearance_version', 2), ('voxel_filter', True),
                                 ('samples', 4)):
                changed = dict(side, **{field: value})
                (root / 'b.capture.json').write_text(json.dumps(changed))
                self.assertFalse(report({'runs': runs}, root)['evidence_complete'], field)
            (root / 'b.capture.json').write_text(json.dumps(side))
            benchmark = {'msaa_samples': 1, 'voxel_filter': True}
            (root / 'b.json').write_text(json.dumps(benchmark))
            runs[1]['report'] = 'b.json'
            self.assertFalse(report({'runs': runs}, root)['evidence_complete'])
            runs[1].pop('report')
            changed = dict(side)
            changed.pop('visual')
            (root / 'b.capture.json').write_text(json.dumps(changed))
            self.assertFalse(report({'runs': runs}, root)['evidence_complete'])
            (root / 'b.capture.json').write_text(json.dumps(side))
            np.array([1, .5, 1, 1], dtype='<f4').tofile(root / 'b.f32')
            comparison = capture_pair(*runs, root)
            self.assertEqual(comparison['depth']['coverage_disagreement_pixels'], 2)
            self.assertIsNone(comparison['depth']['max_normalized_error'])
            np.array([float('nan')] * 4, dtype='<f4').tofile(root / 'b.f32')
            self.assertFalse(report({'runs': runs}, root)['evidence_complete'])
            (root / 'b.f32').write_bytes(b'')
            self.assertFalse(report({'runs': runs}, root)['evidence_complete'])
            side.pop('source_version')
            (root / 'b.capture.json').write_text(json.dumps(side))
            with self.assertRaisesRegex(ValueError, 'missing capture provenance'):
                capture_pair(*runs, root)


if __name__ == '__main__':
    unittest.main()
