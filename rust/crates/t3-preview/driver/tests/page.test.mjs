import {test} from "node:test";
import assert from "node:assert/strict";
import * as Page from "../page.mjs";

test("net errors preserve names containing digits",()=>{
  for(const [text,description,code]of [["net::ERR_CONNECTION_REFUSED","ERR_CONNECTION_REFUSED",-102],["page.goto: net::ERR_HTTP2_PROTOCOL_ERROR at https://localhost/","ERR_HTTP2_PROTOCOL_ERROR",-2],["net::ERR_TLS13_DOWNGRADE_DETECTED","ERR_TLS13_DOWNGRADE_DETECTED",-2]])assert.deepEqual(Page.parseNetError(text),{description,code});
});
test("data URLs never expose media parameters or payloads",()=>{
  for(const input of ["data:text/plain;token=SECRET,payload",'"da\\tta:text/plain,PAYLOAD"',"\x01da\tta:text/plain,SECRET","data:text/plain,head\u2028SECRET"]){const result=Page.withoutDataPayload(input);assert.equal(result,"data:text/plain,…");assert(!result.includes("SECRET"));}
  assert.equal(Page.withoutDataPayload("https://example.com/data:report,2026"),"https://example.com/data:report,2026");
});
test("console redaction preserves page-authored text but clips internal data URLs",()=>{
  const text="Blocked data:text/plain;key=SECRET,payload and more";
  assert.equal(Page.consoleText({args:()=>[],text:()=>text}),"Blocked data:text/plain,…");
  assert.equal(Page.consoleText({args:()=>[{}],text:()=>text}),text);
});
test("evaluation uses by-value results and retries only an unexecuted top-level await parse failure",async()=>{
  const calls=[];const cdp={send:async(method,params)=>{calls.push([method,params]);return calls.length===1?{result:{},exceptionDetails:{text:"Parse",scriptId:"4",exception:{className:"SyntaxError"}}}:{result:{value:{answer:42}}};}};
  assert.deepEqual(await Page.evaluate(cdp,{expression:"await 0; ({answer:42})",returnByValue:false},1000),{answer:42});assert.equal(calls.length,2);assert.equal(calls[0][1].returnByValue,true);assert.equal(calls[1][1].replMode,true);
});
test("runtime and browser SyntaxError rejections never rerun side effects",async()=>{
  for(const details of [{scriptId:"4",stackTrace:{},exception:{className:"SyntaxError",description:"runtime failure"}},{exception:{className:"SyntaxError",description:"browser failure"}}]){
    let calls=0;const cdp={send:async()=>{calls++;return {result:{},exceptionDetails:details};}};await assert.rejects(Page.evaluate(cdp,{expression:"await sideEffect()"},1000));assert.equal(calls,1);
  }
});
test("evaluation refuses oversized UTF-8 results",async()=>{
  const cdp={send:async()=>({result:{value:"é".repeat(33000)}})};await assert.rejects(Page.evaluate(cdp,{expression:"large()"},1000),error=>error.tag==="PreviewAutomationResultTooLargeError"&&error.detail.maximumBytes===64000);
});
test("timed-out evaluation stops execution and never retries a late parse failure",async context=>{
  context.mock.timers.enable({apis:["setTimeout"]});const classic=Promise.withResolvers();const calls=[];const cdp={send:(method)=>{calls.push(method);return method==="Runtime.evaluate"?classic.promise:Promise.resolve({});}};
  const result=assert.rejects(Page.evaluate(cdp,{expression:"await 0; 1"},10),error=>error.tag==="PreviewAutomationTimeoutError");context.mock.timers.tick(10);await result;
  classic.resolve({result:{},exceptionDetails:{scriptId:"4",exception:{className:"SyntaxError"}}});await classic.promise;await Promise.resolve();assert.deepEqual(calls,["Runtime.evaluate","Runtime.terminateExecution"]);
});
test("an unanswered wait condition and screenshot release at their deadlines",async context=>{
  context.mock.timers.enable({apis:["setTimeout"]});
  const page={locator:()=>({count:()=>new Promise(()=>{})})};const waiting=assert.rejects(Page.waitFor(page,{selector:"button",timeoutMs:300}),error=>error.tag==="PreviewAutomationTimeoutError");context.mock.timers.tick(300);await waiting;
  const frame=Promise.withResolvers();const capture=assert.rejects(Page.captureViewport({send:()=>frame.promise},{format:"png"}),error=>error.tag==="PreviewAutomationExecutionError"&&error.message.includes("not producing screenshot frames"));context.mock.timers.tick(3000);await capture;frame.resolve({data:"late"});await frame.promise;
});
test("native and foreign refs are refused before accessing a locator",async()=>{
  let accesses=0;const page={locator:()=>{accesses++;throw new Error("must not run");}};
  for(const locator of ["aria-ref=e1"," aria-ref=e1","css=body >> aria-ref=e1","aria-ref=t3-other-e1"]){await assert.rejects(Page.click(page,{locator}),error=>error.tag==="PreviewAutomationInvalidSelectorError"&&error.detail.staleRef);}
  assert.equal(accesses,0);
});
test("quoted attributes containing aria-ref engine text remain valid CSS",async()=>{
  let selected;const locator={scrollIntoViewIfNeeded:async()=>{},boundingBox:async()=>({x:0,y:0,width:10,height:10}),hover:async()=>{}};const page={locator:selector=>{selected=selector;return locator;}};
  await Page.hover(page,{selector:'button[data-example=" >> aria-ref=e1"]'});assert.equal(selected,'button[data-example=" >> aria-ref=e1"]');
});
function snapshotPage({dotted=[],text="",tree='- button "Save" [ref=e1]'}={}){
  const listeners=new Map();const frame={};const header=Buffer.alloc(24);header.writeUInt32BE(1280,16);header.writeUInt32BE(800,20);
  const cdp={send:async method=>{
    if(method==="Page.getFrameTree")return {frameTree:{frame:{id:"main"}}};
    if(method==="Page.createIsolatedWorld")return {executionContextId:1};
    if(method==="Runtime.evaluate")return {result:{value:dotted}};
    if(method==="Page.captureScreenshot")return {data:header.toString("base64")};
    throw new Error(method);
  }};
  const page={on:(type,fn)=>listeners.set(type,fn),frames:()=>[frame],mainFrame:()=>frame,viewportSize:()=>({width:1280,height:800}),evaluate:async()=>({url:"https://example.com",title:"Title",loading:false,visibleText:text,interactiveElements:[]}),ariaSnapshot:async()=>tree};
  return {page,cdp,listeners,input:{page,cdp,renderScale:1,consoleEntries:[],networkEntries:[],actionTimeline:[]}};
}
test("snapshots mask password values before truncation while preserving role and ref syntax",async()=>{
  const fixture=snapshotPage({dotted:["123","long-secret"],text:"PIN: 123. id 1234 "+"x".repeat(19990)+"long-secret",tree:'- textbox "PIN" [ref=e1]: 123\n- button "long-secret" [ref=e2]\n- link "Download" [ref=e3]:\n  - /url: data:text/plain;charset=SECRET,PAYLOAD'});
  const result=await Page.snapshot(fixture.input);assert(result.visibleText.includes("PIN: ••••••••. id 1234"));assert(!result.visibleText.includes("long-secret"));assert.equal(result.visibleText.length,20000);assert(!result.accessibilityTree.includes("long-secret"));assert(!result.accessibilityTree.includes("PAYLOAD"));assert(result.accessibilityTree.includes("/url: data:text/plain,…"));assert.match(result.accessibilityTree,/textbox "PIN" \[ref=t3-[^\]]+\]: ••••••••/);
});
test("each new snapshot and navigation revokes previously issued refs",async()=>{
  const fixture=snapshotPage();const first=await Page.snapshot(fixture.input);const old=String(first.accessibilityTree).match(/\[ref=([^\]]+)\]/)[1];const second=await Page.snapshot(fixture.input);assert(!second.accessibilityTree.includes(old));
  fixture.page.locator=()=>{throw new Error("stale ref reached locator");};await assert.rejects(Page.click(fixture.page,{locator:`aria-ref=${old}`}),error=>error.tag==="PreviewAutomationInvalidSelectorError");
  const current=String(second.accessibilityTree).match(/\[ref=([^\]]+)\]/)[1];fixture.listeners.get("framenavigated")();await assert.rejects(Page.click(fixture.page,{locator:`aria-ref=${current}`}),error=>error.tag==="PreviewAutomationInvalidSelectorError");
});
test("select and type refuse non-editable controls without sending input",async()=>{
  const locator={evaluate:async()=>null,count:async()=>0,last:()=>locator};
  const page={locator:()=>locator,keyboard:{insertText:async()=>assert.fail("typed into noneditable target")}};
  await assert.rejects(Page.select(page,{locator:"button",values:["x"]}),error=>error.tag==="PreviewAutomationTargetNotEditableError");await assert.rejects(Page.type(page,{text:"secret"}),error=>error.tag==="PreviewAutomationTargetNotEditableError");
});
test("drag rejection is observed while the destination bounding box is pending",async()=>{
  const box={x:0,y:0,width:20,height:20},end=Promise.withResolvers();const error=new Error("drag timed out");const page={locator:selector=>selector==="#card"?{scrollIntoViewIfNeeded:async()=>{},boundingBox:async()=>box,dragTo:()=>Promise.reject(error)}:{boundingBox:()=>end.promise}};
  const unhandled=[];const record=error=>unhandled.push(error);process.on("unhandledRejection",record);
  try{const result=Page.drag(page,{source:"#card",target:"#lane"}).catch(error=>error);await new Promise(resolve=>setImmediate(resolve));end.resolve(box);assert.equal(await result,error);assert.deepEqual(unhandled,[]);}finally{process.off("unhandledRejection",record);}
});
