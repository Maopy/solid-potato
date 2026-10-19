"""Exercise the shipped proxy request handler and real local TCP bridge."""
import asyncio
import json
import sys
import types
from pathlib import Path

class Response:
    @staticmethod
    def make(status, body, headers=None):
        return types.SimpleNamespace(status_code=status, raw_content=body, headers=headers or {})

http = types.SimpleNamespace(HTTPFlow=object, Response=Response)
sys.modules['mitmproxy'] = types.SimpleNamespace(http=http)
addon, bootstrap = sys.argv[1:]
namespace = {'__name__': 'fixture_proxy'}
exec(compile(Path(addon).read_text(), addon, 'exec'), namespace)
boot = json.loads(Path(bootstrap).read_text())
origin = 'https://channels.weixin.qq.com'
room = origin + '/web/pages/live'
def flow(endpoint, body, token='', request_origin=origin):
    req = types.SimpleNamespace(host='channels.weixin.qq.com', scheme='https',
        path='/__live_archive/moderation/v1/' + endpoint, method='POST',
        pretty_url=room, raw_content=json.dumps(body).encode(),
        headers={'referer':room, 'origin':request_origin, 'content-type':'application/json', 'authorization':'Bearer '+token})
    return types.SimpleNamespace(request=req, response=None)
async def main():
    hello = flow('hello',dict(v=1,ticket=boot['ticket'],page_url=room,document_id='proxy-fixture',adapter_id=boot['adapter_id']))
    await namespace['request'](hello)
    assert hello.response.status_code==200
    pairing=json.loads(hello.response.raw_content)
    body=dict(v=1,connection_id=pairing['connection_id'],binding_id=pairing['binding_id'],chunks=[dict(seq=1,capture_id='capture',captured_at_ms=1,channel='native_bridge',category='dispatch',encoding='json_utf8_base64',part=0,parts=1,data='e30=')])
    denied=flow('raw',body,pairing['token'],'https://unrelated.example')
    await namespace['request'](denied)
    assert denied.response.status_code==403
    for _ in range(2):
        request=flow('raw',body,pairing['token'])
        await namespace['request'](request)
        assert request.response.status_code==200, request.response.status_code
        assert json.loads(request.response.raw_content)['acked_seq']==1
    delivery=flow('delivery',dict(v=1,connection_id=pairing['connection_id'],binding_id=pairing['binding_id'],page_url=room,action_id='missing-action',state='unconfirmed',source='timeout'),pairing['token'])
    await namespace['request'](delivery)
    assert delivery.response.status_code==422  # forwarded to the bridge, no fill session
    unknown=flow('unknown',body,pairing['token'])
    await namespace['request'](unknown)
    assert unknown.response.status_code==403
asyncio.run(main())
