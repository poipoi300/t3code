// Run with Node from apps/server; imports production definitions, never copied descriptions.
import { readFile, writeFile, mkdir, readdir } from "node:fs/promises";
import { createRequire, registerHooks } from "node:module";
import { createHash } from "node:crypto";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const dest = fileURLToPath(new URL("./fixtures/", import.meta.url));
const sourcePaths = new Set<string>();
registerHooks({load(url,context,nextLoad) {
  const result=nextLoad(url,context);
  if(!url.startsWith("file:")) return result;
  const path=fileURLToPath(url);
  if(path.startsWith(root) && !path.includes("node_modules") && /\.(ts|json)$/.test(path)) sourcePaths.add(path);
  if(!url.endsWith("/mcp/McpToolAccess.ts")) return result;
  let source=String(result.source);
  const kinds=["reads","readsAsCaller","actsAsCaller","writes","writesWithCapability","writesThreads","startsThreads","writesEnvironment","either","afterForwarding","toLayer"];
  for(const kind of kinds) {
    const declaration=`export const ${kind} =`;
    if(!source.includes(declaration)) throw new Error(`Cannot observe MCP access constructor ${kind}`);
    source=source.replace(declaration,`const __original_${kind} =`);
  }
  source+=`\nconst __accessTrees=new WeakMap(); export const __accessDeclarations=new Map();\n`;
  for(const kind of kinds.filter(kind=>kind!=="toLayer")) {
    const tree=kind==="either" ? `{kind:"either",predicate:args[0],first:__accessTrees.get(args[1]),second:__accessTrees.get(args[2])}`
      :kind==="afterForwarding"?`__accessTrees.get(args[0])`:`{kind:${JSON.stringify(kind)}}`;
    source+=`export const ${kind}=(...args)=>{const result=__original_${kind}(...args);__accessTrees.set(result,${tree});return result;};\n`;
  }
  source+=`export const toLayer=(toolkit,build)=>{const observe=(declarations)=>{for(const [name,value] of Object.entries(declarations)){const tree=__accessTrees.get(value);if(!tree)throw new Error("No observed MCP access declaration for "+name);__accessDeclarations.set(name,tree);}return declarations;};return __original_toLayer(toolkit,Effect.isEffect(build)?Effect.map(build,observe):observe(build));};\n`;
  return {...result,source};
}});
const require = createRequire(resolve(root, "apps/server/package.json"));
const load = (name: string) => import(pathToFileURL(name.startsWith("@t3tools/")
  ? resolve(root, name.replace("@t3tools/", "packages/").replace("/server/", "/src/server/") + ".ts")
  : require.resolve(name)).href);
const { Effect, Layer, Schema, Context, Option } = {
  Effect: await load("effect/Effect"), Layer: await load("effect/Layer"),
  Schema: await load("effect/Schema"), Context: await load("effect/Context"),
  Option: await load("effect/Option"),
};
const { McpServer, McpSchema, Tool } = await load("effect/ai");
const server = (path: string) => import(pathToFileURL(resolve(root, "apps/server/src", path)).href);
const http = await server("mcp/McpHttpServer.ts");
const contracts = await import(pathToFileURL(resolve(root, "packages/contracts/src/index.ts")).href);
const invocation = await server("mcp/McpInvocationContext.ts");

