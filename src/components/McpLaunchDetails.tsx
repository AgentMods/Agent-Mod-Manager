import { PathText } from "./ui";
import type { McpConfig } from "@/lib/mcp-connection";

export function McpLaunchDetails({ config }: { config: McpConfig }) {
  return <>
    <PathText className="rounded-md border bg-muted p-3">{[config.command, ...config.args].join(" ")}</PathText>
    {Object.keys(config.env_secret_refs).length > 0 && <p className="text-xs text-muted-foreground">环境变量：{Object.keys(config.env_secret_refs).join("、")}</p>}

  </>;
}
