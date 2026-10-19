"""Shared proxy dispatcher. Each task keeps its own immutable addon namespace."""
import json
import urllib.parse
from mitmproxy import http

ROUTES = json.loads(__ROUTES_JSON__)
PREFIX = '/__live_archive/moderation/v2/'
for route in ROUTES:
    namespace = {'__name__': 'moderation_task_' + route['id']}
    exec(compile(route.pop('source'), '<moderation-task>', 'exec'), namespace)
    route['addon'] = namespace

def candidates(url):
    p = urllib.parse.urlsplit(url)
    return [r for r in ROUTES if p.scheme == 'https' and p.netloc == r['host'] and p.path.rstrip('/') == r['path']]

def local(flow):
    return flow.request.path.split('?', 1)[0].startswith('/__live_archive/moderation/')

def route_for(flow):
    path = flow.request.path.split('?', 1)[0]
    if local(flow):
        return next((r for r in ROUTES if flow.request.host == r['host'] and path.startswith(r['prefix'])), None)
    matches = candidates(flow.request.pretty_url)
    if matches:
        return matches[0]
    return next((r for r in ROUTES if flow.request.host == r['host']), None)

def loader(matches):
    choices = [{k:r[k] for k in ('id','label','account','prefix','host','path')} for r in matches]
    # TextContent and fixed generated paths keep labels out of HTML/JS parsing.
    return """(() => {
      if (window.__liveArchiveModerationV1 || document.getElementById('live-archive-task-picker')) return;
      const choices=CHOICES;
      const nonce=document.currentScript?.nonce||'';
      const load=choice=>{const s=document.createElement('script');s.src=choice.prefix+'bootstrap.js';if(nonce)s.nonce=nonce;document.head.appendChild(s);};
      if(choices.length===1 && choices[0].host==='live.douyin.com'){load(choices[0]);return;}
      const show=()=>{
        const box=document.createElement('div');box.id='live-archive-task-picker';
        Object.assign(box.style,{position:'fixed',top:'16px',right:'16px',zIndex:'2147483647',background:'#fff',color:'#111',padding:'16px',borderRadius:'8px',maxWidth:'360px',boxShadow:'0 2px 12px #555'});
        const label=document.createElement('p');label.textContent='选择当前直播页对应的场控任务。请核对主播；换房后重新打开页面并重新绑定。';box.appendChild(label);
        for(const choice of choices){const b=document.createElement('button');b.textContent=choice.label+' · '+choice.account+' · '+choice.id;b.style.display='block';b.style.margin='8px 0';b.onclick=()=>{for(const btn of box.querySelectorAll('button'))btn.disabled=true;load(choice);box.remove();};box.appendChild(b);}
        document.body.appendChild(box);
      };
      if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',show,{once:true});else show();
    })();""".replace('CHOICES', json.dumps(choices, ensure_ascii=True), 1).encode('utf-8')

async def request(flow):
    if local(flow):
        flow.response=http.Response.make(403,b'{"error":"task_scope_denied"}',{'Content-Type':'application/json'})
        if flow.request.path.split('?',1)[0] == PREFIX+'bootstrap.js' and flow.request.method=='GET':
            matches=candidates(flow.request.headers.get('referer',''))
            if matches and flow.request.scheme=='https' and flow.request.host==matches[0]['host']:
                flow.response=http.Response.make(200,loader(matches),{'Content-Type':'application/javascript','Cache-Control':'no-store'})
            return
        route=route_for(flow)
        if route:
            await route['addon']['request'](flow)
        return
    route=route_for(flow)
    if route: await route['addon']['request'](flow)

def requestheaders(flow):
    if local(flow): flow.request.stream=False;return
    route=route_for(flow)
    if route: route['addon']['requestheaders'](flow)

def responseheaders(flow):
    if local(flow): flow.response.stream=False;return
    route=route_for(flow)
    if route: route['addon']['responseheaders'](flow)

def response(flow):
    if local(flow): return
    route=route_for(flow)
    if not route: return
    route['addon']['response'](flow)
    if candidates(flow.request.pretty_url) and flow.response and not flow.response.stream and 'text/html' in flow.response.headers.get('content-type','').lower():
        html=flow.response.get_text(strict=False)
        updated=html.replace(route['prefix']+'bootstrap.js',PREFIX+'bootstrap.js')
        if updated!=html:flow.response.set_text(updated)

def http_connect(flow):
    route=route_for(flow)
    if route: route['addon']['http_connect'](flow)
def http_connect_error(flow):
    route=route_for(flow)
    if route: route['addon']['http_connect_error'](flow)
def error(flow):
    route=route_for(flow)
    if route: route['addon']['error'](flow)
def tls_failed_client(data):
    for route in ROUTES:
        if route['addon']['platform_tls'](data):route['addon']['tls_failed_client'](data);break
def tls_failed_server(data):
    for route in ROUTES:
        if route['addon']['platform_tls'](data):route['addon']['tls_failed_server'](data);break
def done():
    for route in ROUTES:route['addon']['done']()
