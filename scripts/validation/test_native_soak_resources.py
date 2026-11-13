"""Protect comparable resource checkpoints without weakening the leak budget."""
import unittest

from soak_native_downloads import resource_growth


class SoakResourceTests(unittest.TestCase):
    def sample(self, state, handles, pid=1, rss=24):
        return {'state': state, 'handles': handles, 'pid': pid, 'rss_mib': rss}

    def test_active_workers_are_recorded_without_becoming_an_idle_leak(self):
        samples = [self.sample('idle', 215), self.sample('active', 243),
                   self.sample('idle', 214), self.sample('idle', 215, pid=2),
                   self.sample('active', 242, pid=2), self.sample('idle', 216, pid=2)]
        result = resource_growth(samples)
        self.assertEqual([r['peak_handles'] for r in result], [243, 242])
        self.assertEqual([r['peak_idle_handle_growth'] for r in result], [0, 1])

    def test_idle_growth_over_sixteen_is_rejected_even_if_final_sample_recovers(self):
        with self.assertRaises(AssertionError):
            resource_growth([self.sample('idle', 215), self.sample('idle', 232),
                             self.sample('idle', 215)])

    def test_peak_memory_is_checked_during_active_transfers(self):
        with self.assertRaises(AssertionError):
            resource_growth([self.sample('idle', 215), self.sample('active', 240, rss=281),
                             self.sample('idle', 215)])

    def test_restart_cannot_hide_missing_idle_checkpoints(self):
        with self.assertRaisesRegex(AssertionError, 'PID 2'):
            resource_growth([self.sample('idle', 215), self.sample('idle', 215),
                             self.sample('idle', 215, pid=2), self.sample('active', 238, pid=2)])


if __name__ == '__main__':
    unittest.main()
