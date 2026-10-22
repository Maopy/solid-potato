const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const source=fs.readFileSync('assets/wechat_capture/pkg/scraper/wxchannels/inject/channels.live.js','utf8');
const helper=source.slice(source.indexOf('  WXU.recording_live_data ='),source.indexOf('  function __wx_copy_live_download_command'));
test('WeChat hands native quality alternatives to recording without modifying playback state',()=>{
 const live={liveSdkInfo:{liveCdnUrl:'https://media.test/low'},room:'current'};
 const candidates=[{url:'https://media.test/low',rate:800},{url:'https://media.test/origin?token=unchanged',rate:4000,video_quality_level_desc:'原画'}];
 let calls=0;
 const WXU={API:{finderJoinLiveMapper:{},createAdapterFromGlobalMapper:(value,mapper,fields,kind)=>{assert.equal(value,live);assert.equal(kind,'poll');assert.deepEqual(Array.from(fields),['room','stream','liveUser']);calls++;return [null,{payload:{channelParams:{cdn_trans_info:candidates}}}];}}};
 vm.runInNewContext(helper,{WXU});const result=WXU.recording_live_data(live);
 assert.equal(calls,1);assert.equal(result.recording_streams[1].url,candidates[1].url);assert.equal(result.liveSdkInfo.liveCdnUrl,live.liveSdkInfo.liveCdnUrl);assert.equal(live.recording_streams,undefined);
 WXU.API.createAdapterFromGlobalMapper=()=>{throw Error('metadata unavailable');};assert.equal(WXU.recording_live_data(live),live);
});
