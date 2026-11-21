import { api } from "./api";

export interface McpConfig {
  id: string;
  name: string;
  description?: string | null;
  transport: "stdio" | "streamable_http";
  command: string | null;
  args: string[];
  url: string | null;
  env_secret_refs: Record<string, string>;
  bearer_secret_ref: string | null;
  credential_url?: string | null;
  enabled: boolean;
  builtin: boolean;
  plugin_slug?: string | null;
}

/** Called only by an explicit Start action, which authorizes the selected config. */
export async function startMcpConnection(config: McpConfig) {
  const expected = config.enabled ? config : { ...config, enabled: true };
  if (!config.enabled && !config.plugin_slug) await api("mcp_update", { config: expected });
  await api("mcp_connect", { expectedConfig: expected, acceptsUnsandboxedStdio: config.transport === "stdio" });
}
