import { Schema, type ContractSchema } from "./effect.ts";

type Codec = import("../../../apps/server/node_modules/effect/dist/Schema.js").Codec<unknown, unknown>;

/**
 * Schemas the TypeScript server persists with the plain codec
 * (`Schema.decodeUnknown*` / `Schema.fromJsonString` without `toCodecJson`).
 * In that dialect an absent `Schema.optional` key must stay absent: `null` is rejected.
 * Everything else (WebSocket RPC, HTTP API, guest HTTP, MCP tool input, client
 * caches) uses `Schema.toCodecJson`, which the main samples already cover.
 */
export const STORED_BOUNDARIES: Readonly<Record<string, string>> = {
  // apps/server/src/persistence/OrchestrationEventStore.ts:127-182
  OrchestrationV2DomainEventJson: "event-store",
  ApplicationEventMetadata: "event-store",
  ApplicationProjectEvent: "event-store",
  // apps/server/src/orchestration-v2/ProjectionStore.ts:1144-1290
  OrchestrationV2AppThreadJson: "projection",
  OrchestrationV2CheckpointJson: "projection",
  OrchestrationV2CheckpointScopeJson: "projection",
  OrchestrationV2ContextHandoffJson: "projection",
  OrchestrationV2ContextTransferJson: "projection",
  OrchestrationV2ConversationMessageJson: "projection",
  OrchestrationV2ExecutionNodeJson: "projection",
  OrchestrationV2PlanArtifact: "projection",
  OrchestrationV2ProviderSessionJson: "projection",
  OrchestrationV2ProviderThreadJson: "projection",
  OrchestrationV2ProviderTurnJson: "projection",
  OrchestrationV2RunAttemptJson: "projection",
  OrchestrationV2RunJson: "projection",
  OrchestrationV2RuntimeRequestJson: "projection",
  OrchestrationV2SubagentJson: "projection",
  OrchestrationV2TurnItemJson: "projection",
  // apps/server/src/persistence/AuthSessions.ts:128, AuthPairingLinks.ts:29
  AuthEnvironmentScopes: "auth-store",
  // apps/server/src/cloud/config.ts:23
  RelayManagedEndpointRuntimeConfig: "cloud-config",
  // apps/server/src/persistence/PullRequestReviewGuides.ts:19
  PullRequestReviewGuide: "review-guide-store",
  // packages/shared/src/schemaJson.ts (settings.json, keybindings.json)
  ServerSettings: "settings-file",
  KeybindingsConfig: "settings-file",
};

export const storedParent = (name: string) => {
  const parent = Object.keys(STORED_BOUNDARIES).find(
    (stored) => name === stored || name.startsWith(`${stored}.`),
  );
  return parent;
};

/** Re-encode RPC-dialect samples into the stored dialect, checked by the real plain decoder. */
export function storedSamples(name: string, schema: ContractSchema, rpcSamples: readonly unknown[]) {
  const json = Schema.toCodecJson(schema) as unknown as Codec;
  const plain = schema as unknown as Codec;
  const decodeRpc = Schema.decodeUnknownSync(json);
  const encodePlain = Schema.encodeUnknownSync(plain);
  const decodePlain = Schema.decodeUnknownSync(plain);
  const values = new Map<string, unknown>();
  const failures: string[] = [];
  for (const sample of rpcSamples) {
    try {
      const stored: unknown = JSON.parse(JSON.stringify(encodePlain(decodeRpc(structuredClone(sample)))));
      decodePlain(structuredClone(stored));
      values.set(JSON.stringify(stored), stored);
    } catch (error) {
      failures.push(`${name}: ${error instanceof Error ? error.message.split("\n")[0] : String(error)}`);
    }
  }
  return { values: [...values.values()], failures };
}
