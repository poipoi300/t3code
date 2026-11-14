export function recordingFrames(session,encoder,width) {
  const frames=new Set();let pending=null;
  const deliver=async()=>{
    while(pending){
      const frame=pending;pending=null;
      const accepted=await encoder.evaluate(([data,width])=>globalThis.__t3Recorder.frame(data,width),frame).catch(()=>true);
      if(!accepted){pending=null;await session.send("Page.stopScreencast").catch(()=>{});}
    }
  };
  session.on("Page.screencastFrame",frame=>{
    void session.send("Page.screencastFrameAck",{sessionId:frame.sessionId}).catch(()=>{});
    pending=[frame.data,width()??frame.metadata.deviceWidth];
    if(frames.size)return;
    const delivered=deliver().finally(()=>frames.delete(delivered));frames.add(delivered);
  });
  return frames;
}
