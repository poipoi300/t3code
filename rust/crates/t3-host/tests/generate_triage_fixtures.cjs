const fs=require("node:fs");
const path=require("node:path");
const vm=require("node:vm");
const {stripTypeScriptTypes}=require("node:module");
const repo=path.resolve(__dirname,"../../../..");
const text=fs.readFileSync(path.join(repo,"apps/server/src/triage/triagePrompt.ts"),"utf8");
const code=stripTypeScriptTypes(text.replace(/^import[\s\S]*?;\r?\n/gm,"").replaceAll("export ",""),{mode:"strip"});
const context=vm.createContext({});vm.runInContext(code+"\nglobalThis.render={buildTriageContext,buildTriageSeedPrompt,TRIAGE_PLAYBOOK};",context);
const base=String.raw`C:\T3 home`;
const state=String.raw`C:\T3 home\userdata`;
const paths={stateDir:state,dbPath:state+String.raw`\statev2.sqlite`,settingsPath:state+String.raw`\settings.json`,logsDir:state+String.raw`\logs`,serviceLogPath:state+String.raw`\logs\boot-service.log`,desktopBackendLogGlob:state+String.raw`\logs\server-child*.log*`,relayClientLogPath:state+String.raw`\logs\relay-client.ndjson`,serverTracePath:state+String.raw`\logs\server.trace.ndjson`,providerEventLogPath:state+String.raw`\logs\provider\events.log`,terminalLogsDir:state+String.raw`\logs\terminals`,providerStatusCacheDir:base+String.raw`\caches`,secretsDir:state+String.raw`\secrets`,sourceCacheDir:base+String.raw`\source`};
const cases=[];
for(const entry of [{kind:"app",client:"Example client"},{kind:"cli",cliVersion:"1.0.0",launchedAs:"t3 triage"}]){
  for(const sourceClone of [undefined,{_tag:"failed",serverVersion:"1.0.0",reason:"unavailable"},...(["cloned","cloning"].flatMap(_tag=>["v1.0.0","main"].map(ref=>({_tag,directory:base+String.raw`\source\commit`,ref,commit:"commit",serverVersion:"1.0.0"}))))]){
    const input={generatedAt:"2026-11-13T00:00:00.000Z",entry,os:"win32 x64 (test)",nodeVersion:"not applicable (native Rust server)",server:"running test",paths,sourceClone};
    cases.push({input,context:context.render.buildTriageContext(input),seed:context.render.buildTriageSeedPrompt({entry,contextFilePath:"context.md"})});
  }
}
fs.writeFileSync(path.join(__dirname,"triage-contexts.json"),JSON.stringify(cases,null,2)+"\n");
const copied=fs.readFileSync(path.join(__dirname,"../src/triage-playbook.md"),"utf8");
if(copied!==context.render.TRIAGE_PLAYBOOK)throw new Error("Copied playbook differs from TypeScript");
