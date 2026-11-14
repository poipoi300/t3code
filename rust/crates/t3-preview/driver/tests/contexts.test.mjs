import {test} from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import {fileURLToPath} from "node:url";
import fixture from "./playwright-fixture.cjs";
import {ServerBrowserContexts} from "../contexts.mjs";
process.env.T3_PREVIEW_PLAYWRIGHT_MODULE=fileURLToPath(new URL("./playwright-fixture.cjs",import.meta.url));
async function setup(options={}){fixture.reset();const directory=await fs.mkdtemp(path.join(os.tmpdir(),"t3-preview-context-test-"));const pool=new ServerBrowserContexts({profilesDir:directory,executable:async()=>"fake-chrome",env:{},...options});return {pool,directory,close:async()=>{await pool.close();await fs.rm(directory,{recursive:true,force:true});}};}
test("concurrent isolated tabs share a browser but never a storage context",async()=>{
  const test=await setup();try{const [a,reused,b,incognito]=await Promise.all([test.pool.contextFor("default","a"),test.pool.contextFor("default","a"),test.pool.contextFor("default","b"),test.pool.contextFor("incognito")]);assert.equal(a,reused);assert.equal(new Set([a,b,incognito]).size,3);assert.equal(fixture.launches.length,1);assert.equal(fixture.persistent.length,0);assert.equal(fixture.launches[0].chromiumSandbox,true);}finally{await test.close();}assert(fixture.contexts.every(context=>context.closed));
});
test("persistent profiles are encoded once and never share storage with isolated agent tabs",async()=>{
  const test=await setup();try{const human=await test.pool.contextFor("work/team");assert.equal(await test.pool.contextFor("work/team"),human);assert.notEqual(await test.pool.contextFor("work/team","agent"),human);assert.equal(fixture.persistent[0][0],path.join(test.directory,"work%2Fteam"));await test.pool.contextFor("..");assert.equal(fixture.persistent[1][0],path.join(test.directory,"%2E%2E"));}finally{await test.close();}
});
test("clearing closes persistent storage before deleting it and leaves isolated peers open",async()=>{
  const test=await setup();try{const human=await test.pool.contextFor("work/team");const agent=await test.pool.contextFor("work/team","agent");const file=path.join(test.directory,"work%2Fteam","Preferences");await fs.writeFile(file,"saved");const close=human.close;human.close=async()=>{assert.equal(await fs.readFile(file,"utf8"),"saved");await close();};await test.pool.clearProfile("work/team");await assert.rejects(fs.stat(file));assert(human.closed);assert(!agent.closed);assert.notEqual(await test.pool.contextFor("work/team"),human);}finally{await test.close();}
});
test("concurrent clears wait for one close and reopening waits for deletion",async()=>{
  const test=await setup();try{const human=await test.pool.contextFor("work");const gate=Promise.withResolvers(),started=Promise.withResolvers();let closes=0;human.close=async()=>{closes++;started.resolve();await gate.promise;human.closed=true;};const first=test.pool.clearProfile("work"),second=test.pool.clearProfile("work");await started.promise;let reopened=false;const reopening=test.pool.contextFor("work").then(value=>{reopened=true;return value;});assert(!reopened);assert.equal(closes,1);gate.resolve();await Promise.all([first,second,reopening]);assert(reopened);assert.equal(closes,1);}finally{await test.close();}
});
test("sandbox failures never trigger an automatic no-sandbox retry",async()=>{
  const test=await setup();try{fixture.fail=new Error("sandbox unavailable");await assert.rejects(test.pool.contextFor("incognito"),/sandbox unavailable/);assert.equal(fixture.launches.length,1);assert.equal(fixture.launches[0].chromiumSandbox,true);await test.pool.contextFor("incognito");assert.equal(fixture.launches.length,2);assert(fixture.launches.every(options=>options.chromiumSandbox));}finally{await test.close();}
});
test("only an explicit sandbox opt-out changes the launch option",async()=>{
  const test=await setup({env:{T3CODE_SERVER_BROWSER_SANDBOX:"0"}});try{await test.pool.contextFor("incognito");assert.equal(fixture.launches[0].chromiumSandbox,false);}finally{await test.close();}
});
test("a scratch page setup failure closes its context and preserves peers",async()=>{
  const test=await setup();try{const peer=await test.pool.contextFor("default","peer");const browser=fixture.browsers[0],create=browser.newContext;let scratch;browser.newContext=async()=>{scratch=await create();scratch.newPage=async()=>{throw new Error("newPage failed");};return scratch;};await assert.rejects(test.pool.scratchPage(),/newPage failed/);assert(scratch.closed);assert(!peer.closed);assert(!browser.closed);}finally{await test.close();}
});
test("a closed pool rejects future work and never relaunches",async()=>{
  const test=await setup();await test.pool.contextFor("default","agent");await test.close();await assert.rejects(test.pool.contextFor("default","agent"),/closed/);await assert.rejects(test.pool.scratchPage(),/closed/);assert.equal(fixture.launches.length,1);
});
