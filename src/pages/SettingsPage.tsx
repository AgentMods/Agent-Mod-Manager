import { McpLaunchDetails } from "@/components/McpLaunchDetails";
import { startMcpConnection, type McpConfig } from "@/lib/mcp-connection";
import { appendPathLeaf, displayPath } from "@/lib/paths";
import { Fragment, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useQueryClient } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpenIcon } from "lucide-react";
import { BoxIcon, DownloadIcon, FileTextIcon, KeyRoundIcon, MoonIcon, PencilIcon, PlayIcon, PlusIcon, RotateCcwIcon, SaveIcon, SearchIcon, ServerIcon, SquareIcon, SunIcon, Trash2Icon } from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { FilePlan } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { Badge, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText } from "@/components/ui";
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Item, ItemActions, ItemContent, ItemGroup, ItemMedia, ItemSeparator, ItemTitle } from "@/components/ui/item";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

type LocalStatus = { startupRequested: boolean; running: boolean; stopping: boolean; endpoint: string | null; tokenRef: string; activeCalls: number };
type Connection = { serverId: string; tools: string[] };

const sections = [["model", "AI 与模型"], ["mcp", "MCP 服务"], ["skills", "Skills"], ["appearance", "外观与存储"]] as const;

export default function SettingsPage() {
  const [tab, setTab] = useState("model");
  return <Page>
    <PageHeader title="设置" subtitle="模型、MCP、Skills 与外观" />
    <Tabs value={tab} onValueChange={setTab} className="gap-6">
      <TabsList aria-label="设置分类" className="max-w-full flex-wrap justify-start group-data-[orientation=horizontal]/tabs:h-auto">
        {sections.map(([id, name]) => <TabsTrigger key={id} value={id} className="h-7.5 flex-none">{name}</TabsTrigger>)}
      </TabsList>
      <TabsContent value="model"><ModelSettings /></TabsContent>
      <TabsContent value="mcp"><McpSettings /></TabsContent>
      <TabsContent value="skills"><SkillsSettings /></TabsContent>
      <TabsContent value="appearance"><AppearanceSettings /></TabsContent>
    </Tabs>
  </Page>;
}

function ModelSettings() {
  return <Card>
    <CardHeader><CardTitle>模型服务</CardTitle><CardDescription>配置 Agent 使用的模型服务、密钥与默认权限策略。</CardDescription></CardHeader>
    <CardContent><AgentConfiguration /></CardContent>
  </Card>;
}

function SecretField({ label, reference }: { label: string; reference: string }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  return <form className="flex flex-col gap-2" onSubmit={async (event) => {
    event.preventDefault(); if (!value.trim()) return;
    setBusy(true); setError(null);
    try { await api("set_secret", { key: reference, value }); setValue(""); notify(`${label}已保存`); }
    catch (failure) { setError(failure); } finally { setBusy(false); }
  }}>
    <Field>
      <FieldLabel htmlFor={`secret-${reference}`}>{label}</FieldLabel>
      <InputGroup>
        <InputGroupInput id={`secret-${reference}`} type="password" autoComplete="new-password" aria-label={label} value={value} onChange={(event) => setValue(event.target.value)} disabled={!backendAvailable || busy} placeholder={reference} />
        <InputGroupAddon align="inline-end">
          <InputGroupButton type="submit" size="xs" aria-label={`保存${label}`} title={`保存${label}`} disabled={!backendAvailable || busy || !value.trim()}>
            {busy && <Spinner />}保存
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
    </Field>
    <ErrorState error={error} />
  </form>;
}

function mcpCredentialFields(config: McpConfig) {
  const bindings = Object.entries(config.env_secret_refs);
  if (config.bearer_secret_ref) bindings.unshift(["Bearer 密钥", config.bearer_secret_ref]);
  const labelsByReference = new Map<string, string[]>();
  for (const [label, reference] of bindings) {
    if (!reference) continue;
    const labels = labelsByReference.get(reference);
    if (labels) labels.push(label);
    else labelsByReference.set(reference, [label]);
  }
  return [...labelsByReference].map(([reference, labels]) => ({ reference, label: labels.join(" / ") }));
}

