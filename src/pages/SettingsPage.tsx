import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { CheckIcon, KeyRoundIcon, LinkIcon, MoonIcon, PencilIcon, PlayIcon, PlusIcon, RotateCcwIcon, SaveIcon, ServerIcon, SquareIcon, SunIcon, Trash2Icon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { FilePlan } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { Badge, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";

type McpConfig = {id: string; name: string; transport: "stdio" | "streamable_http"; command: string | null; args: string[]; url: string | null; env_secret_refs: Record<string, string>; bearer_secret_ref: string | null; enabled: boolean; builtin: boolean};
type LocalStatus = {running: boolean; stopping: boolean; endpoint: string | null; tokenRef: string; activeCalls: number};
type Connection = {id: string; serverId?: string; server_id?: string; toolCount?: number; tool_count?: number};

const sections = [["model", "AI 与模型"], ["sources", "Mod 来源"], ["mcp", "MCP 服务"], ["skills", "Skills"], ["appearance", "外观与存储"]] as const;

export default function SettingsPage() {
  const [tab, setTab] = useState("model");
  return <Page>
    <PageHeader title="设置" subtitle="模型、Mod 来源、MCP 与外观"/>
    <Tabs value={tab} onValueChange={setTab} className="gap-6">
      <div className="max-w-full overflow-x-auto"><TabsList aria-label="设置分类">{sections.map(([id, name]) => <TabsTrigger key={id} value={id}>{name}</TabsTrigger>)}</TabsList></div>
      <TabsContent value="model"><ModelSettings/></TabsContent>
      <TabsContent value="sources"><SourceSettings/></TabsContent>
      <TabsContent value="mcp"><McpSettings/></TabsContent>
      <TabsContent value="skills"><SkillsSettings/></TabsContent>
      <TabsContent value="appearance"><AppearanceSettings/></TabsContent>
    </Tabs>
  </Page>;
}

function ModelSettings() {
  return <Card>
    <CardHeader><CardTitle>模型服务</CardTitle><CardDescription>配置 Agent 使用的模型服务、密钥与默认权限策略。</CardDescription></CardHeader>
    <CardContent><AgentConfiguration/></CardContent>
  </Card>;
}

function SecretField({label, reference}: {label: string; reference: string}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  const client = useQueryClient();
  return <form className="flex flex-col gap-2" onSubmit={async (event) => {
    event.preventDefault(); if (!value.trim()) return;
    setBusy(true); setError(null);
    try {await api("set_secret", {key: reference, value}); setValue(""); await client.invalidateQueries({queryKey: ["sources"]}); notify(`${label}已保存`);}
    catch (failure) {setError(failure);} finally {setBusy(false);}
  }}>
    <Field>
      <FieldLabel htmlFor={`secret-${reference}`}>{label}</FieldLabel>
      <InputGroup>
        <InputGroupInput id={`secret-${reference}`} type="password" autoComplete="new-password" aria-label={label} value={value} onChange={(event) => setValue(event.target.value)} disabled={!backendAvailable || busy} placeholder={reference}/>
        <InputGroupAddon align="inline-end">
          <InputGroupButton type="submit" size="icon-xs" aria-label={`保存${label}`} title={`保存${label}`} disabled={!backendAvailable || busy || !value.trim()}>{busy ? <Spinner/> : <KeyRoundIcon/>}</InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
    </Field>
    <ErrorState error={error}/>
  </form>;
}

function SourceSettings() {
  const sources = useBackendQuery<Array<{source_id: string; state: string; message: string}>>(["sources"], "list_mod_sources");
  return <Card>
    <CardHeader><CardTitle>来源凭据</CardTitle><CardDescription>Agent 的通用网页搜索使用 Brave Search。未配置时会提示配置，Mod 来源搜索仍可使用。</CardDescription></CardHeader>
    <CardContent className="flex flex-col gap-6">
      <ErrorState error={sources.error}/>
      {!!sources.data?.length && <ItemGroup className="gap-2">
        {sources.data.map((source) => <Item key={source.source_id} variant="outline" size="sm">
          <ItemContent><ItemTitle>{source.source_id}</ItemTitle>{source.message && <ItemDescription className="line-clamp-none break-words">{source.message}</ItemDescription>}</ItemContent>
          <ItemActions><Badge tone={source.state === "available" ? "success" : "warning"}>{source.state}</Badge></ItemActions>
        </Item>)}
      </ItemGroup>}
      <Separator/>
      <FieldGroup>
        <SecretField label="CurseForge API Key" reference="sources.curseforge"/>
        <SecretField label="Nexus API Key" reference="sources.nexus"/>
        <SecretField label="3DM API Key" reference="sources.3dm"/>
        <SecretField label="Brave Search API Key" reference="sources.brave"/>
      </FieldGroup>
    </CardContent>
  </Card>;
}

function McpSettings() {
  const configs = useBackendQuery<McpConfig[]>(["mcp", "configs"], "mcp_list");
  const connections = useBackendQuery<Connection[]>(["mcp", "connections"], "mcp_connections");
  const local = useBackendQuery<LocalStatus>(["mcp", "local"], "mcp_server_status");
  const [edit, setEdit] = useState<McpConfig | null>(null);
  const [fresh, setFresh] = useState(false);
  const [confirm, setConfirm] = useState<McpConfig | "local" | null>(null);
  const [acknowledged, setAcknowledged] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  const client = useQueryClient();
  async function perform(action: () => Promise<unknown>) {
    setBusy(true); setError(null);
    try {await action(); await client.invalidateQueries({queryKey: ["mcp"]}); setConfirm(null); notify("MCP 配置已更新");}
    catch (failure) {setError(failure);} finally {setBusy(false);}
  }
  const connected = (id: string) => connections.data?.some((connection) => (connection.id || connection.serverId || connection.server_id) === id);
  return <div className="flex flex-col gap-6">
    <Card>
      <CardHeader>
        <CardTitle>外部 MCP 连接</CardTitle>
        <CardDescription>连接第三方 MCP 服务，扩展 Agent 可用的工具。</CardDescription>
        <CardAction><Button variant="outline" disabled={!backendAvailable} onClick={() => {setFresh(true); setEdit({id: `custom_${Date.now()}`, name: "自定义 MCP", transport: "streamable_http", command: null, args: [], url: "https://", env_secret_refs: {}, bearer_secret_ref: null, enabled: false, builtin: false});}}><PlusIcon data-icon="inline-start"/>添加连接</Button></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <ErrorState error={configs.error || connections.error || local.error || error}/>
        {!configs.data?.length && !configs.isLoading && !configs.error && <EmptyState icon={<ServerIcon/>} title="尚未配置外部 MCP 连接"/>}
        {(configs.data || []).map((config) => <div className="flex flex-col gap-3" key={config.id}>
          <Item variant="outline">
            <ItemMedia variant="icon"><ServerIcon/></ItemMedia>
            <ItemContent className="min-w-0"><ItemTitle>{config.name}</ItemTitle><ItemDescription className="line-clamp-none"><PathText>{config.url || [config.command, ...config.args].join(" ")}</PathText></ItemDescription></ItemContent>
            <ItemActions>
              <Badge tone={connected(config.id) ? "success" : "neutral"}>{connected(config.id) ? "已连接" : config.enabled ? "未连接" : "已停用"}</Badge>
              <IconButton title={`编辑 ${config.name}`} onClick={() => {setFresh(false); setEdit(config);}}><PencilIcon/></IconButton>
              {connected(config.id)
                ? <IconButton title={`断开 ${config.name}`} disabled={busy} onClick={() => {void perform(() => api("mcp_disconnect", {id: config.id}));}}><SquareIcon/></IconButton>
                : <IconButton title={`连接 ${config.name}`} disabled={busy || !config.enabled} onClick={() => {setAcknowledged(false); setConfirm(config);}}><LinkIcon/></IconButton>}
            </ItemActions>
          </Item>
          {[...new Set([config.bearer_secret_ref, ...Object.values(config.env_secret_refs)].filter((reference): reference is string => !!reference))].map((reference) => <SecretField key={reference} label={`${config.name} 密钥`} reference={reference}/>)}
        </div>)}
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>本地 MCP 服务</CardTitle>
        <CardDescription>让外部 Agent 通过受控工具使用本机的 Agent Mod Manager。</CardDescription>
        <CardAction><Badge tone={local.data?.running ? "success" : "neutral"}>{local.data?.stopping ? "正在等待事务完成" : local.data?.running ? "运行中" : "未开启"}</Badge></CardAction>
      </CardHeader>
      <CardContent className="flex flex-col items-start gap-3">
        {local.data?.endpoint && <PathText>{local.data.endpoint}</PathText>}
        {local.data?.running
          ? <Button variant="outline" disabled={busy} onClick={() => {void perform(() => api("mcp_server_stop"));}}><SquareIcon data-icon="inline-start"/>停止服务</Button>
          : <Button variant="outline" disabled={!backendAvailable || busy || local.data?.stopping} onClick={() => {setAcknowledged(false); setConfirm("local");}}><PlayIcon data-icon="inline-start"/>开启服务</Button>}
        {local.data?.running && <p className="text-sm text-muted-foreground">加密令牌引用：{local.data.tokenRef}。活动调用 {local.data.activeCalls}。</p>}
      </CardContent>
    </Card>

    {edit && <McpEditor config={edit} fresh={fresh} onClose={() => setEdit(null)} onSaved={async () => {setEdit(null); await client.invalidateQueries({queryKey: ["mcp"]});}}/>}
    {confirm && <Modal title={confirm === "local" ? "开启本地 MCP 服务" : "确认外部 MCP 连接"} onClose={() => {if (!busy) setConfirm(null);}} className="sm:max-w-xl" actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setConfirm(null)}>取消</Button>
      <Button disabled={busy || (confirm !== "local" && confirm.transport === "stdio" && !acknowledged)} onClick={() => {void perform(() => confirm === "local" ? api("mcp_server_start", {confirmation: "START_LOCAL_MCP"}) : api("mcp_connect", {expectedConfig: confirm, acceptsUnsandboxedStdio: acknowledged}));}}><CheckIcon data-icon="inline-start"/>确认连接</Button>
    </>}>
      {confirm === "local" ? <p className="text-sm">允许持有本机令牌的外部 Agent 调用受控工具。文件写入沿用桌面权限策略，审批只能在桌面完成。</p> : <>
        <pre className="max-h-72 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-all whitespace-pre-wrap">{JSON.stringify(confirm, null, 2)}</pre>
        {confirm.transport === "stdio" && <Label className="items-start font-normal"><Checkbox checked={acknowledged} onCheckedChange={(checked) => setAcknowledged(checked === true)}/>我信任此进程。它具有当前用户的操作系统权限，不受本地路径与“不删除”规则保护。</Label>}
      </>}
      <ErrorState error={error}/>
    </Modal>}
  </div>;
}