// Use the same registration-only service doubles as the production context-budget test.
const budget = await readFile(resolve(root, "apps/server/src/provider/AgentContextBudget.test.ts"), "utf8");
const declarations = budget.slice(budget.indexOf("const toolHandlerServices"), budget.indexOf("const utf8Bytes"));
const imports: Record<string, unknown> = { Layer, process };
for (const match of budget.matchAll(/import \{([^}]+)\} from "([^"]+)";/gs)) {
  const names = match[1].split(",").map((x) => x.trim()).filter(Boolean);
  if (!names.some((name) => declarations.includes(name))) continue;
  const module = match[2].startsWith(".")
    ? await import(pathToFileURL(resolve(root, "apps/server/src/provider", match[2])).href)
    : await load(match[2]);
  for (const name of names) imports[name] = module[name];
}
imports.NodeServices = await load("@effect/platform-node/NodeServices");
imports.NodeCrypto = await load("@effect/platform-node/NodeCrypto");
const services = Function(...Object.keys(imports), `${declarations}; return toolHandlerServices;`)(...Object.values(imports));
const toolkits: Record<string, any> = {};
const httpSource=await readFile(resolve(root,"apps/server/src/mcp/McpHttpServer.ts"),"utf8");
const toolkitNames=[...new Set([...httpSource.matchAll(/from "\.\/toolkits\/([^/]+)\/tools\.ts"/g)].map(match=>match[1]))];
if(!toolkitNames.length) throw new Error("No TypeScript toolkit imports found");
for (const name of toolkitNames) {
  const module = await server(`mcp/toolkits/${name}/tools.ts`);
  const key = name === "toolSearch" ? "ToolSearchToolkit" : name[0].toUpperCase() + name.slice(1) + "Toolkit";
  toolkits[name] = module[key];
}
const layer = Layer.mergeAll(http.layerTools, http.layerToolSearch).pipe(
  Layer.provideMerge(http.layerInProcessMcpServer), Layer.provide(services),
);
const packageJson = JSON.parse(await readFile(resolve(root, "apps/server/package.json"), "utf8"));
const scope = (subagent: boolean, linkedEnvironments: boolean, client = false) => ({
  environmentId: "environment-fixture", requestNamespace: "fixture", issuedAt: 0,
  thread: client ? undefined : { threadId: "thread-fixture", providerSessionId: "fixture", providerInstanceId: "codex", subagent },
  client: client ? { sessionId: "fixture", label: "fixture", access: "read-only" } : undefined,
  capabilities: new Set(["orchestration", "worktree", "pull-requests", "preview", "device"]), linkedEnvironments,
});
const client = McpSchema.McpServerClient.of({
  clientId: 1, protocolVersion: "2025-06-18", clientCapabilities: {}, clientInfo: {name: "fixture", version: "1"},
  initializePayload: {protocolVersion: "2025-06-18", capabilities: {}, clientInfo: {name: "fixture", version: "1"}},
  getClient: Effect.die("unused"),
});
const fixture = await Effect.runPromise(Effect.scoped(Effect.gen(function* () {
  const mcp = yield* McpServer.McpServer;
  const modes: Record<string, unknown> = {};
  const legacyModes: Record<string, unknown> = {};
  const tools = mcp.tools.map(({tool}: any) => tool);
  const definitions: Record<string, unknown> = {};
  for (const [kit, toolkit] of Object.entries(toolkits)) for (const tool of Object.values(toolkit.tools) as any[]) {
    const inputSchema = Tool.getJsonSchema(tool);
    const nullPaths = new Set<string>();
    const trimPaths = new Set<string>();
    const nonBlankPaths = new Set<string>();
    const optionsPaths = new Set<string>();
    const jsonPaths = new Set<string>();
    const visitNulls = (schema: any, path: string, depth = 0) => {
      if (!schema || depth > 30) return;
      if (Schema.is(schema)(null)) nullPaths.add(path);
      const decode = Schema.decodeUnknownSync(Schema.toCodecJson(schema));
      for (const sample of ["x", "https://example.test/", "09:30", "2026-01-01T00:00:00Z"]) try {
        if (decode(` ${sample} `) === sample) {
          trimPaths.add(path);
          try { decode("   "); } catch { nonBlankPaths.add(path); }
        }
      } catch {}
      try {
        const decoded = decode({__MCP_OPTION__:true});
        if (Array.isArray(decoded) && decoded[0]?.id === "__MCP_OPTION__") optionsPaths.add(path);
      } catch {}
      try {
        const decoded = decode('{"type":"interval","everyMs":60000}');
        if (decoded?.type === "interval") jsonPaths.add(path);
      } catch {}
      const ast = schema.ast;
      if (ast.propertySignatures) for (const field of ast.propertySignatures) visitNulls(Schema.make(field.type), `${path}.${field.name}`, depth + 1);
      if (ast.types) for (const member of ast.types) visitNulls(Schema.make(member), path, depth + 1);
      if (ast.rest) for (const member of ast.rest) visitNulls(Schema.make(member), `${path}[]`, depth + 1);
    };
    visitNulls(tool.parametersSchema, "$");
    definitions[tool.name] = { toolkit: kit, inputSchema, failureMode: tool.failureMode, nullPaths: [...nullPaths], trimPaths:[...trimPaths], nonBlankPaths:[...nonBlankPaths], optionsPaths:[...optionsPaths], jsonPaths:[...jsonPaths], decodedInputSchema:Schema.toJsonSchemaDocument(Schema.toType(tool.parametersSchema)).schema, routed: tool.parametersSchema.fields?.environmentId === contracts.OrchestratorMcpEnvironmentTarget,
      outputSchema: Schema.toJsonSchemaDocument(Schema.toCodecJson(tool.successSchema)).schema,
    };
  }
  const registered=tools.map((tool:any)=>tool.name).sort();
  if(JSON.stringify(registered)!==JSON.stringify(Object.keys(definitions).sort())) throw new Error("MCP definitions do not cover exactly the registered tools");
  const actionSchemas = Object.fromEntries(Object.entries(contracts.PreviewAutomationActInputs).map(([name, schema]) => [name, Schema.toJsonSchemaDocument(Schema.toCodecJson(schema)).schema]));
  const capabilityMessages={preview:new contracts.PreviewAutomationUnavailableError({capability:"preview",environmentId:"fixture"}).message,
    generic:new contracts.McpCapabilityUnavailableError({capability:"__CAPABILITY__",environmentId:"fixture"}).message};
  const errorTags = Object.fromEntries(Object.entries({preview:contracts.PreviewAutomationError,device:contracts.DeviceToolError}).map(([name,schema]) => {
    const tags = new Set<string>();
    const visit = (node:any) => {
      if (!node || typeof node !== "object") return;
      const tag = node?.properties?._tag;
      for (const value of tag?.enum ?? (tag?.const ? [tag.const] : [])) tags.add(value);
      for (const branch of Object.values(node)) visit(branch);
    };
    visit(Schema.toJsonSchemaDocument(Schema.toCodecJson(schema)));
    if (!tags.size) throw new Error(`No error tags for ${name}`);
    return [name,[...tags]];
  }));
  return { serverVersion: packageJson.version, tools, modes, legacyModes, definitions, actionSchemas, capabilityMessages, errorTags };
})).pipe(Effect.provide(layer)));
const Arbitrary = await load("effect/Arbitrary");
const decoderCases: Record<string, unknown> = {};
const boundaryInputs: Record<string, unknown[]> = {
  delegate_task:[{task:"  Task \ufeff",target:{options:{effort:"high",fast:true}}},{task:"   "}],
  schedule_task:[{prompt:" Prompt ",schedule:'{"type":"interval","everyMs":60000}'},{prompt:"Prompt",schedule:'{"type":"interval","everyMs":1}'},{prompt:"Prompt",schedule:"invalid-json"}],
  t3_thread_update:[{threadId:" thread ",action:"rename",title:" Title "},{action:"rename"},{action:"regenerate_title",title:"invalid"}],
  request_secret:[{label:" Label ",reason:" Reason ",value:"  secret  "},{label:"Label",reason:"Reason",value:""},{label:"Label",reason:"Reason",url:"file:///private"}],
  send_to_user:[{message:""},{message:" "},{message:"😀".repeat(2000)},{message:"😀".repeat(2000)+"x"}],
};
for (const toolkit of Object.values(toolkits)) for (const tool of Object.values(toolkit.tools) as any[]) {
  let values;
  try {
    values = await Effect.runPromise(Arbitrary.sampleEffect(Arbitrary.schema(tool.parametersSchema), {count: 2, size: 4, seed: 29747, maxDiscards: 100}));
  } catch {
    const {samplesFor} = await import(pathToFileURL(resolve(root, "rust/tools/export/samples.ts")).href);
    const samples = await samplesFor(tool.parametersSchema);
    values = samples.values.slice(0,2).map((value: unknown) => Schema.decodeUnknownSync(Schema.toCodecJson(tool.parametersSchema))(value));
    if (!values.length) throw new Error(`No input witness for ${tool.name}: ${samples.failures.join("; ")}`);
  }
  const codec = Schema.toCodecJson(tool.parametersSchema);
  decoderCases[tool.name] = values.map((value: unknown) => {
    const wire = JSON.parse(JSON.stringify(Schema.encodeUnknownSync(codec)(value)), (_, value) => typeof value === "string" ? value.toWellFormed() : value);
    const decoded = Schema.decodeUnknownSync(codec)(structuredClone(wire));
    let invalidAction = false;
    if (tool.name === "preview_act") {
      const {action, ...params} = decoded;
      try { Schema.decodeUnknownSync(contracts.PreviewAutomationActInputs[action])(params); } catch { invalidAction = true; }
    }
    return {input: wire, decoded, ...(invalidAction ? {invalidAction: true} : {})};
  });
  for (const input of boundaryInputs[tool.name] ?? []) {
    try { decoderCases[tool.name].push({input,decoded:Schema.decodeUnknownSync(codec)(input)}); }
    catch { decoderCases[tool.name].push({input,invalid:true}); }
  }
}
const { HttpRouter } = await load("effect/http");
const registry = await server("mcp/McpSessionRegistry.ts");
const settings = await server("serverSettings.ts");
let current = scope(false, true);
const app = HttpRouter.toWebHandler(http.layerMcpTransport(Layer.mergeAll(http.layerTools, http.layerToolSearch)).pipe(
  Layer.provide(Layer.mergeAll(
    Layer.mock(registry.McpSessionRegistry)({resolve: () => Effect.sync(() => current)}),
    Layer.mock(settings.ServerSettingsService)({getSettings: Effect.sync(() => ({linkedEnvironmentsEnabled: current.linkedEnvironments}))}),
  )), Layer.provide(services),
), {disableLogger: true});
try {
  for (const subagent of [false, true]) for (const links of [false, true]) for (const external of [false, true]) for (const deferred of [false, true]) {
    current = scope(subagent, links, external);
    const key = `${external ? "client" : subagent ? "subagent" : "thread"}/${links ? "links" : "no-links"}/${deferred ? "deferred" : "full"}`;
    const response = await app.handler(new Request("http://127.0.0.1/mcp", {method: "POST", headers: {
      authorization: "Bearer fixture", accept: "application/json, text/event-stream", "content-type": "application/json",
      "mcp-method": "tools/list", "mcp-protocol-version": "2026-07-28", ...(deferred ? {"t3-tool-exposure": "deferred"} : {}),
    }, body: JSON.stringify({jsonrpc: "2.0", id: 1, method: "tools/list", params: {_meta: {
      "io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientCapabilities": {},
      "io.modelcontextprotocol/clientInfo": {name: "fixture", version: "1"},
    }}})}));
    const body = await response.json();
    if (response.status !== 200 || !body.result) throw new Error(JSON.stringify(body));
    fixture.modes[key] = body.result;
    const headers = {authorization:"Bearer fixture",accept:"application/json, text/event-stream","content-type":"application/json",...(deferred ? {"t3-tool-exposure":"deferred"} : {})};
    const initialized = await app.handler(new Request("http://127.0.0.1/mcp",{method:"POST",headers,body:JSON.stringify({jsonrpc:"2.0",id:2,method:"initialize",params:{protocolVersion:"2025-06-18",capabilities:{},clientInfo:{name:"fixture",version:"1"}}})}));
    const session=initialized.headers.get("mcp-session-id");
    await initialized.arrayBuffer();
    const legacy = await app.handler(new Request("http://127.0.0.1/mcp",{method:"POST",headers:{...headers,"mcp-session-id":session!,"mcp-protocol-version":"2025-06-18"},body:JSON.stringify({jsonrpc:"2.0",id:3,method:"tools/list",params:{}})}));
    const legacyBody=await legacy.json();
    if (!legacyBody.result) throw new Error(JSON.stringify(legacyBody));
    fixture.legacyModes[key]=legacyBody.result;
    await app.handler(new Request("http://127.0.0.1/mcp",{method:"DELETE",headers:{...headers,"mcp-session-id":session!}}));
  }
} finally { await app.dispose(); }
const core = await load("@t3tools/provider-core/server/orchestrationInstructions");
const runtime = await load("@t3tools/provider-core/server/runtimeInstructions");
const codex = await server("provider/CodexDeveloperInstructions.ts");
const claude = await server("orchestration-v2/Adapters/ClaudeAdapterV2.ts");
const instructions: Record<string, unknown> = {
  constants: Object.fromEntries(Object.entries(core).filter(([_, value]) => typeof value === "string" || Array.isArray(value))),
  runtime: runtime.buildRuntimeInstructions({harness: "__HARNESS__", model: "__MODEL__", modelName: "__MODEL_NAME__", reasoningEffort: "__EFFORT__"}),
  runtimeWithoutLinking: runtime.buildRuntimeInstructions({harness: "__HARNESS__", model: "__MODEL__", modelName: "__MODEL_NAME__", reasoningEffort: "__EFFORT__", pullRequestLinking: false}),
};
const messageSource=await readFile(resolve(root,"apps/server/src/mcp/toolkits/message/handlers.ts"),"utf8");
const acknowledgement=messageSource.match(/\bmessage:\s*("(?:[^"\\]|\\.)*")/u);
if(!acknowledgement) throw new Error("send_to_user acknowledgement not found in production handler");
instructions.sendToUserAcknowledgement=JSON.parse(acknowledgement[1]);
const themeSource=await readFile(resolve(root,"apps/server/src/mcp/toolkits/theme/handlers.ts"),"utf8");
const themeInvalid=themeSource.match(/\bmessage:\s*("(?:[^"\\]|\\.)*")/u);
if(!themeInvalid) throw new Error("Theme validation message not found in production handler");
instructions.themeSetInvalidRequest=JSON.parse(themeInvalid[1]);
const peers=await server("peer/PeerForwarding.ts");
instructions.linkedEnvironmentsOff={code:peers.linkedEnvironmentsOff.code,message:peers.linkedEnvironmentsOff.message};
instructions.runningTaskNext=contracts.ORCHESTRATOR_MCP_RUNNING_TASK_NEXT;
const contextBudget=JSON.parse(await readFile(resolve(root,"apps/server/src/provider/agentContextBudget.json"),"utf8"));
instructions.contextBudget=contextBudget;
for (const model of [false, true]) for (const name of [false, true]) for (const effort of [false, true]) for (const linking of [false, true]) {
  instructions[`runtime/${model}/${name}/${effort}/${linking}`] = runtime.buildRuntimeInstructions({harness:"__HARNESS__", ...(model ? {model:"__MODEL__"} : {}), ...(name ? {modelName:"__MODEL_NAME__"} : {}), ...(effort ? {reasoningEffort:"__EFFORT__"} : {}), pullRequestLinking: linking});
}
for (const browser of [false, true]) for (const device of [false, true]) {
  instructions[`codex/${browser}/${device}`] = codex.buildCodexAdditionalContext({model: "__MODEL__", modelName: "__MODEL_NAME__", reasoningEffort: "__EFFORT__"}, {browser, device});
}
for (const mcp of [false, true]) for (const device of [false, true]) {
  const options = claude.makeClaudeQueryOptions({modelSelection: {instanceId: "claude", model: "fixture"}, nativeThreadId: "fixture", resume: false, cwd: null,
    ...(mcp ? {mcpServers: {"t3-code": {type: "http", url: "http://127.0.0.1/mcp"}}} : {}), deviceToolsAvailable: device});
  instructions[`claude/${mcp}/${device}`] = options.systemPrompt.append;
  for (const interactionMode of ["plan", "default"]) {
    const state = {interactionMode, hasT3Mcp: mcp, hasDeviceTools: device};
    instructions[`acp/${interactionMode}/${mcp}/${device}`] = core.t3AcpPromptWithInstructions({prompt: "__PROMPT__", state});
  }
  instructions[`system/${mcp}/${device}`] = core.t3OrchestrationSystemPrompt(mcp, device) ?? null;
  for (const delivered of [false, true]) for (const deviceDelivered of [false, true]) {
    instructions[`prompt/${mcp}/${device}/${delivered}/${deviceDelivered}`] = core.t3OrchestrationPrompt({prompt: "__PROMPT__", hasT3Mcp: mcp, hasDeviceTools: device, orchestrationDelivered: delivered, deviceInstructionsDelivered: deviceDelivered});
  }
}
const budgetClaude=claude.makeClaudeQueryOptions({modelSelection:{instanceId:"claudeAgent",model:"claude-opus-5-5"},nativeThreadId:"context-budget",resume:false,cwd:null,mcpServers:{"t3-code":{type:"http",url:"http://127.0.0.1/mcp"}}}).systemPrompt.append;
const budgetCodex=Object.values(codex.buildCodexAdditionalContext({model:"gpt-6.1-sol",modelName:"GPT-6.1 Sol",reasoningEffort:"high"},{browser:true,device:true})).map((entry:any)=>entry.value).join("");
instructions.budgetContexts={claude:budgetClaude,codex:budgetCodex};
const budgetTools=fixture.tools.filter((tool:any)=>tool.name!=="tool_search");
const measured={claudeSystemAppendBytes:Buffer.byteLength(budgetClaude),codexAdditionalContextBytes:Buffer.byteLength(budgetCodex),mcpToolListJsonBytes:Buffer.byteLength(JSON.stringify(budgetTools.map(({name,description,inputSchema}:any)=>({name,description,inputSchema}))))};
if(JSON.stringify(measured)!==JSON.stringify(contextBudget)) throw new Error(`Context budget differs: ${JSON.stringify(measured)} versus ${JSON.stringify(contextBudget)}`);
const files = {"tools.json": fixture, "instructions.json": instructions, "decoders.json": decoderCases};
await mkdir(dest, {recursive: true});
for (const [name, value] of Object.entries(files)) {
  const data = JSON.stringify(value, null, 2) + "\n";
  const path = resolve(dest, name);
  if (process.argv.includes("--check")) {
    if (await readFile(path, "utf8") !== data) throw new Error(`${name} differs; regenerate MCP fixtures`);
  } else await writeFile(path, data);
}
console.log(JSON.stringify({tools: fixture.tools.length, toolkits: Object.fromEntries(Object.entries(toolkits).map(([name, kit]) => [name, Object.keys(kit.tools).length])), modes: Object.keys(fixture.modes).length}));

