// Passive, bounded discovery of PK-related page events. This never invokes a
// platform API or changes the arguments, receiver or result of an event call.
const createWechatPkDiagnostics = (sdk, isCollecting) => {
  const slots=[], limit=300;
  let seen=0, disposed=false;
  const interesting=value=>typeof value==='string' && value.length<=160 && /pk|battle|link.?mic|live.?mic|score/i.test(value);
  const record=(category,name,payload)=>{
    if(disposed || !BOOT.raw_messages || !isCollecting() || seen>=limit || !interesting(name))return;
    seen++;
    try { sdk.captureRaw?.('pk_diagnostic',category,{event_name:name,payload}); } catch (_) {}
  };
  const attach=bus=>{
    for(const key of ['emit','trigger','publish']) {
      try {
        const descriptor=Object.getOwnPropertyDescriptor(bus,key), base=bus[key];
        if(typeof base!=='function' || descriptor && (!('value' in descriptor) || !descriptor.configurable && !descriptor.writable))continue;
        const hook=function(...args) { record('event_bus',args[0],args.slice(1,3)); return Reflect.apply(base,this,args); };
        Object.defineProperty(bus,key,descriptor?{...descriptor,value:hook}:{value:hook,writable:true,configurable:true});
        slots.push({bus,key,descriptor,hook});
      } catch (_) { /* Frozen event buses are left unchanged. */ }
    }
    return slots.length;
  };
  const onMessage=event=>{
    try {
      if(event.origin!==location.origin)return;
      const data=event.data;
      if(!data || typeof data!=='object' || Array.isArray(data))return;
      const props=Object.getOwnPropertyDescriptors(data);
      for(const key of ['type','event','eventName','name','action','method','cmd']) {
        const name=props[key]?.value;
        if(interesting(name)){record('window_message',name,data);return;}
      }
    } catch (_) {}
  };
  window.addEventListener('message',onMessage);
  return {attach,dispose:()=>{
    disposed=true;
    window.removeEventListener('message',onMessage);
    for(const {bus,key,descriptor,hook} of slots){
      try { if(bus[key]===hook){ if(descriptor)Object.defineProperty(bus,key,descriptor);else delete bus[key]; } } catch (_) {}
    }
    slots.length=0;
  }};
};