function McpEditor({config, fresh, onClose, onSaved}: {config: McpConfig; fresh: boolean; onClose: () => void; onSaved: () => Promise<void>}) {
  const [draft, setDraft] = useState(config);
  const [env, setEnv] = useState(JSON.stringify(config.env_secret_refs, null, 2));
  const [busy, setBusy] = useState(false);
  const [remove, setRemove] = useState(false);
  const [error, setError] = useState<unknown>(null);
  async function save() {
    setBusy(true); setError(null);
    try {
      if (remove) {await api("mcp_remove", {id: draft.id});}
      else {
        const next = {...draft, command: draft.transport === "stdio" ? draft.command : null, args: draft.transport === "stdio" ? draft.args : [], url: draft.transport === "streamable_http" ? draft.url : null, env_secret_refs: draft.transport === "stdio" ? JSON.parse(env) : {}, bearer_secret_ref: draft.transport === "streamable_http" ? draft.bearer_secret_ref || null : null};
        await api(fresh ? "mcp_create" : "mcp_update", {config: next});
      }
      await onSaved();
    } catch (failure) {setError(failure);} finally {setBusy(false);}
  }
  return <Modal title={remove ? "移除 MCP 配置" : fresh ? "添加 MCP 连接" : "编辑 MCP 连接"} onClose={() => {if (!busy) onClose();}} className="sm:max-w-xl" actions={<>
    {!fresh && !remove && <IconButton title="移除配置" className="mr-auto" onClick={() => setRemove(true)}><Trash2Icon/></IconButton>}
    <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
    <Button variant={remove ? "destructive" : "default"} disabled={busy} onClick={() => {void save();}}>{busy ? <Spinner data-icon="inline-start"/> : <SaveIcon data-icon="inline-start"/>}{remove ? "确认移除" : "保存配置"}</Button>
  </>}>
    {remove ? <p className="text-sm">移除 {draft.name} 的连接配置并断开连接。本地加密凭据仍会保留。</p> : <FieldGroup>
      <Field><FieldLabel htmlFor="mcp-name">名称</FieldLabel><Input id="mcp-name" value={draft.name} onChange={(e) => setDraft({...draft, name: e.target.value})}/></Field>
      <Field>
        <FieldLabel htmlFor="mcp-transport">传输</FieldLabel>
        <NativeSelect id="mcp-transport" className="w-full" value={draft.transport} onChange={(e) => setDraft({...draft, transport: e.target.value as McpConfig["transport"]})}>
          <NativeSelectOption value="streamable_http">Streamable HTTP</NativeSelectOption><NativeSelectOption value="stdio">stdio</NativeSelectOption>
        </NativeSelect>
      </Field>
      {draft.transport === "streamable_http" ? <>
        <Field><FieldLabel htmlFor="mcp-url">URL</FieldLabel><Input id="mcp-url" type="url" value={draft.url || ""} onChange={(e) => setDraft({...draft, url: e.target.value})}/></Field>
        <Field><FieldLabel htmlFor="mcp-bearer">Bearer 密钥引用</FieldLabel><Input id="mcp-bearer" placeholder="mcp.custom.api_key" value={draft.bearer_secret_ref || ""} onChange={(e) => setDraft({...draft, bearer_secret_ref: e.target.value})}/></Field>
      </> : <>
        <Field><FieldLabel htmlFor="mcp-command">命令</FieldLabel><Input id="mcp-command" value={draft.command || ""} onChange={(e) => setDraft({...draft, command: e.target.value})}/></Field>
        <Field><FieldLabel htmlFor="mcp-args">参数，每行一项</FieldLabel><Textarea id="mcp-args" rows={3} value={draft.args.join("\n")} onChange={(e) => setDraft({...draft, args: e.target.value ? e.target.value.split("\n") : []})}/></Field>
        <Field><FieldLabel htmlFor="mcp-env">环境变量与密钥引用 JSON</FieldLabel><Textarea id="mcp-env" rows={4} className="font-mono text-xs" value={env} onChange={(e) => setEnv(e.target.value)}/></Field>
      </>}
      <Label className="font-normal"><Checkbox checked={draft.enabled} onCheckedChange={(checked) => setDraft({...draft, enabled: checked === true})}/>启用此配置</Label>
    </FieldGroup>}
    <ErrorState error={error}/>
  </Modal>;
}

