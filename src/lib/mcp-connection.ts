import { api } from "./api";

export interface McpConfig {
  id: string;
  name: string;
  transport: "stdio" | "streamable_http";
  command: string | null;
  args: string[];
  url: string | null;
  env_secret_refs: Record<string, string>;
  bearer_secret_ref: string | null;
  enabled: boolean;
  builtin: boolean;
}

export async function startMcpConnection(config: McpConfig, acceptsUnsandboxedStdio: boolean) {
  const expected = config.enabled ? config : { ...config, enabled: true };
  if (!config.enabled) await api("mcp_update", { config: expected });
  await api("mcp_connect", { expectedConfig: expected, acceptsUnsandboxedStdio });
}
