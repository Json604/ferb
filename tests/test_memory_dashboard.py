import json
import threading
import urllib.error
import urllib.request

import pytest
from ferb.memory.dashboard import Dashboard


def test_dashboard_token_origin_and_host_boundaries():
    calls=[]
    d=Dashboard(lambda params: calls.append(params) or {'items':[]})
    t=threading.Thread(target=d.serve,daemon=True);t.start()
    try:
        def request(token, origin, host=None):
            headers={'Content-Type':'application/json','Authorization':'Bearer '+token,'Origin':origin}
            if host:headers['Host']=host
            return urllib.request.urlopen(urllib.request.Request(d.origin+'/api',data=b'{"op":"inventory"}',headers=headers))
        for token,origin,host in [('wrong',d.origin,None),(d.token,'https://evil.example',None),
                                 (d.token,d.origin,'evil.example')]:
            with pytest.raises(urllib.error.HTTPError) as e:request(token,origin,host)
            assert e.value.code==403
        with request(d.token,d.origin) as r:assert json.load(r)=={'items':[]}
        assert calls==[{'op':'inventory'}]
        with urllib.request.urlopen(d.origin) as r:
            assert d.token not in r.read().decode()
            assert 'no-store' in r.headers['Cache-Control']
            assert "frame-ancestors 'none'" in r.headers['Content-Security-Policy']
    finally:d.close();t.join()
