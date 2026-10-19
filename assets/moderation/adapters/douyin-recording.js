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
  const add=(url,label)=>{if(typeof url!=='string'||!/^https?:\/\//i.test(url))return;try{const u=new URL(url);if(u.username||u.password)return;if(streams.size<24)streams.set(url,label||'当前播放清晰度');}catch(_){}};
  const scan=(value,depth=0,seen=new Set())=>{
    if(depth>8||!value||typeof value!=='object'||seen.has(value))return;seen.add(value);
    for(const [key,v] of Object.entries(value).slice(0,100)){
      if(typeof v==='string'&&/^(?:flv_pull_url|hls_pull_url|flv|hls|sdk_params|stream_data)$/i.test(key)){
        if(/^https?:\/\//i.test(v))add(v,key);else if(v.length<262144)try{scan(JSON.parse(v),depth+1,seen);}catch(_){}
      } else if(v&&typeof v==='object'){
        if(/^(?:flv_pull_url|hls_pull_url)$/i.test(key))for(const [quality,url] of Object.entries(v))add(url,quality);
        scan(v,depth+1,seen);
      }
    }
  };
  const roomState=value=>{
    // These containers represent the current room, excluding recommendation lists.
    const roots=[value?.data?.data?.[0],value?.app?.initialState?.roomStore?.roomInfo?.room,
      value?.roomStore?.roomInfo?.room,value?.props?.pageProps?.room];
    for(const root of roots)if(root)scan(root);
  };
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
    const update=()=>{refreshStreams();const selected=quality.value;quality.replaceChildren();for(const [url,label] of streams){const o=document.createElement('option');o.value=url;o.textContent=label;quality.appendChild(o);}if(streams.has(selected))quality.value=selected;quality.hidden=streams.size<2;};
    let busy=false;
    const start=async()=>{
      if(busy)throw Error('录制请求处理中');busy=true;button.disabled=true;
      try{
        if(!inRoom())throw Error('直播间已切换，请重新打开页面');
        update();if(!quality.value)throw Error('请先进入并播放直播，再点击录制');
        await api('create',{objects:[{platform:'douyin',content:{profile:{room_url:room,nickname:document.title||'抖音直播'},live:{stream_url:quality.value}}}]});
        status.textContent='已提交录制请求';await render();
      }finally{busy=false;button.disabled=false;}
    };
    const controller={start,showTasks:()=>{box.style.display='block';list.hidden=false;return render();}};
    window.__liveArchiveRecordingController=controller;
    button.onclick=()=>start().catch(e=>status.textContent=e.message);
    const names={waiting:'等待启动',downloading:'录制中',paused:'已暂停',finished:'已完成',failed:'失败'};
    const render=async()=>{const data=await api('list');list.replaceChildren();for(const task of data.list||[]){const row=document.createElement('div');const text=document.createElement('span');text.textContent=task.name+' · '+(names[task.status]||task.status)+(task.error?' · '+task.error:'');row.appendChild(text);
      for(const [label,action] of task.status==='downloading'||task.status==='waiting'?[['暂停','pause'],['移除','delete']]:[['继续','start'],['移除','delete']]){const b=document.createElement('button');b.type='button';b.textContent=label;b.onclick=async()=>{b.disabled=true;try{await api(action,{task_ids:[task.id]});await render();}catch(e){status.textContent=e.message;}finally{b.disabled=false;}};row.appendChild(b);}list.appendChild(row);}};
    toggle.onclick=()=>{list.hidden=!list.hidden;if(!list.hidden)render().catch(e=>status.textContent=e.message);};
    box.append(button,quality,toggle,status,list);if(typeof BOOT!=='undefined'&&BOOT.unified_ui)box.style.display='none';document.body.appendChild(box);update();
    const cleanup=()=>{clearInterval(timer);box.remove();if(window.fetch===recordingFetch)window.fetch=original;if(window.__liveArchiveRecordingController===controller){delete window.__liveArchiveRecordingController;delete window.__liveArchiveRecording;}};
    const timer=setInterval(()=>{if(!inRoom()){cleanup();return;}if(!list.hidden)render().catch(()=>{});},2000);
    if(typeof sdk!=='undefined')sdk.onDispose(cleanup);
  };
  if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',mount,{once:true});else mount();
})();
