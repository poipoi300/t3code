// Run with Node from the repository root. Test inputs stay authored in TypeScript.
import { registerHooks } from 'node:module';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';

const root = process.cwd();
const source = 'apps/web/src/components/chat/MessagesTimeline.logic.ts';
const tests = 'apps/web/src/components/chat/MessagesTimeline.logic.test.ts';
const activeTests = 'apps/web/src/components/chat/MessagesTimeline.active-tools.test.ts';
globalThis.__timelineCases = [];
globalThis.__timelineTitles = [];
globalThis.__timelineTitle = '';
const functions = ['deriveMessagesTimelineRows', 'deriveMessagesTimelineRowsWithState', 'computeStableMessagesTimelineRows', 'timelineTurnFoldRunIdsByEntryId', 'liveAnswerPreview', 'resolveAssistantMessageCopyState', 'workEntryDisplayLabel', 'liveWorkEntryLabel'];
const shim = `
const proxy = new Proxy(function(){return proxy}, {get: (_,k) => k === 'then' ? undefined : proxy});
export const expect = proxy;
export function describe(title,fn) { const old=globalThis.__timelineTitle; globalThis.__timelineTitle += title+' / '; try {fn()} finally {globalThis.__timelineTitle=old} }
function test(title,fn,...args) {const old=globalThis.__timelineTitle; globalThis.__timelineTitle+=title; globalThis.__timelineTitles.push(globalThis.__timelineTitle); try {fn(...args)} finally {globalThis.__timelineTitle=old}}
export const it = Object.assign(test,{each: cases => (title,fn) => {for(const c of cases) test(title+' '+JSON.stringify(c),fn,...(Array.isArray(c)?c:[c]))}});
`;
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === 'vite-plus/test') return {url:'data:text/javascript,'+encodeURIComponent(shim),shortCircuit:true};
    try {return next(specifier,context)} catch(error) {
      if (specifier.startsWith('.')) {
        const base = new URL(specifier,context.parentURL);
        for(const suffix of ['.ts','.tsx','/index.ts']) if(existsSync(fileURLToPath(base)+suffix)) return {url:base.href+suffix,shortCircuit:true};
      }
      throw error;
    }
  },
  load(url, context, next) {
    const result = next(url,context);
    if (url !== pathToFileURL(resolve(root,source)).href) return result;
    let code = readFileSync(resolve(root,source),'utf8');
    for (const name of functions) {
      code = code.replace('export function '+name+'(', 'function original_'+name+'(');
      code += `\nexport function ${name}(...args: any[]) { const result=original_${name}(...args); const rows=result?.rows??result?.result;const previous=args[1]?.rows??args[1]?.result;const identity=rows&&previous?{sameArray:rows===previous,sameState:result===args[1],reused:rows.map(row=>previous.indexOf(row))}:undefined; globalThis.__timelineCases.push(JSON.parse(JSON.stringify({test:globalThis.__timelineTitle,function:'${name}',input:args,output:result,identity},(_,v)=>v instanceof Set?[...v]:v instanceof Map?[...v]:v))); return result; }\n`;
    }
    return {...result,source:code};
  }
});
await import(pathToFileURL(resolve(root,tests)).href);
await import(pathToFileURL(resolve(root,activeTests)).href);
const copy = (await import(pathToFileURL(resolve(root,source)).href)).resolveAssistantMessageCopyState;
const citation = ':codex-file-citation{path="outputs/report.xlsx" purpose="output"}';
const template = '::artifact-template{skill_name="artifact-template-hello-world" skill_directory="/templates/hello" display_name="Hello World" artifact_kind="document"}';
const copyProbes = [
  `\\${citation}`, `\`${citation}\``, `[See ${citation}](https://example.com)`,
  `\`\`\`text\n${citation}\n\`\`\``, `~~~text\n${citation}\n~~~`, `    ${citation}`,
  ':codex-followup[Review **document**]{prompt="Review it."}',
  ':codex-file-citation{path="C:\\Users\\test\\[draft]\\report.md" line_range_start="7"}',
  ':codex-file-citation{path="\\\\server\\share\\report.md" line_range_start="7"}',
  ':codex-file-citation{path="/tmp/report%5C.md" line_range_start="7"}',
  ':codex-file-citation{path=" outputs/[draft]*_`<&?#%.md " line_range_start="7"}',
  template, `Before ${template} after`, template.replace('artifact-template-hello-world','wrong-skill'),
  template.replace('/templates/hello','relative/path'), template.replace('document','site'),
  template.replace('document','google-slides'), template.replace('document','image'),
  '\\'+template, '`unclosed '+citation,
];
for(const [i,text] of copyProbes.entries()) {globalThis.__timelineTitle=`copy adapter differential probe ${i+1}`;globalThis.__timelineTitles.push(globalThis.__timelineTitle);copy({text,showCopyButton:true,streaming:false});}
const derive = (await import(pathToFileURL(resolve(root,source)).href)).deriveMessagesTimelineRows;
for(const status of ['inProgress','completed','failed','stopped']) {
  globalThis.__timelineTitle=`upstream theme tool labels and priority ${status}`;globalThis.__timelineTitles.push(globalThis.__timelineTitle);
  const tools=['t3_theme_read','t3_theme_set','t3_theme_read','t3_theme_set'];
  const timelineEntries=tools.map((tool,index)=>({kind:'work',id:`theme-${index}`,createdAt:'2026-01-01T00:00:00Z',entry:{id:`theme-${index}`,createdAt:'2026-01-01T00:00:00Z',runId:null,label:tool,tone:'tool',itemType:'dynamic_tool',toolTitle:`t3-code.${tool}`,toolLifecycleStatus:status}}));
  for(const e of timelineEntries) (await import(pathToFileURL(resolve(root,source)).href)).workEntryDisplayLabel(e.entry,undefined);
  derive({timelineEntries,isWorking:false,turnDiffSummaries:[],supportsConversationRollback:false});
}
for(const [i,fields] of [
  {label:'Read'}, {label:'Read file',toolTitle:'Read'},
  {label:'Search',toolData:{kind:'search'}}, {label:'Read a file',toolData:{kind:'read'}},
  {label:'Run command',toolData:{kind:'execute'}},
].entries()) {
  globalThis.__timelineTitle=`tool metadata classifier differential probe ${i+1}`;globalThis.__timelineTitles.push(globalThis.__timelineTitle);
  const timelineEntries=[0,1].map(index=>({kind:'work',id:`classifier-${index}`,createdAt:'2026-01-01T00:00:00Z',entry:{id:`classifier-${index}`,createdAt:'2026-01-01T00:00:00Z',runId:null,tone:'tool',toolLifecycleStatus:'completed',...fields}}));
  derive({timelineEntries,isWorking:false,turnDiffSummaries:[],supportsConversationRollback:false});
}
globalThis.__timelineTitle='upstream theme-set keeps actionable summary priority';globalThis.__timelineTitles.push(globalThis.__timelineTitle);
derive({timelineEntries:[
  {toolTitle:'t3-code.t3_theme_read',itemType:'dynamic_tool'},
  {toolTitle:'Read',itemType:'dynamic_tool'},
  {command:'pwd',itemType:'command_execution'},
  {toolTitle:'t3-code.t3_theme_set',itemType:'dynamic_tool'},
].map((fields,index)=>({kind:'work',id:`priority-${index}`,createdAt:'2026-01-01T00:00:00Z',entry:{id:`priority-${index}`,createdAt:'2026-01-01T00:00:00Z',runId:null,label:'tool',tone:'tool',toolLifecycleStatus:'completed',...fields}})),isWorking:false,turnDiffSummaries:[],supportsConversationRollback:false});
const destination = resolve(dirname(fileURLToPath(import.meta.url)), 'typescript-cases.json');
const toolDefinitionsSource='packages/shared/src/t3McpToolPresentation.ts';
const {T3_MCP_TOOL_NAMES,resolveT3McpToolDefinition}=await import(pathToFileURL(resolve(root,toolDefinitionsSource)).href);
const definitions=[...T3_MCP_TOOL_NAMES].map(name=>[name,resolveT3McpToolDefinition(name)]);
const hashes = Object.fromEntries([source,tests,activeTests,toolDefinitionsSource].map(path=>[path,createHash('sha256').update(readFileSync(resolve(root,path))).digest('hex')]));
const corpus = {source,tests,activeTests,hashes,titles:globalThis.__timelineTitles,cases:globalThis.__timelineCases};
if(process.argv.includes('--stdout')) process.stdout.write(JSON.stringify(corpus));
else {
  writeFileSync(destination,JSON.stringify(corpus));
  writeFileSync(resolve(dirname(fileURLToPath(import.meta.url)), 'tool_definitions.rs'),'// Generated from packages/shared/src/t3McpToolPresentation.ts by export-tests.mjs.\nconst TOOL_DEFINITIONS: &[(&str, [&str; 4], &str, &str)] = &[\n'+definitions.map(([name,d])=>`    (${JSON.stringify(name)}, [${d.labels.map(JSON.stringify).join(', ')}], ${JSON.stringify(d.icon)}, ${JSON.stringify(d.summaryAction)}),`).join('\n')+'\n];\n');
  writeFileSync(resolve(dirname(fileURLToPath(import.meta.url)), 'test_cases.rs'),globalThis.__timelineTitles.map((title,index)=>`#[test]\nfn ts_case_${String(index+1).padStart(3,'0')}() { check_test(${index}); } // ${title.replaceAll('\n',' ')}`).join('\n')+'\n');
  console.log(JSON.stringify({destination,tests:globalThis.__timelineTitles.length,calls:globalThis.__timelineCases.length}));
}
