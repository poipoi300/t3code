import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const shellRoot = fileURLToPath(new URL(".", import.meta.url));
const uiManifest = join(shellRoot, "../crates/t3-ui/Cargo.toml");
const desktopManifest = join(shellRoot, "desktop/Cargo.toml");
const target = resolve(
  process.env.CARGO_TARGET_DIR || process.env.T3_SHELLS_TARGET || join(shellRoot, "target"),
);
const env: NodeJS.ProcessEnv = {
  ...process.env,
  CARGO_TARGET_DIR: target,
  RUST_MIN_STACK: process.env.RUST_MIN_STACK ?? "268435456",
};
if (env.CARGO_HOME) env.CARGO_HOME = resolve(env.CARGO_HOME);

function cargo(args: string[], capture = false): string {
  const result = spawnSync("cargo", args, {
    cwd: shellRoot,
    env,
    encoding: "utf8",
    stdio: ["inherit", capture ? "pipe" : "inherit", "inherit"],
  });
  if (result.error) throw result.error;
  if (result.signal) throw new Error(`cargo ${args[0]} terminated by ${result.signal}`);
  if (result.status !== 0) process.exit(result.status ?? 1);
  return result.stdout ?? "";
}

type Metadata = {
  packages: { name: string; dependencies: { name: string; source: string | null }[] }[];
};

function nshPin(): { url: string; revision: string } {
  const metadata: Metadata = JSON.parse(
    cargo(["metadata", "--manifest-path", uiManifest, "--no-deps", "--format-version", "1"], true),
  );
  const dependencies = metadata.packages.find((pkg) => pkg.name === "t3-ui")?.dependencies ?? [];
  const sources = new Set(
    dependencies.filter((dep) => dep.name.startsWith("nsh-")).map((dep) => dep.source),
  );
  const [source] = sources;
  const [, url, revision] = source?.match(/^git\+(.+)\?rev=([a-f0-9]{40})$/) ?? [];
  if (sources.size !== 1 || !url || !revision) {
    throw new Error(
      "All NSH dependencies must use the same exact Git revision in t3-ui/Cargo.toml",
    );
  }
  return { url, revision };
}

function main(): void {
  const [action, ...extra] = process.argv.slice(2);
  if (extra.length || (action !== "generate" && action !== "install-cli" && action !== "check")) {
    console.error("Usage: node rust/shells/ui.ts <generate|install-cli|check>");
    process.exitCode = 2;
    return;
  }

  const { url, revision } = nshPin();
  for (const manifest of [desktopManifest, join(shellRoot, "gpui/Cargo.toml")]) {
    const template = readFileSync(`${manifest}.in`, "utf8");
    writeFileSync(
      manifest,
      template.replaceAll("@NSH_GIT@", url).replaceAll("@NSH_REV@", revision),
    );
  }

  if (action === "install-cli") {
    const root = env.CARGO_HOME || join(target, "tools");
    env.CARGO_TARGET_DIR = join(target, "cli-build");
    cargo([
      "install",
      "--git",
      url,
      "--rev",
      revision,
      "--locked",
      "--debug",
      "--root",
      root,
      "nsh-cli",
    ]);
  }

  if (action === "check") {
    // GPUI's ICU dependencies currently do not resolve with t3-i18n's.
    for (const manifest of [uiManifest, desktopManifest]) {
      cargo(["fmt", "--manifest-path", manifest, "--check"]);
      cargo([
        "clippy",
        "--locked",
        "--manifest-path",
        manifest,
        "--all-targets",
        "--",
        "-D",
        "warnings",
      ]);
    }
    env.CC_wasm32_unknown_unknown ??= "clang";
    env.AR_wasm32_unknown_unknown ??= "llvm-ar";
    cargo([
      "check",
      "--locked",
      "--manifest-path",
      uiManifest,
      "--target",
      "wasm32-unknown-unknown",
      "--lib",
    ]);
  }
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
}
