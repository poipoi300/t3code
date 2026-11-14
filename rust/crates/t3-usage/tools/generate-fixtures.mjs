// Run from the repository root with Node 26. Reads only source and this crate's fixtures.
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { claudeUsageFormat } from "../../../../apps/server/src/provider/Drivers/claudeUsage.ts";
import { codexUsageFormat } from "../../../../apps/server/src/provider/Drivers/codexUsage.ts";
import { grokUsageFormat } from "../../../../packages/provider-grok/src/server/usage.ts";
import { piUsageFormat } from "../../../../packages/provider-pi/src/server/usage.ts";
import { museUsageFormat } from "../../../../packages/provider-muse/src/server/usage.ts";
import { readTranscriptRecords } from "../../../../apps/server/src/usage/usageTranscriptReader.ts";
import { UsageAggregator, resolveModelAliases, canonicalModelFilter } from "../../../../apps/server/src/usage/usageAggregation.ts";
import { parseRateTable, priceUsage, createOverrideRateTable, cacheSavingsUsd } from "../../../../apps/server/src/usage/usagePricing.ts";
import { decodeScanCache, makeScanCacheWriter } from "../../../../apps/server/src/usage/usageScanCache.ts";
import { claudeUsageResponseToLimits } from "../../../../apps/server/src/provider/claudeUsageLimits.ts";
import { codexRateLimitsToLimits } from "../../../../apps/server/src/provider/codexUsageLimits.ts";
import { grokUsageResponseToLimits } from "../../../../packages/provider-grok/src/server/usageLimits.ts";
import { cursorUsageResponseToLimits } from "../../../../packages/provider-cursor/src/server/usageLimits.ts";
import * as zlib from "node:zlib";
import { DatabaseSync } from "node:sqlite";
import { readOpenCodeUsage } from "../../../../packages/provider-opencode/src/server/usage.ts";
import { readAntigravityUsage } from "../../../../apps/server/src/provider/Drivers/antigravityUsageReader.ts";
const require = createRequire(new URL("../../../../apps/server/package.json", import.meta.url));
const Effect = await import(pathToFileURL(require.resolve("effect/Effect")).href);
const NodeServices = await import(pathToFileURL(require.resolve("@effect/platform-node/NodeServices")).href);

const cursorCache = await import("../../../../packages/provider-cursor/src/server/accountCache.ts");
const forkRows = await import("../../../../packages/provider-opencode/src/server/usage.fixture.ts");
const dir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../fixtures");
await fs.mkdir(dir, { recursive: true });
const write = (name, value) => fs.writeFile(path.join(dir, name), JSON.stringify(value, null, 2) + "\n");
await write("opencode-forks.json", forkRows);
const formats = { claude: claudeUsageFormat, codex: codexUsageFormat, grok: grokUsageFormat, pi: piUsageFormat, muse: museUsageFormat };
const readers = [];
async function transcript(name, provider, lines, tail = false) {
  const content = lines.map(v => typeof v === "string" ? v : JSON.stringify(v)).join("\n") + (tail ? "" : "\n");
  const file = name + ".jsonl";
  await fs.writeFile(path.join(dir, file), content);
  const expected = await readTranscriptRecords(path.join(dir, file), formats[provider]);
  readers.push({ file, provider, expected });
  return expected;
}

// UsageService.test.ts's claudeCopy / PARTIAL_USAGE / COMPLETE_USAGE.
const claude = (id, output, extra = {}) => ({ type: "assistant", timestamp: "2026-08-01T10:00:00Z", requestId: `req_${id}`, sessionId: "session-1",
  message: { id: `msg_${id}`, model: "claude-fable-5", usage: { input_tokens: 2, cache_creation_input_tokens: 1097, cache_read_input_tokens: 67934, output_tokens: output, ...extra } } });
