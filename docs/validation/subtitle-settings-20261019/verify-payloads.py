import hashlib,json
from pathlib import Path
root=Path('C:/QAWork/Custom Install/HLSDownloader')
checks=[]
for item in json.loads(Path('C:/HLSQA/expected-payloads.json').read_text(encoding='utf-8-sig')):
    matches=list(root.glob(item['pattern']))
    assert len(matches)==1,(item,matches)
    actual=hashlib.sha256(matches[0].read_bytes()).hexdigest()
    assert actual==item['sha256'],item
    checks.append({'file':str(matches[0].relative_to(root)),'sha256':actual})
files=[f for f in root.rglob('*') if f.is_file()]
report={'passed':True,'payloads':checks,'installed_bytes':sum(f.stat().st_size for f in files),'installed_files':len(files),'source_commit':'3d3de7bf7fb003c1af8e4434c15ea8ccdf35e11a'}
Path('C:/HLSQA/payload-verification.json').write_text(json.dumps(report,indent=2),encoding='utf-8')
print(json.dumps(report))