function McpSettings() {
  const configs = useBackendQuery<McpConfig[]>(["mcp", "configs"], "mcp_list");
  const externalConfigs = configs.data?.filter((config) => config.id !== "amm_local") ?? [];
  const startup = useBackendQuery<string[]>(["mcp", "startup"], "mcp_startup_requests");
  const connections = useBackendQuery<Connection[]>(["mcp", "connections"], "mcp_connections");
  const local = useBackendQuery<LocalStatus>(["mcp", "local"], "mcp_server_status");
  const [edit, setEdit] = useState<McpConfig | null>(null);
  const [fresh, setFresh] = useState(false);
  const [credentials, setCredentials] = useState<{
    config: McpConfig;
    trigger: HTMLButtonElement;
  } | null>(null);
  const [confirm, setConfirm] = useState<McpConfig | "local" | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [failures, setFailures] = useState<Record<string, string>>({});
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
  const busy = pending !== null;
  const forget = (id: string) => setFailures(({ [id]: _removed, ...rest }) => rest);
  const refresh = () => client.invalidateQueries({ queryKey: ["mcp"] });
  async function run(id: string, action: () => Promise<unknown>, done: string) {
    setPending(id); setError(null);
    try { await action(); await refresh(); setConfirm(null); notify(done); }
    catch (failure) { setError(failure); } finally { setPending(null); }
  }
  /** Saving `enabled` is part of starting, so there is no separate enable switch. */
  async function start(config: McpConfig, acceptsUnsandboxedStdio: boolean) {
    setPending(config.id); forget(config.id);
    try {
      await startMcpConnection(config, acceptsUnsandboxedStdio);
      await refresh(); setConfirm(null); notify(`${config.name} 已启动`);
    } catch (failure) {
      setFailures((current) => ({ ...current, [config.id]: errorText(failure) }));
      void refresh();
    } finally { setPending(null); }
  }
  return <div className="flex flex-col gap-6">
    <Card className="gap-4 py-4">
      <CardHeader className="px-3 sm:px-5">
        <CardTitle>外部 MCP 连接</CardTitle>
        <CardDescription>已启动的连接会在下次打开应用时自动重连。停止或编辑连接会取消自动启动。</CardDescription>
        <CardAction><Button size="sm" variant="outline" disabled={!backendAvailable} onClick={() => { setFresh(true); setEdit({ id: `custom_${Date.now()}`, name: "自定义 MCP", transport: "streamable_http", command: null, args: [], url: "https://", env_secret_refs: {}, bearer_secret_ref: null, enabled: false, builtin: false }); }}><PlusIcon data-icon="inline-start" />添加连接</Button></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-3 px-3 sm:px-5">
        <ErrorState error={configs.error || startup.error || connections.error || local.error || error} />
        {!externalConfigs.length && !configs.isLoading && !configs.error && <EmptyState icon={<ServerIcon />} title="尚未配置外部 MCP 连接" />}
        {!!externalConfigs.length && <ItemGroup className="rounded-md border" aria-label="外部 MCP 连接">
          {externalConfigs.map((config, index) => {
            const active = connections.data?.find((connection) => connection.serverId === config.id);
            const starting = pending === config.id && !active;
            const failure = failures[config.id];
            const endpoint = config.url || [config.command, ...config.args].join(" ");
            const hasCredentials = Boolean(config.bearer_secret_ref || Object.values(config.env_secret_refs).some(Boolean));
            return <Fragment key={config.id}>
              {index > 0 && <ItemSeparator />}
              <Item role="listitem" size="sm" className="flex-nowrap gap-2 border-0 px-2 py-2 sm:gap-3 sm:px-3">
                <ItemMedia variant="icon" className="hidden sm:flex"><ServerIcon /></ItemMedia>
                <ItemContent className="min-w-0 gap-0.5">
                  <ItemTitle className="max-w-full"><span className="truncate" title={config.name}>{config.name}</span></ItemTitle>
                  <PathText className="block truncate" title={endpoint}>{endpoint}</PathText>
                  {!active && startup.data?.includes(config.id) && <Button variant="link" size="sm" className="h-auto self-start p-0 text-xs" disabled={busy} aria-label={`取消 ${config.name} 自动启动`} onClick={() => { void run(config.id, () => api("mcp_disconnect", { id: config.id }), "已取消自动启动"); }}>取消自动启动</Button>}
                  {failure && !active && <p className="truncate text-xs text-destructive" title={failure}>{failure}</p>}
                </ItemContent>
                <ItemActions className="grid shrink-0 grid-cols-[auto_repeat(3,1.75rem)] gap-0.5 [&_button]:size-7 sm:grid-cols-[auto_repeat(3,2rem)] sm:[&_button]:size-8">
                  <Badge className="mr-1 whitespace-nowrap" tone={active ? "success" : failure ? "danger" : "neutral"}>
                    {active ? <>运行中<span className="hidden sm:inline"> · {active.tools.length} 个工具</span></> : starting ? "启动中" : failure ? "启动失败" : "未启动"}
                  </Badge>
                  {hasCredentials && <IconButton className="col-start-2" title={`配置 ${config.name} 凭据`} aria-haspopup="dialog" onClick={(event) => setCredentials({ config, trigger: event.currentTarget })}><KeyRoundIcon /></IconButton>}
                  <IconButton className="col-start-3" title={`编辑 ${config.name}`} onClick={() => { setFresh(false); setEdit(config); }}><PencilIcon /></IconButton>
                  {active
                    ? <IconButton className="col-start-4" title={`停止 ${config.name}`} disabled={busy} onClick={() => { void run(config.id, () => api("mcp_disconnect", { id: config.id }), `${config.name} 已停止`); }}><SquareIcon /></IconButton>
                    : <IconButton className="col-start-4" title={`启动 ${config.name}`} disabled={!backendAvailable || busy} onClick={() => { if (config.transport === "stdio") { forget(config.id); setConfirm(config); } else void start(config, false); }}>{starting ? <Spinner /> : <PlayIcon />}</IconButton>}
                </ItemActions>
              </Item>
            </Fragment>;
          })}
        </ItemGroup>}
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>本地 MCP 服务</CardTitle>
        <CardDescription>让外部 Agent 通过受控工具使用本机的 Agent Mod Manager。开启后会随应用启动，手动停止即可取消。</CardDescription>
        <CardAction><Badge tone={local.data?.running ? "success" : "neutral"}>{local.data?.stopping ? "正在等待事务完成" : local.data?.running ? "运行中" : "未开启"}</Badge></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col items-start gap-3">
        {local.data?.startupRequested && !local.data.running && <Button variant="link" size="sm" disabled={busy} onClick={() => { void run("local", () => api("mcp_server_stop"), "已取消本地 MCP 自动启动"); }}>取消本地 MCP 自动启动</Button>}
        {local.data?.endpoint && <PathText>{local.data.endpoint}</PathText>}
        {local.data?.running
          ? <Button variant="outline" disabled={busy} onClick={() => { void run("local", () => api("mcp_server_stop"), "本地 MCP 服务已停止"); }}><SquareIcon data-icon="inline-start" />停止服务</Button>
          : <Button variant="outline" disabled={!backendAvailable || busy || local.data?.stopping} onClick={() => setConfirm("local")}><PlayIcon data-icon="inline-start" />开启服务</Button>}
      </CardContent>
    </Card>

    {credentials && <Modal title="配置 MCP 凭据" description={credentials.config.name} className="sm:max-w-md [overflow-wrap:anywhere]"
      onClose={() => setCredentials(null)}
      onCloseAutoFocus={(event) => { event.preventDefault(); credentials.trigger.focus(); }}
      actions={<Button variant="outline" onClick={() => setCredentials(null)}>返回列表</Button>}>
      <p className="text-sm text-muted-foreground">凭据仅在本机加密保存。输入新值可替换已保存的凭据。</p>
      <FieldGroup className="gap-4">
        {mcpCredentialFields(credentials.config).map(({ reference, label }) => <SecretField key={reference} label={label} reference={reference} />)}
      </FieldGroup>
    </Modal>}
    {edit && <McpEditor config={edit} fresh={fresh} onClose={() => setEdit(null)} onSaved={async () => { forget(edit.id); setEdit(null); await refresh(); }} />}
    {confirm && <Modal title={confirm === "local" ? "开启本地 MCP 服务" : `启动 ${confirm.name}`} onClose={() => { if (!busy) setConfirm(null); }} className="sm:max-w-md" actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setConfirm(null)}>取消</Button>
      <Button disabled={busy} onClick={() => { if (confirm === "local") void run("local", () => api("mcp_server_start", { confirmation: "START_LOCAL_MCP" }), "本地 MCP 服务已开启"); else void start(confirm, true); }}>
        {busy ? <Spinner data-icon="inline-start" /> : <PlayIcon data-icon="inline-start" />}{confirm === "local" ? "开启服务" : "启动"}
      </Button>
    </>}>
      {confirm === "local" ? <>
        <p className="text-sm">允许持有本机令牌的外部 Agent 调用受控工具。文件写入沿用桌面权限策略，审批只能在桌面完成。开启成功且未手动停止时，下次启动应用会自动尝试开启。</p>
        <ErrorState error={error} />
      </> : <>
        <McpLaunchDetails config={confirm} />
        <ErrorState error={failures[confirm.id]} />
      </>}
    </Modal>}
  </div>;
}

