from pathlib import Path
import hashlib
import io
import json
import zipfile

root = Path(__file__).resolve().parents[2]
artifacts = root/'artifacts/v7-productization'
base = root/'.tool-cache/convergence-20261029'
old = zipfile.ZipFile(artifacts/'candidate-history/ce9a290b/HLSDownloader-7.0.3-Windows-x64-Portable-candidate.zip')
new = zipfile.ZipFile(artifacts/'candidate/HLSDownloader-7.0.3-Windows-x64-Portable-candidate.zip')
sha = lambda data: hashlib.sha256(data).hexdigest()
components = {}
comparison = []
for name in new.namelist():
    relative = name.removeprefix('HLSDownloader/')
    if relative.startswith('app/resources/') and Path(relative).name in (
        'HLSDownloaderEngine.exe','HLSDownloaderNativeHost.exe','HLSDownloaderPresenter.exe',
        'HLSDownloaderUpdater.exe','ffmpeg.exe','libmpv-2.dll','BUILD-PROVENANCE.json',
    ):
        data = new.read(name)
        components[relative] = sha(data)
        comparison.append({'component':relative, 'old_sha256':sha(old.read(name)), 'new_sha256':sha(data), 'equal':old.read(name)==data})
    if relative.startswith('app/HLSDownloaderDesktop-') and relative.endswith('.jar'):
        components['app/HLSDownloaderDesktop-7.0.3-*.jar'] = sha(new.read(name))
    if relative.startswith('app/resources/extensions/') and relative.endswith('.zip'):
        components[relative] = sha(new.read(name))
        old_zip = zipfile.ZipFile(io.BytesIO(old.read(name)))
        new_zip = zipfile.ZipFile(io.BytesIO(new.read(name)))
        entries = {p:sha(new_zip.read(p)) for p in new_zip.namelist() if not p.endswith('/')}
        old_entries = {p:sha(old_zip.read(p)) for p in old_zip.namelist() if not p.endswith('/')}
        comparison.append({'component':relative,'zip_payload_equal':entries==old_entries,'payload_hashes':entries})
expected = {'msi_sha256':sha((artifacts/'candidate/HLSDownloader-7.0.3-Windows-x64-candidate.msi').read_bytes()),'critical_components':components}
(base/'candidate-queue-runners/expected.json').write_text(json.dumps(expected,indent=2)+'\n',encoding='utf-8')
(base/'candidate-queue-runners/component-comparison.json').write_text(json.dumps(comparison,indent=2)+'\n',encoding='utf-8')
for entry in comparison:
    if not entry['component'].endswith('BUILD-PROVENANCE.json'):
        assert entry.get('equal',entry.get('zip_payload_equal')), entry
print('New MSI',expected['msi_sha256'],'expected installed components',len(components))
print('Core/Host/Presenter/Updater/media and extension payloads unchanged; build provenance differs as expected')
