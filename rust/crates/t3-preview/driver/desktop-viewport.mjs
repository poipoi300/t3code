const deferred=()=>{let resolve;const promise=new Promise(done=>{resolve=done;});return {promise,resolve};};
const within=(promise,ms)=>{let timer;return Promise.race([promise,new Promise(resolve=>{timer=setTimeout(()=>resolve(null),Math.max(0,ms));})]).finally(()=>clearTimeout(timer));};
export class DesktopViewport {
  constructor(cdp,dialog){this.cdp=cdp;this.dialog=dialog;this.value=null;this.stale=true;this.changes=0;this.read=Promise.resolve(null);this.next=null;this.drawn=deferred();this.resizes=new Map();this.sequence=0;}
  changed(){this.stale=true;this.changes++;this.drawn.resolve();this.drawn=deferred();}
  async readSize(){
    const read=async()=>{const {frameTree}=await this.cdp.send('Page.getFrameTree');const {executionContextId}=await this.cdp.send('Page.createIsolatedWorld',{frameId:frameTree.frame.id,worldName:'t3-preview-viewport'});return (await this.cdp.send('Runtime.evaluate',{expression:'({ width: innerWidth, height: innerHeight })',contextId:executionContextId,returnByValue:true})).result.value;};
    const size=await within(read().catch(()=>null),1000);
    if(size?.width>0&&size?.height>0){this.value={width:size.width,height:size.height};return this.value;}
    this.stale=true;return null;
  }
  measure(){return this.next??=this.read.then(()=>{this.next=null;this.stale=false;return this.read=this.readSize();});}
  async layout(){return this.dialog()?this.value:(await this.measure())??this.value;}
  async status(){const changes=this.changes;await (this.stale||this.dialog()?this.layout():this.read);if(this.changes!==changes)await this.layout();return this.value;}
  prepare(setting){const id=++this.sequence;this.resizes.set(id,{size:setting?._tag==='fill'?null:setting,drawn:this.drawn.promise});return id;}
  async resized(id,setting,timeoutMs){
    const old=this.resizes.get(id);this.resizes.delete(id);const deadline=performance.now()+timeoutMs;
    const to=setting._tag==='fill'?null:setting;
    if(old&&(old.size?.width!==to?.width||old.size?.height!==to?.height))await within(old.drawn,Math.min(1000,timeoutMs/2));
    const changes=this.changes;const measured=await within(this.measure(),deadline-performance.now());
    return measured&&this.changes===changes?measured:null;
  }
}
