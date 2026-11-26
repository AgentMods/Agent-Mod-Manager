import { ShieldAlertIcon } from "lucide-react";
import { replaceEqualDeep, useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { api, backendAvailable, errorText } from "@/lib/api";
import { tr } from "@/lib/i18n";
import { refreshMcpQueries, type McpConfig } from "@/lib/mcp-connection";
import { useUI } from "@/lib/store";
import type { Plugin } from "@/lib/types";
import { ErrorState, Modal } from "@/components/ui";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";

const pendingKey = ["mcp", "pending-game-tools"] as const;
const decisionKey = ["mcp", "decide-game-tool"] as const;
type Decision = { action: "approve" | "decline"; config: McpConfig };

function sameConfig(item: McpConfig, expected: McpConfig) {
  return item.id === expected.id && replaceEqualDeep(expected, item) === expected;
}

function ConfigurationValue({ label, value }: { label: string; value: string | string[] | Record<string, string> | null }) {
  // Preserve argument boundaries and make invisible direction changes visible during review.
  const literal = JSON.stringify(value, null, 2).replace(/\p{Bidi_Control}/gu, (char) =>
    `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`);
  return <div className="min-w-0 space-y-1">
    <dt className="text-sm font-medium">{label}</dt>
    <dd><pre dir="ltr" className="whitespace-pre-wrap wrap-anywhere rounded-md bg-muted px-3 py-2 text-start font-mono text-xs">{literal}</pre></dd>
  </div>;
}

export function McpGameToolApproval() {
  const client = useQueryClient();
  const notify = useUI((state) => state.notify);
  const pending = useQuery({
    queryKey: pendingKey,
    queryFn: () => api<McpConfig[]>("mcp_pending_game_tools"),
    enabled: backendAvailable,
    networkMode: "always",
  });
  const request = pending.data?.[0];
  const [reviewed, setReviewed] = useState<McpConfig | null>(null);
  const reviewButton = useRef<HTMLButtonElement>(null);
  const packages = useQuery({
    queryKey: ["plugins"],
    queryFn: () => api<Plugin[]>("list_plugins"),
    enabled: backendAvailable && !!request?.plugin_slug,
    networkMode: "always",
  });
  const packageName = (config: McpConfig) => packages.data?.find(({ slug }) => slug === config.plugin_slug)?.name ?? config.plugin_slug ?? config.name;
  const busy = useIsMutating({ mutationKey: decisionKey }) > 0;
  const decision = useMutation({
    mutationKey: decisionKey,
    networkMode: "always",
    mutationFn: ({ action, config }: Decision) => api<void>(
      action === "approve" ? "mcp_approve_game_tool" : "mcp_decline_game_tool",
      { expectedConfig: config },
    ),
    onSuccess: async (_, { config }) => {
      await client.cancelQueries({ queryKey: pendingKey });
      client.setQueryData<McpConfig[]>(pendingKey, (current) => current?.filter((item) => !sameConfig(item, config)));
    },
    onSettled: async (_, error, { config }) => {
      await refreshMcpQueries(client);
      if (error && !client.getQueryData<McpConfig[]>(pendingKey)?.some((item) => sameConfig(item, config))) notify(errorText(error));
    },
  });

  // Reordering the queue keeps a review open. Replacing its configuration requires a new review.
  const current = reviewed && pending.data?.some((item) => sameConfig(item, reviewed));
  if (reviewed && !busy && !current) setReviewed(null);
  const config = reviewed && (busy || current) ? reviewed : null;
  function respond(action: Decision["action"], config: McpConfig) {
    if (client.isMutating({ mutationKey: decisionKey }) || pending.isError ||
        !client.getQueryData<McpConfig[]>(pendingKey)?.some((item) => sameConfig(item, config))) return;
    decision.mutate({ action, config }, { onSuccess: () => setReviewed(null) });
  }

  if (!backendAvailable || (!request && !pending.isError && !config)) return null;
  const readError = pending.isError && tr("无法读取待授权的适配包工具：{value1}", { value1: errorText(pending.error) });
  const retry = <Button size="sm" variant="outline" disabled={pending.isFetching} onClick={() => { void pending.refetch(); }}>{tr("重试")}</Button>;

  return <>
    {(request || readError) && <Alert variant="warning" role="status" className="shrink-0 rounded-none border-x-0 border-t-0 px-4 py-2">
      <ShieldAlertIcon aria-hidden="true" />
      <AlertDescription className="flex min-w-0 flex-wrap items-center justify-between gap-x-3 gap-y-1.5">
        <span className="min-w-0 flex-1 wrap-anywhere">{readError || (request && tr("适配包「{name}」有工具待授权。", { name: packageName(request) }))}</span>
        {readError ? retry : request && <Button ref={reviewButton} size="sm" variant="outline" disabled={busy} onClick={() => {
          decision.reset();
          setReviewed(request);
        }}>{tr("查看授权请求")}</Button>}
      </AlertDescription>
    </Alert>}
    {config && <Modal title={tr("适配包工具授权")} description={tr("允许后将自动启用并尝试启动此工具。拒绝后，此配置将不再提示。")}
      className="sm:max-w-xl" onClose={() => { if (!busy) setReviewed(null); }}
      onCloseAutoFocus={(event) => {
        if (reviewButton.current) { event.preventDefault(); reviewButton.current.focus(); }
      }}
      actions={<>
        <Button variant="outline" disabled={busy} onClick={() => { setReviewed(null); }}>{tr("返回")}</Button>
        <Button variant="outline" disabled={busy || pending.isError} onClick={() => respond("decline", config)}>
          {busy && decision.variables?.action === "decline" && <Spinner />}{tr("拒绝")}
        </Button>
        <Button disabled={busy || pending.isError} onClick={() => respond("approve", config)}>
          {busy && decision.variables?.action === "approve" && <Spinner />}{tr("允许并启动")}
        </Button>
      </>}>
      <div className="grid gap-1 wrap-anywhere text-sm">
        <p><span className="font-medium">{tr("游戏适配包")}</span> · <bdi>{packageName(config)}</bdi>
          {config.plugin_slug && packageName(config) !== config.plugin_slug && <span className="text-muted-foreground"> (<bdi>{config.plugin_slug}</bdi>)</span>}</p>
        <p><span className="font-medium">MCP</span> · <bdi>{config.name}</bdi></p>
      </div>
      {config.transport === "stdio" && <Alert variant="warning">
        <ShieldAlertIcon aria-hidden="true" />
        <AlertDescription>{tr("此 stdio 进程将在沙箱外以当前系统用户的权限运行。")}</AlertDescription>
      </Alert>}
      <dl className="grid gap-3">
        {config.transport === "stdio" ? <>
          <ConfigurationValue label={tr("命令")} value={config.command} />
          <ConfigurationValue label={tr("启动参数")} value={config.args} />
        </> : <ConfigurationValue label="URL" value={config.url} />}
        {Object.keys(config.env_secret_refs).length > 0 && <ConfigurationValue label={tr("环境变量与密钥引用 JSON")} value={config.env_secret_refs} />}
        {config.bearer_secret_ref && <ConfigurationValue label={tr("Bearer 密钥引用")} value={config.bearer_secret_ref} />}
        {config.credential_url && <ConfigurationValue label={tr("凭据管理网址")} value={config.credential_url} />}
        {!Object.keys(config.env_secret_refs).length && !config.bearer_secret_ref &&
          <div className="text-sm"><dt className="font-medium">{tr("凭据绑定")}</dt><dd className="text-muted-foreground">{tr("无")}</dd></div>}
      </dl>
      {readError ? <div className="space-y-2"><ErrorState error={readError} />{retry}</div> :
        <ErrorState error={decision.error} />}
    </Modal>}
  </>;
}
