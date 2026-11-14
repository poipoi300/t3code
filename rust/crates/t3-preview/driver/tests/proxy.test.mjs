import {test} from "node:test";
import assert from "node:assert/strict";
import net from "node:net";
import os from "node:os";
import {publicProxy,isLocal,readRequest} from "../proxy.mjs";

test("public policy rejects every local and translated private network",()=>{
  for(const address of ["0.0.0.1","10.1.2.3","100.64.0.1","127.0.0.1","169.254.169.254","172.16.1.1","192.168.0.1","198.18.0.1","224.0.0.1","::1","::ffff:127.0.0.1","64:ff9b::7f00:1","64:ff9b::a00:1","2002:7f00:1::1","2001::1","fc00::1","fe80::1","ff00::1"])assert(isLocal(address,net.isIP(address)),address);
  for(const address of ["1.1.1.1","93.184.215.14","2606:4700:4700::1111","64:ff9b::101:101"])assert(!isLocal(address,net.isIP(address)),address);
});
test("public policy rejects the host's public interfaces as well",()=>{for(const entry of Object.values(os.networkInterfaces()).flat())if(entry)assert(isLocal(entry.address,entry.family==="IPv6"?6:4),entry.address);});
test("SOCKS parser waits for complete requests and refuses malformed headers",()=>{
  assert.equal(readRequest(Buffer.from([5,1,0,1])),"short");assert.equal(readRequest(Buffer.from([4,1,0,1,0,0,0,0,0,80])),undefined);assert.equal(readRequest(Buffer.from([5,1,1,1,0,0,0,0,0,80])),undefined);
  assert.deepEqual(readRequest(Buffer.from([5,1,0,1,127,0,0,1,0,80,42])),{command:1,host:"127.0.0.1",port:80,rest:Buffer.from([42])});
});
async function connect(proxy,target){
  return new Promise((resolve,reject)=>{
    const socket=net.connect(proxy.port,"127.0.0.1",()=>socket.write(Buffer.concat([Buffer.from([5,1,0]),target])));let data=Buffer.alloc(0);
    socket.on("error",reject);socket.on("data",chunk=>{data=Buffer.concat([data,chunk]);if(data.length>=12){socket.destroy();resolve(data[3]);}});socket.on("close",()=>{if(data.length<12)reject(new Error("proxy closed without reply"));});
  });
}
test("real SOCKS sockets refuse loopback literal and loopback DNS destinations",async()=>{
  const proxy=await publicProxy();try{
    assert.equal(await connect(proxy,Buffer.from([5,1,0,1,127,0,0,1,0,80])),2);
    const domain=Buffer.from("localhost");assert.equal(await connect(proxy,Buffer.concat([Buffer.from([5,1,0,3,domain.length]),domain,Buffer.from([0,80])])),2);
    assert.equal(await connect(proxy,Buffer.from([5,1,0,1,1,1,1,1,0,0])),7);
  }finally{await proxy.close();}
});
test("closing the proxy closes incomplete client handshakes too",async()=>{
  const proxy=await publicProxy();const socket=await new Promise(resolve=>{const socket=net.connect(proxy.port,"127.0.0.1",()=>resolve(socket));});
  const closed=new Promise(resolve=>socket.once("close",resolve));socket.on("error",()=>{});await proxy.close();await closed;assert(socket.closed);
});
