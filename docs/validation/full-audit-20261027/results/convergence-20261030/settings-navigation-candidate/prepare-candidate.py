from pathlib import Path
import fnmatch,hashlib,json,zipfile
root=Path('artifacts/v7-productization/candidate')
old=Path('artifacts/v7-productization/candidate-history/dbc6759')
report=Path('.tool-cache/ui-settings-navigation-20261030')
sha=lambda b:hashlib.sha256(b).hexdigest()
manifest=json.loads((root/'ARTIFACT-MANIFEST.json').read_text(encoding='utf-8-sig'))
for value in manifest['artifacts'].values():
    assert sha((root/value['path']).read_bytes())==value['sha256'],value['path']
prior=json.loads(Path('.tool-cache/ui-workbench-regression-20261030/runners/expected.json').read_text(encoding='utf-8-sig'))
expected={'msi_sha256':manifest['artifacts']['msi']['sha256'],'critical_components':{}}
with zipfile.ZipFile(root/manifest['artifacts']['portable']['path']) as z:
    for pattern in prior['critical_components']:
        matches=[n for n in z.namelist() if fnmatch.fnmatchcase(n,'HLSDownloader/'+pattern)]
        assert len(matches)==1,(pattern,matches)
        expected['critical_components'][pattern]=sha(z.read(matches[0]))
(report/'runners/expected.json').write_text(json.dumps(expected,indent=2)+'\n',encoding='utf-8')
comparison={}
for family in ('Chromium','Firefox'):
    name=f'extensions/HLSDownloader-7.0.3-{family}.zip'
    with zipfile.ZipFile(old/name) as a,zipfile.ZipFile(root/name) as b:
        files=lambda z:{n:sha(z.read(n)) for n in z.namelist() if not n.endswith('/')}
        af,bf=files(a),files(b)
        comparison[family]={'internal_files_equal':af==bf,'file_count':len(bf),'prior_outer_sha256':sha((old/name).read_bytes()),'current_outer_sha256':sha((root/name).read_bytes()),'files':bf}
comparison['components']={k:{'same_as_previous':v==prior['critical_components'][k],'sha256':v} for k,v in expected['critical_components'].items()}
(report/'results/component-comparison.json').write_text(json.dumps(comparison,indent=2)+'\n',encoding='utf-8')
print(json.dumps({'expected_components':len(expected['critical_components']),'artifacts':manifest['artifacts'],'extensions_equal':{k:comparison[k]['internal_files_equal'] for k in ('Chromium','Firefox')}},indent=2))
