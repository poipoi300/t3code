const noop=()=>{};
export class Screencast {
  constructor(session,options,onFrame,still){Object.assign(this,{session,options,onFrame,still});this.tail=Promise.resolve();this.scale=1;this.motion=false;this.dropped=false;this.recent=[];this.scrolledAt=0;this.frames=new Map();this.sequence=0;this.started=false;this.closed=false;this.paused=false;this.timer=null;session.on('Page.screencastFrame',frame=>this.frame(frame));}
  start(scale){this.scale=scale;return this.tail=this.tail.then(async()=>{if(this.closed||this.paused)return;await this.session.send('Page.stopScreencast').catch(noop);await this.session.send('Page.startScreencast',{format:'jpeg',quality:scale<1?Math.min(this.options.quality,50):this.options.quality,maxWidth:Math.max(1,Math.round(this.options.maxWidth*scale)),maxHeight:Math.max(1,Math.round(this.options.maxHeight*scale))}).catch(noop);});}
  pause(){this.paused=true;return this.tail=this.tail.then(()=>this.session.send('Page.stopScreencast').catch(noop));}
  resume(){this.paused=false;return this.start(this.scale);}
  frame(frame){
    if(this.closed)return;this.started=true;const arrived=Date.now();if(this.frames.size)this.dropped=true;const id=++this.sequence;this.frames.set(id,{nativeId:frame.sessionId,arrived});
    this.recent.push(arrived);while(this.recent[0]<arrived-300)this.recent.shift();
    if(!this.motion&&this.recent.length>=4&&arrived-this.scrolledAt<300){this.motion=true;void this.start(0.5);}
    else if(this.motion&&arrived-this.scrolledAt>200){this.motion=false;this.recent.length=0;void this.start(1);}
    clearTimeout(this.timer);this.timer=setTimeout(()=>{this.timer=null;if(this.closed||(!this.motion&&!this.dropped))return;this.dropped=false;if(this.motion){this.motion=false;this.recent.length=0;void this.start(1);}void this.still().catch(noop);},200);
    this.onFrame(frame.data,id);
  }
  async ack(id){const frame=this.frames.get(id);if(!frame||frame.acknowledged)return;frame.acknowledged=true;await new Promise(resolve=>setTimeout(resolve,Math.max(0,frame.arrived+100-Date.now())));this.frames.delete(id);if(!this.closed)await this.session.send('Page.screencastFrameAck',{sessionId:frame.nativeId}).catch(noop);}
  close(){this.closed=true;clearTimeout(this.timer);this.frames.clear();}
}
