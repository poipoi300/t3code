// Browser-only adapter. Session admission, ownership and event revisions belong to Rust.
import * as fs from "node:fs/promises";
import path from "node:path";
import crypto from "node:crypto";
import readline from "node:readline";
import {fileURLToPath} from "node:url";
import {ServerBrowserContexts,presentAsChrome} from "./contexts.mjs";
import * as Page from "./page.mjs";
import {installPdfDocumentNavigation,isPdfAssetUrl} from "./pdf.mjs";
import {publicProxy} from "./proxy.mjs";
import {withResources} from "./resources.mjs";
import {DesktopViewport} from "./desktop-viewport.mjs";
import {Screencast} from "./screencast.mjs";
import {recordingFrames} from "./recording-frames.mjs";
import {CLIPBOARD_SCRIPT,CLIPBOARD_WORLD,EDITABLE_AT_POINT_SCRIPT,editingCommand,num,modifiersOf,asRecord} from "./scripts.mjs";

const config=JSON.parse(process.env.T3_PREVIEW_CONFIG || "{}");
const host={platform:process.platform,arch:process.arch};
const noop=()=>{};
let output=Promise.resolve();
function write(message) {
  output=output.then(()=>new Promise((resolve,reject)=>{
    const bytes=JSON.stringify(message)+"\n";
    if(Buffer.byteLength(bytes)>150*1024*1024) return reject(new Error("Driver response limit"));
    process.stdout.write(bytes,error=>error?reject(error):resolve());
  }));
  output.catch(()=>process.exit(1));
}
const emit=(event,fields)=>write({event,...fields});
const keyOf=key=>JSON.stringify([key.threadId,key.tabId]);
const tabs=new Map();
const popups=new Map();
const contexts=new ServerBrowserContexts({
  profilesDir:config.profilesDir,
  executable:async()=>config.executable,
  env:{...process.env,T3CODE_SERVER_BROWSER_SANDBOX:config.sandbox?undefined:"0"},
  idleBrowserMs:5*60_000,
  onContextClose:context=>{for(const tab of tabs.values())if(tab.page.context()===context)emit("closed",{key:tab.key});},
});
const failure=(tag,message,detail)=>new Page.ServerBrowserOperationError({tag,message,detail});
const limited=(items,item,limit=200)=>{items.push(item);if(items.length>limit)items.splice(0,items.length-limit);};
const bounded=(promise,ms,fallback)=>{
  let timer;
  return Promise.race([promise,new Promise(resolve=>{timer=setTimeout(()=>resolve(fallback),ms);})]).finally(()=>clearTimeout(timer));
};
const expire=(promise,ms,message)=>{
  let timer;
  return Promise.race([promise,new Promise((_,reject)=>{timer=setTimeout(()=>reject(failure("PreviewAutomationTimeoutError",message)),ms);})]).finally(()=>clearTimeout(timer));
};
const viewerEvent=(tab,viewerId,message)=>emit("viewer",{key:tab.key,viewerId,output:message});
function dialogOf(tab) {return tab.dialog?{type:tab.dialog.type(),message:tab.dialog.message().slice(0,4000),defaultValue:tab.dialog.defaultValue().slice(0,4000)}:null;}
function broadcastDialog(tab) {viewerEvent(tab,null,{type:"dialogState",dialog:dialogOf(tab)});}
function report(tab,navStatus) {
  void tab.cdp.send("Page.getNavigationHistory").catch(()=>null).then(history=>{
    if(tab.closing||tab.crash)return;
    const index=history?.currentIndex??0;
    emit("status",{input:{...tab.key,navStatus,canGoBack:index>0,canGoForward:index<(history?.entries.length??0)-1}});
  });
}
async function reportLoaded(tab) {
  const url=tab.page.url();if(url==="about:blank"||url.startsWith("chrome-error://"))return;
  const generation=tab.navigationGeneration;
  const title=(await bounded(tab.page.title().catch(()=>""),250,"")).slice(0,512);
  if(tab.navigationGeneration!==generation||tab.loading||tab.page.url()!==url)return;
  report(tab,{_tag:"Success",url:Page.withoutDataPayload(url).slice(0,2048),title});
}
async function desktopSize(tab) {
  return tab.desktopSizing.layout();
}
async function status(tab) {
  const raw=Page.withoutDataPayload(tab.page.url());
  const loadError=tab.crash??(raw.startsWith("chrome-error://")?tab.loadError:undefined);
  const url=loadError?.url??(raw==="about:blank"?null:raw);
  const title=url===null||tab.dialog?null:await bounded(tab.page.title().catch(()=>""),250,"").then(value=>value||null);
  const viewport=tab.desktop?await bounded(tab.desktopSizing.status(),250,tab.desktopSizing.value):tab.page.viewportSize();
  return {url,title,loading:tab.loading,dialog:dialogOf(tab),fileChooser:tab.chooser?{multiple:tab.chooser.value.isMultiple(),accept:tab.chooser.accept}:null,...(viewport?{viewport}:{}),...(loadError?{loadError}:{}),downloads:tab.downloads.map(({fileName,path,sizeBytes,url,completedAt})=>({fileName,path,sizeBytes,url,completedAt})),...(tab.openError?{openError:tab.openError}:{})};
}
async function viewport(tab,setting) {
  tab.setting=setting;
  if(!tab.desktop){
    const size=setting?._tag!=="fill"?{width:setting.width,height:setting.height}:[...tab.viewers.values()].map(viewer=>viewer.requested).filter(Boolean).sort((a,b)=>b.order-a.order)[0]??{width:1280,height:800};
    await tab.page.setViewportSize({width:size.width,height:size.height});await zoom(tab);
  }
  await broadcastViewport(tab);
}
async function broadcastViewport(tab) {
  const size=tab.desktop?await desktopSize(tab):tab.page.viewportSize();
  if(!size||tab.closing)return;
  viewerEvent(tab,null,{type:"viewport",width:Math.max(1,Math.round(size.width/(tab.desktop?1:tab.zoomFactor))),height:Math.max(1,Math.round(size.height/(tab.desktop?1:tab.zoomFactor)))});
}
async function zoom(tab) {
  if(tab.desktop)return;
  const size=tab.page.viewportSize();if(!size)return;
  if(tab.zoomFactor===1){await tab.page.setViewportSize(size);return;}
  await tab.cdp.send("Emulation.setDeviceMetricsOverride",{width:Math.max(1,Math.round(size.width/tab.zoomFactor)),height:Math.max(1,Math.round(size.height/tab.zoomFactor)),deviceScaleFactor:2*tab.zoomFactor,mobile:false});
}
async function rendering(tab,snapshot) {
  const color=snapshot.colorScheme??"system";
  if(color!==tab.colorScheme){tab.colorScheme=color;await tab.page.emulateMedia({colorScheme:color==="system"?null:color});}
  const factor=snapshot.zoomFactor??1;
  if(!tab.desktop&&factor!==tab.zoomFactor){tab.zoomFactor=factor;await zoom(tab);await broadcastViewport(tab);}
}
async function recordCopies(cdp) {
  await cdp.send("Page.enable");await cdp.send("Page.addScriptToEvaluateOnNewDocument",{source:CLIPBOARD_SCRIPT,worldName:CLIPBOARD_WORLD,runImmediately:true});
}
async function recordFrame(tab,frame) {
  if(frame===tab.page.mainFrame()||tab.frameSessions.has(frame))return;
  const cdp=await tab.page.context().newCDPSession(frame).catch(()=>null);if(!cdp)return;
  if(tab.frameSessions.has(frame)||frame.isDetached()){await cdp.detach().catch(noop);return;}
  tab.frameSessions.set(frame,cdp);cdp.on("close",()=>{if(tab.frameSessions.get(frame)!==cdp)return;tab.frameSessions.delete(frame);if(!frame.isDetached())void recordFrame(tab,frame);});
  await recordCopies(cdp).catch(noop);
}
async function copied(cdp,since,until) {
  const tree=await cdp.send("Page.getFrameTree").catch(()=>null);if(!tree)return [];
  const ids=node=>[node.frame.id,...(node.childFrames??[]).flatMap(ids)];
  return Promise.all(ids(tree.frameTree).map(frameId=>cdp.send("Page.createIsolatedWorld",{frameId,worldName:CLIPBOARD_WORLD}).then(({executionContextId})=>cdp.send("Runtime.evaluate",{contextId:executionContextId,expression:`globalThis.takeCopy?.(${since}, ${until})`,returnByValue:true})).then(({result})=>typeof result.value==="string"?result.value:"",()=>"")));
}
function chooserCurrent(chooser) {return chooser.element().evaluate(element=>element.ownerDocument===globalThis.document).catch(()=>false);}
function closeChooser(tab) {if(!tab.chooser)return;viewerEvent(tab,null,{type:"fileChooserClosed",id:tab.chooser.id});tab.chooser=null;}
function offerChooser(tab) {
  const open=tab.chooser;if(!open||!tab.controller)return;
  if(open.offeredTo!==tab.controller){if(open.offeredTo)viewerEvent(tab,null,{type:"fileChooserClosed",id:open.id});open.id=crypto.randomUUID();open.offeredTo=tab.controller;}
  viewerEvent(tab,tab.controller,{type:"fileChooser",id:open.id,multiple:open.value.isMultiple(),accept:open.accept});
}
function updateChooserInterception(tab) {
  const intercept=!tab.desktop||tab.agentOperating||tab.controller!==null;
  if(intercept===tab.chooserIntercepted)return;
  tab.chooserIntercepted=intercept;
  if(intercept)tab.page.on("filechooser",tab.onChooser);else tab.page.off("filechooser",tab.onChooser);
}
async function saveDownload(tab,download) {
  const id=crypto.randomUUID();const directory=path.join(config.downloadsDir,crypto.createHash("sha256").update(`${tab.key.threadId}\0${tab.key.tabId}`).digest("hex"));
  const destination=path.join(directory,id);const generation=tab.aborted?.url===download.url()?tab.aborted.generation:null;
  if(generation!==null){clearTimeout(tab.aborted.timer);tab.aborted=null;}
  let offered;
  try {
    if(await download.failure())return;
    await fs.mkdir(directory,{recursive:true,mode:0o700});await fs.chmod(directory,0o700);
    await download.saveAs(destination);await fs.chmod(destination,0o600);
    const {size}=await fs.stat(destination);if(tab.closing||size>1024**3){await fs.rm(destination,{force:true});return;}
    const saved={id,fileName:download.suggestedFilename().slice(0,255)||"download",path:destination,sizeBytes:size,url:Page.withoutDataPayload(download.url()).slice(0,2048),completedAt:new Date().toISOString()};
    tab.downloads.push(saved);for(const old of tab.downloads.splice(0,tab.downloads.length-20))await fs.rm(old.path,{force:true}).catch(noop);
    offered={id,fileName:saved.fileName};
    if(!(generation===tab.navigationGeneration&&tab.page.url()==="about:blank")&&tab.controller)viewerEvent(tab,tab.controller,{type:"download",...offered,sizeBytes:size});
  }catch(error){await fs.rm(destination,{force:true}).catch(noop);emit("diagnostic",{message:`Server preview download failed: ${error}`});}
  finally {
    if(generation!==null&&!tab.closing)settleAborted(tab,generation,download.url(),offered);
    await download.delete().catch(noop);
  }
}
function settleAborted(tab,generation,url,download) {
  if(tab.navigationGeneration!==generation)return;tab.loading=false;
  if(tab.page.url()!=="about:blank"){void reportLoaded(tab);return;}
  report(tab,{_tag:"LoadFailed",url:Page.withoutDataPayload(url).slice(0,2048),title:"",code:-3,description:"ERR_ABORTED",...(download?{download}:{})});
}
function settleStopped(tab,generation) {
  if(tab.navigationGeneration!==generation||!tab.loading)return;
  clearTimeout(tab.aborted?.timer);tab.aborted=null;tab.loading=false;
  if(tab.page.url()==="about:blank")report(tab,{_tag:"Idle"});else void reportLoaded(tab);
}
function observeNavigation(tab) {
  const page=tab.page,generations=new WeakMap(),main=request=>request.isNavigationRequest()&&request.frame()===page.mainFrame();
  page.on("request",request=>{
    if(!main(request))return;
    const stopped=tab.pendingInitialRequestUrl===request.url().split("#")[0]&&tab.stoppedGeneration===tab.navigationGeneration;
    tab.pendingInitialRequestUrl=null;generations.set(request,++tab.navigationGeneration);clearTimeout(tab.aborted?.timer);tab.aborted=null;
    if(stopped){tab.stoppedGeneration=tab.navigationGeneration;tab.loading=true;settleStopped(tab,tab.navigationGeneration);return;}
    tab.crash=null;tab.crashedUrl=null;tab.loading=true;tab.loadError=null;report(tab,{_tag:"Loading",url:Page.withoutDataPayload(request.url()).slice(0,2048),title:""});
  });
  page.on("load",()=>{tab.pendingInitialRequestUrl=null;tab.loading=false;void reportLoaded(tab);});
  page.on("framenavigated",frame=>{const open=tab.chooser;if(open)void chooserCurrent(open.value).then(current=>{if(!current&&tab.chooser?.value===open.value)closeChooser(tab);});if(frame===page.mainFrame()&&!tab.loading)void reportLoaded(tab);if(!tab.desktop)void recordFrame(tab,frame);});
  page.on("requestfailed",request=>{
    const errorText=request.failure()?.errorText??"";
    limited(tab.networkEntries,{url:Page.withoutDataPayload(request.url()),method:request.method(),status:null,failed:true,errorText,timestamp:new Date().toISOString()});
    if(!main(request)||(generations.get(request)??0)!==tab.navigationGeneration)return;
    if(errorText.includes("ERR_ABORTED")){
      const url=request.url(),generation=tab.navigationGeneration;
      if(tab.stoppedGeneration===generation){settleStopped(tab,generation);return;}
      clearTimeout(tab.aborted?.timer);tab.aborted={url,generation,timer:setTimeout(()=>{if(tab.aborted?.generation!==generation)return;tab.aborted=null;if(!tab.closing)settleAborted(tab,generation,url);},5000)};return;
    }
    tab.loading=false;const {code,description}=Page.parseNetError(errorText);const url=Page.withoutDataPayload(request.url()).slice(0,2048);tab.loadError={url,code,description};report(tab,{_tag:"LoadFailed",url,title:"",code,description});
  });
}
async function open(command) {
  const snapshot=command.snapshot;const key={threadId:snapshot.threadId,tabId:snapshot.tabId};
  if(tabs.has(keyOf(key)))return status(tabs.get(keyOf(key)));
  let context,page,cdp,desktop,isolated=command.isolated;
  try {
    const popup=command.popupId?popups.get(command.popupId):null;
    if(command.popupId&&!popup)throw failure("PreviewAutomationTabNotFoundError","Popup closed before adoption.");
    if(popup){popups.delete(command.popupId);page=popup;context=page.context();isolated=false;}
    else if(command.desktopEndpoint){desktop=await contexts.connectDesktopPage(command.desktopEndpoint);page=desktop.page;context=page.context();await page.emulateMedia({colorScheme:null});}
    else {context=await contexts.contextFor(snapshot.profileId??"default",isolated?keyOf(key):undefined);await context.grantPermissions(["clipboard-write"]);page=await context.newPage();}
    cdp=await context.newCDPSession(page);await installPdfDocumentNavigation(cdp);
    if(!desktop){await presentAsChrome(cdp,host);await recordCopies(cdp);}
    const tab={key,page,cdp,desktop,isolated,profileId:snapshot.profileId,setting:snapshot.viewport??{_tag:"fill"},colorScheme:"system",zoomFactor:1,viewers:new Map(),frameSessions:new Map(),downloads:[],chooser:null,dialog:null,controller:null,generation:0,loading:false,loadError:command.loadError??null,crash:snapshot.navStatus._tag==="LoadFailed"&&snapshot.navStatus.description==="Page crashed"?command.loadError:null,crashedUrl:command.crashedUrl??null,pendingInitialRequestUrl:null,stoppedGeneration:null,closing:false,recording:null,consoleEntries:[],networkEntries:[],actionTimeline:[],navigationGeneration:0,aborted:null,desktopViewport:null,captureTail:Promise.resolve(),copyReads:Promise.resolve(),copyKeyedUntil:0};
    page.setDefaultTimeout(15000);page.setDefaultNavigationTimeout(15000);
    if(!desktop)await viewport(tab,tab.setting);await rendering(tab,snapshot);
    if(desktop){tab.desktopSizing=new DesktopViewport(cdp,()=>tab.dialog!==null);const remeasure=()=>{tab.desktopSizing.changed();if(tab.viewers.size)void broadcastViewport(tab).catch(noop);};cdp.on("Page.frameResized",remeasure);cdp.on("Page.loadEventFired",remeasure);cdp.on("Page.javascriptDialogClosed",()=>{tab.dialog=null;broadcastDialog(tab);remeasure();});await cdp.send("Page.enable");}
    observeNavigation(tab);
    page.on("response",response=>limited(tab.networkEntries,{url:Page.withoutDataPayload(response.url()),method:response.request().method(),status:response.status(),failed:false,timestamp:new Date().toISOString()}));
    page.on("console",message=>limited(tab.consoleEntries,{level:message.type(),text:Page.consoleText(message).slice(0,2000),timestamp:new Date().toISOString()}));
    page.on("dialog",dialog=>{if(tab.closing&&dialog.type()==="beforeunload"){void dialog.accept().catch(noop);return;}tab.dialog=dialog;broadcastDialog(tab);});
    page.on("download",download=>void saveDownload(tab,download));
    let chooserTail=Promise.resolve();
    tab.onChooser=chooser=>{chooserTail=chooserTail.then(async()=>{const accept=await chooser.element().getAttribute("accept").catch(()=>"");if(!await chooserCurrent(chooser))return;closeChooser(tab);tab.chooser={id:crypto.randomUUID(),value:chooser,accept:(accept??"").slice(0,1024),offeredTo:null};offerChooser(tab);}).catch(noop);};updateChooserInterception(tab);
    page.on("popup",popup=>{if(tab.closing){void popup.close().catch(noop);return;}const popupId=crypto.randomUUID();popups.set(popupId,popup);emit("popup",{opener:key,popupId,url:/^https?:/i.test(popup.url())?popup.url():null});});
    page.on("close",()=>{if(!tab.closing&&!tab.crash)emit("closed",{key});});page.on("crash",()=>{if(tab.closing||tab.crash)return;if(desktop){emit("closed",{key});return;}tab.loading=false;tab.crash={url:Page.withoutDataPayload(page.url()).slice(0,2048),code:-2,description:"Page crashed"};emit("crashed",{key,url:page.url()});});
    tabs.set(keyOf(key),tab);
    if(popup)for(const frame of page.frames())void recordFrame(tab,frame);
    if(!popup&&snapshot.navStatus._tag==="Loading"&&(!desktop||isPdfAssetUrl(snapshot.navStatus.url))){
      tab.loading=true;tab.pendingInitialRequestUrl=snapshot.navStatus.url.split("#")[0];
      tab.initialNavigation=navigate(tab,{url:snapshot.navStatus.url,timeoutMs:15000},true).catch(error=>{tab.openError={tag:"PreviewAutomationExecutionError",message:String(error)};throw error;});
      void tab.initialNavigation.catch(noop);
    }else if(desktop)void page.waitForLoadState("load").then(()=>reportLoaded(tab),noop);
    return {...await status(tab),initialPending:!!tab.initialNavigation};
  }catch(error){if(desktop)await desktop.browser.close().catch(noop);else {await page?.close().catch(noop);if(isolated)await context?.close().catch(noop);}throw error;}
}
function assertNavigation(tab) {
  if(!tab.page.url().startsWith("chrome-error://")||!tab.loadError)return;
  const detail={...tab.loadError};let url;
  try{const parsed=new URL(detail.url);parsed.username="";parsed.password="";parsed.search="";parsed.hash="";url=Page.withoutDataPayload(parsed.href);}catch{url="the requested page";}
  throw failure("PreviewAutomationExecutionError",`Navigation to ${url} failed: ${detail.description}`,{...detail,url});
}
async function leaveBusyPage(tab,cdp,budgetMs) {if(!tab.dialog)await Page.stopUnresponsiveScript(cdp,budgetMs);}
async function navigate(tab,input,initial=false) {
  if(!initial)tab.pendingInitialRequestUrl=null;
  const timeout=input.timeoutMs??15000;const started=performance.now();
  await leaveBusyPage(tab,tab.cdp,timeout/2);
  const navigation=tab.page.goto(input.url,{timeout:Math.max(1,timeout-(performance.now()-started)),waitUntil:"commit"});
  if(input.readiness==="none")return pendingWork(tab,navigation);
  await navigation;
  await tab.page.waitForLoadState(input.readiness==="domContentLoaded"?"domcontentloaded":"load",{timeout:Math.max(1,timeout-(performance.now()-started))}).catch(error=>{if(error.name!=="TimeoutError")throw error;});
  assertNavigation(tab);return status(tab);
}
async function closeTab(key,unload=false) {
  const tab=tabs.get(keyOf(key));if(!tab)return;
  tabs.delete(keyOf(key));tab.closing=true;clearTimeout(tab.aborted?.timer);
  for(const viewer of tab.viewers.values())await detachViewer(tab,viewer);
  if(tab.desktop){
    if(unload&&tab.dialog)await bounded(tab.page.close().catch(noop),5000);
    else if(unload) {
      let unloaded;const pagehide=new Promise(resolve=>{unloaded=resolve;});
      const binding=`__t3PreviewUnloaded${crypto.randomUUID().replaceAll("-","")}`;
      void (async()=>{await tab.page.exposeFunction(binding,unloaded);await tab.page.evaluate(name=>{globalThis.addEventListener("pagehide",()=>Reflect.apply(Reflect.get(globalThis,name),globalThis,[]));},binding);await tab.page.close({runBeforeUnload:true});})().catch(unloaded);
      await bounded(pagehide,5000);
    }
    await tab.desktop.browser.close().catch(noop);
  }
  else {
    const closed=tab.page.isClosed()?Promise.resolve():new Promise(resolve=>tab.page.once("close",resolve));
    const timer=setTimeout(()=>void tab.page.close().catch(noop),5000);
    void tab.page.close({runBeforeUnload:tab.dialog===null&&!tab.crash}).catch(()=>tab.page.close()).catch(noop);
    await closed.finally(()=>clearTimeout(timer));if(tab.isolated)await tab.page.context().close().catch(noop);
  }
  await tab.recording?.encoder.close().catch(noop);
  const directory=path.join(config.downloadsDir,crypto.createHash("sha256").update(`${key.threadId}\0${key.tabId}`).digest("hex"));await fs.rm(directory,{recursive:true,force:true}).catch(noop);
}
async function captureLock(tab,run) {
  const capture=tab.captureTail.then(async()=>{if(tab.closing)throw new Error("Preview tab closed");return run();});tab.captureTail=capture.then(noop,noop);return capture;
}
async function snapshot(tab,input) {
  return captureLock(tab,async()=>{
    for(const viewer of tab.viewers.values())await viewer.stream.pause();
    await tab.recording?.session.send("Page.stopScreencast").catch(noop);
    try {
      const result=await Page.snapshot({page:tab.page,cdp:tab.cdp,renderScale:tab.desktop?"display":2,fullScreenshot:input.fullScreenshot,consoleEntries:tab.consoleEntries,networkEntries:tab.networkEntries,actionTimeline:tab.actionTimeline});
      return tab.page.url().startsWith("chrome-error://")&&tab.loadError?{...result,url:tab.loadError.url,loadError:tab.loadError}:result;
    }finally{
      for(const viewer of tab.viewers.values())await viewer.stream.resume();
      if(tab.recording)await tab.recording.session.send("Page.startScreencast",{format:"jpeg",quality:90,everyNthFrame:1}).catch(noop);
    }
  });
}
async function pointer(tab,point,phase) {
  if(tab.viewers.size===0&&!tab.recording&&!tab.desktop)return;
  const show=kind=>{viewerEvent(tab,null,{type:"pointer",phase:kind,...point,sequence:++pointerSequence});void tab.recording?.encoder.evaluate(([x,y,click])=>globalThis.__t3Recorder?.cursor(x,y,click),[point.x,point.y,kind==="click"]).catch(noop);};
  show("move");await new Promise(resolve=>setTimeout(resolve,160));if(phase==="click"){show("click");await new Promise(resolve=>setTimeout(resolve,40));}
}
let pointerSequence=0;
async function startRecording(tab,input) {
  return captureLock(tab,async()=>{
    if(tab.recording)return {tabId:tab.key.tabId,recording:true,startedAt:tab.recording.startedAt};
    const encoder=await contexts.scratchPage();let session;
    try {
      await encoder.evaluate(Page.recordingEncoderScript(input.frameRate??30));
      const first=await Page.captureViewport(tab.cdp,{format:"jpeg",quality:90});
      await encoder.evaluate(([data,width])=>globalThis.__t3Recorder.frame(data,width),[first,tab.page.viewportSize()?.width??1280]);
      session=await tab.page.context().newCDPSession(tab.page);const frames=recordingFrames(session,encoder,()=>tab.page.viewportSize()?.width);
      await session.send("Page.startScreencast",{format:"jpeg",quality:90,everyNthFrame:1});
      if(tab.closing)throw new Error("Tab closed while recording started");
      tab.recording={encoder,session,frames,startedAt:new Date().toISOString()};return {tabId:tab.key.tabId,recording:true,startedAt:tab.recording.startedAt};
    }catch(error){await encoder.close().catch(noop);await session?.detach().catch(noop);throw error;}
  });
}
async function stopRecording(tab) {
  return captureLock(tab,async()=>{
    const recording=tab.recording;if(!recording)throw failure("PreviewAutomationRecordingNotActiveError","No recording is active for this tab.");
    let published;
    try {
      await recording.session.send("Page.stopScreencast").catch(noop);await Promise.all(recording.frames);await recording.session.detach().catch(noop);
      if(!tab.closing&&!tab.dialog){const final=await Page.captureFinalRecordingFrame(tab.cdp,90);if(final)await recording.encoder.evaluate(([data,width])=>globalThis.__t3Recorder.frame(data,width),[final,tab.page.viewportSize()?.width??1280]).catch(noop);}
      const {mimeType,count,bytes}=await recording.encoder.evaluate(()=>globalThis.__t3Recorder.stop());
      if(bytes>50*1024*1024)throw failure("PreviewAutomationRecordingTooLargeError","The recording is larger than the attachment limit.");
      if(!mimeType||!bytes)throw failure("PreviewAutomationExecutionError","The recording captured no frames.");
      const extension=mimeType.startsWith("video/mp4")?"mp4":"webm";const id=`pending-${crypto.randomUUID()}-${extension}`;
      published=path.join(config.attachmentsDir,`${id}.${extension}`);await fs.mkdir(config.attachmentsDir,{recursive:true});
      if(bytes>=8*1024*1024){const staged=published+".partial";try{const [download]=await Promise.all([recording.encoder.waitForEvent("download",{timeout:5000}),recording.encoder.evaluate(()=>globalThis.__t3Recorder.download())]);await download.saveAs(staged);if((await fs.stat(staged)).size!==bytes)throw new Error("Recording download incomplete");await fs.rename(staged,published);}finally{await fs.rm(staged,{force:true}).catch(noop);}}
      else {const chunks=[];for(let index=0;index<count;index++)chunks.push(Buffer.from(await recording.encoder.evaluate(index=>globalThis.__t3Recorder.chunk(index),index),"base64"));const data=Buffer.concat(chunks);if(data.length!==bytes)throw new Error("Recording transfer incomplete");await fs.writeFile(published,data);}
      return {id,tabId:tab.key.tabId,path:published,mimeType:mimeType.split(";")[0],sizeBytes:bytes,createdAt:new Date().toISOString(),uploadedAttachmentId:id};
    }catch(error){if(published)await fs.rm(published,{force:true}).catch(noop);throw error;}
    finally{await recording.encoder.close().catch(noop);tab.recording=null;}
  });
}
async function operate(command) {
  const tab=tabs.get(keyOf(command.key));if(!tab)throw failure("PreviewAutomationTabNotFoundError","The server preview tab does not exist.");
  if(command.generation!==tab.generation){Page.invalidateRefs(tab.page);tab.generation=command.generation;}
  const input=command.input;const operation=command.operation;
  if(operation==="status")return status(tab);
  if(operation==="awaitInitialNavigation"){await tab.initialNavigation;return status(tab);}
  if(operation==="invalidate"){Page.invalidateRefs(tab.page);if(Object.hasOwn(input,"controller")){tab.controller=input.controller;updateChooserInterception(tab);offerChooser(tab);}return {};}
  if(operation==="waitPending"){const item=pending.get(input.id);if(item?.tab!==tab)return {};try{await item.work;}finally{pending.delete(input.id);}return {};}
  if(operation==="prepareResize")return tab.desktop?{resizeToken:tab.desktopSizing.prepare(tab.setting)}:{};
  if(operation==="download")return tab.downloads.find(download=>download.id===input.id)??null;
  if(operation==="dialog"){const dialog=tab.dialog;if(!dialog)throw new Error("No dialog is pending.");if(input.accept)await dialog.accept(input.promptText);else await dialog.dismiss();if(tab.dialog===dialog)tab.dialog=null;Page.invalidateRefs(tab.page);broadcastDialog(tab);return status(tab);}
  const event={id:crypto.randomUUID(),action:operation,status:"running",startedAt:new Date().toISOString()};
  const record=["navigate","click","type","hover","select","drag","upload","press","scroll"].includes(operation);if(record)limited(tab.actionTimeline,event,50);
  tab.agentOperating=!["snapshot","waitFor"].includes(operation);updateChooserInterception(tab);
  try {
    let result;
    switch(operation) {
      case "navigate": result=await navigate(tab,input);break;
      case "resize": {if(!tab.desktop)await viewport(tab,input.setting);else tab.setting=input.setting;const measured=tab.desktop?await tab.desktopSizing.resized(input.resizeToken,input.setting,command.timeoutMs):tab.page.viewportSize();result={tabId:tab.key.tabId,setting:input.setting,...(measured?{viewport:measured}:{})};break;}
      case "setColorScheme":await rendering(tab,{colorScheme:input.colorScheme,zoomFactor:tab.zoomFactor});result={tabId:tab.key.tabId,colorScheme:input.colorScheme};break;
      case "snapshot":result=await snapshot(tab,input);break;
      case "evaluate":result=await Page.evaluate(tab.cdp,input,command.timeoutMs);break;
      case "waitFor":result=await Page.waitFor(tab.page,input);break;
      case "recordingStart":result=await startRecording(tab,input);break;
      case "recordingStop":result=await stopRecording(tab);break;
      case "click":result=await Page.click(tab.page,input,(point,phase)=>pointer(tab,point,phase));break;
      case "hover":result=await Page.hover(tab.page,input,(point,phase)=>pointer(tab,point,phase));break;
      case "drag":result=await Page.drag(tab.page,input,(point,phase)=>pointer(tab,point,phase));break;
      case "select":result=await Page.select(tab.page,input);break;
      case "type":result=await Page.type(tab.page,input);break;
      case "press":result=await Page.press(tab.page,input);break;
      case "scroll":result=await Page.scroll(tab.page,input);break;
      case "upload":
        if(input.paths.some(file=>!path.isAbsolute(file)))throw new Error("Upload paths must be absolute.");
        if(!await Page.setInputFiles(tab.page,input)){const chooser=tab.chooser;if(!chooser)throw new Error("No file picker is open. Click its upload control or pass the file input's locator.");if(input.paths.length>1&&!chooser.value.isMultiple())throw new Error("This file picker accepts one file.");if(input.paths.length)await chooser.value.setFiles(input.paths,{timeout:input.timeoutMs??15000});if(tab.chooser===chooser)closeChooser(tab);}break;
      default:throw new Error(`Unsupported browser operation: ${operation}`);
    }
    if(record){event.status="succeeded";event.completedAt=new Date().toISOString();}return result??{};
  }catch(error){if(record){event.status="failed";event.completedAt=new Date().toISOString();event.error=String(error.message??error).split("\n")[0];}throw error;}
  finally{tab.agentOperating=false;updateChooserInterception(tab);if(command.generation!==tab.generation)Page.invalidateRefs(tab.page);}
}
let resizeOrder=0;
async function attachViewer(command) {
  const tab=tabs.get(keyOf(command.key));if(!tab)throw failure("PreviewAutomationTabNotFoundError","Preview tab closed.");
  const session=await tab.page.context().newCDPSession(tab.page);
  const viewer={id:command.viewerId,session,pressedKeys:new Map(),pressedButtons:new Map(),requested:null,copyReads:Promise.resolve()};
  tab.viewers.set(viewer.id,viewer);
  const still=()=>captureLock(tab,async()=>{const data=await bounded(Page.captureViewport(session,{format:"jpeg",quality:command.quality}),2000,null).catch(()=>null);if(data&&!tab.closing)viewerEvent(tab,viewer.id,{type:"frame",data,frameId:0});});
  viewer.stream=new Screencast(session,command,(data,id)=>viewerEvent(tab,viewer.id,{type:"frame",data,frameId:id}),still);
  await broadcastViewport(tab);await viewer.stream.start(1);if(!viewer.stream.started)void still();
  return {};
}
async function ackViewer(command) {
  const viewer=tabs.get(keyOf(command.key))?.viewers.get(command.viewerId);await viewer?.stream.ack(command.frameId);return {};
}
async function releaseInput(viewer) {
  for(const {key,code} of viewer.pressedKeys.values())await viewer.session.send("Input.dispatchKeyEvent",{type:"keyUp",key,code}).catch(noop);viewer.pressedKeys.clear();
  for(const [button,point] of viewer.pressedButtons)await viewer.session.send("Input.dispatchMouseEvent",{type:"mouseReleased",button,...point,buttons:0,clickCount:1}).catch(noop);viewer.pressedButtons.clear();viewer.requested=null;
}
async function detachViewer(tab,viewer) {viewer.stream.close();await releaseInput(viewer);await viewer.stream.pause();await viewer.session.detach().catch(noop);tab.viewers.delete(viewer.id);if(tab.controller===viewer.id)tab.controller=null;updateChooserInterception(tab);}
async function viewerInput(command) {
  const tab=tabs.get(keyOf(command.key));const viewer=tab?.viewers.get(command.viewerId);if(!viewer)return {};
  const message=asRecord(command.input);if(!message)return {};
  const session=viewer.session;const modifiers=modifiersOf(message);
  tab.controller=viewer.id;updateChooserInterception(tab);offerChooser(tab);
  switch(message.type) {
    case "releaseInput":await releaseInput(viewer);if(tab.controller===viewer.id)tab.controller=null;updateChooserInterception(tab);break;
    case "mouse": {
      const type=message.action==="down"?"mousePressed":message.action==="up"?"mouseReleased":"mouseMoved";
      const button=["none","left","middle","right"].includes(message.button)?message.button:"none";
      await session.send("Input.dispatchMouseEvent",{type,x:num(message.x),y:num(message.y),button,buttons:num(message.buttons),clickCount:num(message.clickCount,type==="mouseMoved"?0:1),modifiers});
      if(button!=="none"){if(message.action==="down")viewer.pressedButtons.set(button,{x:num(message.x),y:num(message.y)});else if(message.action==="up")viewer.pressedButtons.delete(button);}break;
    }
    case "wheel":viewer.stream.scrolledAt=Date.now();await session.send("Input.dispatchMouseEvent",{type:"mouseWheel",x:num(message.x),y:num(message.y),deltaX:num(message.deltaX),deltaY:num(message.deltaY),modifiers});break;
    case "key": {
      const key=typeof message.key==="string"?message.key:"",code=typeof message.code==="string"?message.code:"";const text=typeof message.text==="string"?message.text:undefined;
      if(message.action==="up"){await session.send("Input.dispatchKeyEvent",{type:"keyUp",key,code,modifiers});viewer.pressedKeys.delete(code||key);break;}
      const command=editingCommand(key,modifiers);const copying=!tab.desktop&&["copy","cut"].includes(command?.commands[0]);
      if(copying)while(Date.now()<=tab.copyKeyedUntil)await new Promise(resolve=>setTimeout(resolve,Math.max(0,tab.copyKeyedUntil+1-Date.now())));
      const since=Date.now();
      await session.send("Input.dispatchKeyEvent",{type:text?"keyDown":"rawKeyDown",key,code,modifiers,...(text?{text,unmodifiedText:text}:{}),...command,windowsVirtualKeyCode:num(message.keyCode,key.length===1?key.toUpperCase().charCodeAt(0):0)});
      viewer.pressedKeys.set(code||key,{key,code});
      if(copying){
        const until=Date.now();
        tab.copyKeyedUntil=until;
        const requestId=typeof message.requestId==="string" && message.requestId.length>0 && message.requestId.length<=128
          ? message.requestId : undefined;
        tab.copyReads=tab.copyReads.then(async()=>{
          const copies=await Promise.all([tab.cdp,...tab.frameSessions.values()].map(cdp=>copied(cdp,since,until)));
          const text=copies.flat().find(Boolean)??"";
          if(text || requestId)viewerEvent(tab,viewer.id,{type:"clipboard",text,requestId});
        }).catch(noop);
        return pendingWork(tab,tab.copyReads);
      }
      break;
    }
    case "text":if(typeof message.text==="string"&&message.text.length)await session.send("Input.insertText",{text:message.text.slice(0,10000)});break;
    case "resize": {
      const width=Math.min(Math.round(num(message.width)),3840),height=Math.min(Math.round(num(message.height)),2160);if(width<100||height<100)break;
      viewer.requested={width,height,order:++resizeOrder};if(tab.setting._tag!=="fill"||tab.desktop)break;await tab.page.setViewportSize({width,height});await broadcastViewport(tab);break;
    }
    case "viewport":await viewport(tab,message.setting);break;
    case "stopLoading":{const generation=tab.navigationGeneration;tab.stoppedGeneration=generation;await session.send("Page.stopLoading");settleStopped(tab,generation);break;}
    case "navigate":tab.pendingInitialRequestUrl=null;await leaveBusyPage(tab,session);return pendingWork(tab,tab.page.goto(message.url,{waitUntil:"commit",timeout:15000}));
    case "history":tab.pendingInitialRequestUrl=null;await leaveBusyPage(tab,session);return pendingWork(tab,num(message.delta)<0?tab.page.goBack({waitUntil:"commit",timeout:15000}):tab.page.goForward({waitUntil:"commit",timeout:15000}));
    case "reload":tab.pendingInitialRequestUrl=null;await leaveBusyPage(tab,session);return pendingWork(tab,tab.crashedUrl?tab.page.goto(tab.crashedUrl,{waitUntil:"commit",timeout:15000}):message.ignoreCache===true?session.send("Page.reload",{ignoreCache:true}):tab.page.reload({waitUntil:"commit",timeout:15000}));
    case "probe": {const x=num(message.x),y=num(message.y);const result=await session.send("Runtime.evaluate",{expression:`(${EDITABLE_AT_POINT_SCRIPT})(${x},${y})`,returnByValue:true});viewerEvent(tab,viewer.id,{type:"probe",x,y,editable:result.result.value===true});break;}
  }return {};
}
const pending=new Map();
function pendingWork(tab,work){const id=crypto.randomUUID();pending.set(id,{tab,work:bounded(Promise.resolve(work).catch(noop),16000)});return {pending:id};}
async function answerChooser(command) {
  const tab=tabs.get(keyOf(command.key));const open=tab?.chooser;
  if(!open||open.id!==command.chooserId||!open.offeredTo||open.offeredTo!==tab.controller)return false;
  const files=command.files.map(({name,mimeType,data})=>({name,mimeType,buffer:Buffer.from(data,"base64")}));
  if(files.length)await open.value.setFiles(open.value.isMultiple()?files:files.slice(0,1));if(tab.chooser===open)closeChooser(tab);return true;
}

