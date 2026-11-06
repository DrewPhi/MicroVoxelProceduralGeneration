"""Capture identity is observational and selected experiments never expose all env."""
import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from adaptive_motion_check import capture_provenance


class ProvenanceTests(unittest.TestCase):
    def test_actual_binary_digest_and_allowlisted_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / 'binary'
            binary.write_bytes(b'actual executable bytes')
            with patch('adaptive_motion_check.subprocess.run') as run:
                run.side_effect = [subprocess.CompletedProcess([], 0, 'abc123\n', ''),
                                   subprocess.CompletedProcess([], 0, ' M source.rs\n', '')]
                result = capture_provenance(binary, root=directory, environ={
                    'FRONTIER_NATIVE_PROFILE_REUSE': '1', 'SECRET_TOKEN': 'secret',
                    'FRONTIER_UNKNOWN_SECRET': 'secret'})
            self.assertEqual(result['binary_sha256'], hashlib.sha256(binary.read_bytes()).hexdigest())
            self.assertEqual(result['observed_checkout_commit'], 'abc123')
            self.assertTrue(result['observed_checkout_dirty'])
            self.assertIn('not a guaranteed binary build revision', result['checkout_identity_note'])
            self.assertEqual(result['experiments']['FRONTIER_NATIVE_PROFILE_REUSE'], '1')
            self.assertNotIn('secret', str(result))
            self.assertTrue(result['timestamp_utc'].endswith('+00:00'))

    def test_unavailable_binary_and_git_remain_unknown(self):
        with patch('adaptive_motion_check.subprocess.run', side_effect=OSError('missing git')):
            result = capture_provenance('/nonexistent/frontier-binary', environ={})
        self.assertIsNone(result['binary_sha256'])
        self.assertIsNone(result['observed_checkout_commit'])
        self.assertIsNone(result['observed_checkout_dirty'])
        self.assertIsNone(result['experiments']['FRONTIER_NATIVE_PROFILE_REUSE'])


if __name__ == '__main__':
    unittest.main()