function McpEditor({ config, fresh, onClose, onSaved }: { config: McpConfig; fresh: boolean; onClose: () => void; onSaved: () => Promise<void> }) {
  const [draft, setDraft] = useState(config);
  const [env, setEnv] = useState(JSON.stringify(config.env_secret_refs, null, 2));
  const [busy, setBusy] = useState(false);
  const [remove, setRemove] = useState(false);
  const [error, setError] = useState<unknown>(null);
  async function save() {
    setBusy(true); setError(null);
    try {
      if (remove) { await api("mcp_remove", { id: draft.id }); }
      else {
        const next = { ...draft, command: draft.transport === "stdio" ? draft.command : null, args: draft.transport === "stdio" ? draft.args : [], url: draft.transport === "streamable_http" ? draft.url : null, env_secret_refs: draft.transport === "stdio" ? JSON.parse(env) : {}, bearer_secret_ref: draft.transport === "streamable_http" ? draft.bearer_secret_ref || null : null };
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
      <Field>
        <FieldLabel htmlFor="mcp-transport">传输</FieldLabel>
        <NativeSelect id="mcp-transport" className="w-full" value={draft.transport} onChange={(e) => setDraft({ ...draft, transport: e.target.value as McpConfig["transport"] })}>
          <NativeSelectOption value="streamable_http">Streamable HTTP</NativeSelectOption><NativeSelectOption value="stdio">stdio</NativeSelectOption>
        </NativeSelect>
      </Field>
      {draft.transport === "streamable_http" ? <>
        <Field><FieldLabel htmlFor="mcp-url">URL</FieldLabel><Input id="mcp-url" type="url" value={draft.url || ""} onChange={(e) => setDraft({ ...draft, url: e.target.value })} /></Field>
        <Field><FieldLabel htmlFor="mcp-bearer">Bearer 密钥引用</FieldLabel><Input id="mcp-bearer" placeholder="mcp.custom.api_key" value={draft.bearer_secret_ref || ""} onChange={(e) => setDraft({ ...draft, bearer_secret_ref: e.target.value })} /></Field>
      </> : <>
        <Field><FieldLabel htmlFor="mcp-command">命令</FieldLabel><Input id="mcp-command" value={draft.command || ""} onChange={(e) => setDraft({ ...draft, command: e.target.value })} /></Field>
        <Field><FieldLabel htmlFor="mcp-args">参数，每行一项</FieldLabel><Textarea id="mcp-args" rows={3} value={draft.args.join("\n")} onChange={(e) => setDraft({ ...draft, args: e.target.value ? e.target.value.split("\n") : [] })} /></Field>
        <Field><FieldLabel htmlFor="mcp-env">环境变量与密钥引用 JSON</FieldLabel><Textarea id="mcp-env" rows={4} className="font-mono text-xs" value={env} onChange={(e) => setEnv(e.target.value)} /></Field>
      </>}
    </FieldGroup>}
    <ErrorState error={error} />
  </Modal>;
}

function AppearanceSettings() {
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const navigate = useUI((s) => s.navigate);
  const bootstrap = useBackendQuery<{ dataRoot: string; storageMigrationError?: string }>(["bootstrap"], "get_bootstrap_state");
  const [destination, setDestination] = useState<{ parent: string; path: string } | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [storageError, setStorageError] = useState<unknown>(null);

  async function chooseStorage() {
    setStorageError(null);
    try {
      const parent = await open({ directory: true, multiple: false, title: "选择存储位置" });
      if (typeof parent === "string" && parent) setDestination({ parent, path: appendPathLeaf(parent, "Agent Mod Manager") });
    } catch (error) {
      setStorageError(error);
    }
  }

  async function migrateStorage() {
    if (!destination || restarting) return;
    setRestarting(true);
    setStorageError(null);
    try {
      await api("migrate_storage", { parent: destination.parent });
    } catch (error) {
      setStorageError(error);
      setRestarting(false);
    }
  }
  return <div className="flex flex-col gap-6">
    <Card>
      <CardHeader><CardTitle>外观</CardTitle><CardDescription>选择界面主题，设置会保存在本机。</CardDescription></CardHeader>
      <CardContent>
        <ToggleGroup type="single" variant="outline" aria-label="主题" value={theme} onValueChange={(value) => { if (value && value !== theme) toggleTheme(); }}>
          <ToggleGroupItem value="light" aria-label="亮色"><SunIcon />亮色</ToggleGroupItem>
          <ToggleGroupItem value="dark" aria-label="暗色"><MoonIcon />暗色</ToggleGroupItem>
        </ToggleGroup>
      </CardContent>
    </Card>
    <Card>
      <CardHeader><CardTitle>存储</CardTitle><CardDescription>管理器数据（数据库、快照、回收站、下载）所在目录。</CardDescription></CardHeader>
      <CardContent className="flex flex-col items-start gap-4">
        <PathText>{bootstrap.data?.dataRoot ? displayPath(bootstrap.data.dataRoot) : "未连接桌面服务"}</PathText>
        <div className="flex flex-wrap gap-2">
          <Button variant="outline" disabled={!backendAvailable || !bootstrap.data || restarting} onClick={chooseStorage}><FolderOpenIcon data-icon="inline-start" />更改存储目录</Button>
          <Button variant="outline" disabled={!backendAvailable || !bootstrap.data || restarting} onClick={() => navigate("onboarding")}><RotateCcwIcon data-icon="inline-start" />重新查看新手引导</Button>
        </div>
        <ErrorState error={bootstrap.error || (bootstrap.data?.storageMigrationError && `迁移失败，仍在使用原目录。${bootstrap.data.storageMigrationError}`) || (!destination && storageError)} />
      </CardContent>
      <AlertDialog open={destination !== null} onOpenChange={(open) => { if (!open && !restarting) setDestination(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>迁移存储目录</AlertDialogTitle>
            <AlertDialogDescription>应用将重启并把管理器数据复制到下方的新目录。原目录会保留为备份。</AlertDialogDescription>
          </AlertDialogHeader>
          {destination && <PathText>{displayPath(destination.path)}</PathText>}
          <ErrorState error={storageError} />
          <AlertDialogFooter>
            <AlertDialogCancel disabled={restarting}>取消</AlertDialogCancel>
            <Button disabled={restarting} onClick={migrateStorage}>
              {restarting && <Spinner data-icon="inline-start" />}
              {restarting ? "正在重启…" : "迁移并重启"}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Card>
  </div>;
}

type SkillInfo = {
  name: string;
  description: string;
  enabled: boolean;
  kind: "system" | "standalone" | "builtin" | "plugin" | "unknown";
  valid: boolean;
  issues: string[];
};

const systemSkillPresentation = new Map([
  ["system-adapter-authoring", { title: "创建与修改适配包", icon: BoxIcon }],
  ["system-web-search", { title: "联网搜索", icon: SearchIcon }],
  ["system-mod-download", { title: "从网站下载 Mod", icon: DownloadIcon }],
]);

function skillPresentation(skill: SkillInfo) {
  return (skill.kind === "system" ? systemSkillPresentation.get(skill.name) : undefined)
    ?? { title: skill.name, icon: FileTextIcon };
}

function SkillTable({ skills, source, busy, onInspect, onToggle }: {
  skills: SkillInfo[];
  source: string;
  busy: boolean;
  onInspect: (name: string) => void;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  return <div className="rounded-md border">
    <Table>
      <TableHeader>
        <TableRow className="bg-muted/50">
          <TableHead className="w-full">名称</TableHead>
          <TableHead>来源</TableHead>
          <TableHead>状态</TableHead>
          <TableHead>启用</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>{skills.map((skill) => {
        const { title, icon: Icon } = skillPresentation(skill);
        return <TableRow key={skill.name}>
          <TableCell className="whitespace-normal">
            <div className="flex items-start gap-2.5">
              <Icon aria-hidden="true" className="mt-0.5 hidden size-4 shrink-0 text-muted-foreground sm:block" />
              <div className="min-w-0 wrap-break-word">
                <Button variant="link" className="h-auto justify-start p-0 text-left font-medium whitespace-normal" onClick={() => onInspect(skill.name)} disabled={busy || !skill.valid}>{title}</Button>
                <p className="text-xs text-muted-foreground">{skill.description}</p>
                {skill.issues.map((issue) => <p className="text-xs text-warning" key={issue}>{issue}</p>)}
              </div>
            </div>
          </TableCell>
          <TableCell>{source}</TableCell>
          <TableCell><Badge tone={skill.valid ? "success" : "warning"} className="whitespace-nowrap">{skill.valid ? "已校验" : "不可用"}</Badge></TableCell>
          <TableCell><Checkbox aria-label={`启用 ${title}`} checked={skill.enabled} disabled={busy || !backendAvailable || (!skill.valid && !skill.enabled)} onCheckedChange={(checked) => onToggle(skill.name, checked === true)} /></TableCell>
        </TableRow>;
      })}</TableBody>
    </Table>
  </div>;
}
function SkillsSettings() {
  const skills = useBackendQuery<SkillInfo[]>(["skills"], "list_skills");
  const [importing, setImporting] = useState(false);
  const [name, setName] = useState("");
  const [markdown, setMarkdown] = useState("");
  const [selected, setSelected] = useState<{ skill: SkillInfo; markdown: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const reviewPlan = useUI((s) => s.reviewPlan);
  const client = useQueryClient();
  async function inspect(name: string) {
    setBusy(true); setError(null);
    try { setSelected(await api("get_skill", { name })); }
    catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function install() {
    setBusy(true); setError(null);
    try {
      const preview = await api<{ plan: FilePlan }>("plan_install_skill", { name, markdown });
      setImporting(false);
      reviewPlan(preview.plan, () => { setName(""); setMarkdown(""); void client.invalidateQueries({ queryKey: ["skills"] }); });
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function uninstall(skill: SkillInfo) {
    setBusy(true); setError(null);
    try {
      const preview = await api<{ plan: FilePlan }>("plan_uninstall_skill", { name: skill.name });
      setSelected(null);
      reviewPlan(preview.plan, () => {
        void client.invalidateQueries({ queryKey: ["skills"] });
        void client.invalidateQueries({ queryKey: ["trash"] });
      });
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function toggle(name: string, enabled: boolean) {
    setBusy(true); setError(null);
    try { await api("set_skill_enabled", { name, enabled }); await skills.refetch(); }
    catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  const systemSkills = skills.data?.filter((skill) => skill.kind === "system");
  const standaloneSkills = skills.data?.filter((skill) => skill.kind === "standalone");

  return <div className="flex flex-col gap-4">
    <ErrorState error={skills.error || (selected || importing ? null : error)} />
    <Card role="region" aria-labelledby="system-skills-title">
      <CardHeader className="px-3 sm:px-6">
        <CardTitle id="system-skills-title">系统 Skills</CardTitle>
        <CardDescription>管理 Agent 用于创建适配包、联网搜索和下载 Mod 的内置指引。</CardDescription>
      </CardHeader>
      <CardContent className="px-3 sm:px-6">
        {skills.isLoading && <Spinner />}
        {systemSkills && (systemSkills.length > 0
          ? <SkillTable skills={systemSkills} source="系统内置" busy={busy} onInspect={inspect} onToggle={toggle} />
          : <EmptyState title="暂无系统 Skills" />)}
      </CardContent>
    </Card>
    <Card role="region" aria-labelledby="standalone-skills-title">
      <CardHeader className="px-3 sm:px-6">
        <CardTitle id="standalone-skills-title">手动安装的 Skills</CardTitle>
        <CardDescription>导入声明式 SKILL.md，为 Agent 添加操作说明。</CardDescription>
        <CardAction><Button variant="outline" disabled={!backendAvailable || busy} onClick={() => { setError(null); setImporting(true); }}><PlusIcon data-icon="inline-start" />导入 Skill</Button></CardAction>
      </CardHeader>
      <CardContent className="px-3 sm:px-6">
        {standaloneSkills && (standaloneSkills.length > 0
          ? <SkillTable skills={standaloneSkills} source="手动安装" busy={busy} onInspect={inspect} onToggle={toggle} />
          : <EmptyState title="暂无手动安装的 Skills" />)}
      </CardContent>
    </Card>
    {selected && <Modal title={skillPresentation(selected.skill).title} onClose={() => { if (!busy) setSelected(null); }} actions={<>
      {selected.skill.kind === "standalone" && <IconButton title={`预览卸载 Skill ${selected.skill.name}`} className="mr-auto" disabled={!backendAvailable || busy || !selected.skill.valid} onClick={() => { void uninstall(selected.skill); }}><Trash2Icon /></IconButton>}
      <Button variant="outline" disabled={busy} onClick={() => setSelected(null)}>关闭</Button>
    </>}>
      <pre className="max-h-96 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-words whitespace-pre-wrap">{selected.markdown}</pre>
      {selected.skill.kind === "system" && selected.skill.name === "system-web-search" &&
        <SecretField label="Brave Search API Key" reference="sources.brave" />}
      <ErrorState error={error} />
    </Modal>}
    {importing && <Modal title="导入声明式 Skill" onClose={() => { if (!busy) setImporting(false); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setImporting(false)}>取消</Button>
      <Button disabled={busy || !name.trim() || !markdown.trim()} onClick={() => { void install(); }}>预览文件计划</Button>
    </>}>
      <FieldGroup>
        <Field><FieldLabel htmlFor="skill-name">名称</FieldLabel><Input id="skill-name" aria-label="Skill 名称" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-game-guide" /></Field>
        <Field><FieldLabel htmlFor="skill-md">SKILL.md</FieldLabel><Textarea id="skill-md" aria-label="SKILL.md" rows={12} className="font-mono text-xs" value={markdown} onChange={(event) => setMarkdown(event.target.value)} /><FieldDescription>粘贴完整的 SKILL.md（含 frontmatter）。</FieldDescription></Field>
      </FieldGroup>
      <ErrorState error={error} />
    </Modal>}
  </div>;
}