await transcript("claude-copies", "claude", [claude(1, 3), claude(1, 264), claude(2, 7)], true);
await transcript("claude-advisor", "claude", [claude(1, 42, { speed: "fast", cache_creation: { ephemeral_1h_input_tokens: 400 }, iterations: [
  { type: "advisor_message", model: "claude-opus-5-5", input_tokens: 20, output_tokens: 5, cache_creation_input_tokens: 4, cache_creation: { ephemeral_1h_input_tokens: 3 } },
  { type: "advisor_message", model: "<synthetic>", output_tokens: 100 }, { type: "other" } ] }),
  { ...claude(3, 5), message: { ...claude(3, 5).message, model: "<synthetic>" } }, "{broken", { type: "user", message: { usage: {} } }]);
// usageTranscriptReader.test.ts helpers, including the interrupted-turn reproduction.
const meta = (extra = {}) => ({ type: "session_meta", timestamp: "2026-08-01T10:00:00Z", payload: { type: "session_meta", id: "codex-session-1", ...extra } });
const context = (model = "gpt-6-astra", turn = "turn-1") => ({ type: "turn_context", timestamp: "2026-08-01T10:00:01Z", payload: { type: "turn_context", model, turn_id: turn } });
const tier = service_tier => ({ type: "event_msg", payload: { type: "thread_settings_applied", thread_settings: { service_tier } } });
const usage = (input, output, extra = {}) => ({ input_tokens: input, output_tokens: output, ...extra });
const count = (last, total, seconds = 5) => ({ type: "event_msg", timestamp: `2026-08-01T10:00:${String(seconds).padStart(2,"0")}Z`, payload: { type: "token_count", info: { last_token_usage: last, ...(total === undefined ? {} : { total_token_usage: total }) } } });
const request = (last, id, seconds = 5) => ({ type: "token_usage_record", timestamp: `2026-08-01T10:00:${String(seconds).padStart(2,"0")}Z`, payload: { usage: last, ...(id ? { response_id: id } : {}) } });
await transcript("codex-legacy", "codex", [meta({model_provider:"openai"}), context(), tier("ultrafast"), count(usage(100,9)), count(usage(100,9)), count(usage(100,"invalid")), count(usage(100,21), undefined,8)],true);
await transcript("codex-ledger", "codex", [meta({model_provider:"openai"}),context(),request(usage(100,20),"resp_1",1),count(usage(100,20),usage(100,20),2),request(usage(150,10),"resp_2",3),
  { type:"event_msg",payload:{type:"turn_aborted"}},context("gpt-6-sol","turn-2"),request(usage(200,30),"resp_3",5),count(usage(200,30),usage(450,60),6)]);
await transcript("codex-cumulative", "codex", [meta(),context(),count(usage(100,20),usage(500,60),5),count(usage(100,20),usage(500,60),5),count(usage(10,2),usage(510,62),8),
 count(usage(9,1),usage(20,3),9),count(usage(5,1),usage(25,4),10),count(usage(100,20),usage(500,60),5),count(usage(4,1),usage(29,5),11)]);
await transcript("codex-unreported", "codex", [meta(),context(),count(usage(100,20),usage(100,20),1),request(usage(150,10),null,2),request(usage(200,30),null,3),count(usage(200,30),usage(450,60),4)]);
await transcript("codex-fork", "codex", [meta({forked_from_id:"parent"}),context(),count(usage(100,20),usage(100,20),0),count(usage(200,30),usage(300,50),0),count(usage(5,1),usage(305,51),8)]);
await transcript("codex-validation", "codex", [meta(),context(),count(usage(100,20,{cached_input_tokens:101})),count(usage(100,20,{reasoning_output_tokens:21})),count(usage(100,20,{total_tokens:121})),
 count(usage(0,0),usage(0,0)),count(usage(20,5),usage(20,5)),count(usage(20,5),usage(20,5)),count(usage(20,5),usage(20,5),9)]);

