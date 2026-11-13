"""Check authenticated hosted-run reports against one frozen candidate.

receive-v7-candidate.ps1 authenticates the workflow, source and artifact ZIP
digest before invoking this checker. Local JSON alone is not remote provenance.
"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


def verify(kind, manifest_path, evidence, candidate_run_id=None):
    manifest = json.loads(manifest_path.read_text(encoding='utf-8-sig'))
    files = {}

    def read(name):
        paths = list(evidence.rglob(name))
        if len(paths) != 1:
            raise ValueError(f'Missing or ambiguous report: {name}')
        files[name] = hashlib.sha256(paths[0].read_bytes()).hexdigest()
        return json.loads(paths[0].read_text(encoding='utf-8-sig'))

    identity = read('identity.json')
    for field in ('validation_source_commit', 'candidate_source_commit'):
        if identity.get(field) != manifest['source_commit']:
            raise ValueError(f'Cross-source regression is not release evidence: {field}')
    digest = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
    if identity.get('candidate_manifest_sha256') != digest:
        raise ValueError('Regression used another candidate manifest')
    if candidate_run_id is not None and str(identity.get('candidate_run_id')) != str(candidate_run_id):
        raise ValueError('Regression used another candidate workflow run')

    def passed(name):
        report = read(name)
        if report.get('passed') is not True:
            raise ValueError(f'Regression did not pass: {name}')
        return report

    components = {Path(name).name: digest for name, digest in manifest['components'].items()}
    if kind == 'browser':
        for browser in ('edge', 'firefox'):
            media = read(f'media-{browser}.json')
            if not isinstance(media, list) or len(media) < 24:
                raise ValueError(f'Media matrix incomplete: {browser}')
            response = passed(f'response-{browser}.json')
            takeover = passed(f'takeover-{browser}.json')
            if not takeover.get('real_native_host') or not takeover.get('user_tasks_preserved'):
                raise ValueError(f'Actual runtime handoff not established: {browser}')
            for name in ('HLSDownloaderEngine.exe', 'HLSDownloaderNativeHost.exe', 'HLSDownloaderPresenter.exe'):
                if takeover['components'].get(name) != components[name]:
                    raise ValueError(f'Takeover component mismatch: {browser}/{name}')
                if response['isolation']['binaries'][name]['sha256'] != components[name]:
                    raise ValueError(f'Response component mismatch: {browser}/{name}')
    elif kind == 'native':
        workbench = passed('workbench.json')
        media = passed('native-media.json')
        https = passed('public-https.json')
        protocols = passed('protocols.json')
        decoded_media = [passed(name+'.json') for name in (
            'media-download', 'media-live', 'media-audio-failure', 'media-audio-segment')]
        tracks = passed('media-track-progress.json')
        frames = passed('native-frames.json')
        tail = passed('range-tail.json')
        soak = passed('sustained-downloads.json')
        if workbench['components'].get('HLSDownloaderEngine.exe') != components['HLSDownloaderEngine.exe'] or workbench['components'].get('HLSDownloaderPresenter.exe') != components['HLSDownloaderPresenter.exe']:
            raise ValueError('Workbench report used other components')
        for report in (frames, soak, https, protocols, *decoded_media):
            if report.get('engine_sha256') != components['HLSDownloaderEngine.exe']:
                raise ValueError('Native report used another Core')
        if tracks['components']['app/resources/HLSDownloaderEngine.exe'] != components['HLSDownloaderEngine.exe']:
            raise ValueError('Media progress report used another Core')
        if https.get('source_commit') != manifest['source_commit']:
            raise ValueError('Public HTTPS report used another frozen source')
        if frames.get('presenter_sha256') != components['HLSDownloaderPresenter.exe']:
            raise ValueError('Native report used another Presenter')
        if media['binaries']['engine'] != components['HLSDownloaderEngine.exe'] or media['binaries']['ui'] != components['HLSDownloaderPresenter.exe']:
            raise ValueError('Native media report used other components')
        if tail['engine_sha256']['candidate'] != components['HLSDownloaderEngine.exe']:
            raise ValueError('Range-tail report used another Core')
        if soak.get('actual_seconds', 0) < 1800 or not soak.get('restarts') or not soak.get('downloads'):
            raise ValueError('Sustained populated download run is incomplete')
        if frames.get('frame_p95_ms', float('inf')) > 33 or frames.get('frame_count', 0) < 100:
            raise ValueError('Native frame threshold failed')
        if {item['slint_scale_factor'] for item in frames.get('layouts', [])} != {1, 1.25, 1.5, 2}:
            raise ValueError('Native scale/layout acceptance is incomplete')
    else:
        raise ValueError(f'Unknown regression kind: {kind}')
    return {'schema': 1, 'passed': True, 'kind': kind, 'source_commit': manifest['source_commit'],
            'candidate_manifest_sha256': digest, 'reports_sha256': files}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--kind', choices=('browser', 'native'), required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--candidate-run-id')
    args = parser.parse_args()
    print(json.dumps(verify(args.kind, args.manifest, args.evidence, args.candidate_run_id)), flush=True)


if __name__ == '__main__':
    main()
