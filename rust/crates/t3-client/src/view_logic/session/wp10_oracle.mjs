import { registerHooks, createRequire } from "node:module";
import { readFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";
import { pathToFileURL, fileURLToPath } from "node:url";

const root = process.cwd();
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === "./lib/utils" && context.parentURL.endsWith("/terminal-links.ts")) return { url: "data:text/javascript,export const isMacPlatform = () => false;", shortCircuit: true };
    try { return next(specifier, context); } catch (error) {
      if (specifier.startsWith(".")) {
        const base = new URL(specifier, context.parentURL);
        for (const suffix of [".ts", ".tsx", "/index.ts"]) {
          if (existsSync(fileURLToPath(base) + suffix)) return { url: base.href + suffix, shortCircuit: true };
        }
      }
      throw error;
    }
  },
  load(url, context, next) {
    if (url.endsWith("/markdownImageGallery.ts")) {
      const source = readFileSync(fileURLToPath(url), "utf8");
      return { format: "module-typescript", source: source.slice(source.indexOf("export function workGroupImageGallery(")), shortCircuit: true };
    }
    return next(url, context);
  },
});
const load = path => import(pathToFileURL(resolve(root, path)).href);
const links = await load("packages/shared/src/threadLinks.ts");
const terminal = await load("apps/web/src/terminal-links.ts");
const documents = await load("apps/mobile/src/lib/markdownDocument.ts");
const gallery = await load("apps/web/src/components/chat/markdownImageGallery.ts");
const settings = await load("apps/web/src/components/settings/SettingsPanels.logic.ts");
const workspace = await load("packages/client-runtime/src/workspaceBasenameLookup.ts");
const toolSummary = await load("packages/client-runtime/src/t3ToolSummary.ts");
const mobileRequire = createRequire(resolve(root, "apps/mobile/package.json"));
const { unified } = await import(pathToFileURL(mobileRequire.resolve("unified")).href);
const { default: remarkParse } = await import(pathToFileURL(mobileRequire.resolve("remark-parse")).href);
const { default: remarkGfm } = await import(pathToFileURL(mobileRequire.resolve("remark-gfm")).href);
const { remarkKeepWindowsProsePaths } = await load("packages/shared/src/markdownWindowsPaths.ts");
const parser = unified().use(remarkParse).use(remarkGfm).use(remarkKeepWindowsProsePaths);
const hosted = "https://app.t3.codes/threads/there/t1";
const markdowns = [
  `https://example.com/?next=${hosted}`, `[Example](https://example.com/?next=${hosted})`,
  `[ ${hosted} ](https://example.com)`, `[${hosted}](<https://example.com/a b> "Title")`,
  `[a](https://example.com/x_(y)) ${hosted}`, `(${hosted}) **${hosted}**`, `[[[ [old](${hosted})`,
  `\\\\[old](${hosted})`, `\\\\\\\\[old](${hosted})`, `\\[old](${hosted})`,
  `\\[ ${hosted} ](https://example.com)`, "\\[old](t3-thread://v2/there/t1) \\<t3-thread://v2/there/t1>",
  `[label](t3-thread://v1/a)`, `[label](<t3-thread://v2/env/a(b)>)`, `<t3-thread://v1/a>`,
  `\`${hosted}\``, `~~~\n${hosted}\n~~~`, `${hosted}/longer`,
  `${hosted}?a=1`, `[${hosted}](foo "title")`, `![image](${hosted})`,
];
const definitionInputs = [
  "<details>\n<summary>A</summary>\n\n[config][c] ![B][a\\]b]\n\n</details>\n\n[c]: C:\\work\\.t3\\settings.json\n[a\\]b]: https://example.com/a",
  "See [d].\n\n<details>\n<summary>A</summary>\n\n[d]: https://example.com\n\n</details>",
  'See [d].\n\n<details><summary>A</summary>a</details>\n\n> [d]:\n> https://example.com\n> "Title"',
  "See [the\n  doc].\n\n<details><summary>A</summary>a</details>\n[The Doc]: https://example.com",
  "[part rest] and [part > rest]\n\n<details><summary>A</summary>a</details>\n\n[part\n    > rest]: https://example.com/a\n\n[part rest]: https://example.com/b",
  "😀\n\n[c]: C:\\work\\a\n\n```\n[hidden]: /hidden\n```",
  "Plain text.",
];
const definitions = definitionInputs.map(markdown => {
  const flattened = documents.markdownWithHtml(markdown);
  return { flattened, root: parser.parse(flattened), expected: documents.markdownReferenceDefinitions(markdown) };
});
const fragments = [
  { fragment: "See [x].\n\n[X]: https://inside.example", definitions: ["[ x ]: https://outside.example"] },
  { fragment: "See [SS].\n\n[ß]: https://inside.example", definitions: ["[SS]: https://outside.example"] },
  { fragment: "[config][c] ![B][a\\]b]", definitions: definitions[0].expected },
  { fragment: "See [d].", definitions: definitions[2].expected },
  { fragment: "Plain text.", definitions: ["[d]: /repo/a"] },
  { fragment: "See [d].", definitions: [] },
].map(input => ({ ...input, flattened: documents.markdownWithHtml(input.fragment), root: parser.parse(documents.markdownWithHtml(input.fragment)), expected: documents.withReferenceDefinitions(input.fragment,input.definitions) }));
const commands = [
  ["cd /work/repo-c && cd /work/repo-b && git diff", "git status", "/bin/zsh -lc \"cd '/work/my repo'; ls\"", "cd -P /work/repo-a # not this; cd /work/commented", "cd /work/repo-c\npwd"],
  ["cd docs && cd ../other && cd ~/repo && cd $ROOT/x && cd /work/*/src", "abcd /work/a; echo cd /work/b; git -C /work/c status; cd /work/d", "echo \"cd /work/g\"; echo 'x; cd /work/h' && grep -c \"cd /work/i\" notes", "echo bash -c 'cd /work/j'; cd -", "cat <<EOF > notes\ncd /work/k\nEOF", "cd /work/e\\ f && cd `pwd`/x"],
  ["cd 'C:\\work\\my repo'; cd /a; cd /b", "sh -c 'cd /inner && pwd'", "cd /a"],
  Array.from({length:80},(_,i)=>`cd /root-${i}`),
  ["cd /a\r\ncd /b", "cd /c\uFEFF; cd /d"],
];
const directoryCases = commands.map(commands => {
  const items = [...commands,"the reply","cd /later"].map((input,i)=>({sourceThreadId:"thread",sourceItemId:`item-${i}`,position:i,visibility:"local",item:{type:"command_execution",id:`item-${i}`,threadId:"thread",ordinal:i,input,status:"completed",title:null,startedAt:null,completedAt:null,updatedAt:"2026-01-01T00:00:00Z",runId:null,nodeId:null,providerThreadId:null,providerTurnId:null,nativeItemRef:null,parentItemId:null}}));
  const reply = {sourceThreadId:"thread",sourceItemId:`item-${commands.length}`};
  return {items,reply,expected:workspace.agentWorkingDirectories(items,reply)};
});
const terminalInputs = ["src/main.ts","/repo/src/main.ts","C:\\repo\\src\\main.ts","app/(auth)/page.tsx"].flatMap(path=>[`${path}(42,7): error TS2322: Type mismatch`,`${path}(42,7): warning TS1000: x`]);
terminalInputs.push("PS C:\\dev\\folder (42,7)> ","/tmp/report(42,7)","src/Program.cs(3,10): error CS1002: x");
const editTraces = [
  [["edit","1"],["edit",""],["edit","3"],["edit","30"],["commit"],["commit"],["sync",30]],
  ...["","0","91","1.5","abc","0x1e","3e1","+30","30.0","0b11110","0x+1e"].map(draft=>[["edit",draft],["commit"]]),
  [["edit","30"],["commit"],["sync",30],["fail",1],["commit"],["edit","3"],["edit","30"],["commit"]],
  [["edit","30"],["commit"],["edit","14"],["commit"],["fail",1],["sync",14]],
  [["edit","30"],["commit"],["edit","60"],["commit"],["sync",30],["fail",2]],
  [["edit","1"],["cancel"],["commit"],["edit","4"],["sync",30]],
];
const stateCases = editTraces.map(steps=>{
  let field=settings.autoSettleDaysField(14);
  const states=[];
  for (const [operation,value] of steps) {
    let submit=null;
    if(operation==="commit") {const result=settings.commitAutoSettleDaysField(field);field=result.field;submit=result.submit;}
    else field=settings[{edit:"editAutoSettleDaysField",sync:"syncAutoSettleDaysField",fail:"failAutoSettleDaysField",cancel:"cancelAutoSettleDaysField"}[operation]](field,value);
    states.push({field,submit});
  }
  return {steps,states};
});
const viewed=[{path:"/first",item:{src:"first"}},{path:"/pending",item:null},{path:"/second",item:{src:"second"}},{path:"/third",item:{src:"third"}}];
const galleries=["/second","/other",null].map(clicked=>{const preview={images:[{src:"second#fresh",actionsSource:{token:"fresh"}}],index:0};return {viewed,clicked,preview,expected:gallery.workGroupImageGallery(viewed,clicked,preview)};});
const themeSummaries = ["theme-read","theme-set"].flatMap(action => [[],["completed"],["completed","completed"],["failed","failed"],["unfinished","completed"],["completed","failed"]].map(outcomes => {
  const calls=outcomes.map(outcome=>({input:null,output:null,outcome}));
  return {action,calls,expected:toolSummary.summarizeT3ToolCalls(action,calls)};
}));
console.log(JSON.stringify({definitions,fragments,directoryCases,stateCases,galleries,themeSummaries,terminal:terminalInputs.map(input=>({input,expected:terminal.extractTerminalLinks(input)})),threadLinks:markdowns.map(markdown=>({markdown,expected:links.replaceThreadLinks(markdown,()=>"REPLACED"),refs:links.threadLinkRefs(markdown,"local")}))}));
