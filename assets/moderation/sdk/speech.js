// Local recognition is separate from chat sending, recording and moderation.
(()=>{
  let state={enabled:false,state:'off',recent:[],error:null},pending=null,epoch=0;
  const scope=()=>({v:1,connection_id:session.connection_id,binding_id:session.binding_id,page_url:BOOT.room_url});
  const call=async(action,extra={})=>{
    if(!session||stopped||draining||!sameRoom())throw Error('直播页尚未连接');
    return post('speech',{scope:scope(),action,...extra},session.token);
  };
  const source=()=>{
    if(BOOT.adapter_id==='douyin.live_page'){
      const controller=window.__liveArchiveRecordingController;
      if(!controller?.getAudioSource)throw Error('请刷新直播页后开启语音');
      return controller.getAudioSource();
    }
    const check=window.WXU?.check_live_existing;
    if(typeof check!=='function')throw Error('直播音频尚未就绪');
    const [error,profile,live]=check({silence:true});
    if(error||!live)throw Error('请先进入并播放直播');
    const url=live.liveSdkInfo?.liveCdnUrl||live.liveInfo?.cdnUrl||live.live_info?.cdnUrl||live.cdnUrl||live.url;
    if(typeof url!=='string'||!/^https?:\/\//i.test(url))throw Error('暂未获取直播音频地址');
    return url;
  };
  sdk.getSpeechStatus=()=>({...state,recent:[...state.recent]});
  sdk.refreshSpeech=()=>{
    if(!BOOT.speech_recognition||!session||stopped||draining)return Promise.resolve(sdk.getSpeechStatus());
    if(pending)return pending;
    const version=epoch;
    pending=call('status').then(value=>{if(version===epoch&&!stopped)state=value;return sdk.getSpeechStatus();})
      .catch(()=>{if(version===epoch)state={...state,state:'disconnected',error:'语音连接中断'};return sdk.getSpeechStatus();})
      .finally(()=>{pending=null;});return pending;
  };
  sdk.setSpeechEnabled=async enabled=>{
    const version=++epoch;
    // Clearing is immediate; an old status response cannot restore old text.
    if(!enabled)state={enabled:false,state:'off',recent:[],error:null};
    const value=await call(enabled?'start':'stop',enabled?{stream_url:source()}:{});
    if(version===epoch&&!stopped)state=value;
    return sdk.getSpeechStatus();
  };
  sdk.onDispose(()=>{epoch++;state={enabled:false,state:'off',recent:[],error:null};});
})();
