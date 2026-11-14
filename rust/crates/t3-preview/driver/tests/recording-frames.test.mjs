import {test} from "node:test";
import assert from "node:assert/strict";
import {EventEmitter} from "node:events";
import {recordingFrames} from "../recording-frames.mjs";
test("recording acknowledges immediately and decodes only the first and latest pending frames",async()=>{
 const session=new EventEmitter(),acks=[],delivered=[],gate=Promise.withResolvers();session.send=async(method,input)=>{acks.push([method,input]);};
 const encoder={evaluate:async(_fn,frame)=>{delivered.push(frame);if(delivered.length===1)await gate.promise;return true;}};
 const frames=recordingFrames(session,encoder,()=>1280);
 for(let i=1;i<=4;i++)session.emit("Page.screencastFrame",{sessionId:i,data:String(i),metadata:{deviceWidth:2000}});
 assert.deepEqual(acks.map(([,input])=>input.sessionId),[1,2,3,4]);assert.deepEqual(delivered,[["1",1280]]);
 gate.resolve();await Promise.all(frames);assert.deepEqual(delivered,[["1",1280],["4",1280]]);assert.equal(frames.size,0);
});
test("a full encoder stops the screencast and discards the pending frame",async()=>{
 const session=new EventEmitter(),calls=[],gate=Promise.withResolvers();session.send=async(method)=>calls.push(method);let encoded=0;
 const frames=recordingFrames(session,{evaluate:async()=>{encoded++;await gate.promise;return false;}},()=>null);
 session.emit("Page.screencastFrame",{sessionId:1,data:"a",metadata:{deviceWidth:400}});session.emit("Page.screencastFrame",{sessionId:2,data:"b",metadata:{deviceWidth:400}});
 gate.resolve();await Promise.all(frames);assert.equal(encoded,1);assert.equal(calls.at(-1),"Page.stopScreencast");
});
