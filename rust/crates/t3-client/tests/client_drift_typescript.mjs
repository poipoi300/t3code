import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
import vm from "node:vm";
import { connectDeviceApprovalPrompts, createApprovalPromptQueue, describeConnectDevice } from "../../../../apps/mobile/src/features/connection/connectDeviceApproval.ts";
import { connectDeviceVerificationCode } from "../../../../packages/contracts/src/auth.ts";
import { isWebUrl } from "../../../../apps/web/src/browser/webUrl.ts";

const settings = readFileSync("apps/web/src/components/settings/ConnectionsSettings.logic.ts", "utf8");
const context = vm.createContext({});
vm.runInContext(stripTypeScriptTypes(settings.slice(settings.indexOf("export function connectDeviceName")).replaceAll("export function", "function")), context);
const clients = [
  undefined,
  { deviceType: "desktop" },
  { deviceType: "desktop", label: "Chrome", os: "macOS", browser: "Chrome", ipAddress: "127.0.0.1" },
  { deviceType: "desktop", os: "macOS", browser: "Chrome" },
  { deviceType: "mobile", browser: "Safari" },
  { deviceType: "mobile", os: "Android" },
];
const thumbprints = ["", "key", "ABCDEFGH", "abcdefgh", "01234567", "-_??😀éA", "AAAAAAA😀", "abcdefghijklmnopqrstuvwxyz0123456789"];
const urls = ["https://example.com", "http://localhost:3000", " HTTPS://example.com ", "https:example.com", "\u0001https://example.com\u001f", "ht\ttps://example.com", "\u00a0https://example.com", "\ufeffhttps://example.com", "javascript:alert(1)", "file:///private", "//example.com", "bad"];
const link = (id, ms, request) => ({ id, scopes: [], subject: "cloud-connect", createdAt: { epochMilliseconds: ms }, expiresAt: { epochMilliseconds: ms + 600_000 }, ...(request && { connectDeviceRequest: request }) });
const pairingLinks = [link("newer", 2000, { verificationCode: "WXYZ-2345" }), link("plain", 500), link("older", 1000, { verificationCode: "ABCD-EFGH", client: clients[3] })];
const queues = [
  [["offer", [{ pairingLinkId: "a", host: "mini" }, { pairingLinkId: "b", host: "mini" }]], ["offer", [{ pairingLinkId: "a", host: "mini" }]], ["answer", "retry"], ["answer", "done"]],
  [["offer", [{ pairingLinkId: "a", host: "mini" }]], ["answer", "later"], ["offer", []]],
  [["offer", [{ pairingLinkId: "a", host: "mini" }]], ["drop", "mini"], ["answer", "retry"], ["offer", [{ pairingLinkId: "a", host: "mini" }]]],
  [["offer", [{ pairingLinkId: "a", host: "mini" }, { pairingLinkId: "b", host: "mini" }, { pairingLinkId: "c", host: "laptop" }]], ["drop", "mini"], ["answer", "done"]],
];
const runQueue = (commands) => {
  const shown = [];
  let answer;
  const queue = createApprovalPromptQueue((prompt, respond) => { shown.push(prompt.pairingLinkId); answer = respond; });
  return commands.map(([operation, value]) => {
    if (operation === "offer") queue.offer(value);
    if (operation === "drop") queue.drop((prompt) => prompt.host === value);
    if (operation === "answer") answer(value);
    return { shown: [...shown], known: [...queue.known].sort() };
  });
};
console.log(JSON.stringify({
  clients: clients.map((client) => ({ client: client ?? null, short: context.connectDeviceName({ client }), detailed: context.describeConnectDeviceClient({ client }), mobile: describeConnectDevice(client) })),
  codes: thumbprints.map((input) => ({ input, output: connectDeviceVerificationCode(input) })),
  urls: urls.map((input) => ({ input, output: isWebUrl(input) })),
  prompts: connectDeviceApprovalPrompts({ environmentLabel: "Mac mini", pairingLinks, shownIds: new Set(["newer"]) }),
  queues: queues.map((commands) => ({ commands, outputs: runQueue(commands) })),
}));