await transcript("grok", "grok", [{timestamp:1785578400,params:{sessionId:"s",update:{sessionUpdate:"turn_completed",prompt_id:"p",usage:{inputTokens:100,outputTokens:20,costUsdTicks:1e10,modelUsage:{
 "grok-a":{inputTokens:80,cachedReadTokens:20,outputTokens:10,costUsdTicks:4e9},"grok-b":{inputTokens:20,outputTokens:10}}}}}},
 {timestamp:1785578401,params:{sessionId:"s",update:{sessionUpdate:"turn_completed",usage:{inputTokens:20,cachedReadTokens:5,outputTokens:2,reasoningTokens:8}}}}]);
await transcript("pi", "pi", [{type:"session",id:"pi-session"},{type:"message",id:"m1",timestamp:"2026-08-01T10:00:00Z",message:{role:"assistant",provider:"anthropic",model:"alias",responseModel:"claude-fable-5",usage:{input:10,output:20,cacheRead:30,cacheWrite:4,reasoning:50,cost:{total:0}}}},
 {type:"message",id:"t1",timestamp:"2026-08-01T10:00:01Z",message:{role:"toolResult",usage:{input:2,cost:{total:0.2}}}},{type:"compaction",id:"c1",timestamp:"2026-08-01T10:00:02Z",usage:{input:5,output:2}}]);
await transcript("muse", "muse", [{schema_version:1,record_type:"event",payload_type:"runtime.session",payload_schema_version:1,id:"envelope-1",recorded_at:1785578400123456,
 stream:{kind:"session",id:"session-1"},payload:{source_run_record_id:"inference-1",event:{kind:"model_completed",model:"muse-wonder-full-1.0",usage:{input_tokens:100,cached_tokens:60,cache_read_tokens:60,cache_write_tokens:10,output_tokens:20,reasoning_tokens:8}}}}]);
for (const r of [...readers]) {
  if (!r.file.startsWith("codex")) continue;
  const compressed=r.file+".zst";await fs.writeFile(path.join(dir,compressed),zlib.zstdCompressSync(await fs.readFile(path.join(dir,r.file))));
  readers.push({file:compressed,provider:r.provider,expected:await readTranscriptRecords(path.join(dir,compressed),formats[r.provider])});
}
await write("readers.json", readers);

