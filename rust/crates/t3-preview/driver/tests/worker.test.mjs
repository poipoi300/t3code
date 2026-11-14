import {test,after} from "node:test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import {fileURLToPath} from "node:url";
import playwright from "./playwright-fixture.cjs";
process.env.T3_PREVIEW_PLAYWRIGHT_MODULE=fileURLToPath(new URL("./playwright-fixture.cjs",import.meta.url));
import {EventEmitter} from "node:events";
const directory=await fs.mkdtemp(path.join(os.tmpdir(),"t3-preview-worker-test-"));
process.env.T3_PREVIEW_CONFIG=JSON.stringify({downloadsDir:path.join(directory,"downloads"),profilesDir:path.join(directory,"profiles")});
const {dispatch,tabs,navigate,status,observeNavigation}=await import("../worker.mjs");
after(async()=>{await dispatch({command:"shutdown"});await fs.rm(directory,{recursive:true,force:true});});
function fixture(){
 const calls=[];const session=new EventEmitter();session.send=async(method,input)=>{calls.push([method,input]);return method==="Page.getNavigationHistory"?{currentIndex:0,entries:[{}]}:{result:{value:0}};};
 const page={url:()=>"about:blank",title:async()=>"",viewportSize:()=>({width:1280,height:800}),goto:async(url,input)=>calls.push(["goto",url,input]),goBack:async()=>calls.push(["back"]),goForward:async()=>calls.push(["forward"]),reload:async()=>calls.push(["reload"]),on:()=>{},off:()=>{}};
 const key={threadId:"test",tabId:crypto.randomUUID()};const viewer={id:"viewer",session};
 const tab={key,page,cdp:session,viewers:new Map([[viewer.id,viewer]]),frameSessions:new Map(),downloads:[],controller:null,onChooser:()=>{},chooser:null,dialog:null,closing:false,loading:true,navigationGeneration:3,stoppedGeneration:null,pendingInitialRequestUrl:null,loadError:null,crash:null};tabs.set(JSON.stringify([key.threadId,key.tabId]),tab);
 return {tab,calls,session,send:input=>dispatch({command:"viewerInput",key,viewerId:viewer.id,input}),close:()=>tabs.delete(JSON.stringify([key.threadId,key.tabId]))};
}
test("Stop sends Page.stopLoading and settles blank or committed pages without detaching",async()=>{
 for(const url of ["about:blank","http://localhost:3000/loaded"]){const f=fixture();try{f.tab.page.url=()=>url;await f.send({type:"stopLoading"});assert.equal(f.tab.loading,false);assert.equal(f.tab.stoppedGeneration,3);assert(f.calls.some(([method])=>method==="Page.stopLoading"));assert.equal(f.tab.viewers.size,1);}finally{f.close();}}
});
test("all viewer navigation paths probe the renderer before navigating and skip dialogs",async()=>{
 for(const input of [{type:"navigate",url:"http://localhost:3000"},{type:"history",delta:-1},{type:"history",delta:1},{type:"reload"},{type:"reload",ignoreCache:true}]){const f=fixture();try{await f.send(input);assert.equal(f.calls[0][0],"Runtime.evaluate");assert(f.calls.length>1);f.calls.length=0;f.tab.dialog={};await f.send(input);assert.notEqual(f.calls[0][0],"Runtime.evaluate");}finally{f.close();}}
});
test("agent navigation charges a bounded renderer probe before goto",async t=>{
 let now=1000;t.mock.method(performance,"now",()=>now);const f=fixture();try{f.tab.page.waitForLoadState=async()=>{};const original=f.session.send;f.session.send=async(method,input)=>{if(method==="Runtime.evaluate")now+=40;return original(method,input);};await navigate(f.tab,{url:"http://localhost:3000",timeoutMs:100});assert.equal(f.calls[0][0],"Runtime.evaluate");assert.equal(f.calls.find(([method])=>method==="goto")[2].timeout,60);}finally{f.close();}
});
test("crash status survives blank replacement and a reload uses the full original URL",async()=>{
 const f=fixture();try{f.tab.loading=false;f.tab.crash={url:"http://localhost:3000/redacted",code:-2,description:"Page crashed"};f.tab.crashedUrl="http://localhost:3000/"+"x".repeat(2200);const state=await status(f.tab);assert.deepEqual(state.loadError,f.tab.crash);assert.equal(state.url,f.tab.crash.url);await f.send({type:"reload"});assert.equal(f.calls.find(([method])=>method==="goto")[1],f.tab.crashedUrl);}finally{f.close();}
});

test("Stop settles an initial request emitted before or after it without leaving an aborted timer",async()=>{
 for(const late of [false,true])for(const abort of [false,true]){
  const f=fixture();try{const events=new EventEmitter(),frame={};Object.assign(f.tab.page,{on:(name,fn)=>events.on(name,fn),mainFrame:()=>frame});f.tab.networkEntries=[];f.tab.pendingInitialRequestUrl="http://localhost:3000/initial";observeNavigation(f.tab);
   const request={isNavigationRequest:()=>true,frame:()=>frame,url:()=>"http://localhost:3000/initial#hash",method:()=>"GET",failure:()=>({errorText:"net::ERR_ABORTED"})};
   if(!late)events.emit("request",request);await f.send({type:"stopLoading"});if(late)events.emit("request",request);if(abort)events.emit("requestfailed",request);
   assert.equal(f.tab.loading,false);assert.equal(f.tab.aborted,null);assert.equal(f.tab.pendingInitialRequestUrl,null);
   await f.send({type:"navigate",url:"http://localhost:3000/initial"});events.emit("request",request);assert.equal(f.tab.loading,true);
  }finally{f.close();}
 }
});

test("initial appearance precedes goto and a pending initial navigation permits Stop",async()=>{
 playwright.reset();playwright.holdNavigation=true;
 const snapshot={threadId:"initial",tabId:"initial",profileId:null,headless:true,automationOwner:null,viewport:{_tag:"freeform",width:1200,height:900},colorScheme:"dark",zoomFactor:1.25,navStatus:{_tag:"Loading",url:"http://localhost:3000/start"}};
 const state=await dispatch({command:"open",snapshot,isolated:true});
 const page=playwright.pages[0];await page.gotoStarted.promise;const navigationIndex=playwright.trace.findIndex(([method])=>method==="goto");
 assert(navigationIndex>=0);assert(playwright.trace.slice(0,navigationIndex).some(([method,input])=>method==="media"&&input.colorScheme==="dark"));assert(playwright.trace.slice(0,navigationIndex).some(([method,input])=>method==="Emulation.setDeviceMetricsOverride"&&input.width===960));
 assert.equal(state.initialPending,true);assert.equal(state.loading,true);
 const key={threadId:"initial",tabId:"initial"};const tab=tabs.get(JSON.stringify([key.threadId,key.tabId]));tab.viewers.set("stop",{id:"stop",session:tab.cdp});
 await dispatch({command:"viewerInput",key,viewerId:"stop",input:{type:"stopLoading"}});assert.equal(tab.loading,false);
 tab.viewers.clear();page.navigation.resolve();await tab.initialNavigation;await dispatch({command:"close",key});
});
