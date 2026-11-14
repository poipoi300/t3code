import fs from "node:fs/promises";
import {stripTypeScriptTypes} from "node:module";

const root = new URL("../../../../", import.meta.url);
const source = name => new URL(`apps/server/src/preview/${name}.ts`, root);
const output = name => new URL(`../driver/${name}.mjs`, import.meta.url);
const imports = /^import\s[\s\S]*?;\r?\n/gm;
const strip = source => stripTypeScriptTypes(source).replace(/[ \t]+(?=\r?$)/gm, "");
let page = (await fs.readFile(source("ServerBrowserPage"), "utf8")).replace(imports, "");
page = page.replace("Schema.is(BrowserControlInterrupted)", 'value => value?.name === "BrowserControlInterrupted"');
page = page.replace(/export class ServerBrowserOperationError extends Schema.TaggedError[\s\S]*?\) \{\}/, "export class ServerBrowserOperationError extends Error { constructor(input) { super(input.message); Object.assign(this, input); } }");
page = page.replace("Schema.is(ServerBrowserOperationError)", "value => value instanceof ServerBrowserOperationError");
if (page.includes("Schema.")) throw new Error("Unconverted Effect schema");
page = 'import * as NodeCrypto from "node:crypto";\nimport * as NodeUtil from "node:util";\nconst constVoid = () => {};\nconst PROVIDER_SEND_TURN_MAX_FILE_BYTES = 50 * 1024 * 1024;\n' + page;
await fs.writeFile(output("page"), strip(page));
let contexts = (await fs.readFile(source("ServerBrowserContexts"), "utf8")).replace(imports, "");
contexts = contexts.replace("NodeModule.createRequire(import.meta.url)", 'NodeModule.createRequire(NodePath.join(process.cwd(), "package.json"))').replace('requirePlaywright("playwright-core")', 'requirePlaywright(process.env.T3_PREVIEW_PLAYWRIGHT_MODULE || "playwright-core")');
contexts = 'import * as NodeFSP from "node:fs/promises";\nimport * as NodeModule from "node:module";\nimport * as NodeOS from "node:os";\nimport * as NodePath from "node:path";\nconst constVoid = () => {};\nconst INCOGNITO_BROWSER_PROFILE_ID = "incognito";\nconst sandboxDisabled = env => env.T3CODE_SERVER_BROWSER_SANDBOX === "0";\n' + contexts;
await fs.writeFile(output("contexts"), strip(contexts));
