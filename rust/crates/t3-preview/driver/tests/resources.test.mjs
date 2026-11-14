import {test} from 'node:test';
import assert from 'node:assert/strict';
import {withResources} from '../resources.mjs';
const deferred=()=>{let resolve;const promise=new Promise(r=>{resolve=r;});return {promise,resolve};};
const timeout=()=>new Error('deadline');
test('a stalled CDP command closes browser and proxy independently',async t=>{
  t.mock.timers.enable({apis:['setTimeout']});
  const ready=deferred(),closed=[];
  const run=withResources(100,async own=>{
    await own('proxy',async value=>{closed.push(value);});
    await own('browser',async value=>{closed.push(value);return new Promise(()=>{});});
    ready.resolve();return new Promise(()=>{});
  },timeout);
  const rejection=assert.rejects(run,/deadline/);await ready.promise;t.mock.timers.tick(100);await rejection;
  assert.deepEqual(closed,['browser','proxy']);
});
test('a browser launch completing after timeout closes its late browser',async t=>{
  t.mock.timers.enable({apis:['setTimeout']});
  const launch=deferred(),ready=deferred(),lateClosed=deferred();const closed=[];
  const run=withResources(100,async own=>{
    await own('proxy',async value=>{closed.push(value);});ready.resolve();
    await own(await launch.promise,async value=>{closed.push(value);lateClosed.resolve();});
  },timeout);
  const rejection=assert.rejects(run,/deadline/);await ready.promise;t.mock.timers.tick(100);await rejection;
  assert.deepEqual(closed,['proxy']);launch.resolve('browser');await lateClosed.promise;
  assert.deepEqual(closed,['proxy','browser']);
});
test('successful and failed captures dispose all owned resources',async()=>{
  for(const fail of [false,true]){
    const closed=[];const run=withResources(1000,async own=>{
      await own('proxy',async x=>{closed.push(x);});await own('browser',async x=>{closed.push(x);});
      if(fail)throw new Error('CDP disconnected');return 'png';
    },timeout);
    if(fail)await assert.rejects(run,/CDP disconnected/);else assert.equal(await run,'png');
    assert.deepEqual(closed,['browser','proxy']);
  }
});
