// Late browser launches must close too, even after their caller's deadline has expired.
export async function withResources(timeoutMs, work, timeoutError) {
  const resources=[];
  let closed=false,timer;
  const own=async(value,dispose)=>{
    if(closed){await dispose(value);throw timeoutError();}
    resources.push(()=>dispose(value));return value;
  };
  const close=()=>{
    closed=true;
    const pending=resources.splice(0).reverse().map(dispose=>Promise.resolve().then(dispose));
    // One broken CDP connection must not prevent closing the independent SOCKS listener.
    return Promise.allSettled(pending);
  };
  try {
    return await Promise.race([
      work(own),
      new Promise((_,reject)=>{timer=setTimeout(()=>{void close();reject(timeoutError());},timeoutMs);}),
    ]);
  } finally {
    clearTimeout(timer);
    const cleanup=close();
    let cleanupTimer;
    await Promise.race([cleanup,new Promise(resolve=>{cleanupTimer=setTimeout(resolve,1000);})]).finally(()=>clearTimeout(cleanupTimer));
  }
}
