// Regenerate with Node from the TypeScript authority; never touches OS services.
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const source = fs.readFileSync(path.resolve(__dirname, "../../../../apps/server/src/cloud/bootService.ts"), "utf8");
function extract(name) {
  const start = source.indexOf(`function ${name}(`);
  const body = source.indexOf("): string {", start) + "): string ".length;
  const end = source.indexOf("\n}", body) + 2;
  return `function ${name}(...args) { return (function(plan, options, value) ${source.slice(body, end)})(args[0], args[1], args[0]); }`;
}
function functionBody(name, params) {
  const start = source.indexOf(`function ${name}(`);
  const body = source.indexOf("): string {", start) + "): string ".length;
  const end = source.indexOf("\n}", body) + 2;
  return `function ${name}(${params}) ${source.slice(body, end)}`;
}
const code = [
  "const SERVICE_UNIT_ENV = 'T3_BOOT_SERVICE_UNIT'; const BOOT_SERVICE_UNIT_FILE = 't3code.service'; const BOOT_SERVICE_PLIST_FILE = 'com.t3tools.t3code.service.plist'; const BOOT_SERVICE_LAUNCHD_LABEL = 'com.t3tools.t3code.service'; const BOOT_SERVICE_TASK_FILE = 't3code-task.xml';",
  "class BootServiceBaseDirUnsupportedError extends Error {}",
  source.match(/const systemdUnsafeValue = \(plan: BootServicePlan\) =>\s*([^;]+);/)[0].replace(": BootServicePlan", ""),
  source.match(/const quoteWindowsArgument = [^\n]+/)[0].replace(": string", ""),
  functionBody("escapeSystemdSpecifiers", "value"), functionBody("quoteSystemdValue", "value"), functionBody("escapeXmlText", "value"),
  functionBody("renderBootServiceUnit", "plan"), functionBody("renderBootServicePlist", "plan, options"), functionBody("renderBootServiceTask", "plan, options"),
  "globalThis.render = {renderBootServiceUnit, renderBootServicePlist, renderBootServiceTask};",
].join("\n");
const context = vm.createContext({}); vm.runInContext(code, context);
const plan = {program:["/home/test/T3 Code/runtime/versions/1.2.3/t3", "__service-launcher"], baseDir:"/home/test/T3 Code%name", logPath:"/home/test/T3 Code/logs/boot-service.log", installBinDir:"/home/test/.local/bin"};
const windows = {program:["C:\\T3 Code\\runtime\\versions\\1.2.3\\t3.exe", "__service-launcher"], baseDir:"C:\\T3 & Code\\", logPath:"C:\\T3 Code\\logs\\boot-service.log"};
const fixture = {
  plan, windows,
  systemd:context.render.renderBootServiceUnit(plan),
  launchd:context.render.renderBootServicePlist(plan,{homeDir:"/home/test",environmentPath:"/usr/bin:/x&y"}),
  task:context.render.renderBootServiceTask(windows,{userId:"DOMAIN\\user&other",homeDir:"C:\\Users\\test",conhostPath:"C:\\Windows\\System32\\conhost.exe"}),
};
fs.writeFileSync(path.join(__dirname,"service-definitions.json"),JSON.stringify(fixture,null,2)+"\n");