const ratesDocument = {"claude-fable-5":{input_cost_per_token:1e-5,output_cost_per_token:5e-5,cache_read_input_token_cost:1e-6,cache_creation_input_token_cost:1.25e-5,cache_creation_input_token_cost_above_1hr:2e-5,provider_specific_entry:{fast:2}}};
await write("rates.json",ratesDocument);
const rates=parseRateTable(ratesDocument);
const record=(extra={})=>({provider:"claude",timestampMs:Date.parse("2026-08-07T04:05:13.944Z"),model:"claude-fable-5",sessionId:"session-a",totals:{uncachedInputTokens:100,cachedInputTokens:1000,cacheCreationTokens:10,outputTokens:50,reasoningTokens:0},reportedCostUsd:null,speed:"standard",dedupeKey:null,...extra});
const aggregates=[];
function aggregate(name,records,extra={}) {
 const options={timeZone:"UTC",sinceDay:"2026-08-01",untilDay:"2026-08-31",...extra};
 const a=new UsageAggregator({...options,rates,modelAliases:resolveModelAliases(extra.aliases??{}),priceOverrides:createOverrideRateTable(extra.overrides??{})});
 const accepted=records.map((r,i)=>a.add(r,extra.sources?.[i]));
 aggregates.push({name,records,options,accepted,expected:a.finish()});
}
aggregate("category and speed",[record(),record({speed:"fast"})]);
const partial=record({dedupeKey:"msg_1:",speed:"fast",reportedCostUsd:1,totals:{...record().totals,cachedInputTokens:1200,outputTokens:3,reasoningTokens:5}});
const complete=record({dedupeKey:"msg_1:",model:"claude-fable-5[1m]",timestampMs:Date.parse("2026-08-08T12:00:00Z"),sessionId:"session-fork",reportedCostUsd:2,totals:{uncachedInputTokens:2,cachedInputTokens:1000,cacheCreationTokens:40,outputTokens:264,reasoningTokens:135}});
aggregate("copies first bucket",[partial,complete,complete],{sources:["0.jsonl","1.jsonl","2.jsonl"]});
aggregate("first outside",[record({dedupeKey:"key",timestampMs:Date.parse("2026-07-01T12:00:00Z")}),record({dedupeKey:"key"})]);
for(const timeZone of ["UTC","America/Los_Angeles","Asia/Kathmandu","+00:01","not-a-zone"])aggregate("zone "+timeZone,[record(),record({timestampMs:Date.parse("2026-08-01T18:14:59.999Z")}),record({timestampMs:Date.parse("2026-08-01T18:15:00.000Z")}),record({timestampMs:Date.parse("2026-08-01T23:58:59.999Z")}),record({timestampMs:Date.parse("2026-08-01T23:59:00.000Z")})],{timeZone});
const since=Date.parse("2026-08-06T04:37:00.000Z"),until=since+86400000;
aggregate("hourly bounds",[-1,0,23*3600000,86400000-1,86400000].map(d=>record({timestampMs:since+d})),{resolution:"hour",sinceTimeMs:since,untilTimeMs:until});
aggregate("unpriced",[record({model:"kimi-k3"})]);aggregate("reported",[record({reportedCostUsd:1.25})]);
aggregate("alias",[record(),record({model:"example-preview",rateModel:"wrong"})],{aliases:{"example-preview":"claude-fable-5"}});
aggregate("original filter",[record(),record({model:"preview"}),record({provider:"codex"})],{aliases:{preview:"claude-fable-5"},modelFilter:{include:[{provider:"claude",model:"claude-fable-5"}]}});
aggregate("override",[record({reportedCostUsd:10})],{overrides:{"claude-fable-5":{inputCostPerMillionTokens:2,outputCostPerMillionTokens:8}}});
aggregate("cache ttl",[record({cacheCreation1hTokens:7,speed:"fast"})]);
await write("aggregation.json",aggregates);
const filters=[undefined,{}, {exclude:[]},{include:[]},{include:[{model:"b"},{model:"a",provider:"claude"},{model:"b"}],exclude:[{model:"b"},{model:"b"}]}];
await write("filters.json",filters.map(input=>({input:input??null,expected:canonicalModelFilter(input)??null})));

