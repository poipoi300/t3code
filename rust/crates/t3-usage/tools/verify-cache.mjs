import fs from "node:fs/promises";
import assert from "node:assert/strict";
import { decodeScanCache } from "../../../../apps/server/src/usage/usageScanCache.ts";
import { claudeUsageFormat } from "../../../../apps/server/src/provider/Drivers/claudeUsage.ts";
import { codexUsageFormat } from "../../../../apps/server/src/provider/Drivers/codexUsage.ts";
import { grokUsageFormat } from "../../../../packages/provider-grok/src/server/usage.ts";
import { piUsageFormat } from "../../../../packages/provider-pi/src/server/usage.ts";
import { museUsageFormat } from "../../../../packages/provider-muse/src/server/usage.ts";

const formats = new Map(Object.entries({claude:claudeUsageFormat,codex:codexUsageFormat,grok:grokUsageFormat,pi:piUsageFormat,muse:museUsageFormat}));
const {document,expected,kind} = JSON.parse(await fs.readFile(process.argv[2],"utf8"));
if (kind === "cursor") {
    const {decodeCursorAccountCaches}=await import("../../../../packages/provider-cursor/src/server/accountCache.ts");
    assert.deepEqual([...decodeCursorAccountCaches(document)],expected);
} else assert.deepEqual([...decodeScanCache(document,formats)], expected);
