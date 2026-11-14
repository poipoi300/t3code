import {test} from 'node:test';
import assert from 'node:assert/strict';
import {DesktopViewport} from '../desktop-viewport.mjs';
const deferred=()=>{let resolve;const promise=new Promise(done=>{resolve=done;});return {promise,resolve};};
function fixture(){const calls=[],reads=[];let started=deferred();return {calls,reads,get started(){return started.promise;},cdp:{send:async method=>{calls.push(method);if(method==='Page.getFrameTree')return {frameTree:{frame:{id:'main'}}};if(method==='Page.createIsolatedWorld')return {executionContextId:1};const result=deferred();reads.push(result);const previous=started;started=deferred();previous.resolve();return result.promise.then(value=>({result:{value}}));}}};}
test('concurrent size readers serialize and share the next read',async()=>{
  const f=fixture(),size=new DesktopViewport(f.cdp,()=>false);
  const first=size.measure();await f.started;
  const second=size.measure(),third=size.measure();assert.equal(second,third);assert.equal(f.reads.length,1);
  const started=f.started;f.reads[0].resolve({width:800,height:600});assert.deepEqual(await first,{width:800,height:600});await started;
  f.reads[1].resolve({width:390,height:844});assert.deepEqual(await second,{width:390,height:844});assert.equal(f.reads.length,2);
});
test('a resize during a status read forces a fresh read',async()=>{
  const f=fixture(),size=new DesktopViewport(f.cdp,()=>false);
  const status=size.status();await f.started;size.changed();const started=f.started;
  f.reads[0].resolve({width:800,height:600});await started;f.reads[1].resolve({width:400,height:700});
  assert.deepEqual(await status,{width:400,height:700});assert.equal(size.stale,false);
});
test('dialogs use the cached viewport without running page script',async()=>{
  const f=fixture(),size=new DesktopViewport(f.cdp,()=>true);size.value={width:800,height:600};
  assert.deepEqual(await size.status(),size.value);assert.deepEqual(f.calls,[]);
});
test('an unchanged fill setting measures without waiting for a nonexistent resize',async t=>{
  t.mock.timers.enable({apis:['setTimeout']});const f=fixture(),size=new DesktopViewport(f.cdp,()=>false);size.value={width:800,height:600};
  const fill={_tag:'fill'};const resized=size.resized(size.prepare(fill),fill,1000);await f.started;f.reads[0].resolve({width:800,height:600});assert.deepEqual(await resized,{width:800,height:600});
});
test('a stalled viewport read times out and a late response cannot replace the cache',async t=>{
  t.mock.timers.enable({apis:['setTimeout']});const f=fixture(),size=new DesktopViewport(f.cdp,()=>false);size.value={width:800,height:600};
  const read=size.measure();await f.started;t.mock.timers.tick(1000);assert.equal(await read,null);assert.equal(size.stale,true);
  f.reads[0].resolve({width:10,height:10});assert.deepEqual(size.value,{width:800,height:600});
});
