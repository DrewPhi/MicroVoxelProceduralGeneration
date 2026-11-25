"""Do not accept fast unresolved frames or interrupted benchmarks as a pass."""
import unittest

from adaptive_report import summarize


class AdaptiveReportTests(unittest.TestCase):
    def settled(self):
        return {
            'frames': {'sample_count': 100, 'accepted_frames': 100, 'p95_ms': 12},
            'adaptive': {'pixel_error': 4, 'coverage_ready': True, 'quality_ready': True},
            'adaptive_route_quality': {
                'measured_frames': 100, 'coverage_ready_frames': 100,
                'quality_ready_frames': 100, 'unbounded_error_frames': 0,
                'max_finite_visible_error_px': 3.99,
            },
            'benchmark_requested_seconds': 12,
            'route_elapsed_seconds': 12,
        }

    def test_settled_static_numerical_screen(self):
        self.assertTrue(summarize(self.settled())['numerical_screen_pass'])

    def test_cached_home_does_not_excuse_bad_outbound_frames(self):
        report = self.settled()
        report['adaptive_route_quality']['quality_ready_frames'] = 99
        self.assertFalse(summarize(report)['numerical_screen_pass'])

    def test_timeout_and_interruption_cannot_pass(self):
        self.assertFalse(summarize({})['numerical_screen_pass'])
        report = self.settled()
        report['benchmark_interrupted'] = True
        self.assertFalse(summarize(report)['numerical_screen_pass'])
        report['benchmark_interrupted'] = False
        report['route_elapsed_seconds'] = 5
        self.assertFalse(summarize(report)['numerical_screen_pass'])

    def test_rolling_timing_window_does_not_hide_slow_early_frames(self):
        report = self.settled()
        report['frames']['sample_count'] = 50
        self.assertFalse(summarize(report)['numerical_screen_pass'])


if __name__ == '__main__':
    unittest.main()
