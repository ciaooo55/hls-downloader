import json, sys
from pathlib import Path
from smoke_v7_portable_app import Pipe
p = Pipe()
try:
    response = p.request({'type':'subtitle','request_id':1,'request':{'action':'configuration'}})
finally:
    p.close()
assert response.get('type') == 'subtitle', response
result = response['result']
assert result.get('ok') is True, result
assert 'api_key' not in result
Path(sys.argv[1]).write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
print(json.dumps(result, ensure_ascii=False))