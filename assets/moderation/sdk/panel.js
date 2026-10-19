// One visible control entry on both platforms; backend ACK governs the switch.
const mountModerationPanel=()=>{
  if(!document.body||typeof document.createElement!=='function'||typeof document.getElementById!=='function'||document.getElementById('live-archive-assistant'))return;
  const box=document.createElement('section');box.id='live-archive-assistant';
  Object.assign(box.style,{position:'fixed',right:'20px',top:'16px',zIndex:'2147483647',background:'#fff',color:'#222',padding:'12px',border:'1px solid #ccc',borderRadius:'8px',font:'14px sans-serif',maxWidth:'320px',boxShadow:'0 2px 10px #888'});
  const platform=BOOT.adapter_id==='wechat_channels.desktop_probe'?'视频号':'抖音';
  let expanded=false;
  const launcher=document.createElement('button');launcher.type='button';launcher.setAttribute('aria-controls','live-archive-assistant-details');
  Object.assign(launcher.style,{background:'#0f766e',color:'#fff',border:'0',borderRadius:'18px',padding:'9px 13px',cursor:'pointer',font:'13px sans-serif',minHeight:'36px'});
  const details=document.createElement('div');details.id='live-archive-assistant-details';
  launcher.onclick=()=>{expanded=!expanded;render();};
  const toggle=document.createElement('button');toggle.type='button';toggle.setAttribute('role','switch');
  const info=document.createElement('div');info.style.whiteSpace='pre-line';
  const record=document.createElement('button');record.type='button';record.textContent='录制直播';record.hidden=!BOOT.recording;
  const tasks=document.createElement('button');tasks.type='button';tasks.textContent='下载任务';tasks.hidden=!BOOT.recording;
  for(const button of [toggle,record,tasks])Object.assign(button.style,{background:'#0f766e',color:'#fff',border:'0',borderRadius:'5px',padding:'7px 10px',margin:'8px 5px 5px 0',cursor:'pointer',font:'14px sans-serif'});
  const notice=document.createElement('div');
  toggle.onclick=async()=>{toggle.disabled=true;try{await sdk.setModerationEnabled(!sdk.getStatus().moderation_enabled);notice.textContent='';}catch(e){notice.textContent=e.message;}finally{render();}};
  record.onclick=async()=>{record.disabled=true;try{await sdk.startRecording();notice.textContent='已提交录制请求';}catch(e){notice.textContent=e.message;}finally{record.disabled=false;}};
  tasks.onclick=()=>{if(window.__liveArchiveRecordingController)window.__liveArchiveRecordingController.showTasks().catch(e=>notice.textContent=e.message);else if(window.WXU?.downloader?.show)window.WXU.downloader.show();else notice.textContent='下载列表尚未就绪，可在统一控制页查看。';};
  const render=()=>{const s=sdk.getStatus();
    const state=s.stopped||s.bridge_error||s.decoder?.last_error?'异常':!s.connected?'连接中':s.moderation_enabled?'已开启':'已关闭';
    launcher.textContent=expanded?'直播助手 · '+platform+'　收起 ▴':'场控 · '+state+' ▾';
    launcher.title=(expanded?'收起':'展开')+'直播助手 · '+platform+'（场控'+state+'）';
    launcher.setAttribute('aria-expanded',String(expanded));
    details.hidden=!expanded;details.style.display=expanded?'block':'none';
    Object.assign(box.style,{padding:expanded?'12px':'0',background:expanded?'#fff':'transparent',border:expanded?'1px solid #ccc':'0',boxShadow:expanded?'0 2px 10px #888':'none',maxWidth:'calc(100vw - 40px)',maxHeight:'calc(100vh - 32px)',overflowY:'auto'});
    toggle.textContent=s.moderation_enabled?'关闭场控':'开启场控';toggle.setAttribute('aria-checked',String(s.moderation_enabled));toggle.disabled=!s.connected||s.stopped||s.draining;
    info.textContent=(s.stopped?'连接已停止，请刷新直播页':s.connected?'已连接':'未连接')+' · 场控'+(s.moderation_enabled?'已开启':'已关闭')+(s.bridge_error?'\n'+s.bridge_error:'')+(s.fill?.bridge?.blocked?'\n发送未确认，已暂停后续发送':'');
    if(BOOT.adapter_id==='douyin.live_page'){
      const counts=s.delivery?.observed||s.delivery?.generated||{},comments=counts['chat.received']||0,gifts=counts['gift.observed']||0;
      info.textContent+='\n已采集：评论 '+comments+' · 礼物 '+gifts;
      if(s.decoder?.last_error)info.textContent+='\n消息解析异常：'+s.decoder.last_error;
      const reason=s.fill?.input_reason,names={page_hidden:'直播页在后台',input_not_found:'未找到评论输入框',input_not_unique:'发现多个评论输入框',input_busy:'评论框正在编辑',input_disabled:'评论框不可用',input_hidden:'评论框不可见'};
      if(reason)info.textContent+='\n'+(names[reason]||reason);
    }
    const t=s.timed_messages;if(t?.enabled&&t.min_comments>0)info.textContent+='\n定时文案：'+(t.active?'评论 '+t.comments_since_last+'/'+t.min_comments+' · '+(t.remaining_ms>0?'等待间隔':'间隔已到'):'已暂停');};
  for(const node of [toggle,record,tasks,info,notice])details.appendChild(node);
  box.appendChild(launcher);box.appendChild(details);document.body.appendChild(box);render();
  const timer=setInterval(render,1000);sdk.onDispose(()=>{clearInterval(timer);render();});
};
if(document.body)mountModerationPanel();else document.addEventListener('DOMContentLoaded',mountModerationPanel,{once:true});
