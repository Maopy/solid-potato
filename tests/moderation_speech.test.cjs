const test=require('node:test'),assert=require('node:assert/strict'),vm=require('node:vm'),fs=require('node:fs');
const source=fs.readFileSync('assets/moderation/sdk/speech.js','utf8');
test('central speech requests are acknowledged once and report unavailable audio',async()=>{
 const requests=[];
 let unavailable=false;
 const context={window:{__liveArchiveRecordingController:{getAudioSource:()=>{if(unavailable)throw Error('音频尚未就绪');return 'https://media.test/live';}}},BOOT:{adapter_id:'douyin.live_page',room_url:'room'},session:{connection_id:'connection',binding_id:'binding',token:'token'},stopped:false,draining:false,sameRoom:()=>true,sdk:{onDispose(){}},post:async(path,body)=>{requests.push(body);return {enabled:body.action==='start',state:'listening',recent:[]};}};
 vm.runInNewContext(source,context);
 await context.sdk.applySpeechRequest('first');await context.sdk.applySpeechRequest('first');
 assert.deepEqual(requests.map(r=>r.action),['start','status']);
 assert.ok(requests.every(r=>r.request_id==='first'));
 unavailable=true;await context.sdk.applySpeechRequest('second');
 assert.equal(requests.at(-1).action,'status');assert.equal(requests.at(-1).error,'音频尚未就绪');
});
for(const platform of ['douyin','wechat'])test(platform+' speech uses its own scoped transport and clears recent text across stop and dispose',async()=>{
 let remote={enabled:false,state:'off',recent:[],error:null},hold=null;const requests=[],disposers=[];
 const context={window:{__liveArchiveRecordingController:{getAudioSource:()=> 'https://media.example.test/dy.flv'},WXU:{check_live_existing:()=>[null,{}, {liveSdkInfo:{liveCdnUrl:'https://media.example.test/wx.flv'}}]}},BOOT:{speech_recognition:true,room_url:'room',adapter_id:platform==='douyin'?'douyin.live_page':'wechat_channels.desktop_probe'},session:{connection_id:'connection',binding_id:'binding',token:'token'},stopped:false,draining:false,sameRoom:()=>true,sdk:{onDispose:fn=>disposers.push(fn)},post:async(path,body,token)=>{requests.push({path,body,token});if(hold&&body.action==='status')return new Promise(resolve=>{hold=resolve});if(body.action==='start')remote={enabled:true,state:'listening',recent:['你好。'],error:null};if(body.action==='stop')remote={enabled:false,state:'off',recent:[],error:null};return structuredClone(remote);}};
 vm.runInNewContext(source,context);await context.sdk.setSpeechEnabled(true);
 assert.equal(requests[0].path,'speech');assert.equal(requests[0].token,'token');
 assert.equal(requests[0].body.scope.binding_id,'binding');assert.match(requests[0].body.stream_url,new RegExp(platform==='douyin'?'dy.flv':'wx.flv'));
 assert.equal(context.sdk.getSpeechStatus().recent[0],'你好。');
 hold=true;const pending=context.sdk.refreshSpeech();await context.sdk.setSpeechEnabled(false);
 hold({enabled:true,state:'listening',recent:['过期文本。']});await pending;
 assert.equal(context.sdk.getSpeechStatus().recent.length,0);assert.equal(context.sdk.getSpeechStatus().enabled,false);
 await context.sdk.setSpeechEnabled(true);for(const dispose of disposers)dispose();assert.equal(context.sdk.getSpeechStatus().recent.length,0);
 context.stopped=true;await assert.rejects(context.sdk.setSpeechEnabled(true),/尚未连接/);
 assert.ok(requests.every(r=>r.path==='speech'),'never uses recording or comment endpoints');
});