const PAGE_ORIGIN="http://t3-page.localhost",PAGE_URL=PAGE_ORIGIN+"/page.html";
async function renderHtml(command) {
  return withResources(command.command==="htmlCapture"?19000:5500,async own=>{
    const proxy=await own(await publicProxy(),proxy=>proxy.close());
    const {createRequire}=await import("node:module");const require=createRequire(path.join(process.cwd(),"package.json"));const {chromium}=require(process.env.T3_PREVIEW_PLAYWRIGHT_MODULE||"playwright-core");
    const browser=await own(await chromium.launch({executablePath:config.executable,headless:true,chromiumSandbox:config.sandbox,args:["--disable-gpu","--hide-scrollbars","--mute-audio","--block-new-web-contents",`--proxy-server=socks5://127.0.0.1:${proxy.port}`,"--proxy-bypass-list=<-loopback>","--force-webrtc-ip-handling-policy=disable_non_proxied_udp"]}),browser=>browser.close());
    const context=await browser.newContext({deviceScaleFactor:1,acceptDownloads:false});
    await context.addInitScript("delete window.RTCPeerConnection; delete window.webkitRTCPeerConnection;");
    const render=async width=>{
      const page=await context.newPage();const cdp=await context.newCDPSession(page);let omitted=0;const messages=[];
      const append=(level,text)=>{if(!["log","info","warning","error"].includes(level))return;if(messages.length>=20){omitted++;return;}text=text.replaceAll(PAGE_URL,"page.html");if(text.length>500){const end=text.charCodeAt(499)>=0xd800&&text.charCodeAt(499)<=0xdbff?499:500;text=text.slice(0,end)+"…";}messages.push({level,text});};
      page.on("console",message=>append(({debug:"log",dir:"log",dirxml:"log",table:"log",trace:"log",assert:"error"})[message.type()]??message.type(),message.text()));
      page.on("pageerror",error=>append("error",String(error)));page.on("popup",popup=>void popup.close().catch(noop));
      const {frameTree}=await cdp.send("Page.getFrameTree");
      cdp.on("Fetch.requestPaused",event=>void(async()=>{
        const url=event.request.url.split("#")[0];
        if(url===PAGE_URL){await cdp.send("Fetch.fulfillRequest",{requestId:event.requestId,responseCode:200,responseHeaders:[{name:"Content-Type",value:"text/html; charset=utf-8"}],body:Buffer.from(command.html).toString("base64")});return;}
        if(event.resourceType==="Document"&&event.frameId!==frameTree.frame.id&&!url.startsWith(PAGE_ORIGIN+"/"))await cdp.send("Fetch.continueRequest",{requestId:event.requestId});
        else await cdp.send("Fetch.failRequest",{requestId:event.requestId,errorReason:"AccessDenied"});
      })().catch(()=>cdp.send("Fetch.failRequest",{requestId:event.requestId,errorReason:"Failed"}).catch(noop)));
      await cdp.send("Fetch.enable",{patterns:[{urlPattern:PAGE_ORIGIN+"/*"},{urlPattern:"*",resourceType:"Document"}]});
      await page.setViewportSize({width,height:800});await page.emulateMedia({colorScheme:command.appearance??"dark"});
      await page.goto(PAGE_URL+command.fragment,{waitUntil:"load",timeout:18000});
      await page.evaluate("document.fonts.ready.then(() => new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>resolve(true)))))");
      const contentHeight=Math.max(0,Math.ceil(await page.evaluate("(()=>{const root=document.documentElement;return root.scrollHeight>root.clientHeight?root.scrollHeight:root.getBoundingClientRect().height;})()")));
      if(command.command==="htmlMeasure"){await page.close();return [width,contentHeight];}
      const capturedHeight=Math.max(1,Math.min(contentHeight,4000));const {data:png}=await cdp.send("Page.captureScreenshot",{format:"png",captureBeyondViewport:true,clip:{x:0,y:0,width,height:capturedHeight,scale:1}});
      if(omitted)messages.push({level:"warning",text:`${omitted} more console messages were omitted.`});
      for(const [host,address]of proxy.fakeIpHosts)append("warning",`DNS returned blocked 198.18.0.0/15 addresses, which may come from Clash or Mihomo fake-IP DNS. Inline the resource, add its host to fake-ip-filter, or switch DNS to redir-host. Affected host: ${host} (${address})`);
      return {png,contentHeight,capturedHeight,consoleMessages:messages};
    };
    if(command.command==="htmlCapture")return await render(command.width);
    const widths=[...command.widths];const results=[];let at=0;await Promise.all(Array.from({length:Math.min(3,widths.length)},async()=>{for(;;){const index=at++;if(index>=widths.length)return;results[index]=await render(widths[index]);}}));return results;
  },()=>failure("PreviewAutomationTimeoutError","HTML rendering timed out."));
}
async function dispatch(command) {
  switch(command.command) {
    case "executable":config.executable=command.path;return {};
    case "open":return open(command);
    case "discardPopup": {const popup=popups.get(command.popupId);popups.delete(command.popupId);await popup?.close().catch(noop);return {};}
    case "operate":return operate(command);
    case "adjust": {const tab=tabs.get(keyOf(command.snapshot));if(!tab)return {};await viewport(tab,command.snapshot.viewport??{_tag:"fill"});await rendering(tab,command.snapshot);if(command.request?.clear==="cookies")await tab.cdp.send("Network.clearBrowserCookies");if(command.request?.clear==="cache")await tab.cdp.send("Network.clearBrowserCache");if(command.request?.hardReload){tab.pendingInitialRequestUrl=null;await leaveBusyPage(tab,tab.cdp);await tab.cdp.send("Page.reload",{ignoreCache:true});}return {};}
    case "close":await closeTab(command.key,command.unload);return {};
    case "clearProfile":await contexts.clearProfile(command.profileId);return {};
    case "viewerAttach":return attachViewer(command);
    case "viewerAck":return ackViewer(command);
    case "viewerDetach": {const tab=tabs.get(keyOf(command.key));const viewer=tab?.viewers.get(command.viewerId);if(viewer)await detachViewer(tab,viewer);return {};}
    case "viewerInput":return viewerInput(command);
    case "answerFileChooser":return answerChooser(command);
    case "htmlCapture":case "htmlMeasure":return renderHtml(command);
    case "shutdown":for(const tab of [...tabs.values()])await closeTab(tab.key);for(const popup of popups.values())await popup.close().catch(noop);await contexts.close();return {};
    default:throw new Error("Unknown driver command");
  }
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
await fs.rm(config.downloadsDir,{recursive:true,force:true}).catch(error=>emit("diagnostic",{message:`Could not clear earlier downloads: ${error}`}));
const lines=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
lines.on("line",line=>{
  if(Buffer.byteLength(line)>150*1024*1024){process.exitCode=1;lines.close();return;}
  let command;try{command=JSON.parse(line);}catch{process.exitCode=1;lines.close();return;}
  void dispatch(command).then(result=>{write({id:command.id,ok:true,result:result??null});},cause=>{const error=Page.toOperationError(cause,command.timeoutMs);write({id:command.id,ok:false,error:{tag:error.tag,message:error.message,...(error.detail===undefined?{}:{detail:error.detail})}});});
});
lines.on("close",()=>void contexts.close().finally(()=>process.exit()));

}
export {dispatch,tabs,navigate,status,observeNavigation};