function AppearanceSettings() {
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const navigate = useUI((s) => s.navigate);
  const bootstrap = useBackendQuery<{dataRoot: string}>(["bootstrap"], "get_bootstrap_state");
  return <div className="flex flex-col gap-6">
    <Card>
      <CardHeader><CardTitle>外观</CardTitle><CardDescription>选择界面主题，设置会保存在本机。</CardDescription></CardHeader>
      <CardContent>
        <ToggleGroup type="single" variant="outline" aria-label="主题" value={theme} onValueChange={(value) => {if (value && value !== theme) toggleTheme();}}>
          <ToggleGroupItem value="light" aria-label="亮色"><SunIcon/>亮色</ToggleGroupItem>
          <ToggleGroupItem value="dark" aria-label="暗色"><MoonIcon/>暗色</ToggleGroupItem>
        </ToggleGroup>
      </CardContent>
    </Card>
    <Card>
      <CardHeader><CardTitle>存储</CardTitle><CardDescription>管理器数据（数据库、快照、回收站、下载）所在目录。</CardDescription></CardHeader>
      <CardContent className="flex flex-col items-start gap-4">
        <PathText>{bootstrap.data?.dataRoot || "未连接桌面服务"}</PathText>
        <Button variant="outline" disabled={!backendAvailable || !bootstrap.data} onClick={() => navigate("onboarding")}><RotateCcwIcon data-icon="inline-start"/>重新查看新手引导</Button>
        <ErrorState error={bootstrap.error}/>
      </CardContent>
    </Card>
  </div>;
}

