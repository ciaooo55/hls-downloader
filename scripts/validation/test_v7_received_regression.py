"""Release evidence must never accept another source, candidate or binary."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from zipfile import ZipFile

from validation_workspace import test_tmp
from verify_v7_received_regression import MEDIA_MODES, candidate_media_identity, verify


class ReceivedRegressionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=test_tmp())
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.manifest = self.root/'ARTIFACT-MANIFEST.json'
        self.components = {name: str(i)*64 for i, name in enumerate((
            'HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe', 'HLSDownloaderPresenter.exe'), 1)}
        self.extensions = {family: {'path': f'{family}.zip', 'sha256': str(i)*64}
                           for i, family in enumerate(('Chromium', 'Firefox'), 4)}
        self.write(self.manifest.name, {'source_commit': 'a'*40,
            'extensions': self.extensions,
            'components': {f'app/resources/{k}': v for k, v in self.components.items()}})
        self.write('identity.json', {'validation_source_commit': 'a'*40,
            'candidate_source_commit': 'a'*40, 'candidate_run_id': '123',
            'candidate_manifest_sha256': hashlib.sha256(self.manifest.read_bytes()).hexdigest()})
        for browser, family in (('edge', 'Chromium'), ('firefox', 'Firefox')):
            self.write(f'media-{browser}.json', {'passed': True, 'browser': browser,
                'cases': [{'mode': mode, 'passed': True} for mode in MEDIA_MODES],
                'components': self.components, 'extension_sha256': self.extensions[family]['sha256']})
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

    def test_failed_or_unmarked_media_case_is_rejected(self):
        for browser in ('edge', 'firefox'):
            name = f'media-{browser}.json'
            original = json.loads((self.root/name).read_text())
            for status in (False, None, 1):
                with self.subTest(browser=browser, status=status):
                    report = json.loads(json.dumps(original))
                    case = report['cases'][0]
                    if status is None:
                        del case['passed']
                    else:
                        case['passed'] = status
                    self.write(name, report)
                    with self.assertRaisesRegex(ValueError, f'Media case did not pass: {browser}'):
                        verify('browser', self.manifest, self.root, '123')
            self.write(name, original)

    def test_missing_media_mode_is_rejected_even_with_24_rows(self):
        name = 'media-edge.json'
        report = json.loads((self.root/name).read_text())
        report['cases'].pop()
        self.write(name, report)
        with self.assertRaisesRegex(ValueError, 'Media matrix incomplete'):
            verify('browser', self.manifest, self.root, '123')
        report['cases'].append(report['cases'][0])
        self.assertEqual(len(report['cases']), 24)
        self.write(name, report)
        with self.assertRaisesRegex(ValueError, 'Media matrix incomplete'):
            verify('browser', self.manifest, self.root, '123')

    def test_legacy_and_unsuccessful_media_reports_are_rejected(self):
        name = 'media-edge.json'
        report = json.loads((self.root/name).read_text())
        for invalid in (report['cases'], {**report, 'passed': False}):
            self.write(name, invalid)
            with self.assertRaisesRegex(ValueError, 'Media regression did not pass'):
                verify('browser', self.manifest, self.root, '123')

    def test_media_component_hash_mismatch_is_rejected(self):
        name = 'media-firefox.json'
        report = json.loads((self.root/name).read_text())
        report['components']['HLSDownloaderEngine.exe'] = '0'*64
        self.write(name, report)
        with self.assertRaisesRegex(ValueError, 'Media component mismatch: firefox/HLSDownloaderEngine.exe'):
            verify('browser', self.manifest, self.root, '123')

    def test_media_extension_hash_mismatch_is_rejected(self):
        name = 'media-firefox.json'
        report = json.loads((self.root/name).read_text())
        report['extension_sha256'] = self.extensions['Chromium']['sha256']
        self.write(name, report)
        with self.assertRaisesRegex(ValueError, 'Media extension mismatch: firefox'):
            verify('browser', self.manifest, self.root, '123')

    def test_media_identity_hashes_actual_files_and_rejects_a_different_build(self):
        runtime = self.root/'runtime'
        resources = runtime/'app'/'resources'
        resources.mkdir(parents=True)
        for name in self.components:
            (resources/name).write_bytes(name.encode())
        extension = self.root/'extension'
        extension.mkdir()
        (extension/'manifest.json').write_bytes(b'{}')
        archive_path = self.root/self.extensions['Chromium']['path']
        with ZipFile(archive_path, 'w') as archive:
            archive.writestr('manifest.json', b'{}')
        identity = candidate_media_identity(self.manifest, runtime, extension, 'edge')
        self.assertEqual(identity['components'], {
            name: hashlib.sha256(name.encode()).hexdigest() for name in self.components})
        self.assertEqual(identity['extension_sha256'], hashlib.sha256(archive_path.read_bytes()).hexdigest())
        (extension/'manifest.json').write_bytes(b'{"version":"other"}')
        with self.assertRaisesRegex(ValueError, 'Media extension differs from candidate archive'):
            candidate_media_identity(self.manifest, runtime, extension, 'edge')

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
        self.write('public-https.json', {'passed': True, 'engine_sha256': engine, 'source_commit': 'a'*40})
        for name in ('protocols', 'media-download', 'media-live', 'media-audio-failure', 'media-audio-segment'):
            self.write(name+'.json', {'passed': True, 'engine_sha256': engine})
        self.write('media-track-progress.json', {'passed': True, 'components': {'app/resources/HLSDownloaderEngine.exe': engine}})
        self.write('native-frames.json', {'passed': True, 'engine_sha256': engine,
            'presenter_sha256': presenter, 'frame_count': 200, 'frame_p95_ms': 20})
        self.write('range-tail.json', {'passed': True, 'engine_sha256': {'candidate': engine}})
        self.write('sustained-downloads.json', {'passed': True, 'engine_sha256': engine,
            'actual_seconds': 60, 'restarts': [{}], 'downloads': [{}]})
        with self.assertRaisesRegex(ValueError, 'incomplete'):
            verify('native', self.manifest, self.root, '123')


if __name__ == '__main__':
    unittest.main()
