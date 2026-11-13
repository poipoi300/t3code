import { spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { connect } from "node:net";
const entry = __ENTRY__;
const bound = __BOUND__;
const refused = new Map([["cdp", ["--platform", "--udid", "--config", "--session"]], ["connection", ["--platform", "--udid"]]]);
const managed = ["connect", "disconnect", "auth", "daemon"];
const args = process.argv.slice(2);
const end = args.indexOf("--");
const options = end < 0 ? args : args.slice(0, end);
const valued = new Set(["--activity", "--all", "--android-device-allowlist", "--app", "--appium-version", "--artifact", "--artifacts-dir", "--aws-app-arn", "--aws-device-arn", "--aws-interaction-mode", "--aws-project-arn", "--aws-region", "-b", "--baseline", "--bearer-token", "--bundle-url", "--button", "--config", "--count", "--crop-on", "--custom-network", "-d", "--daemon-auth-token", "--daemon-base-url", "--daemon-server-mode", "--daemon-transport", "--delay-ms", "--depth", "--device", "--device-orientation", "--dsym", "--duration-ms", "--fps", "--from", "--geo-location", "--github-actions-artifact", "--header", "--hold-ms", "--host", "--image-tag", "--in-app", "--include", "--interval-ms", "--ios-simulator-device-set", "--ios-xctest-derived-data-path", "--ios-xctest-env-dir", "--ios-xctestrun-file", "--jitter-px", "--keyframes", "--kind", "--language", "--launch-args", "--launch-console", "--launch-url", "--lease-backend", "--lease-id", "--level", "--listen-host", "--locale", "--max-passes", "--max-steps", "--metro-host", "--metro-kind", "--metro-port", "--network-profile", "--on-error", "--os", "--os-version", "--out", "--pattern", "--pause-ms", "--pixel-density", "--pixels", "--plan-digest", "--platform", "--pointer-count", "--port", "--probe-timeout-ms", "--project-root", "--provider", "--provider-app", "--provider-appium-version", "--provider-build", "--provider-custom-network", "--provider-device-orientation", "--provider-device-type", "--provider-geo-location", "--provider-language", "--provider-locale", "--provider-network-hosts", "--provider-network-profile", "--provider-os-version", "--provider-project", "--provider-session", "--provider-session-name", "--provider-timezone", "--proxy-base-url", "--public-base-url", "--quality", "--record-as", "--remote-config", "--reporter", "--report-junit", "--retention-ms", "--retries", "--run-id", "--runtime-file", "-s", "--scale", "--scope", "--search-path", "--serial", "--session", "--session-isolation", "--session-lock", "--settle-quiet", "--shard-all", "--shard-split", "--startup-timeout-ms", "--state-dir", "--status-host", "--steps", "--steps-file", "--surface", "--system", "--target", "--target-app", "--template", "--tenant", "--threshold", "--timeout", "--timezone", "--udid", "--until", "--wait"]);
let name = "";
for (let index = 0; index < args.length; index++) {
  const arg = args[index];
  if (arg === "--") { name = args[index + 1] ?? ""; break; }
  if (!arg.startsWith("-")) { name = arg; break; }
  if (!arg.includes("=") && valued.has(arg)) index++;
}
const informational = args.length === 0 || ["help", "--help", "-h", "--version", "version"].includes(args[0]) || options.includes("--help") || options.includes("-h");
if (!informational && managed.includes(name)) {
  console.error("T3 Code manages this command's connection to the device, so `" + name + "` is not available through it.");
  process.exit(1);
}
if (!informational && name === "device") {
  console.error("`device status` reads this machine's device claims, not those on the device's host, so it is not available through this command. Quote the DEVICE_IN_USE error instead.");
  process.exit(1);
}
const skip = refused.get(name) ?? [];
if (!informational) for (let index = 0; index + 1 < bound.length; index += 2) if (!skip.includes(bound[index]) && !options.some(arg => arg === bound[index] || arg.startsWith(bound[index] + "="))) options.push(bound[index], bound[index + 1]);
const command = end < 0 ? options : [...options, ...args.slice(end)];
const hasValue = flag => { const index = options.indexOf(flag); return options.some(arg => arg.startsWith(flag + "=") && arg.length > flag.length + 1) || (index >= 0 && !!options[index + 1] && !options[index + 1].startsWith("--")); };
if (!informational && !skip.includes("--session") && !(hasValue("--config") && hasValue("--session"))) {
  console.error("Call device_open first and run the agent-device command it returns.");
  process.exit(1);
}
if (!existsSync(entry)) {
  console.error("This agent-device command is out of date because T3 Code moved or updated its runtime. Call device_open again for a fresh one.");
  process.exit(1);
}
const valueOf = flag => { const inline = options.find(arg => arg.startsWith(flag + "=")); if (inline) return inline.slice(flag.length + 1); const index = options.indexOf(flag); return index >= 0 ? options[index + 1] : undefined; };
const endpoint = () => { try { const url = new URL(valueOf("--daemon-base-url") ?? JSON.parse(readFileSync(valueOf("--config"), "utf8")).daemonBaseUrl); return ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname) && url.port ? url : undefined; } catch { return undefined; } };
const local = informational || refused.has(name) ? undefined : endpoint();
if (local) {
  const failure = await new Promise(resolve => {
    const socket = connect({ host: local.hostname.replace(/^\[|\]$/g, ""), port: Number(local.port) });
    socket.setTimeout(3000, () => { socket.destroy(); resolve("ETIMEDOUT"); });
    socket.once("connect", () => { socket.destroy(); resolve(undefined); });
    socket.once("error", error => resolve(error.code ?? "ERROR"));
  });
  if (failure === "EPERM" || failure === "EACCES") {
    console.error("This shell cannot reach T3 Code's device connection at " + local.host + " (connect " + failure + "): a network sandbox is blocking localhost. Run this command again with network access, outside the sandbox.");
    process.exit(1);
  }
  if (failure) {
    console.error("T3 Code's device connection at " + local.host + " is not reachable from this shell (connect " + failure + "). Call device_open again to reconnect; if that succeeds and this repeats, this shell runs in a network sandbox, so run the command with network access, outside the sandbox.");
    process.exit(1);
  }
}
const env = { ...process.env, ELECTRON_RUN_AS_NODE: "1" };
delete env.AGENT_DEVICE_DAEMON_BASE_URL;
delete env.AGENT_DEVICE_DAEMON_AUTH_TOKEN;
delete env.AGENT_DEVICE_CONFIG;
const child = spawn(__NODE__, [entry, ...command], { stdio: "inherit", env });
child.on("error", error => { console.error(error.message); process.exitCode = 1; });
child.on("exit", code => { process.exitCode = code ?? 1; });