type SkillInfo = {name: string; description: string; location: string; source: string; enabled: boolean; kind: string; valid: boolean; issues: string[]};
function SkillsSettings() {
  const skills = useBackendQuery<SkillInfo[]>(["skills"], "list_skills");
  const [importing, setImporting] = useState(false);
  const [name, setName] = useState("");
  const [markdown, setMarkdown] = useState("");
  const [selected, setSelected] = useState<{skill: SkillInfo; markdown: string} | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const reviewPlan = useUI((s) => s.reviewPlan);
  const client = useQueryClient();
  async function inspect(name: string) {
    setBusy(true); setError(null);
    try {setSelected(await api("get_skill", {name}));}
    catch (failure) {setError(failure);} finally {setBusy(false);}
  }
  async function install() {
    setBusy(true); setError(null);
    try {
      const preview = await api<{plan: FilePlan}>("plan_install_skill", {name, markdown});
      setImporting(false);
      reviewPlan(preview.plan, () => {setName(""); setMarkdown(""); void client.invalidateQueries({queryKey: ["skills"]});});
    } catch (failure) {setError(failure);} finally {setBusy(false);}
  }
  async function uninstall(skill: SkillInfo) {
    setBusy(true); setError(null);
    try {
      const preview = await api<{plan: FilePlan}>("plan_uninstall_skill", {name: skill.name});
      setSelected(null);
      reviewPlan(preview.plan, () => {
        void client.invalidateQueries({queryKey: ["skills"]});
        void client.invalidateQueries({queryKey: ["trash"]});
      });
    } catch (failure) {setError(failure);} finally {setBusy(false);}
  }
  return <Card>
    <CardHeader>
      <CardTitle>Skills</CardTitle>
      <CardDescription>声明式 Skill 为 Agent 提供游戏相关的操作说明。</CardDescription>
      <CardAction><Button variant="outline" disabled={!backendAvailable || busy} onClick={() => {setError(null); setImporting(true);}}><PlusIcon data-icon="inline-start"/>导入 Skill</Button></CardAction>
    </CardHeader>
    <CardContent className="flex flex-col gap-4">
      <ErrorState error={skills.error || error}/>
      <div className="rounded-md border">
        <Table>
          <TableHeader><TableRow><TableHead>名称</TableHead><TableHead>来源</TableHead><TableHead>状态</TableHead><TableHead>启用</TableHead></TableRow></TableHeader>
          <TableBody>{skills.data?.map((skill) => <TableRow key={skill.name}>
            <TableCell className="whitespace-normal">
              <Button variant="link" className="h-auto p-0 font-medium" onClick={() => {void inspect(skill.name);}} disabled={busy || !skill.valid}>{skill.name}</Button>
              <p className="text-xs text-muted-foreground">{skill.description}</p>
              {skill.issues.map((issue) => <p className="text-xs text-warning" key={issue}>{issue}</p>)}
            </TableCell>
            <TableCell>{skill.kind}</TableCell>
            <TableCell><Badge tone={skill.valid ? "success" : "warning"}>{skill.valid ? "已校验" : "不可用"}</Badge></TableCell>
            <TableCell><Checkbox aria-label={`启用 ${skill.name}`} checked={skill.enabled} disabled={busy || !backendAvailable || (!skill.valid && !skill.enabled)} onCheckedChange={async (checked) => {
              const enabled = checked === true; setBusy(true); setError(null);
              try {await api("set_skill_enabled", {name: skill.name, enabled}); await skills.refetch();}
              catch (failure) {setError(failure);} finally {setBusy(false);}
            }}/></TableCell>
          </TableRow>)}</TableBody>
        </Table>
      </div>
    </CardContent>
    {selected && <Modal title={selected.skill.name} onClose={() => {if (!busy) setSelected(null);}} actions={<>
      {selected.skill.kind === "standalone" && <IconButton title={`预览卸载 Skill ${selected.skill.name}`} className="mr-auto" disabled={!backendAvailable || busy || !selected.skill.valid} onClick={() => {void uninstall(selected.skill);}}><Trash2Icon/></IconButton>}
      <Button variant="outline" disabled={busy} onClick={() => setSelected(null)}>关闭</Button>
    </>}>
      <pre className="max-h-96 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-words whitespace-pre-wrap">{selected.markdown}</pre><ErrorState error={error}/>
    </Modal>}
    {importing && <Modal title="导入声明式 Skill" onClose={() => {if (!busy) setImporting(false);}} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setImporting(false)}>取消</Button>
      <Button disabled={busy || !name.trim() || !markdown.trim()} onClick={() => {void install();}}>预览文件计划</Button>
    </>}>
      <FieldGroup>
        <Field><FieldLabel htmlFor="skill-name">名称</FieldLabel><Input id="skill-name" aria-label="Skill 名称" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-game-guide"/></Field>
        <Field><FieldLabel htmlFor="skill-md">SKILL.md</FieldLabel><Textarea id="skill-md" aria-label="SKILL.md" rows={12} className="font-mono text-xs" value={markdown} onChange={(event) => setMarkdown(event.target.value)}/><FieldDescription>粘贴完整的 SKILL.md（含 frontmatter）。</FieldDescription></Field>
      </FieldGroup>
      <ErrorState error={error}/>
    </Modal>}
  </Card>;
}