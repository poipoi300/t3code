import {test} from 'node:test';
import assert from 'node:assert/strict';
import {Screencast} from '../screencast.mjs';
function fixture(){const sent=[],frames=[],listeners=new Map();let stills=0;const stream=new Screencast({send:async(method,params)=>{sent.push([method,params]);},on:(type,fn)=>listeners.set(type,fn)},{quality:70,maxWidth:1280,maxHeight:800},(data,id)=>frames.push([data,id]),async()=>{stills++;});return {stream,sent,frames,frame:id=>listeners.get('Page.screencastFrame')({sessionId:id,data:'frame'}),get stills(){return stills;}};}
test('scrolling bursts reduce quality and dimensions, then settle to a sharp still',async t=>{
  t.mock.timers.enable({apis:['Date','setTimeout'],now:1000});const f=fixture();await f.stream.start(1);f.stream.scrolledAt=Date.now();
  for(let id=1;id<=4;id++)f.frame(id);await f.stream.tail;
  assert.deepEqual(f.sent.at(-1),['Page.startScreencast',{format:'jpeg',quality:50,maxWidth:640,maxHeight:400}]);
  t.mock.timers.tick(200);await f.stream.tail;
  assert.deepEqual(f.sent.at(-1),['Page.startScreencast',{format:'jpeg',quality:70,maxWidth:1280,maxHeight:800}]);assert.equal(f.stills,1);assert.equal(f.frames.length,4);f.stream.close();
});
test('each frame ACK is paced and sent once with the original Chromium id',async t=>{
  t.mock.timers.enable({apis:['Date','setTimeout'],now:1000});const f=fixture();f.frame(99);const ack=f.stream.ack(1);await f.stream.ack(1);
  assert.deepEqual(f.sent,[]);t.mock.timers.tick(100);await ack;assert.deepEqual(f.sent,[['Page.screencastFrameAck',{sessionId:99}]]);assert.equal(f.stream.frames.size,0);f.stream.close();
});
test('capture pause serializes restarts and disconnect cancels settle stills',async t=>{
  t.mock.timers.enable({apis:['Date','setTimeout'],now:1000});const f=fixture();await f.stream.start(1);await f.stream.pause();const count=f.sent.length;await f.stream.start(0.5);assert.equal(f.sent.length,count);
  await f.stream.resume();assert.equal(f.sent.at(-1)[1].maxWidth,640);f.frame(1);f.frame(2);f.stream.close();t.mock.timers.tick(200);assert.equal(f.stills,0);
});
