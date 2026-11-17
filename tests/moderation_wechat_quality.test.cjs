const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync('assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.live.js','utf8');
const helper=source.slice(source.indexOf('  WXU.recording_live_data ='),source.indexOf('  function __wx_copy_live_download_command'));

for(const unified of [true,false])test(`WeChat ${unified?'unified':'standalone'} UI decides whether to create the legacy icon before the moderation SDK loads`,async()=>{
 let created=0,inserted=0,feeds=0;
 const listeners={},observers=[],log={Info(){return this;},Str(){return this;},JSON(){return this;},Msg(){}};
 const host={children:[{}],insertBefore(){inserted++;}};
 const WXU={API:{},before_menu_items:[],after_menu_items:[],log,
  observe_node({onOk}){observers.push(onOk);onOk(host);},
  onFetchFeedProfile(fn){listeners.profile=fn;},onJoinLive(fn){listeners.live=fn;},
  set_live_feed(){feeds++;}};
 const window={__d_config:{unifiedUI:unified}};
 vm.runInNewContext(source,{window,WXU,WXEnv:{get:()=>''},
  Icons:{download_btn8(){created++;return {};}},__wx_channels_live_store__:{},
  setTimeout:()=>1,clearTimeout(){},console});
 assert.equal(window.__liveArchiveModerationV1,undefined,'the assistant has not loaded yet');
 assert.equal(created,unified?0:1);assert.equal(inserted,unified?0:1);
 listeners.profile({username:'synthetic-host'});
 await listeners.live({liveSdkInfo:{liveCdnUrl:'https://media.test/live'}});
 for(const onOk of observers)onOk(host);
 assert.equal(created,unified?0:1,'no transient or duplicate recording icon');
 assert.equal(feeds,1,'recording metadata is still populated without a legacy button');
});
test('WeChat hands native quality alternatives to recording without modifying playback state',()=>{
 const live={liveSdkInfo:{liveCdnUrl:'https://media.test/low'},room:'current'};
 const candidates=[{url:'https://media.test/low',rate:800},{url:'https://media.test/origin?token=unchanged',rate:4000,video_quality_level_desc:'原画'}];
 let calls=0;
 const WXU={API:{finderJoinLiveMapper:{},createAdapterFromGlobalMapper:(value,mapper,fields,kind)=>{assert.equal(value,live);assert.equal(kind,'poll');assert.deepEqual(Array.from(fields),['room','stream','liveUser']);calls++;return [null,{payload:{channelParams:{cdn_trans_info:candidates}}}];}}};
 vm.runInNewContext(helper,{WXU});const result=WXU.recording_live_data(live);
 assert.equal(calls,1);assert.equal(result.recording_streams[1].url,candidates[1].url);assert.equal(result.liveSdkInfo.liveCdnUrl,live.liveSdkInfo.liveCdnUrl);assert.equal(live.recording_streams,undefined);
 WXU.API.createAdapterFromGlobalMapper=()=>{throw Error('metadata unavailable');};assert.equal(WXU.recording_live_data(live),live);
});
