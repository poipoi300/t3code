import { Schema, type ContractSchema } from "./effect.ts";

const object = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === "object" && !Array.isArray(v);
function* mutations(sample: unknown): Generator<[string, unknown]> {
  if (typeof sample === "string") {
    yield ["s-empty", ""]; yield ["s-pad", ` ${sample} `]; yield ["s-blank", "  "]; return;
  }
  if (typeof sample === "number") {
    for (const [k, v] of Object.entries({neg: -1, frac: sample + .5, zero: 0, big: 1e21, str: String(sample)})) yield [`n-${k}`, v];
    return;
  }
  if (!object(sample)) return;
  yield ["extra", {...sample, __extra: 1}];
  function* visit(value: Record<string, unknown>, prefix: string[]): Generator<[string, unknown]> {
    for (const [key, v] of Object.entries(value)) {
      const path = [...prefix, key];
      const replace = (remove: boolean, replacement?: unknown) => {
        const copy = structuredClone(sample) as Record<string, unknown>;
        let parent = copy;
        for (const p of prefix) parent = parent[p] as Record<string, unknown>;
        if (remove) delete parent[key]; else parent[key] = replacement;
        return copy;
      };
      yield [`del:${path.join(".")}`, replace(true)];
      if (v !== null) yield [`null:${path.join(".")}`, replace(false, null)];
      const variants = typeof v === "string" ? {empty:"", pad:` ${v} `, junk:"zz-not-a-known-value", num:7}
        : typeof v === "number" ? {neg:-1, frac:v+.5, zero:0, big:1e21, str:String(v)}
        : typeof v === "boolean" ? {flip:!v}
        : Array.isArray(v) ? {"arr-junk":[...v,"zz-not-a-known-value"], "arr-empty":[]} : {};
      for (const [kind, replacement] of Object.entries(variants)) yield [`${kind}:${path.join(".")}`, replace(false, replacement)];
      if (object(v) && prefix.length < 1) yield* visit(v, path);
    }
  }
  yield* visit(sample, []);
}

export function differentialVectors(schemas: Map<string, ContractSchema>, samples: Map<string, {values: unknown[]}>) {
  const rows: string[] = [];
  for (const [name, schema] of [...schemas].sort(([a],[b]) => a < b ? -1 : a > b ? 1 : 0)) {
    if (/^rpc\.(AgentRpcs|ClientRpcs|CompatAgentRpcs|CompatClientRpcs)\./.test(name)) continue;
    const codec = Schema.toCodecJson(schema);
    const decode = Schema.decodeUnknownSync(codec);
    const encode = Schema.encodeUnknownSync(codec);
    const values = [...(samples.get(name)?.values ?? [])].sort((a,b) => JSON.stringify(b).length - JSON.stringify(a).length);
    const seen = new Set<string>();
    for (const sample of values.slice(0,6)) for (const [kind,input] of mutations(sample)) {
      if (seen.has(kind) || seen.size >= 80) continue;
      seen.add(kind);
      let accepted = false, output: unknown = null;
      try { output = JSON.parse(JSON.stringify(encode(decode(structuredClone(input))))); accepted = true; } catch { /* Expected rejection is an oracle result. */ }
      rows.push(JSON.stringify({s:name,k:kind,i:input,t:accepted,o:output}));
    }
  }
  return rows.join("\n") + "\n";
}
