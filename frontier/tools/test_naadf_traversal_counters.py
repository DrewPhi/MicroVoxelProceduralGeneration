import unittest
from naadf_traversal_counters import summarize


class CountersTest(unittest.TestCase):
    def metadata(self):
        primary = [0] * 32
        primary[0] = 10
        primary[24] = 10
        primary[7] = 5
        primary[9] = primary[10] = 2
        primary[11] = 3
        primary[15] = 17
        primary[16] = 1
        return {'naadf_traversal_diagnostics': {
            'instrumented': True, 'valid_for_timing': False, 'scope': 'fixture',
            'distance_units': 'native cells', 'primary': primary,
            'shadow': [0] * 32, 'beam': [0] * 32}}

    def test_wide_distance_and_absent_ratios(self):
        result = summarize(self.metadata())
        self.assertEqual(result['primary']['saved_native_cells'], (1 << 32) + 17)
        self.assertEqual(result['primary']['height_success_fraction'], 0.4)
        self.assertIsNone(result['shadow']['height_success_fraction'])

    def test_rejects_timing_or_incomplete_counters(self):
        m = self.metadata()
        m['naadf_traversal_diagnostics']['valid_for_timing'] = True
        with self.assertRaises(ValueError):
            summarize(m)
        m = self.metadata()
        m['naadf_traversal_diagnostics']['primary'][0] += 1
        with self.assertRaises(ValueError):
            summarize(m)


if __name__ == '__main__':
    unittest.main()
