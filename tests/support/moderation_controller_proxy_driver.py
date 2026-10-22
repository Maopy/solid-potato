"""Run shared proxy dispatch against real independent local HTTP bridges."""
import asyncio,json,sys,types,subprocess,tempfile
from pathlib import Path
class Response:
    def __init__(self,status,body,headers=None):self.status_code=status;self.raw_content=body;self.headers=headers or {};self.stream=False
    @staticmethod
    def make(status,body,headers=None):return Response(status,body,headers)
    def get_text(self,strict=False):return self.raw_content.decode()
    def set_text(self,text):self.raw_content=text.encode()
sys.modules['mitmproxy']=types.SimpleNamespace(http=types.SimpleNamespace(HTTPFlow=object,Response=Response))
ns={'__name__':'controller_fixture'}
exec(compile(Path(sys.argv[1]).read_text(),sys.argv[1],'exec'),ns)
def flow(host,path,referer,body=None,token='',origin=None,method='POST'):
    return types.SimpleNamespace(metadata={},request=types.SimpleNamespace(host=host,scheme='https',path=path,pretty_url='https://'+host+path,method=method,headers={'referer':referer,'origin':origin or 'https://'+host,'content-type':'application/json','authorization':'Bearer '+token},raw_content=json.dumps(body or {}).encode()),response=None)
async def call(route,endpoint,body=None,token='',method='POST'):
    f=flow(route['host'],route['prefix']+endpoint,'https://'+route['host']+route['path'],body,token,method=method)
    await ns['request'](f);return f.response
async def main():
    routes=ns['ROUTES'];pairs={}
    # HTML injection and loader route are shared, retaining CSP nonce.
    for host in ['channels.weixin.qq.com','live.douyin.com']:
        r=next(r for r in routes if r['host']==host);url='https://'+host+r['path']
        f=flow(host,r['path'],url,method='GET');f.response=Response(200,b'<html><head><script nonce="fixture-nonce">x</script></head><body></body></html>',{'content-type':'text/html'})
        ns['requestheaders'](f);ns['responseheaders'](f);assert not f.response.stream;ns['response'](f)
        assert '/__live_archive/moderation/v2/bootstrap.js' in f.response.get_text()
        f=flow(host,'/__live_archive/moderation/v2/bootstrap.js',url,method='GET');await ns['request'](f);assert f.response.status_code==200
        js=f.response.get_text();choices=json.loads(js.split('const choices=')[1].split(';')[0]);assert len(choices)==(3 if host=='channels.weixin.qq.com' else 1)
        with tempfile.TemporaryDirectory() as td:
            script=Path(td)/'loader.js';script.write_text(js)
            test=Path(__file__).with_name('moderation_task_picker_driver.cjs')
            out=subprocess.run(['node',str(test),str(script),host],capture_output=True,text=True)
            assert out.returncode==0,out.stderr
            if host == 'channels.weixin.qq.com':
                script.write_bytes(ns['loader']([r]))
                out=subprocess.run(['node',str(test),str(script),host],capture_output=True,text=True)
                assert out.returncode==0,out.stderr
    for r in routes:
        response=await call(r,'bootstrap.js',method='GET');assert response.status_code==200
        boot=json.loads(response.get_text().split('const BOOT = ')[1].split(';')[0]);assert boot['prefix']==r['prefix']
        body=dict(v=1,ticket=boot['ticket'],page_url='https://'+r['host']+r['path'],document_id='document-'+r['id'],adapter_id=boot['adapter_id'])
        response=await call(r,'hello',body);assert response.status_code==200;pairs[r['id']]=json.loads(response.raw_content)
    for r in routes:
        pair=pairs[r['id']]
        body=dict(v=1,connection_id=pair['connection_id'],binding_id=pair['binding_id'],chunks=[dict(seq=1,capture_id='same-id',captured_at_ms=1,channel='native_bridge',category='dispatch',encoding='json_utf8_base64',part=0,parts=1,data='e30=')])
        response=await call(r,'raw',body,pair['token']);assert response.status_code==200
    a=next(r for r in routes if r['id']=='a');b=next(r for r in routes if r['id']=='b');p=pairs['a']
    cross=await call(b,'poll',dict(v=1,connection_id=p['connection_id'],binding_id=p['binding_id'],page_url='https://'+b['host']+b['path']),p['token']);assert cross.status_code==422
    for path in ['/__live_archive/moderation/v1/events','/__live_archive/moderation/v2/tasks/missing/raw']:
        f=flow(a['host'],path,'https://'+a['host']+a['path']);await ns['request'](f);assert f.response.status_code==403
    for r in routes:
        p=pairs[r['id']];response=await call(r,'goodbye',dict(v=1,connection_id=p['connection_id'],binding_id=p['binding_id'],page_url='https://'+r['host']+r['path']),p['token']);assert response.status_code==200
asyncio.run(main())
