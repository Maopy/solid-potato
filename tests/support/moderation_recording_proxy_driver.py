"""Composite proxy -> authenticated local recording runtime, no platform traffic."""
import asyncio,json,sys,types,time
from pathlib import Path
class Response:
    def __init__(self,status,body,headers=None):self.status_code=status;self.raw_content=body;self.headers=headers or {};self.stream=False
    @staticmethod
    def make(status,body,headers=None):return Response(status,body,headers)
    def get_text(self,strict=False):return self.raw_content.decode()
    def set_text(self,text):self.raw_content=text.encode()
sys.modules['mitmproxy']=types.SimpleNamespace(http=types.SimpleNamespace(HTTPFlow=object,Response=Response))
ns={'__name__':'recording_fixture'}
exec(compile(Path(sys.argv[1]).read_text(),sys.argv[1],'exec'),ns)
def flow(host,path,referer=None,body=None,method='POST'):
    request=types.SimpleNamespace(host=host,scheme='https',path=path,pretty_url='https://'+host+path,method=method,headers={'referer':referer or 'https://'+host+('/123' if host=='live.douyin.com' else '/web/pages/live'),'origin':'https://'+host,'content-type':'application/json'},raw_content=json.dumps(body or {}).encode())
    request.get_text=lambda strict=False:request.raw_content.decode()
    return types.SimpleNamespace(metadata={},request=request,response=None)
async def call(host,path,body=None,method='POST',referer=None):
    f=flow(host,path,referer,body,method);await ns['request'](f);assert f.response is not None;return f.response
async def state(id):
    r=await call('live.douyin.com','/api/v1/download_task/detail?id='+id,method='GET');return json.loads(r.raw_content)['data']
async def wait(id,status):
    for _ in range(100):
        s=await state(id)
        if s['status']==status:return s
        await asyncio.sleep(.1)
    raise AssertionError((id,status,s))
async def main():
    for host,path in [('channels.weixin.qq.com','/web/pages/live'),('live.douyin.com','/123')]:
        f=flow(host,path,method='GET');f.response=Response(200,b'<html><head><script nonce="fixture-nonce">x</script></head><body></body></html>',{'content-type':'text/html'})
        ns['requestheaders'](f);ns['responseheaders'](f);ns['response'](f)
        html=f.response.get_text();assert '/__live_archive/moderation/' in html
        assert ('/__assets/inject/channels.live.js' if host=='channels.weixin.qq.com' else '/__live_archive/recording.js') in html
        if host=='channels.weixin.qq.com':assert '<script nonce="fixture-nonce">window.__d_config=' in html
        r=await call(host,('/__assets/inject/channels.live.js' if host=='channels.weixin.qq.com' else '/__live_archive/recording.js'),method='GET');assert r.status_code==200
    # Existing moderation bootstrap remains routable through the same dispatcher.
    r=await call('live.douyin.com','/__live_archive/moderation/v2/tasks/dy/bootstrap.js',method='GET');assert r.status_code==200
    assert 'keyword_replies' in r.get_text()
    r=await call('live.douyin.com','/api/v1/download_task/list',method='GET',referer='https://evil.test/123');assert r.status_code==403
    body={'objects':[{'platform':'douyin','content':{'profile':{'room_url':'https://live.douyin.com/999','nickname':'wrong'},'live':{'stream_url':'https://media.example.test/dy.flv'}}}]}
    r=await call('live.douyin.com','/api/v1/download_task/create',body);assert r.status_code==422
    body['objects'][0]['content']['profile']={'room_url':'https://live.douyin.com/123','nickname':'抖音主播'}
    r=await call('live.douyin.com','/api/v1/download_task/create',body);assert r.status_code==200
    dy=json.loads(r.raw_content)['data']['tasks'][0]['data']['id'];await wait(dy,'downloading')
    wxbody={'objects':[{'platform':'wxchannels','content':{'profile':{'username':'wx-host','nickname':'视频号主播'},'live':{'liveSdkInfo':{'liveCdnUrl':'https://media.example.test/wx.flv'}}}}]}
    r=await call('channels.weixin.qq.com','/api/v1/download_task/create',wxbody);assert r.status_code==200
    wx=json.loads(r.raw_content)['data']['tasks'][0]['data']['id'];await wait(wx,'downloading')
    # Let both fake encoders produce segments before pausing one.
    await asyncio.sleep(.8)
    r=await call('live.douyin.com','/api/v1/download_task/pause',{'task_ids':[dy]});assert r.status_code==200
    await wait(dy,'paused');await asyncio.sleep(.8)
    assert (await state(wx))['status']=='downloading'
    r=await call('live.douyin.com','/api/v1/download_task/start',{'task_ids':[dy]});assert r.status_code==200
    await wait(dy,'downloading');await asyncio.sleep(.8)
    # Removing an active task stops it but preserves already recorded files.
    r=await call('channels.weixin.qq.com','/api/v1/download_task/delete',{'task_ids':[wx]});assert r.status_code==200
    await asyncio.sleep(.8)
    r=await call('live.douyin.com','/api/v1/download_task/list',method='GET');ids=[t['id'] for t in json.loads(r.raw_content)['data']['list']];assert wx not in ids and dy in ids
    # A failed finalization must be visible and keep its TS for recovery.
    wxbody['objects'][0]['content']['profile']['username']='wx-bad'
    wxbody['objects'][0]['content']['live']['liveSdkInfo']['liveCdnUrl']='https://media.example.test/fail.flv'
    r=await call('channels.weixin.qq.com','/api/v1/download_task/create',wxbody);assert r.status_code==200
    bad=json.loads(r.raw_content)['data']['tasks'][0]['data']['id'];await wait(bad,'downloading');await asyncio.sleep(.8)
    await call('channels.weixin.qq.com','/api/v1/download_task/pause',{'task_ids':[bad]})
    s=await wait(bad,'failed');assert s['error'] and 'MP4' in s['error']
    await call('channels.weixin.qq.com','/api/v1/download_task/delete',{'task_ids':[bad]})
asyncio.run(main())
