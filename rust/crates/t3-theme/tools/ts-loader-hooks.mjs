// Node module-resolution hooks that let the TypeScript sources under apps/web/src
// run unbundled: extensionless relative imports resolve to .ts/.tsx, and the two
// modules that need the app runtime (atom registry, server state) are stubbed.
import { existsSync, statSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const STUBS = {
  "/rpc/atomRegistry": "export const appAtomRegistry = { get: () => [] }; export const subscribeAtomChanges = () => () => {};",
  "/state/server": "export const themeServerEnvironmentThemesAtom = {};",
};

export async function resolve(specifier, context, nextResolve) {
  if (specifier.startsWith(".") && context.parentURL?.startsWith("file:")) {
    for (const [suffix, source] of Object.entries(STUBS)) {
      if (specifier.endsWith(suffix)) {
        return { url: `data:text/javascript,${encodeURIComponent(source)}`, shortCircuit: true };
      }
    }
    const base = join(dirname(fileURLToPath(context.parentURL)), specifier);
    for (const candidate of [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts")]) {
      if (existsSync(candidate) && statSync(candidate).isFile()) {
        return { url: pathToFileURL(candidate).href, shortCircuit: true };
      }
    }
  }
  return nextResolve(specifier, context);
}