const pricing=[];
for(const doc of [ratesDocument,{"openai/gpt-x":{input_cost_per_token:1e-6,output_cost_per_token:5e-6},"reseller/gpt-x":{input_cost_per_token:2e-6,output_cost_per_token:7e-6}},
 {"a/model":{input_cost_per_token:1,output_cost_per_token:5},"b/model":{input_cost_per_token:2,output_cost_per_token:5}},
 {"tier":{input_cost_per_token:1,output_cost_per_token:5,cache_read_input_token_cost:0.1,input_cost_per_token_above_200k_tokens:2,output_cost_per_token_above_200k_tokens:10,input_cost_per_token_priority:3,output_cost_per_token_priority:15}}]){
 for(const model of ["claude-fable-5","claude-fable-5[1m]","gpt-x","model","tier","unknown","sonnet"]){for(const speed of ["standard","fast","ultrafast"]){
 const r=record({model,speed,totals:{uncachedInputTokens:200001,cachedInputTokens:1000,cacheCreationTokens:10,outputTokens:50,reasoningTokens:20},cacheCreation1hTokens:7});
 for(const cost of [null,1.25]){r.reportedCostUsd=cost;pricing.push({document:doc,record:{...r},expected:priceUsage(parseRateTable(doc),r),savings:cacheSavingsUsd(parseRateTable(doc),r)});}
 }}
}
await write("pricing.json",pricing);
const cache=new Map();
for(const r of readers.filter(r=>r.file.endsWith(".jsonl")))cache.set("/"+r.file,{size:100,mtimeMs:100,provider:r.provider,records:r.expected.records,tailRecords:r.expected.tailRecords,malformedRecords:r.expected.malformedRecords,position:r.expected.position});
const encoded=JSON.parse([...makeScanCacheWriter()(cache,{sources:{"claude\u0000/projects":{dir:"/projects",volumeId:"1:2"}}})].join(""));
const cacheCases=[];
for(const version of [3,4,5,6,7,8,10,11,12,13,14,15,16]){const document={...encoded,version};cacheCases.push({name:"version "+version,document,expected:[...decodeScanCache(document,new Map(Object.entries(formats)))]});}
for(const [key,value] of [["o",-1],["o",1.5],["gl",65],["gl",-1],["gh","x"],["mr",-1],["mr",0.5],["cs",{broken:true}]]){
 const document=structuredClone(encoded);for(const v of Object.values(document.files))v[key]=value;
 cacheCases.push({name:"bad "+key+JSON.stringify(value),document,expected:[...decodeScanCache(document,new Map(Object.entries(formats)))]});
}
await write("cache.json",cacheCases);
const cursorCaches = new Map([["credential-hash", {accountKey:"fixture-account",sinceMs:0,untilMs:100,fetchedAtMs:101,records:[record({provider:"cursor",model:"cursor-grok-4-thinking-high-fast",sessionId:"conversation",timestampMs:50})]}]]);
const cursorDocument=JSON.parse(cursorCache.encodeCursorAccountCaches(cursorCaches));
await write("cursor-cache.json",{document:cursorDocument,expected:[...cursorCache.decodeCursorAccountCaches(cursorDocument)]});
const checkedAt="2026-08-01T10:00:00.000Z";
const limits=[];
for(const provider of ["codex","claude","grok","cursor"]){
 const inputs=provider==="codex"?[{primary:{usedPercent:105,resetsAt:1785578500},secondary:{usedPercent:50,resetsAt:1785588500}},{planType:"free",primary:{usedPercent:5}},{limitId:"model-x",primary:{usedPercent:90}},{individualLimit:{remainingPercent:20,resetsAt:1785578500}}]
 :provider==="claude"?[{rate_limits_available:false},{rate_limits_available:true,rate_limits:{five_hour:{utilization:40,resets_at:"2026-08-01T12:00:00Z"},seven_day:{utilization:90,resets_at:null},model_scoped:[{display_name:"Opus 5.5",utilization:20,resets_at:null}],extra_usage:{is_enabled:true,monthly_limit:100,used_credits:25}}}]
 :provider==="grok"?[{config:{creditUsagePercent:50,currentPeriod:{type:"USAGE_PERIOD_TYPE_WEEKLY",start:"2026-08-01T00:00:00Z",end:"2026-08-08T00:00:00Z"}}},{config:{currentPeriod:{type:"MONTHLY",start:"2026-08-01T00:00:00Z",end:"2026-09-01T00:00:00Z"}}},{}]
 :[{billingCycleStart:"1785578400000",billingCycleEnd:"1788256800000",planUsage:{totalPercentUsed:40,autoPercentUsed:110,apiPercentUsed:20}},{}];
 for(const input of inputs){const expected=provider==="codex"?codexRateLimitsToLimits({snapshot:input,checkedAt}):provider==="claude"?claudeUsageResponseToLimits({response:input,checkedAt}).limits:provider==="grok"?grokUsageResponseToLimits(input,checkedAt):cursorUsageResponseToLimits(input,checkedAt);limits.push({provider,input,checkedAt,expected});}
}
await write("limits.json",limits);
// SQLite layouts and protobuf helpers from usage.test.ts and antigravityUsageReader.test.ts.
const dbRoot=path.join(dir,"databases");await fs.mkdir(dbRoot,{recursive:true});
const oc=path.join(dbRoot,"opencode");await fs.mkdir(oc,{recursive:true});
await fs.rm(path.join(oc,"opencode.db"),{force:true});
const odb=new DatabaseSync(path.join(oc,"opencode.db"));
odb.exec("CREATE TABLE message (id TEXT,session_id TEXT,data TEXT,time_created INTEGER);CREATE TABLE session_message (id TEXT,session_id TEXT,data TEXT,time_created INTEGER,type TEXT)");
const insert=odb.prepare("INSERT INTO message VALUES (?,?,?,?)");
for(let i=0;i<130;i++)insert.run(`msg-${i}`,"session-1",JSON.stringify({role:"assistant",modelID:"claude-sonnet-4-5",time:{created:1780000000000},tokens:{input:100,output:20,reasoning:5,cache:{read:10,write:2}},body:"x".repeat(1024)}),1780000000000);
insert.run("bad","s","{invalid",1780000000000);insert.run("user","s",JSON.stringify({role:"user",tokens:{input:999999}}),1780000000000);
odb.prepare("INSERT INTO session_message VALUES (?,?,?,?,?)").run("gateway","session-2",JSON.stringify({model:{id:"gpt-x-free",providerID:"opencode-go"},tokens:{input:5,output:2},cost:0}),1780000000000,"assistant");odb.close();
await fs.mkdir(path.join(oc,"storage/message/session-1"),{recursive:true});
await fs.writeFile(path.join(oc,"storage/message/session-1/msg-0.json"),JSON.stringify({role:"assistant",modelID:"claude-sonnet-4-5",time:{created:1780000000000},tokens:{input:999999}}));
await fs.writeFile(path.join(oc,"storage/message/session-1/legacy.json"),JSON.stringify({role:"assistant",sessionID:"session-1",modelID:"legacy-model",time:{created:1780000000000},tokens:{input:8,output:3}}));
const ag=path.join(dbRoot,"antigravity");await fs.mkdir(ag,{recursive:true});
const vn=n=>{const b=[];do{const v=n%128;n=Math.floor(n/128);b.push(v+(n?128:0));}while(n);return b;};
const pn=(f,n)=>[...vn(f*8),...vn(n)];const pb=(f,b)=>[...vn(f*8+2),...vn(b.length),...b];const pt=(f,s)=>pb(f,[...Buffer.from(s)]);
const tables="CREATE TABLE gen_metadata(idx INTEGER,data BLOB);CREATE TABLE steps(idx INTEGER,metadata BLOB)";
for(const name of ["a-usage.db","z-evidence.db"])await fs.rm(path.join(ag,name),{force:true});
const adb=new DatabaseSync(path.join(ag,"a-usage.db"));adb.exec(tables);
const primary=[...pn(1,9001),...pn(2,100),...pn(3,40),...pn(4,5),...pn(5,20),...pn(9,10),...pt(11,"response-1")];
const retry=[...pn(1,1026),...pn(2,12),...pn(3,3),...pt(11,"retry-1")];
const generation=pb(1,[...pn(3,9001),...pb(4,primary),...pb(17,pb(2,retry)),...pb(9,pb(4,pn(1,1780000000)))]);
adb.prepare("INSERT INTO gen_metadata VALUES (?,?)").run(1,new Uint8Array(generation));
adb.prepare("INSERT INTO steps VALUES (?,?)").run(1,new Uint8Array([...pb(9,primary),...pb(8,pn(1,1780000000))]));adb.close();
const evidence=new DatabaseSync(path.join(ag,"z-evidence.db"));evidence.exec(tables);evidence.prepare("INSERT INTO gen_metadata VALUES (?,?)").run(1,new Uint8Array(pb(1,[...pn(3,9001),...pt(21,"Gemini 9.1 Flash (High)")])));evidence.close();
const ocExpected=await Effect.runPromise(readOpenCodeUsage(oc,0).pipe(Effect.provide(NodeServices.layer)));
const agExpected=await Effect.runPromise(readAntigravityUsage(ag,0).pipe(Effect.provide(NodeServices.layer)));
await write("databases.json",{opencode:ocExpected.files.flatMap(f=>f.records),antigravity:agExpected.files.flatMap(f=>f.records)});
console.log(`Generated ${readers.length} transcript, ${aggregates.length} aggregation, ${pricing.length} pricing, ${cacheCases.length} cache and ${limits.length} limits cases in ${dir}`);