const rustTypes = new Map<string, string>();
async function discoverTypes(dir: string, module: string[] = []) {
  for (const entry of await readdir(dir, {withFileTypes: true})) {
    if (entry.isDirectory()) { await discoverTypes(resolve(dir, entry.name), [...module, entry.name]); continue; }
    if (!entry.name.endsWith(".rs")) continue;
    const text = await readFile(resolve(dir, entry.name), "utf8");
    const path = [...module, entry.name.slice(0, -3)].filter((part) => part !== "mod");
    for (const match of text.matchAll(/pub (?:struct|enum|type) (\w+)(?:[\s(<={;])/g)) {
      if (!rustTypes.has(match[1])) rustTypes.set(match[1], `t3_contracts::${path.join("::")}::${match[1]}`);
    }
  }
}
await discoverTypes(resolve(root, "rust/crates/t3-contracts/src"));
const traitName = (kit: string) => kit[0].toUpperCase() + kit.slice(1) + "Service";
const fieldName = (kit: string) => kit.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase());
const methods: Record<string, {name: string; input: string; output: string}[]> = {};
const typeFor = (text: string, field: string) => {
  const match = text.match(new RegExp(`\\b${field}: (\\w+)\\s*[,\\n]`));
  return match && rustTypes.get(match[1]);
};
for (const [kit, toolkit] of Object.entries(toolkits)) {
  const text = await readFile(resolve(root, `apps/server/src/mcp/toolkits/${kit}/tools.ts`), "utf8");
  methods[kit] = Object.keys(toolkit.tools).map((name) => {
    const start = text.indexOf(`Tool.make("${name}"`);
    const block = start < 0 ? undefined : text.slice(start).split(/\n(?:export )?const /)[0];
    return {name, input: block && typeFor(block, "parameters") || "DecodedInput", output: block && typeFor(block, "success") || "Value"};
  });
}
let generated = `// Generated by export.ts from the toolkit declarations and available contract types.\nuse crate::{CallContext, DecodedInput, McpError, Result};\nuse async_trait::async_trait;\nuse serde_json::Value;\nuse std::sync::Arc;\n\n`;
const serviceOwner:Record<string,string> = {attachment:"t3-workspace::AttachmentStore",device:"t3-device::DeviceService",environment:"t3-config::EnvironmentMcpService",html:"t3-preview::HtmlService",message:"t3-mcp::message",orchestrator:"t3-orchestration::OrchestratorMcpService",preview:"t3-preview::PreviewService",previewControls:"t3-preview::PreviewControlsService",project:"t3-orchestration::ProjectService",pullRequests:"t3-orchestration::ThreadPullRequestService",theme:"t3-server::EnvironmentThemeService",thread:"t3-orchestration::ThreadManagementService",toolSearch:"t3-mcp::ToolExposure",worktree:"t3-git::WorktreeMcpService"};
for (const [kit, tools] of Object.entries(methods)) {
  generated += `#[async_trait]\npub trait ${traitName(kit)}: Send + Sync {\n`;
  for (const tool of tools) generated += `    async fn ${tool.name}(&self, _context: CallContext, _input: ${tool.input}) -> Result<${tool.output}> { ${tool.name==="send_to_user"?"crate::message::send_to_user(_input)":`Err(crate::adapters::not_available("${tool.name}", "${serviceOwner[kit]}::${tool.name}"))`} }\n`;
  generated += `}\n\n`;
}
generated += `pub struct Unavailable;\n`;
for (const kit of Object.keys(methods)) generated += `impl ${traitName(kit)} for Unavailable {}\n`;
generated += `\n#[derive(Clone)]\npub struct Services {\n`;
for (const kit of Object.keys(methods)) generated += `    pub ${fieldName(kit)}: Arc<dyn ${traitName(kit)}>,\n`;
generated += `}\nimpl Default for Services {\n    fn default() -> Self { Self {\n`;
for (const kit of Object.keys(methods)) generated += `        ${fieldName(kit)}: Arc::new(Unavailable),\n`;
generated += `    }}\n}\nimpl Services {\n    pub async fn dispatch(&self, name: &str, context: CallContext, input: DecodedInput) -> Result<Value> {\n        match name {\n`;
for (const [kit, tools] of Object.entries(methods)) for (const tool of tools) {
  generated += `            "${tool.name}" => serde_json::to_value(self.${fieldName(kit)}.${tool.name}(context, ${tool.input === "DecodedInput" ? "input" : "input.decode()?"}).await?).map_err(|_| McpError::Internal),\n`;
}
generated += `            _ => Err(McpError::InvalidInput(format!("Tool '{name}' not found"))),\n        }\n    }\n}\n`;
const generatedPath = resolve(dest, "../src/services.rs");
function formatRust(source:string) {
  const result=spawnSync("rustfmt",["--edition","2024","--emit","stdout"],{input:source,encoding:"utf8"});
  if (result.status!==0) throw new Error(result.stderr || String(result.error));
  return result.stdout;
}
generated=formatRust(generated);
if (process.argv.includes("--check")) {
  const actual=await readFile(generatedPath,"utf8");
  const expected=generated;
  if (actual!==expected) {
    let index=0;while (actual[index]===expected[index] && index<actual.length) index++;
    throw new Error(`services.rs differs at ${index}: ${actual.slice(index,index+80)} versus ${expected.slice(index,index+80)}`);
  }
} else await writeFile(generatedPath, generated);

let routingTest = `#![allow(clippy::unwrap_used)]\n// Generated routing coverage over real TypeScript codec witnesses.\nuse t3_mcp::{*, services::*};\nuse async_trait::async_trait;\nuse serde_json::{Value,json};\nuse std::sync::{Arc,Mutex};\nstruct Recorder(Arc<Mutex<Vec<String>>>);\n`;
for (const [kit, tools] of Object.entries(methods)) {
  routingTest += `#[async_trait]\nimpl ${traitName(kit)} for Recorder {\n`;
  for (const tool of tools) routingTest += `    async fn ${tool.name}(&self, _context: CallContext, _input: ${tool.input}) -> Result<${tool.output}> { self.0.lock().unwrap().push("${tool.name}".into()); Err(McpError::failure("recorded", "recorded")) }\n`;
  routingTest += `}\n`;
}
routingTest += `struct Store;\n#[async_trait]\nimpl AccessStore for Store {async fn thread_shell(&self,_id:&t3_contracts::base_schemas::ThreadId)->Result<Option<ThreadShell>> {Ok(None)}}\n#[tokio::test]\nasync fn every_tool_decodes_and_dispatches_to_its_own_service_method_once() {\nlet calls=Arc::new(Mutex::new(Vec::new()));\nlet services=Services {\n`;
for (const kit of Object.keys(methods)) routingTest += `    ${fieldName(kit)}:Arc::new(Recorder(calls.clone())),\n`;
routingTest += `};\nlet scope:InvocationScope=serde_json::from_value(json!({"environmentId":"environment","requestNamespace":"fixture","issuedAt":0,"capabilities":["orchestration"],"client":{"sessionId":"fixture","label":"fixture","access":"full-access"}})).unwrap();\nlet context=access::authorize(Arc::new(Store),scope,AccessRule::Reads,"t3_thread_read",&json!({}),false).await.unwrap();\nlet witnesses:Value=serde_json::from_str(include_str!("../fixtures/decoders.json")).unwrap();\nfor (name,cases) in witnesses.as_object().unwrap() {\n    let case=&cases[0];\n    if case["invalidAction"]==true {continue;}\n    let input=decode_input(name,case["input"].clone()).unwrap_or_else(|error|panic!("{name}: {error}"));\n    let before=calls.lock().unwrap().len();\n    let error=services.dispatch(name,context.clone(),input).await.unwrap_err();\n    assert!(matches!(error,McpError::Failure {ref code,..} if code=="recorded"),"{name}: {error}");\n    let calls=calls.lock().unwrap();\n    assert_eq!(calls.len(),before+1,"{name}");\n    assert_eq!(calls.last().unwrap(),name);\n}\n}\n`;
const routingPath=resolve(dest,"../tests/routing.rs");
await mkdir(resolve(dest,"../tests"),{recursive:true});
routingTest=formatRust(routingTest);
if (process.argv.includes("--check")) {
  if (await readFile(routingPath,"utf8")!==routingTest) throw new Error("routing.rs differs; regenerate");
} else await writeFile(routingPath,routingTest);

const adapters = {
  orchestration:{feature:"orchestration",type:"orchestration::OrchestrationAdapter",tools:["t3_thread_send","t3_thread_wait","t3_thread_interrupt","t3_thread_update","t3_workflow_stop","t3_background_wait","t3_thread_configure","t3_thread_organize","t3_pending_request","t3_thread_transfer","t3_queue","t3_project_read","t3_project_list","t3_thread_send_attachments"]},
  scheduler:{feature:"scheduler",type:"scheduler::SchedulerAdapter",tools:["list_scheduled_tasks","delete_scheduled_task","schedule_task","run_scheduled_task_now"]},
  worktree:{feature:"git",type:"worktree::WorktreeAdapter",tools:["t3_worktree_list"]},
};
const nativeMethods:Record<string,string[]> = {
  t3_attachment_prepare_upload:["t3-workspace::AttachmentStore::issue_owned_upload"],
  t3_attachment_discard:["t3-workspace::AttachmentStore::discard_owned"],
  t3_thread_send_attachments:["t3-orchestration::ThreadMessageIntake::send_to_thread"],
  t3_thread_send:["t3-orchestration::ThreadManagementService::send_to_thread"],
  t3_thread_wait:["t3-orchestration::ThreadManagementService::wait_for_thread"],
  t3_thread_interrupt:["t3-orchestration::ThreadManagementService::interrupt_thread","t3-orchestration::ThreadManagementService::stop_thread","t3-orchestration::ThreadManagementService::stop_delegated_tasks","t3-orchestration::EventStore::read"],
  t3_workflow_stop:["t3-orchestration::ThreadStopService::stop_workflow"],
  t3_background_wait:["t3-orchestration::ThreadStopService::await_background_tasks"],
  t3_thread_update:["t3-orchestration::Dispatcher::dispatch_with_limit","t3-orchestration::ProjectionReader::shell"],
  t3_thread_configure:["t3-orchestration::Dispatcher::dispatch_with_limit","t3-orchestration::ProjectionReader::shell"],
  t3_thread_organize:["t3-orchestration::Dispatcher::dispatch_with_limit"],
  t3_pending_request:["t3-orchestration::ThreadManagementService::records","t3-orchestration::Dispatcher::dispatch_with_limit"],
  t3_thread_transfer:["t3-orchestration::ThreadManagementService::records","t3-orchestration::Dispatcher::dispatch_with_limit"],
  t3_queue:["t3-orchestration::ThreadManagementService::records","t3-orchestration::queued_runs_in_delivery_order","t3-orchestration::Dispatcher::dispatch_with_limit"],
  t3_project_list:["t3-orchestration::stores::projects::list"],
  t3_project_read:["t3-orchestration::stores::projects::get"],
  list_scheduled_tasks:["t3-scheduler::ScheduledTaskService::list","t3-orchestration::ProjectionReader::shell"],
  delete_scheduled_task:["t3-scheduler::ScheduledTaskService::get","t3-scheduler::ScheduledTaskService::delete"],
  schedule_task:["t3-scheduler::ScheduledTaskService::get","t3-scheduler::ScheduledTaskService::upsert"],
  run_scheduled_task_now:["t3-scheduler::ScheduledTaskService::get","t3-scheduler::ScheduledTaskService::run_now"],
  t3_worktree_list:["t3-git::Git::list_branches"],
};
let integrated=`// Generated by export.ts: feature-isolated bindings for concrete sibling services.\nuse crate::{CallContext,DecodedInput,Result,Services,services::*};\nuse async_trait::async_trait;\nuse serde_json::Value;\nuse std::sync::Arc;\n#[derive(Default)]\npub struct ServiceAdapters {\n`;
for (const [name,adapter] of Object.entries(adapters)) integrated+=`#[cfg(feature="${adapter.feature}")] pub ${name}:Option<Arc<super::${adapter.type}>>,\n`;
integrated+=`}\nimpl ServiceAdapters { pub fn install(self:Arc<Self>,services:&mut Services) {\n`;
for (const kit of Object.keys(methods)) integrated+=`services.${fieldName(kit)}=self.clone();\n`;
integrated+=`} }\n`;
for (const [kit,tools] of Object.entries(methods)) {
  integrated+=`#[async_trait]\nimpl ${traitName(kit)} for ServiceAdapters {\n`;
  for (const tool of tools) {
    const route=Object.entries(adapters).find(([_,adapter])=>adapter.tools.includes(tool.name));
    if (!route) continue;
    const [name,adapter]=route;
    integrated+=`async fn ${tool.name}(&self,_context:CallContext,_input:${tool.input})->Result<${tool.output}> {\n#[cfg(feature="${adapter.feature}")] if let Some(adapter)=&self.${name} {return ${traitName(kit)}::${tool.name}(adapter.as_ref(),_context,_input).await;}\nErr(super::not_available("${tool.name}","${serviceOwner[kit]}::${tool.name}"))\n}\n`;
  }
  integrated+=`}\n`;
}
integrated=formatRust(integrated);
const integratedPath=resolve(dest,"../src/adapters/integrated.rs");
if(process.argv.includes("--check")) {
  if(await readFile(integratedPath,"utf8")!==integrated) throw new Error("integrated.rs differs; regenerate");
} else await writeFile(integratedPath,integrated);
const bindings=Object.fromEntries(Object.entries(methods).flatMap(([kit,tools])=>tools.map(tool=> {
  const route=Object.entries(adapters).find(([_,adapter])=>adapter.tools.includes(tool.name));
  const host = ["preview","previewControls","device","html"].includes(kit) || kit === "attachment";
  return [tool.name,{toolkit:kit,feature:route?.[1].feature ?? null,adapter:route ? `adapters::${route[1].type}::${tool.name}` : null,
    expectedServiceMethod:`${serviceOwner[kit]}::${tool.name}`, serviceMethods:nativeMethods[tool.name] ?? [],
    status:["tool_search","send_to_user"].includes(tool.name)?"built-in":route?"adapter":host?"host-trait":"missing-service-method"}];
})));
const access=await server("mcp/McpToolAccess.ts");
function declaration(tree:any):any {
  if(!tree) throw new Error("Unobserved nested access declaration");
  return tree.kind==="either"?{kind:tree.kind,first:declaration(tree.first),second:declaration(tree.second)}:{kind:tree.kind};
}
function accessRule(tree:any,input:any):string {return tree.kind==="either"?accessRule(tree.predicate(input)?tree.first:tree.second,input):tree.kind;}
const accessInputs=[{},...["list","read","respond","status","edit","rename","fork"].map(action=>({action})),{modelSelection:{}},{scheduledTaskId:"existing"}];
for(const [name,binding] of Object.entries(bindings)) {
  const tree=access.__accessDeclarations.get(name);
  binding.accessDeclaration=declaration(tree);
  binding.accessCases=accessInputs.map(input=>({input,rule:accessRule(tree,input)}));
}
const bindingsPath=resolve(dest,"bindings.json");
const bindingsText=JSON.stringify(bindings,null,2)+"\n";
if(process.argv.includes("--check")) {if(await readFile(bindingsPath,"utf8")!==bindingsText) throw new Error("bindings.json differs; regenerate");}
else await writeFile(bindingsPath,bindingsText);
for(const relative of ["rust/crates/t3-mcp/export.ts","apps/server/src/provider/agentContextBudget.json"]) sourcePaths.add(resolve(root,relative));
const hashes:Record<string,string>={};
const relativeSources=[...sourcePaths].map(path=>path.slice(root.length).replaceAll("\\","/")).sort();
for(const relative of relativeSources) hashes[relative]=createHash("sha256").update((await readFile(resolve(root,relative),"utf8")).replaceAll("\r\n","\n")).digest("hex");
for(const relative of ["tools.json","decoders.json","bindings.json","instructions.json"]) hashes[`rust/crates/t3-mcp/fixtures/${relative}`]=createHash("sha256").update((await readFile(resolve(dest,relative),"utf8")).replaceAll("\r\n","\n")).digest("hex");
const manifest=JSON.stringify(hashes,null,2)+"\n";
const manifestPath=resolve(dest,"source-hashes.json");
if(process.argv.includes("--check")) {if(await readFile(manifestPath,"utf8")!==manifest) throw new Error("Source hashes differ; regenerate MCP fixtures");}
else await writeFile(manifestPath,manifest);
