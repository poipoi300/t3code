const {EventEmitter}=require('node:events');
const fixture={launches:[],persistent:[],browsers:[],contexts:[],fail:null};
function context(){const events=new EventEmitter();const value={closed:false,on:(type,fn)=>events.on(type,fn),pages:()=>[],newPage:async()=>({once:()=>{}}),close:async()=>{if(!value.closed){value.closed=true;events.emit('close');}}};fixture.contexts.push(value);return value;}
function browser(){const events=new EventEmitter(),contexts=[];const value={closed:false,on:(type,fn)=>events.on(type,fn),contexts:()=>contexts.filter(c=>!c.closed),newContext:async()=>{const value=context();contexts.push(value);return value;},close:async()=>{if(value.closed)return;value.closed=true;for(const value of contexts)await value.close();events.emit('disconnected');}};fixture.browsers.push(value);return value;}
fixture.chromium={launch:async options=>{fixture.launches.push(options);if(fixture.fail){const error=fixture.fail;fixture.fail=null;throw error;}return browser();},launchPersistentContext:async(directory,options)=>{fixture.persistent.push([directory,options]);if(fixture.fail){const error=fixture.fail;fixture.fail=null;throw error;}return context();}};
fixture.reset=()=>{fixture.launches=[];fixture.persistent=[];fixture.browsers=[];fixture.contexts=[];fixture.fail=null;};
module.exports=fixture;
