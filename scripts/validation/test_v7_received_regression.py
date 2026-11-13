"""Release evidence must never accept another source, candidate or binary."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from validation_workspace import test_tmp
from verify_v7_received_regression import verify


class ReceivedRegressionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=test_tmp())
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.manifest = self.root/'ARTIFACT-MANIFEST.json'
        self.components = {name: str(i)*64 for i, name in enumerate((
            'HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe', 'HLSDownloaderPresenter.exe'), 1)}
        self.write(self.manifest.name, {'source_commit': 'a'*40,
            'components': {f'app/resources/{k}': v for k, v in self.components.items()}})
        self.write('identity.json', {'validation_source_commit': 'a'*40,
            'candidate_source_commit': 'a'*40, 'candidate_run_id': '123',
            'candidate_manifest_sha256': hashlib.sha256(self.manifest.read_bytes()).hexdigest()})
        for browser in ('edge', 'firefox'):
            self.write(f'media-{browser}.json', [{'mode': f'case-{i}'} for i in range(24)])
            self.write(f'takeover-{browser}.json', {'passed': True, 'real_native_host': True,
                'user_tasks_preserved': True, 'components': self.components})
            self.write(f'response-{browser}.json', {'passed': True, 'isolation': {
                'binaries': {k: {'sha256': v} for k, v in self.components.items()}}})

    def write(self, name, data):
        (self.root/name).write_text(json.dumps(data), encoding='utf-8')

    def test_same_candidate_reports_are_accepted(self):
        result = verify('browser', self.manifest, self.root, '123')
        self.assertTrue(result['passed'])
        self.assertEqual(len(result['reports_sha256']), 7)

    def test_source_overlay_is_rejected_even_when_every_check_passed(self):
        path = self.root/'identity.json'
        data = json.loads(path.read_text())
        data['validation_source_commit'] = 'b'*40
        self.write(path.name, data)
        with self.assertRaisesRegex(ValueError, 'Cross-source'):
            verify('browser', self.manifest, self.root, '123')

    def test_another_candidate_run_or_manifest_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'another candidate workflow'):
            verify('browser', self.manifest, self.root, '124')
        self.manifest.write_text(self.manifest.read_text()+'\n')
        with self.assertRaisesRegex(ValueError, 'another candidate manifest'):
            verify('browser', self.manifest, self.root, '123')

    def test_actual_runtime_component_swap_is_rejected(self):
        path = self.root/'takeover-firefox.json'
        data = json.loads(path.read_text())
        data['components']['HLSDownloaderEngine.exe'] = '0'*64
        self.write(path.name, data)
        with self.assertRaisesRegex(ValueError, 'component mismatch'):
            verify('browser', self.manifest, self.root, '123')

    def test_ambiguous_or_failed_report_is_rejected(self):
        nested = self.root/'duplicate'
        nested.mkdir()
        (nested/'identity.json').write_bytes((self.root/'identity.json').read_bytes())
        with self.assertRaisesRegex(ValueError, 'ambiguous'):
            verify('browser', self.manifest, self.root, '123')
        (nested/'identity.json').unlink()
        self.write('response-edge.json', {'passed': False})
        with self.assertRaisesRegex(ValueError, 'did not pass'):
            verify('browser', self.manifest, self.root, '123')

    def test_short_soak_cannot_satisfy_native_release_gate(self):
        engine = self.components['HLSDownloaderEngine.exe']
        presenter = self.components['HLSDownloaderPresenter.exe']
        self.write('workbench.json', {'passed': True, 'components': self.components})
        self.write('native-media.json', {'passed': True, 'binaries': {'engine': engine, 'ui': presenter}})
        self.write('native-frames.json', {'passed': True, 'engine_sha256': engine,
            'presenter_sha256': presenter, 'frame_count': 200, 'frame_p95_ms': 20})
        self.write('range-tail.json', {'passed': True, 'engine_sha256': {'candidate': engine}})
        self.write('sustained-downloads.json', {'passed': True, 'engine_sha256': engine,
            'actual_seconds': 60, 'restarts': [{}], 'downloads': [{}]})
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            verify('native', self.manifest, self.root, '123')


if __name__ == '__main__':
    unittest.main()
