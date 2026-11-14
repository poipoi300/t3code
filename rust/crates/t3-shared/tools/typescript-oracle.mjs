// Execute the current TypeScript suites and record calls at the pure-helper boundary.
import { registerHooks } from 'node:module';
import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';
import { AsyncLocalStorage } from 'node:async_hooks';

const root = process.cwd();
const cases = [];
const titles = [];
let title = '';
const context = new AsyncLocalStorage();
const pendingTests = [];
let selectedSuites;
const clean = v => JSON.parse(JSON.stringify(v, (k,v) => typeof v === 'function' ? undefined : typeof v === 'string' ? v.toWellFormed() : v) ?? 'null');
globalThis.__sharedRecord = (name,args,result) => {
  cases.push({name,args:clean(args),expected:clean(result),test:context.getStore()??title});
  return result;
};
let nextState = 1;
const stateIds = new WeakMap();
globalThis.__sharedAppend = (fn,args) => {
  const [state, chunk, max] = args;
  const input = stateIds.get(state) ?? 0;
  const result = fn(...args);
  const id = nextState++;
  stateIds.set(result,id);
  cases.push({name:'append',args:[input,chunk,max??65536],id,expected:clean({text:result.text,pending:result.pending,truncated:result.truncated}),test:title});
  return result;
};
globalThis.__sharedStateCall = (name,fn,args) => {
  const result=fn(...args);
  cases.push({name,args:[stateIds.get(args[0])??0],expected:clean(result),test:title});
  return result;
};
globalThis.__sharedScore = (name,fn,args) => {
  const result=fn(...args);
  return globalThis.__sharedRecord(name,args,result && {...result, hint:result.explain?.()});
};
globalThis.__sharedRankGroups = (fn,args) => {
  const [groups,score]=args;
  const scores=[];
  const result=fn(groups,(item,options,group)=>{const match=score(item,options,group);scores.push([groups.findIndex(g=>g.group===group),groups.find(g=>g.group===group).items.indexOf(item),options.typos,match]);return match});
  globalThis.__sharedRecord('rankGroups',[groups,scores],result.map(g=>({group:g.group,indices:g.ranked.map(r=>groups.find(x=>x.group===g.group).items.indexOf(r.item))})));
  return result;
};
globalThis.__sharedRankMessages = (fn,args) => {
  const [matches,subjectOf,query,now,options]=args;
  const inputs=matches.map(m=>({...m,subject:subjectOf(m)}));
  const result=fn(...args);
  globalThis.__sharedRecord('rankMessages',[inputs,query,now,options],result.map(m=>matches.indexOf(m)));
  return result;
};
globalThis.__sharedTest = (name,fn,...args) => { const old=title; title+=name; titles.push(title); try{const result=context.run(title,()=>fn(...args));if(result?.then)pendingTests.push(result)}finally{title=old} };
globalThis.__sharedDescribe = (name,fn) => {if(selectedSuites&&!selectedSuites.has(name))return;const old=title; title+=name+' / ';try{fn()}finally{title=old}};
globalThis.__sharedExpect = (actual) => {
  const make = (negate=false) => {
    const test = fn => {let failed=false;try{fn()}catch{failed=true} assert.equal(failed,negate,`TypeScript assertion failed in ${title}`)};
    const obj = {
      toBe: expected => test(()=>assert.equal(actual,expected)),
      toEqual: expected => test(()=>assert.deepEqual(actual,expected)),
      toBeNull: ()=>test(()=>assert.equal(actual,null)),
      toBeUndefined: ()=>test(()=>assert.equal(actual,undefined)),
      toBeDefined: ()=>test(()=>assert.notEqual(actual,undefined)),
      toBeLessThan: n=>test(()=>assert.ok(actual<n)),
      toBeLessThanOrEqual: n=>test(()=>assert.ok(actual<=n)),
      toBeGreaterThan: n=>test(()=>assert.ok(actual>n)),
      toBeGreaterThanOrEqual: n=>test(()=>assert.ok(actual>=n)),
      toContain: n=>test(()=>assert.ok(actual.includes(n))),
      toMatch: n=>test(()=>assert.match(actual,n)),
      toHaveLength: n=>test(()=>assert.equal(actual.length,n)),
      toMatchObject: n=>test(()=>{const subset=(a,b)=>{for(const k of Object.keys(b)){if(b[k]&&typeof b[k]==='object')subset(a[k],b[k]);else assert.deepEqual(a[k],b[k])}};subset(actual,n)}),
    };
    Object.defineProperty(obj,'not',{get:()=>make(!negate)});
    return obj;
  };
  return make();
};
const shim = `export const expect=globalThis.__sharedExpect; export const describe=globalThis.__sharedDescribe;
const test=globalThis.__sharedTest; export const it=Object.assign(test,{each:cases=>(name,fn)=>{for(const c of cases)test(name+' '+JSON.stringify(c),fn,...(Array.isArray(c)?c:[c]))}});`;
const wrappers = {
  'packages/shared/src/terminalOutput.ts': {
    appendTerminalOutput:'return globalThis.__sharedAppend(original_appendTerminalOutput,args)',
    terminalOutputResumeText:"return globalThis.__sharedStateCall('resume',original_terminalOutputResumeText,args)",
    normalizeTerminalOutput:"return globalThis.__sharedRecord('normalize',args,original_normalizeTerminalOutput(...args))",
    terminalOutputTail:"const result=original_terminalOutputTail(...args);return globalThis.__sharedRecord('tail',[Array.from({length:args[0].length},(_,i)=>args[0].charCodeAt(i)),args[1]],result)",
    terminalOutputPlainText:"return globalThis.__sharedRecord('plain',args,original_terminalOutputPlainText(...args))",
    terminalOutputSpans:"return globalThis.__sharedRecord('spans',args,original_terminalOutputSpans(...args))",
  },
  'packages/shared/src/threadFindText.ts': Object.fromEntries(['searchableMessageSegments','searchablePlanSegments'].map(n=>[n,`return globalThis.__sharedRecord('${n}',args,original_${n}(...args))`])),
  'packages/shared/src/threadSearch.ts':Object.fromEntries(['countThreadSearchOccurrences','findThreadSearchOccurrences'].map(n=>[n,`return globalThis.__sharedRecord('${n}',args,original_${n}(...args))`])),
  'packages/shared/src/paletteSearch.ts': {
    ...Object.fromEntries(['foldSearchQuery','pickedQuery','recordThreadOpen','findSearchTokenRanges','messageSearchTokens','threadRecentUse'].map(n=>[n,`return globalThis.__sharedRecord('${n}',args,original_${n}(...args))`])),
    messageMatcher:"const fn=original_messageMatcher(...args);return text=>globalThis.__sharedRecord('matchMessage',[text,args[0]],fn(text))",
    scoreThreadSearch:"return globalThis.__sharedScore('scoreThreadSearch',original_scoreThreadSearch,args)",
    scoreSearchTerms:"return globalThis.__sharedScore('scoreSearchTerms',original_scoreSearchTerms,args)",
    rankSearchGroups:"return globalThis.__sharedRankGroups(original_rankSearchGroups,args)",
    rankMessageMatches:"return globalThis.__sharedRankMessages(original_rankMessageMatches,args)",
  },
  'packages/shared/src/toolOutput.ts':Object.fromEntries(['compactDynamicToolOutput','htmlRenderFromToolItem','mcpAppFromToolItem'].map(n=>[n,`return globalThis.__sharedRecord('${n}',args,original_${n}(...args))`])),
  'packages/contracts/src/terminal.ts':Object.fromEntries(['terminalSequenceStateAfter','terminalSequenceEnd','retainedTerminalHistory'].map(n=>[n,`const result=original_${n}(...args);const input=args.slice();${n==='retainedTerminalHistory'?'':"input[1]=Array.from({length:args[1].length},(_,i)=>args[1].charCodeAt(i));"}return globalThis.__sharedRecord('${n}',input,result)`])),
};
registerHooks({
  resolve(specifier,context,next) {
    if(specifier==='vite-plus/test')return {url:'data:text/javascript,'+encodeURIComponent(shim),shortCircuit:true};
    try{return next(specifier,context)}catch(error){
      if(specifier.startsWith('.')){const base=new URL(specifier,context.parentURL);for(const suffix of ['.ts','.tsx','/index.ts'])if(existsSync(fileURLToPath(base)+suffix))return {url:base.href+suffix,shortCircuit:true}}
      throw error;
    }
  },
  load(url,context,next) {
    if(url===pathToFileURL(resolve(root,'apps/server/src/orchestration-v2/ThreadSearch.ts')).href){
      const source=readFileSync(fileURLToPath(url),'utf8');
      const short=source.slice(source.indexOf('function shortTokenSpellings('),source.indexOf('/**',source.indexOf('function shortTokenSpellings(')));
      const build=source.slice(source.indexOf('function buildMessageSearch('),source.indexOf('// A thread shows'));
      return {format:'module-typescript',shortCircuit:true,source:`import {messageSearchTokens} from ${JSON.stringify(pathToFileURL(resolve(root,'packages/shared/src/paletteSearch.ts')).href)};const NON_ASCII=/[\\u0080-\\uffff]/;${short}${build}\nexport function narrow(query,hasIndex){return globalThis.__sharedRecord('messageSearch',[query,hasIndex],buildMessageSearch(query,hasIndex));}`};
    }
    const entry=Object.entries(wrappers).find(([path])=>url===pathToFileURL(resolve(root,path)).href);
    if(!entry)return next(url,context);
    const [path,functions]=entry;
    let source=readFileSync(resolve(root,path),'utf8');
    for(const [name,body] of Object.entries(functions)){
      const pattern=new RegExp('export function '+name+'(\\s*[(<])');
      assert.ok(pattern.test(source),`missing ${name}`);
      source=source.replace(pattern,'function original_'+name+'$1');
      source+=`\nexport function ${name}(...args: any[]) { ${body}; }\n`;
    }
    return {format:'module-typescript',source,shortCircuit:true};
  }
});
for(const path of ['terminalOutput','threadFindText','threadSearch','paletteSearch','toolOutput'])await import(pathToFileURL(resolve(root,`packages/shared/src/${path}.test.ts`)).href);
for(const [path,suite] of [['htmlRender','htmlRenderFromToolItem'],['mcpApp','mcpAppFromToolItem']]){
  selectedSuites=new Set([suite]);
  await import(pathToFileURL(resolve(root,`packages/shared/src/${path}.test.ts`)).href);
  await Promise.all(pendingTests.splice(0));
}
selectedSuites=undefined;
const terminal = await import(pathToFileURL(resolve(root,'packages/shared/src/terminalOutput.ts')).href);
const history = await import(pathToFileURL(resolve(root,'packages/contracts/src/terminal.ts')).href);
const find = await import(pathToFileURL(resolve(root,'packages/shared/src/threadFindText.ts')).href);
const palette = await import(pathToFileURL(resolve(root,'packages/shared/src/paletteSearch.ts')).href);
const {narrow}=await import(pathToFileURL(resolve(root,'apps/server/src/orchestration-v2/ThreadSearch.ts')).href);
const tools = await import(pathToFileURL(resolve(root,'packages/shared/src/toolOutput.ts')).href);
const fixtureRoot=resolve(fileURLToPath(new URL('../tests/fixtures/',import.meta.url)));
title='package09 current-source rendered and embed corpus';
for(const c of JSON.parse(readFileSync(resolve(fixtureRoot,'transport_find_text.json'),'utf8'))){
  if(c.item.type==='proposed_plan')find.searchablePlanSegments(c.item.markdown,c.cwd??undefined);
  else find.searchableMessageSegments({...c.item,role:c.item.type==='user_message'?'user':'assistant'},c.cwd??undefined,c.skills);
}
for(const c of JSON.parse(readFileSync(resolve(fixtureRoot,'transport_find_embeds.json'),'utf8'))){
  const item=c.item;
  globalThis.__sharedRecord('embeds',[item],!!(item.type==='dynamic_tool'&&((item.status==='completed'&&tools.htmlRenderFromToolItem(item))||tools.mcpAppFromToolItem(item))));
}
title='generated deterministic cases';
for(const input of ['discard\ufefftail','prefix\x1b[31mOK','prefix\x1b]0;HIDDEN\x07OK','prefix\x1bPignored\x1b\\OK','prefix\x1b_ignored\x1b\\OK','aaaa\nx名😀yz\n','prefix\x1b[31ĀOK','prefix\x1b[31脛OK','prefix\x1b]text\x18OK']){
  for(let max=0;max<=new TextEncoder().encode(input).length+1;max++)history.retainedTerminalHistory(input,max);
}
let seed=0x9e3779b9;
const random=n=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return seed%n};
const atoms=['abc','😀','界','é','\ufeff','\n','\r','\b','\x1b[31m','\x1b[0m','\x1b[2K','\x1b[1A','\x1b[3G','\x1b7','\x1b8','\x1b]title\x07','\x1bPpayload\x1b\\','\u009b','\u009d','\x1b[38:2::12:34:56m','\x1b#8','\x1bc','\x1b[?25l','\x1b[99999999999999999999999C','\x1b[18446744073709551616m','\x1b]title\nnext','\x1b]title\x1b[32m'];
for(let i=0;i<320;i++){
  const input=Array.from({length:3+random(20)},()=>atoms[random(atoms.length)]).join('');
  const max=[3,20,128,65536][random(4)];
  let state=terminal.EMPTY_TERMINAL_OUTPUT;
  const chars=Array.from(input);
  while(chars.length)state=terminal.appendTerminalOutput(state,chars.splice(0,1+random(5)).join(''),max);
  terminal.terminalOutputResumeText(state);
  terminal.terminalOutputSpans(state.text, i%2?terminal.TERMINAL_PALETTES.dark:terminal.TERMINAL_PALETTES.light,1+random(20));
  for(const budget of [0,1,2,5,17,64])history.retainedTerminalHistory(input,budget);
  const units=Array.from({length:input.length+1},(_,i)=>i);
  const cut=units[random(units.length)];
  const stateBefore=history.terminalSequenceStateAfter(0,input,0,cut);
  history.terminalSequenceStateAfter(stateBefore,input,cut);
  history.terminalSequenceEnd(stateBefore,input.slice(cut));
}
title='generated mid-line rewrites, run limits and colour parameters';
const feed=(chunks,max)=>{let state=terminal.EMPTY_TERMINAL_OUTPUT;for(const chunk of chunks)state=terminal.appendTerminalOutput(state,chunk,max);terminal.terminalOutputResumeText(state);};
const alternating=(from,count)=>Array.from({length:count},(_,i)=>(from+i)%2?'\x1b[0mx':'\x1b[31mx').join('');
feed(['L'.repeat(70000),'\rX'.repeat(300),'\x1b[31m'+'\rY'.repeat(40)+'\x1b[0m','\x1b[2500G\x1b[32mmid\x1b[0m\r\x1b[2499Gab\x1b[1K','\r\x1b[33m'+'Z'.repeat(5000)]);
feed(['L'.repeat(3000),'\rX'.repeat(500),'\x1b[31m\r'+'Y'.repeat(10)+'\x1b[0m'+'\rY'.repeat(10),'\x1b[5G\x1b[32mlong-overlap\x1b[0m\x1b[3G\x1b[31m'+'q'.repeat(40)]);
for(const max of [300,4096])feed([Array.from({length:3000},(_,i)=>`\x1b[3${i%7+1}mX`).join(''),'\x1b[0m\r\x1b[10Gmid','tail'],max);
// After the run limit flattens old runs, a mid-line write merges the plain run with an unstyled neighbour.
const coloured=count=>Array.from({length:count},(_,i)=>`\x1b[3${i%7+1}mx`).join('');
for(const tail of [alternating(0,1900),alternating(1,1901),coloured(1900)])feed([alternating(0,2049),'\x1b[0m\r\x1b[1796GZ','\x1b[0m\x1b[3000G'+tail]);
feed([alternating(0,2049),'\x1b[0m\x1b[1796G\x1b[2K',alternating(0,10),'\r\x1b[1790G\x1b[1K'+alternating(1,1900)]);
const lineAtoms=['\r','\x1b[3G','\x1b[9G','\x1b[40G','\x1b[31m','\x1b[32m','\x1b[0m','X','abcdef','\x1b[1K','\x1b[K','\x1b[2K','L'.repeat(50),'\x1b[38;2;1;2;3m','\x1b[1A','\n','\b','\u{1F600}'];
for(let i=0;i<200;i++){
  const max=[16,64,300][random(3)];
  feed(Array.from({length:1+random(4)},()=>Array.from({length:4+random(30)},()=>lineAtoms[random(lineAtoms.length)]).join('')),max);
}
for(const colour of ['38;2;18446744073709551616;0;0','48;2;4294967296;4294967551;2147483648','38;2;9007199254740993;9007199254740995;300','38:2::18446744073709551616:1:2','38;2;1'+'0'.repeat(400)+';7;8','38;5;256','38;5;18446744073709551616','48:5:255'])feed([`a\x1b[${colour}mX\x1b[0mb`]);
for(const text of ['foo﻿bar','foo\u0085bar','see foo ﻿ bar now','foo bar','foo bar'])for(const q of ['foo bar','see foo'])palette.messageMatcher(q)(text);
const review='sectionId="turn:2" sectionTitle="Turn 2" filePath="hidden.ts" startIndex="3" endIndex="14" rangeLabel="L4"';
for(const text of [`Before\n<review_commenté ${review}>\nKeep comment.\n</review_comment>\nAfter`,`Before <review_commentx ${review}>note</review_comment> after`,'<review_commenté>',['<preview_annotation>','Preview annotation:','Id: ann_2','Page: Checkout','Comment: First','</review_commenté','Second','</preview_annotation>','after'].join('\n')])find.searchableMessageSegments({role:'user',text,streaming:false});
const queries=['auth','café','ΟΣ',"don't",'搜索','🚀','ui','fix sidebar','railwya','t3code','İ','a\u0301','名'];
for(const input of ['名😀 café\r\x1b[2K🚀 résumé\n','\ufeffdata\x1b[3','\x1b]title😀\x07ok']){
  const bytes=new TextEncoder().encode(input);
  for(let cut=0;cut<=bytes.length;cut++){
    const decoder=new TextDecoder('utf-8',{ignoreBOM:true});
    const chunks=[Array.from(bytes.slice(0,cut)),Array.from(bytes.slice(cut))];
    let state=terminal.EMPTY_TERMINAL_OUTPUT;
    for(const chunk of chunks)state=terminal.appendTerminalOutput(state,decoder.decode(Uint8Array.from(chunk),{stream:true}));
    state=terminal.appendTerminalOutput(state,decoder.decode());
    globalThis.__sharedRecord('utf8Feed',[chunks],{text:state.text,pending:state.pending,truncated:state.truncated});
  }
}
for(let i=0;i<500;i++){
  const q=queries[random(queries.length)];
  const text=queries[random(queries.length)]+' '+q+' '+queries[random(queries.length)];
  palette.messageMatcher(q)(text);
  narrow(q,i%2===0);
  palette.scoreThreadSearch(palette.foldSearchQuery(q),{title:text,activityAt:0},1790000000000,{typos:i%2===0});
  find.searchableMessageSegments({role:i%2?'user':'assistant',text:`**${text}**\n\n- \`${q}\``,streaming:false});
}
process.stdout.write(JSON.stringify({titles,cases}));
