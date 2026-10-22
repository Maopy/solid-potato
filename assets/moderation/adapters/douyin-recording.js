// Explicit page-owned media capture. Does not interact with chat inputs.
(() => {
  if(window.__liveArchiveRecording)return;
  window.__liveArchiveRecording=true;
  const room=typeof userscriptCurrentRoom==='function'?BOOT.room_url:location.origin+location.pathname.replace(/\/$/,'');
  const inRoom=()=>typeof userscriptCurrentRoom==='function'?userscriptCurrentRoom():location.origin+location.pathname.replace(/\/$/,'')===room;
  const api=async(path,body)=>{
    if(typeof userscriptRecordingTransport==='function')return userscriptRecordingTransport(path,body);
    const r=await fetch('/api/v1/download_task/'+path,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});
    const value=await r.json();if(!r.ok||value.code!==0)throw Error(value.msg||'录制服务暂不可用');return value.data;
  };
  const streams=new Map();
  let metadataReceived=false,anchorName=null;
  const tier=name=>{name=String(name||'').toLowerCase();return /origin|原画|蓝光/.test(name)?6:/uhd|4k|超高清/.test(name)?5:/full_hd|fhd|1080|超清/.test(name)?4:/^hd\d*$|高清|720/.test(name)?3:/^sd\d*$|标清/.test(name)?2:/^ld\d*$|流畅/.test(name)?1:0;};
  const qualityInfo=(label,params={})=>{
    if(typeof params==='string')try{params=JSON.parse(params);}catch(_){params={};}
    const dimensions=String(params?.resolution||'').match(/(\d+)\s*[xX*]\s*(\d+)/);
    return {label:label||'当前播放清晰度',tier:tier(label),pixels:dimensions?Number(dimensions[1])*Number(dimensions[2]):Number(params?.width||0)*Number(params?.height||0),bitrate:Number(params?.vbitrate||params?.bitrate||0)||0};
  };
  const compare=(a,b)=>a.tier-b.tier||a.pixels-b.pixels||a.bitrate-b.bitrate;
  const add=(url,label,params)=>{if(typeof url!=='string'||!/^https?:\/\//i.test(url))return;try{const u=new URL(url);if(u.username||u.password)return;const info=qualityInfo(label,params),old=streams.get(url);if(!old||compare(info,old)>0)streams.set(url,info);if(streams.size>24){const entries=[...streams].sort((a,b)=>compare(b[1],a[1]));streams.clear();for(const entry of entries.slice(0,24))streams.set(...entry);}}catch(_){}};
  const scan=(value,depth=0,seen=new Set(),label='')=>{
    if(depth>10||!value||typeof value!=='object'||seen.has(value))return;seen.add(value);
    for(const [key,v] of Object.entries(value).slice(0,100)){
      const hint=tier(key)?key:label;
      if(typeof v==='string'&&/^(?:flv_pull_url|hls_pull_url|flv|hls|sdk_params|stream_data)$/i.test(key)){
        if(/^https?:\/\//i.test(v))add(v,hint||key,value.sdk_params||value);else if(v.length<262144)try{scan(JSON.parse(v),depth+1,seen,hint);}catch(_){}
      } else if(v&&typeof v==='object'){
        if(/^(?:flv_pull_url|hls_pull_url)$/i.test(key))for(const [quality,url] of Object.entries(v))add(url,quality);
        scan(v,depth+1,seen,hint);
      }
    }
  };
  const roomState=value=>{
    // These containers represent the current room, excluding recommendation lists.
    const roots=[value?.data?.data?.[0],value?.app?.initialState?.roomStore?.roomInfo?.room,
      value?.roomStore?.roomInfo?.room,value?.props?.pageProps?.room];
    for(const root of roots)if(root){scan(root.stream_url||root);const name=root.owner?.nickname||root.owner_user?.nickname;if(typeof name==='string'&&name.trim())anchorName=name.trim();}
  };
  // Reuse the room-scoped fetch/XHR metadata captured from document-start,
  // including responses that arrived before the local pairing completed.
  const observeRoomMetadata=(value,url)=>{
    try{const u=new URL(url,location.href);if(inRoom()&&/(^|\.)douyin\.com$/.test(u.hostname)&&u.pathname.includes('/webcast/room/web/enter')&&u.searchParams.get('web_rid')===new URL(room).pathname.slice(1)){metadataReceived=true;roomState(value);}}catch(_){}
  };
  if(typeof sdk!=='undefined')sdk.observeRoomMetadata=observeRoomMetadata;
  // Resource timing buffers can be cleared by the player. Retain observed
  // media URLs as they load instead of relying on a later snapshot alone.
  let mediaObserver;
  if(typeof PerformanceObserver==='function')try{mediaObserver=new PerformanceObserver(list=>{if(inRoom())for(const item of list.getEntries())if(/\.(?:flv|m3u8)(?:[?#]|$)/i.test(item.name))add(item.name,'页面已播放清晰度');});mediaObserver.observe({type:'resource',buffered:true});}catch(_){}
  // Observe only the current room's existing enter response, preserving fetch's promise.
  const original=window.fetch;
  const recordingFetch=function(...args){const result=original.apply(this,args);try{if(!inRoom())return result;const u=new URL(typeof args[0]==='string'?args[0]:args[0].url,location.href);if(u.pathname.includes('/webcast/room/web/enter')&&u.searchParams.get('web_rid')===new URL(room).pathname.slice(1))void result.then(r=>r.clone().json()).then(v=>roomState(v)).catch(()=>{});}catch(_){}return result;};
  window.fetch=recordingFetch;
  const refreshStreams=()=>{
    // The current video source and room state are preferred to stale resources.
    for(const v of document.querySelectorAll('video'))add(v.currentSrc||v.src,'当前播放清晰度');
    roomState(window.__INITIAL_STATE__);roomState(window.__NEXT_DATA__);
    for(const script of document.querySelectorAll('script#RENDER_DATA,script#__NEXT_DATA__')){
      const text=script.textContent||'';if(text.length>2097152)continue;
      try{roomState(JSON.parse(text));}catch(_){try{roomState(JSON.parse(decodeURIComponent(text)));}catch(_){}}
    }
    // Only URLs that this page actually loaded, never fabricate CDN paths.
    for(const item of performance.getEntriesByType('resource').slice(-500))if(/\.(?:flv|m3u8)(?:[?#]|$)/i.test(item.name))add(item.name,'页面已播放清晰度');
  };
  const mount=()=>{
    if(!document.body)return;
    const box=document.createElement('div');box.id='live-archive-recording';
    Object.assign(box.style,{position:'fixed',right:'20px',top:'180px',zIndex:'2147483646',background:'#fff',color:'#222',padding:'10px',borderRadius:'8px',fontSize:'13px',maxWidth:'320px',boxShadow:'0 2px 8px #888'});
    const button=document.createElement('button');button.textContent='录制直播';button.type='button';
    const quality=document.createElement('select');quality.title='选择页面已提供的直播清晰度';
    const status=document.createElement('div');status.textContent='点击录制当前直播';
    const toggle=document.createElement('button');toggle.type='button';toggle.textContent='下载任务';
    const list=document.createElement('div');list.hidden=true;
    // Recording always uses the highest advertised quality, independently of playback.
    const update=()=>{refreshStreams();quality.replaceChildren();const entries=[...streams].sort((a,b)=>compare(b[1],a[1]));for(const [url,info] of entries){const o=document.createElement('option');o.value=url;o.textContent=info.label;quality.appendChild(o);}quality.value=entries[0]?.[0]||'';quality.disabled=true;quality.hidden=streams.size<2;};
    let busy=false;
    const start=async()=>{
      if(busy)throw Error('录制请求处理中');busy=true;button.disabled=true;
      try{
        if(!inRoom())throw Error('直播间已切换，请重新打开页面');
        update();if(!quality.value)throw Error('请先进入并播放直播，再点击录制');
        const created=await api('create',{objects:[{platform:'douyin',content:{profile:{room_url:room,nickname:anchorName||(document.title||'抖音直播').replace(/ - 抖音直播$/,'').replace(/的抖音直播间$/,'')},live:{stream_url:quality.value}}}]});
        status.textContent='';await render();return created;
      }finally{busy=false;button.disabled=false;}
    };
    const controller={start,getStatus:()=>({stream_count:streams.size,room_metadata_received:metadataReceived,quality:streams.get(quality.value)?.label||null}),showTasks:()=>{box.style.display='block';list.hidden=false;return render();}};
    window.__liveArchiveRecordingController=controller;
    button.onclick=()=>start().catch(e=>status.textContent=e.message);
    const names={waiting:'等待启动',downloading:'录制中',paused:'已暂停',finished:'已完成',failed:'失败'};
    const render=async()=>{const data=await api('list');list.replaceChildren();for(const task of data.list||[]){const row=document.createElement('div');const text=document.createElement('span');text.textContent=task.name+' · '+(names[task.status]||task.status)+(task.error?' · '+task.error:'');row.appendChild(text);
      for(const [label,action] of task.status==='downloading'||task.status==='waiting'?[['暂停','pause'],['移除','delete']]:[['继续','start'],['移除','delete']]){const b=document.createElement('button');b.type='button';b.textContent=label;b.onclick=async()=>{b.disabled=true;try{await api(action,{task_ids:[task.id]});await render();}catch(e){status.textContent=e.message;}finally{b.disabled=false;}};row.appendChild(b);}list.appendChild(row);}};
    toggle.onclick=()=>{list.hidden=!list.hidden;if(!list.hidden)render().catch(e=>status.textContent=e.message);};
    box.append(button,quality,toggle,status,list);if(typeof BOOT!=='undefined'&&BOOT.unified_ui)box.style.display='none';document.body.appendChild(box);update();
    const cleanup=()=>{clearInterval(timer);mediaObserver?.disconnect();if(typeof sdk!=='undefined'&&sdk.observeRoomMetadata===observeRoomMetadata)delete sdk.observeRoomMetadata;box.remove();if(window.fetch===recordingFetch)window.fetch=original;if(window.__liveArchiveRecordingController===controller){delete window.__liveArchiveRecordingController;delete window.__liveArchiveRecording;}};
    const timer=setInterval(()=>{if(!inRoom()){cleanup();return;}if(!list.hidden)render().catch(()=>{});},2000);
    if(typeof sdk!=='undefined')sdk.onDispose(cleanup);
  };
  if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',mount,{once:true});else mount();
})();
