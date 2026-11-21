import { startMcpConnection, type McpConfig } from "@/lib/mcp-connection";
import { Fragment, useEffect, useId, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useMutation, useMutationState, useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronDownIcon, ExternalLinkIcon, KeyRoundIcon, PencilIcon, PlayIcon, PlusIcon, SaveIcon, ServerIcon, SquareIcon, Trash2Icon } from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import { Badge, EmptyState, ErrorState, IconButton, Modal, PathText } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Item, ItemActions, ItemContent, ItemGroup, ItemMedia, ItemSeparator, ItemTitle } from "@/components/ui/item";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

type LocalStatus = { startupRequested: boolean; running: boolean; stopping: boolean; endpoint: string | null; tokenRef: string; activeCalls: number };
type Connection = { serverId: string; tools: string[] };

type ConnectionAction = "enable" | "disable";
type ConnectionChange = { action: ConnectionAction; targets: McpConfig[]; singleSuccessMessage?: string };
const connectionChangeKey = ["mcp", "change-connections"] as const;

function isCredentialUrl(value: string) {
  const authority = /^https?:\/\/([^/?#]+)/i.exec(value)?.[1];
  if (!authority || authority.includes("@") || /[\s\\]/.test(value)) return false;
  try {
    const url = new URL(value);
    return Boolean(url.hostname) && !url.username && !url.password;
  } catch {
    return false;
  }
}

function isThreeDmHttpEndpoint(config: McpConfig) {
  if (config.transport !== "streamable_http" || !config.url) return false;
  try {
    const endpoint = new URL(config.url).href;
    const threeDmMcp = "https://mod.3dmgame.com/mcp";
    return endpoint === threeDmMcp || endpoint === threeDmMcp + "/";
  } catch {
    return false;
  }
}

function mcpCredentialFields(config: McpConfig) {
  const bindings = Object.entries(config.env_secret_refs);
  if (config.bearer_secret_ref) bindings.unshift([isThreeDmHttpEndpoint(config) ? "3DM API Key" : "Bearer 密钥", config.bearer_secret_ref]);
  const labelsByReference = new Map<string, string[]>();
  for (const [label, reference] of bindings) {
    if (!reference) continue;
    const labels = labelsByReference.get(reference);
    if (labels) labels.push(label);
    else labelsByReference.set(reference, [label]);
  }
  return [...labelsByReference].map(([reference, labels]) => ({ reference, label: labels.join(" / ") }));
}

function McpCredentialDialog({ config, trigger, onClose }: { config: McpConfig; trigger: HTMLButtonElement; onClose: () => void }) {
  const formId = useId();
  const fields = mcpCredentialFields(config);
  const [values, setValues] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  const pending = fields.filter(({ reference }) => values[reference]?.trim());
  const credentialUrl = config.credential_url;

  async function save() {
    if (saving || !pending.length) return;
    setSaving(true);
    setError(null);
    for (const { reference, label } of pending) {
      try {
        await api("set_secret", { key: reference, value: values[reference] });
        setValues((current) => ({ ...current, [reference]: "" }));
      } catch (failure) {
        setError(`${label} 保存失败：${errorText(failure)}`);
        setSaving(false);
        return;
      }
    }
    notify("MCP 凭据已保存");
    onClose();
  }

  return <Modal title="配置 MCP 凭据" description={config.name} className="sm:max-w-md [overflow-wrap:anywhere]"
    onClose={() => { if (!saving) onClose(); }}
    onCloseAutoFocus={(event) => { event.preventDefault(); trigger.focus(); }}
    actions={<div className="flex justify-end gap-2">
      <Button type="button" variant="outline" disabled={saving} onClick={onClose}>返回</Button>
      <Button type="submit" form={formId} disabled={!backendAvailable || saving || !pending.length}>
        {saving && <Spinner data-icon="inline-start" />}保存
      </Button>
    </div>}>
    <p className="text-sm text-muted-foreground">凭据仅在本机加密保存。输入新值可替换已保存的凭据。</p>
    <form id={formId} onSubmit={(event) => { event.preventDefault(); void save(); }}>
      <FieldGroup className="gap-4">
        {fields.map(({ reference, label }, index) => <Field key={reference}>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <FieldLabel htmlFor={`secret-${reference}`}>{label}</FieldLabel>
            {index === 0 && credentialUrl && isCredentialUrl(credentialUrl) && <Button asChild variant="link" size="xs">
              <a href={credentialUrl} target="_blank" rel="noreferrer" onClick={(event) => {
                if (!backendAvailable) return;
                event.preventDefault();
                setError(null);
                void openUrl(credentialUrl).catch(setError);
              }}>获取 Key <ExternalLinkIcon aria-hidden="true" /></a>
            </Button>}
          </div>
          <Input id={`secret-${reference}`} type="password" autoComplete="new-password" placeholder={reference}
            value={values[reference] ?? ""} disabled={!backendAvailable || saving}
            onChange={(event) => setValues({ ...values, [reference]: event.target.value })} />
        </Field>)}
      </FieldGroup>
    </form>
    <ErrorState error={error} />
  </Modal>;
}

export function McpTools({ onboarding = false, onBusyChange }: { onboarding?: boolean; onBusyChange?: (busy: boolean) => void }) {
  const configs = useBackendQuery<McpConfig[]>(["mcp", "configs"], "mcp_list");
  const externalConfigs = configs.data?.filter((config) => config.id !== "amm_local") ?? [];
  const startup = useBackendQuery<string[]>(["mcp", "startup"], "mcp_startup_requests");
  const connections = useBackendQuery<Connection[]>(["mcp", "connections"], "mcp_connections");
  const local = useBackendQuery<LocalStatus>(["mcp", "local"], "mcp_server_status", undefined, !onboarding);
  const [edit, setEdit] = useState<McpConfig | null>(null);
  const [fresh, setFresh] = useState(false);
  const [credentials, setCredentials] = useState<{
    config: McpConfig;
    trigger: HTMLButtonElement;
  } | null>(null);
  const [confirmLocalStart, setConfirmLocalStart] = useState(false);
  const [localBusy, setLocalBusy] = useState(false);
  const [failures, setFailures] = useState<Record<string, { action: ConnectionAction; message: string }>>({});
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  const client = useQueryClient();
  useEffect(() => {
    if (!backendAvailable) return;
    let mounted = true;
    let unlisten: (() => void) | undefined;
    const sync = async () => {
      if (!mounted) return;
      // A first fetch may still reflect pre-restore state when an event arrives.
      await client.cancelQueries({ queryKey: ["mcp"] });
      if (mounted) await client.invalidateQueries({ queryKey: ["mcp"] });
    };
    void listen("mcp://restore-progress", () => { void sync(); }).then((stop) => {
      if (!mounted) { stop(); return; }
      unlisten = stop;
      void sync(); // Catch restores that finished before subscription.
    }).catch((cause) => { if (mounted) setError(cause); });
    return () => { mounted = false; unlisten?.(); };
  }, [client]);
  const forget = (id: string) => setFailures(({ [id]: _removed, ...rest }) => rest);
  const refresh = () => client.invalidateQueries({ queryKey: ["mcp"] });
  const change = useMutation({
    mutationKey: connectionChangeKey,
    networkMode: "always",
    mutationFn: async ({ action, targets, singleSuccessMessage }: ConnectionChange) => {
      setError(null);
      const results = await Promise.all(targets.map(async (config) => {
        forget(config.id);
        try {
          if (action === "enable") await startMcpConnection(config);
          else await api("mcp_disconnect", { id: config.id });
        } catch (cause) {
          setFailures((current) => ({ ...current, [config.id]: { action, message: errorText(cause) } }));
          return false;
        }
        // Show completed connections while slower tool processes are still starting.
        if (targets.length > 1) void client.invalidateQueries({ queryKey: ["mcp", "connections"] }, { cancelRefetch: false });
        return true;
      }));
      await refresh();
      const succeeded = results.filter(Boolean).length;
      if (singleSuccessMessage) {
        if (succeeded) notify(singleSuccessMessage);
      } else {
        const failed = targets.length - succeeded;
        notify(`已${action === "enable" ? "启用" : "禁用"} ${succeeded} 个连接${failed ? `，${failed} 个失败` : ""}`);
      }
    },
    onError: setError,
  });
  const [pendingChange] = useMutationState({
    filters: { mutationKey: connectionChangeKey, status: "pending" },
    select: (mutation) => mutation.state.variables as ConnectionChange,
  });
  const busy = localBusy || !!pendingChange;
  useEffect(() => { onBusyChange?.(busy); }, [busy, onBusyChange]);

  function changeConnections(action: ConnectionAction, targets: McpConfig[], singleSuccessMessage?: string) {
    if (!targets.length || localBusy || client.isMutating({ mutationKey: connectionChangeKey })) return;
    change.mutate({ action, targets, singleSuccessMessage });
  }
  async function runLocal(action: () => Promise<unknown>, done: string) {
    setLocalBusy(true); setError(null);
    try { await action(); await refresh(); setConfirmLocalStart(false); notify(done); }
    catch (failure) { setError(failure); } finally { setLocalBusy(false); }
  }

  const runningIds = new Set(connections.data?.map(({ serverId }) => serverId));
  const toEnable = externalConfigs.filter(({ id }) => !runningIds.has(id));
  const toDisable = externalConfigs.filter(({ id }) => runningIds.has(id) || startup.data?.includes(id));
  const canEnable = !busy && configs.isSuccess && connections.isSuccess && toEnable.length > 0;
  const canDisable = !busy && configs.isSuccess && connections.isSuccess && startup.isSuccess && toDisable.length > 0;
  const bulkAction = pendingChange && !pendingChange.singleSuccessMessage ? pendingChange.action : null;
  const allRunning = externalConfigs.length > 0 && toEnable.length === 0;
  const primaryAction = bulkAction ?? (!onboarding && allRunning ? "disable" : "enable");
  const enable = primaryAction === "enable";
  const bulkControls = <div className="inline-flex" role="group" aria-label="批量管理 MCP 工具">
    <Button size="sm" variant="outline" className={cn(!onboarding && "rounded-r-none")} disabled={enable ? !canEnable : !canDisable}
      onClick={() => changeConnections(primaryAction, enable ? toEnable : toDisable)}>
      {bulkAction ? <Spinner /> : enable ? <PlayIcon /> : <SquareIcon />}
      {bulkAction ? (enable ? "正在启用…" : "正在禁用…") : (enable ? "全部启用" : "全部禁用")}
    </Button>
    {!onboarding && <DropdownMenu>
      <DropdownMenuTrigger asChild><Button size="icon-sm" variant="outline" className="rounded-l-none border-l-0" aria-label="选择批量操作" disabled={!canEnable && !canDisable}><ChevronDownIcon /></Button></DropdownMenuTrigger>
      <DropdownMenuContent align="end"><DropdownMenuGroup>
        <DropdownMenuItem disabled={!canEnable} onSelect={() => changeConnections("enable", toEnable)}><PlayIcon />全部启用</DropdownMenuItem>
        <DropdownMenuItem disabled={!canDisable} onSelect={() => changeConnections("disable", toDisable)}><SquareIcon />全部禁用</DropdownMenuItem>
      </DropdownMenuGroup></DropdownMenuContent>
    </DropdownMenu>}
  </div>;
  const connectionsList = <>
    <ErrorState error={configs.error || startup.error || connections.error || (!onboarding && local.error) || error} />
    {configs.isLoading && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground"><Spinner />正在读取 MCP 工具…</p>}
    {!externalConfigs.length && !configs.isLoading && !configs.error && <EmptyState icon={<ServerIcon />} title="尚未配置外部 MCP 连接" />}
    {!!externalConfigs.length && <ItemGroup className="rounded-md border" aria-label="外部 MCP 连接">
      {externalConfigs.map((config, index) => {
        const active = connections.data?.find((connection) => connection.serverId === config.id);
        const failure = failures[config.id];
        const pending = pendingChange?.targets.some((target) => target.id === config.id) && !failure ? pendingChange.action : null;
        const starting = pending === "enable" && !active;
        const stopping = pending === "disable" && !!active;
        const description = config.description?.trim();
        const endpoint = config.url || [config.command, ...config.args].join(" ");
        const hasCredentials = Boolean(config.bearer_secret_ref || Object.values(config.env_secret_refs).some(Boolean));
        return <Fragment key={config.id}>
          {index > 0 && <ItemSeparator />}
          <Item role="listitem" size="sm" className="flex-nowrap gap-2 border-0 px-2 py-2 sm:gap-3 sm:px-3">
            <ItemMedia variant="icon" className="hidden sm:flex"><ServerIcon /></ItemMedia>
            <ItemContent className="min-w-0 gap-0.5">
              <ItemTitle className="w-full">
                <span className={description ? "max-w-3/5 shrink-0 truncate" : "truncate"} title={config.name}>{config.name}</span>
                {description && <Tooltip>
                  <TooltipTrigger asChild>
                    <span tabIndex={0} className="truncate rounded-xs text-xs font-normal text-muted-foreground focus-visible:outline-ring">{description}</span>
                  </TooltipTrigger>
                  <TooltipContent collisionPadding={8} className="max-w-[min(24rem,var(--radix-tooltip-content-available-width))]">
                    <span className="block max-h-[calc(var(--radix-tooltip-content-available-height)-1rem)] overflow-y-auto whitespace-pre-wrap wrap-anywhere">{description}</span>
                  </TooltipContent>
                </Tooltip>}
              </ItemTitle>
              {config.plugin_slug && <span className="truncate text-xs text-muted-foreground" title={config.plugin_slug}>来自适配包 · {config.plugin_slug}</span>}
              <PathText className="block truncate" title={endpoint}>{endpoint}</PathText>
              {!active && startup.data?.includes(config.id) && <Button variant="link" size="sm" className="h-auto self-start p-0 text-xs" disabled={busy} aria-label={`取消 ${config.name} 自动启动`} onClick={() => { changeConnections("disable", [config], "已取消自动启动"); }}>取消自动启动</Button>}
              {failure && (!active || failure.action === "disable") && <p className="truncate text-xs text-destructive" title={failure.message}>{failure.action === "disable" && "禁用失败："}{failure.message}</p>}
            </ItemContent>
            <ItemActions className={cn("grid shrink-0 gap-0.5 [&_button]:size-7 sm:[&_button]:size-8", onboarding
                ? "grid-cols-[auto_repeat(2,1.75rem)] sm:grid-cols-[auto_repeat(2,2rem)]"
                : "grid-cols-[auto_repeat(3,1.75rem)] sm:grid-cols-[auto_repeat(3,2rem)]")}>
              <Badge className="mr-1 whitespace-nowrap" tone={active ? "success" : failure ? "danger" : "neutral"}>
                {stopping ? "停止中" : active ? <>运行中<span className="hidden sm:inline"> · {active.tools.length} 个工具</span></> : starting ? "启动中" : failure?.action === "enable" ? "启动失败" : "未启动"}
              </Badge>
              {hasCredentials && <IconButton className="col-start-2" title={`配置 ${config.name} 凭据`} aria-haspopup="dialog" disabled={busy} onClick={(event) => setCredentials({ config, trigger: event.currentTarget })}><KeyRoundIcon /></IconButton>}
              {!onboarding && !config.plugin_slug && <IconButton className="col-start-3" title={`编辑 ${config.name}`} disabled={busy} onClick={() => { setFresh(false); setEdit(config); }}><PencilIcon /></IconButton>}
              {active
                ? <IconButton className="col-start-[-2]" title={`停止 ${config.name}`} disabled={busy} onClick={() => changeConnections("disable", [config], `${config.name} 已停止`)}>{stopping ? <Spinner /> : <SquareIcon />}</IconButton>
                : <IconButton className="col-start-[-2]" title={`启动 ${config.name}`} disabled={!backendAvailable || busy} onClick={() => changeConnections("enable", [config], `${config.name} 已启动`)}>{starting ? <Spinner /> : <PlayIcon />}</IconButton>}
            </ItemActions>
          </Item>
        </Fragment>;
      })}
    </ItemGroup>}
  </>;
  return <div className="flex flex-col gap-6">
    {onboarding ? <div className="flex flex-col gap-3"><div className="self-end">{bulkControls}</div>{connectionsList}</div> : <Card className="gap-4 py-4">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between px-3 sm:px-5">
        <CardTitle>可用工具</CardTitle>
        <CardAction className="flex flex-wrap items-center gap-2">{bulkControls}<Button size="sm" variant="outline" disabled={!backendAvailable || busy} onClick={() => { setFresh(true); setEdit({ id: `custom_${Date.now()}`, name: "自定义 MCP", transport: "streamable_http", command: null, args: [], url: "https://", env_secret_refs: {}, bearer_secret_ref: null, credential_url: null, enabled: false, builtin: false }); }}><PlusIcon data-icon="inline-start" />添加连接</Button></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 px-3 sm:px-5">
        {connectionsList}
      </CardContent>
    </Card>}

    {!onboarding && <Card>
      <CardHeader>
        <CardTitle>本地工具</CardTitle>
        <CardDescription>让外部 Agent 通过受控工具使用本机的 Agent Mod Manager。开启后会随应用启动，手动停止即可取消。</CardDescription>
        <CardAction><Badge tone={local.data?.running ? "success" : "neutral"}>{local.data?.stopping ? "正在等待事务完成" : local.data?.running ? "运行中" : "未开启"}</Badge></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col items-start gap-3">
        {local.data?.startupRequested && !local.data.running && <Button variant="link" size="sm" disabled={busy} onClick={() => { void runLocal(() => api("mcp_server_stop"), "已取消本地工具自动启动"); }}>取消本地工具自动启动</Button>}
        {local.data?.endpoint && <PathText>{local.data.endpoint}</PathText>}
        {local.data?.running
          ? <Button variant="outline" disabled={busy} onClick={() => { void runLocal(() => api("mcp_server_stop"), "本地工具已停止"); }}><SquareIcon data-icon="inline-start" />停止工具</Button>
          : <Button variant="outline" disabled={!backendAvailable || busy || local.data?.stopping} onClick={() => setConfirmLocalStart(true)}><PlayIcon data-icon="inline-start" />开启工具</Button>}
      </CardContent>
    </Card>}

    {credentials && <McpCredentialDialog {...credentials} onClose={() => setCredentials(null)} />}
    {edit && <McpEditor config={edit} fresh={fresh} onClose={() => setEdit(null)} onSaved={async () => { forget(edit.id); setEdit(null); await refresh(); }} />}
    {confirmLocalStart && <Modal title="开启本地工具" onClose={() => { if (!busy) setConfirmLocalStart(false); }} className="sm:max-w-md" actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setConfirmLocalStart(false)}>取消</Button>
      <Button disabled={busy} onClick={() => { void runLocal(() => api("mcp_server_start", { confirmation: "START_LOCAL_MCP" }), "本地工具已开启"); }}>
        {busy ? <Spinner data-icon="inline-start" /> : <PlayIcon data-icon="inline-start" />}开启工具
      </Button>
    </>}>
      <p className="text-sm">允许持有本机令牌的外部 Agent 调用受控工具。文件写入沿用桌面权限策略，审批只能在桌面完成。开启成功且未手动停止时，下次启动应用会自动尝试开启。</p>
      <ErrorState error={error} />
    </Modal>}
  </div>;
}

function McpEditor({ config, fresh, onClose, onSaved }: { config: McpConfig; fresh: boolean; onClose: () => void; onSaved: () => Promise<void> }) {
  const [draft, setDraft] = useState(config);
  const isThreeDm = isThreeDmHttpEndpoint(draft);
  const [env, setEnv] = useState(JSON.stringify(config.env_secret_refs, null, 2));
  const [busy, setBusy] = useState(false);
  const [remove, setRemove] = useState(false);
  const [error, setError] = useState<unknown>(null);
  async function save() {
    setBusy(true); setError(null);
    try {
      if (remove) { await api("mcp_remove", { id: draft.id }); }
      else {
        const credentialUrl = draft.credential_url?.trim() || null;
        if (credentialUrl && !isCredentialUrl(credentialUrl)) {
          throw new Error("获取 Key 链接必须是有效的 HTTP 或 HTTPS 网址，且不能包含用户名或密码。");
        }
        const stdio = draft.transport === "stdio";
        const next = {
          ...draft,
          description: draft.description?.trim() || null,
          credential_url: credentialUrl,
          command: stdio ? draft.command : null,
          args: stdio ? draft.args : [],
          url: stdio ? null : draft.url,
          env_secret_refs: stdio ? JSON.parse(env) : {},
          bearer_secret_ref: stdio ? null : draft.bearer_secret_ref || null,
        };
        await api(fresh ? "mcp_create" : "mcp_update", { config: next });
      }
      await onSaved();
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  return <Modal title={remove ? "移除 MCP 配置" : fresh ? "添加 MCP 连接" : "编辑 MCP 连接"} onClose={() => { if (!busy) onClose(); }} className="sm:max-w-xl" actions={<>
    {!fresh && !remove && <IconButton title="移除配置" className="mr-auto" onClick={() => setRemove(true)}><Trash2Icon /></IconButton>}
    <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
    <Button variant={remove ? "destructive" : "default"} disabled={busy} onClick={() => { void save(); }}>{busy ? <Spinner data-icon="inline-start" /> : <SaveIcon data-icon="inline-start" />}{remove ? "确认移除" : "保存配置"}</Button>
  </>}>
    {remove ? <p className="text-sm">移除 {draft.name} 的连接配置并断开连接。本地加密凭据仍会保留。</p> : <FieldGroup>
      <Field><FieldLabel htmlFor="mcp-name">名称</FieldLabel><Input id="mcp-name" value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></Field>
      <Field><FieldLabel htmlFor="mcp-description">描述（可选）</FieldLabel><Input id="mcp-description" value={draft.description ?? ""} onChange={(e) => setDraft({ ...draft, description: e.target.value })} /></Field>
      <Field>
        <FieldLabel htmlFor="mcp-transport">传输</FieldLabel>
        <NativeSelect id="mcp-transport" className="w-full" value={draft.transport} onChange={(e) => setDraft({ ...draft, transport: e.target.value as McpConfig["transport"] })}>
          <NativeSelectOption value="streamable_http">Streamable HTTP</NativeSelectOption><NativeSelectOption value="stdio">stdio</NativeSelectOption>
        </NativeSelect>
      </Field>
      {draft.transport === "streamable_http" ? <>
        <Field><FieldLabel htmlFor="mcp-url">URL</FieldLabel><Input id="mcp-url" type="url" value={draft.url || ""} onChange={(e) => setDraft({ ...draft, url: e.target.value })} /></Field>
        <Field>
          <FieldLabel htmlFor="mcp-bearer">{isThreeDm ? "3DM API Key 引用" : "Bearer 密钥引用"}</FieldLabel>
          <Input id="mcp-bearer" aria-describedby={isThreeDm ? "mcp-key-hint" : undefined} placeholder="mcp.custom.api_key" value={draft.bearer_secret_ref || ""} onChange={(e) => setDraft({ ...draft, bearer_secret_ref: e.target.value })} />
          {isThreeDm && <FieldDescription id="mcp-key-hint">留空可匿名连接。</FieldDescription>}
        </Field>
      </> : <>
        <Field><FieldLabel htmlFor="mcp-command">命令</FieldLabel><Input id="mcp-command" value={draft.command || ""} onChange={(e) => setDraft({ ...draft, command: e.target.value })} /></Field>
        <Field><FieldLabel htmlFor="mcp-args">参数，每行一项</FieldLabel><Textarea id="mcp-args" rows={3} value={draft.args.join("\n")} onChange={(e) => setDraft({ ...draft, args: e.target.value ? e.target.value.split("\n") : [] })} /></Field>
        <Field><FieldLabel htmlFor="mcp-env">环境变量与密钥引用 JSON</FieldLabel><Textarea id="mcp-env" rows={4} className="font-mono text-xs" value={env} onChange={(e) => setEnv(e.target.value)} /></Field>
      </>}
      <Field>
        <FieldLabel htmlFor="mcp-credential-url">获取 Key 链接（可选）</FieldLabel>
        <Input id="mcp-credential-url" type="url" placeholder="https://example.com/api-keys" value={draft.credential_url ?? ""} onChange={(e) => setDraft({ ...draft, credential_url: e.target.value })} />
      </Field>
    </FieldGroup>}
    <ErrorState error={error} />
  </Modal>;
}
