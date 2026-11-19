import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, KeyRound, Link, Moon, Pencil, Play, Plus, Save, Server, Square, Sun, Trash2 } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "../lib/api";
import { useUI } from "../lib/store";
import type { AppSettings, FilePlan, PermissionMode } from "../lib/types";
import { Badge, Button, ErrorState, IconButton, Modal, PageHeader } from "../components/ui";

type McpConfig = {id: string; name: string; transport: "stdio" | "streamable_http"; command: string | null; args: string[]; url: string | null; env_secret_refs: Record<string, string>; bearer_secret_ref: string | null; enabled: boolean; builtin: boolean};
type LocalStatus = {running: boolean; stopping: boolean; endpoint: string | null; tokenRef: string; activeCalls: number};
type Connection = {id: string; serverId?: string; server_id?: string; toolCount?: number; tool_count?: number};

export default function SettingsPage() {
  const [tab, setTab] = useState("model");
  return <div className="page settings-page"><PageHeader title="设置"/>
    <div className="tabs" style={{maxWidth: "100%", overflowX: "auto"}} role="tablist" aria-label="设置分类">{[["model", "AI 与模型"], ["sources", "Mod 来源"], ["mcp", "MCP 服务"], ["skills", "Skills"], ["appearance", "外观与存储"]].map(([id, name]) => <button role="tab" aria-selected={tab === id} className={tab === id ? "active" : ""} onClick={() => setTab(id)} key={id}>{name}</button>)}</div>
    {tab === "model" && <ModelSettings/>}{tab === "sources" && <SourceSettings/>}{tab === "mcp" && <McpSettings/>}{tab === "skills" && <SkillsSettings/>}{tab === "appearance" && <AppearanceSettings/>}
  </div>;
}

function ModelSettings() {
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const [draft, setDraft] = useState<AppSettings | null>(null);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  const client = useQueryClient();
  useEffect(() => {if (settings.data) setDraft(settings.data);}, [settings.data]);
  async function save() {
    if (!draft) return;
    setBusy(true); setError(null);
    try {
      const next = {...draft, api_key_ref: key ? "model.api_key" : draft.api_key_ref};
      if (key) {await api("set_secret", {key: "model.api_key", value: key}); setKey("");}
      await api("set_settings", {settings: next});
      await client.invalidateQueries({queryKey: ["settings"]}); notify("模型设置已保存");
    } catch (failure) {setError(failure);}
    finally {setBusy(false);}
  }
  return <section className="page-section"><h2>模型服务</h2><ErrorState error={settings.error || error}/>
    <form className="settings-form" onSubmit={(event) => {event.preventDefault(); void save();}}>
      <label className="field">API 地址<input aria-label="API 地址" type="url" placeholder="https://api.example.com/v1" value={draft?.base_url || ""} disabled={!draft || busy} onChange={(event) => setDraft((value) => value && ({...value, base_url: event.target.value}))} required/></label>
      <label className="field">模型名称<input aria-label="模型名称" value={draft?.model || ""} disabled={!draft || busy} onChange={(event) => setDraft((value) => value && ({...value, model: event.target.value}))} required/></label>
      <label className="field">API 密钥<input aria-label="API 密钥" type="password" autoComplete="new-password" value={key} disabled={!draft || busy} placeholder={draft?.api_key_ref ? "已加密保存，留空保留" : "可选，使用本地加密存储"} onChange={(event) => setKey(event.target.value)}/></label>
      <label className="field">上下文预算<input aria-label="上下文预算" type="number" min={1} max={16777216} step={1} value={draft?.context_budget || 32768} disabled={!draft || busy} onChange={(event) => setDraft((value) => value && ({...value, context_budget: Number(event.target.value)}))} required/></label>
      <label className="field">默认权限策略<select aria-label="默认权限策略" value={draft?.permission_mode || "request_approval"} disabled={!draft || busy} onChange={(event) => setDraft((value) => value && ({...value, permission_mode: event.target.value as PermissionMode}))}><option value="request_approval">逐次审批</option><option value="auto_approve">已登记游戏目录内自动批准</option><option value="full_trust">完全信任</option></select></label>
      {draft?.permission_mode === "full_trust" && <p className="warning-text">完全信任会跳过工具确认，包括外部服务写入与发布。路径限制仍生效；第三方 stdio 进程不受本地文件引擎约束。</p>}
      <div><Button type="submit" className="primary" disabled={!backendAvailable || !draft || busy}><Save size={16}/>保存设置</Button></div>
    </form>
  </section>;
}

function SecretField({label, reference}: {label: string; reference: string}) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const notify = useUI((s) => s.notify);
  const client = useQueryClient();
  return <form className="secret-field" onSubmit={async (event) => {
    event.preventDefault(); if (!value.trim()) return;
    setBusy(true); setError(null);
    try {await api("set_secret", {key: reference, value}); setValue(""); await client.invalidateQueries({queryKey: ["sources"]}); notify(`${label}已保存`);}
    catch (failure) {setError(failure);} finally {setBusy(false);}
  }}><label className="field">{label}<div className="toolbar"><input type="password" autoComplete="new-password" aria-label={label} value={value} onChange={(event) => setValue(event.target.value)} disabled={!backendAvailable || busy} placeholder={reference}/><IconButton type="submit" title={`保存${label}`} disabled={!backendAvailable || busy || !value.trim()}><KeyRound size={16}/></IconButton></div></label><ErrorState error={error}/></form>;
}

function SourceSettings() {
  const sources = useBackendQuery<Array<{source_id: string; state: string; message: string}>>(["sources"], "list_mod_sources");
  return <section className="page-section"><h2>来源凭据</h2><ErrorState error={sources.error}/>
    <div className="source-status-list">{sources.data?.map((source) => <div className="list-row" key={source.source_id}><strong>{source.source_id}</strong><Badge tone={source.state === "available" ? "success" : "warning"}>{source.state}</Badge><span className="muted">{source.message}</span></div>)}</div>
    <SecretField label="CurseForge API Key" reference="sources.curseforge"/><SecretField label="Nexus API Key" reference="sources.nexus"/><SecretField label="3DM API Key" reference="sources.3dm"/>
    <SecretField label="Brave Search API Key" reference="sources.brave"/>
    <p className="muted">Agent 的通用网页搜索使用 Brave Search。未配置时会提示配置，Mod 来源搜索仍可使用。</p>
  </section>;
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
  return <section className="page-section"><div className="section-heading"><h2>外部 MCP 连接</h2><Button disabled={!backendAvailable} onClick={() => {setFresh(true); setEdit({id: `custom_${Date.now()}`, name: "自定义 MCP", transport: "streamable_http", command: null, args: [], url: "https://", env_secret_refs: {}, bearer_secret_ref: null, enabled: false, builtin: false});}}><Plus size={16}/>添加连接</Button></div>
    <ErrorState error={configs.error || connections.error || local.error || error}/>
    {(configs.data || []).map((config) => <div className="mcp-row" key={config.id}><div className="list-row"><Server size={21}/><div className="grow"><strong>{config.name}</strong><p className="path-text">{config.url || [config.command, ...config.args].join(" ")}</p></div><Badge tone={connected(config.id) ? "success" : "neutral"}>{connected(config.id) ? "已连接" : config.enabled ? "未连接" : "已停用"}</Badge><IconButton title={`编辑 ${config.name}`} onClick={() => {setFresh(false); setEdit(config);}}><Pencil size={16}/></IconButton>
      {connected(config.id) ? <IconButton title={`断开 ${config.name}`} disabled={busy} onClick={() => {void perform(() => api("mcp_disconnect", {id: config.id}));}}><Square size={16}/></IconButton> : <IconButton title={`连接 ${config.name}`} disabled={busy || !config.enabled} onClick={() => {setAcknowledged(false); setConfirm(config);}}><Link size={16}/></IconButton>}</div>
      {[...new Set([config.bearer_secret_ref, ...Object.values(config.env_secret_refs)].filter((reference): reference is string => !!reference))].map((reference) => <SecretField key={reference} label={`${config.name} 密钥`} reference={reference}/>)}</div>)}
    <section className="page-section"><div className="section-heading"><h2>本地 MCP 服务</h2><Badge tone={local.data?.running ? "success" : "neutral"}>{local.data?.stopping ? "正在等待事务完成" : local.data?.running ? "运行中" : "未开启"}</Badge></div>
      {local.data?.endpoint && <p className="path-text">{local.data.endpoint}</p>}
      {local.data?.running ? <Button disabled={busy} onClick={() => {void perform(() => api("mcp_server_stop"));}}><Square size={16}/>停止服务</Button> : <Button disabled={!backendAvailable || busy || local.data?.stopping} onClick={() => {setAcknowledged(false); setConfirm("local");}}><Play size={16}/>开启服务</Button>}
      {local.data?.running && <p className="muted">加密令牌引用：{local.data.tokenRef}。活动调用 {local.data.activeCalls}。</p>}
    </section>
    {edit && <McpEditor config={edit} fresh={fresh} onClose={() => setEdit(null)} onSaved={async () => {setEdit(null); await client.invalidateQueries({queryKey: ["mcp"]});}}/>}
    {confirm && <Modal title={confirm === "local" ? "开启本地 MCP 服务" : "确认外部 MCP 连接"} onClose={() => {if (!busy) setConfirm(null);}} actions={<><Button disabled={busy} onClick={() => setConfirm(null)}>取消</Button><Button className="primary" disabled={busy || (confirm !== "local" && confirm.transport === "stdio" && !acknowledged)} onClick={() => {void perform(() => confirm === "local" ? api("mcp_server_start", {confirmation: "START_LOCAL_MCP"}) : api("mcp_connect", {expectedConfig: confirm, acceptsUnsandboxedStdio: acknowledged}));}}><Check size={16}/>确认连接</Button></>}>
      {confirm === "local" ? <p>允许持有本机令牌的外部 Agent 调用受控工具。文件写入沿用桌面权限策略，审批只能在桌面完成。</p> : <><pre className="json-preview">{JSON.stringify(confirm, null, 2)}</pre>{confirm.transport === "stdio" && <label className="checkbox-row"><input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)}/>我信任此进程。它具有当前用户的操作系统权限，不受本地路径与“不删除”规则保护。</label>}</>}
      <ErrorState error={error}/>
    </Modal>}
  </section>;
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
  return <Modal title={remove ? "移除 MCP 配置" : fresh ? "添加 MCP 连接" : "编辑 MCP 连接"} onClose={() => {if (!busy) onClose();}} actions={<>{!fresh && !remove && <IconButton title="移除配置" onClick={() => setRemove(true)}><Trash2 size={16}/></IconButton>}<Button disabled={busy} onClick={onClose}>取消</Button><Button className={remove ? "danger" : "primary"} disabled={busy} onClick={() => {void save();}}><Save size={16}/>{remove ? "确认移除" : "保存配置"}</Button></>}>
    {remove ? <p>移除 {draft.name} 的连接配置并断开连接。本地加密凭据仍会保留。</p> : <div className="settings-form"><label className="field">名称<input value={draft.name} onChange={(e) => setDraft({...draft, name: e.target.value})}/></label><label className="field">传输<select value={draft.transport} onChange={(e) => setDraft({...draft, transport: e.target.value as McpConfig["transport"]})}><option value="streamable_http">Streamable HTTP</option><option value="stdio">stdio</option></select></label>
      {draft.transport === "streamable_http" ? <><label className="field">URL<input type="url" value={draft.url || ""} onChange={(e) => setDraft({...draft, url: e.target.value})}/></label><label className="field">Bearer 密钥引用<input placeholder="mcp.custom.api_key" value={draft.bearer_secret_ref || ""} onChange={(e) => setDraft({...draft, bearer_secret_ref: e.target.value})}/></label></> : <><label className="field">命令<input value={draft.command || ""} onChange={(e) => setDraft({...draft, command: e.target.value})}/></label><label className="field">参数，每行一项<textarea rows={3} value={draft.args.join("\n")} onChange={(e) => setDraft({...draft, args: e.target.value ? e.target.value.split("\n") : []})}/></label><label className="field">环境变量与密钥引用 JSON<textarea rows={4} value={env} onChange={(e) => setEnv(e.target.value)}/></label></>}
      <label className="checkbox-row"><input type="checkbox" checked={draft.enabled} onChange={(e) => setDraft({...draft, enabled: e.target.checked})}/>启用此配置</label></div>}
    <ErrorState error={error}/>
  </Modal>;
}

function AppearanceSettings() {
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const bootstrap = useBackendQuery<{dataRoot: string}>(["bootstrap"], "get_bootstrap_state");
  return <section className="page-section"><h2>外观</h2><div className="segmented" role="group" aria-label="主题"><Button className={theme === "light" ? "active" : ""} onClick={() => {if (theme !== "light") toggleTheme();}}><Sun size={16}/>亮色</Button><Button className={theme === "dark" ? "active" : ""} onClick={() => {if (theme !== "dark") toggleTheme();}}><Moon size={16}/>暗色</Button></div><h2>存储</h2><p className="path-text">{bootstrap.data?.dataRoot || "未连接桌面服务"}</p><ErrorState error={bootstrap.error}/></section>;
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
  return <section className="page-section"><div className="section-heading"><h2>Skills</h2><Button disabled={!backendAvailable || busy} onClick={() => {setError(null); setImporting(true);}}><Plus size={16}/>导入 Skill</Button></div>
    <ErrorState error={skills.error || error}/><div className="table-wrap"><table><thead><tr><th>名称</th><th>来源</th><th>状态</th><th>启用</th></tr></thead><tbody>{skills.data?.map((skill) => <tr key={skill.name}><td><button className="skill-name" onClick={() => {void inspect(skill.name);}} disabled={busy || !skill.valid}>{skill.name}</button><small>{skill.description}</small>{skill.issues.map((issue) => <small className="warning-text" key={issue}>{issue}</small>)}</td><td>{skill.kind}</td><td><Badge tone={skill.valid ? "success" : "warning"}>{skill.valid ? "已校验" : "不可用"}</Badge></td><td><input type="checkbox" aria-label={`启用 ${skill.name}`} checked={skill.enabled} disabled={busy || !backendAvailable || (!skill.valid && !skill.enabled)} onChange={async (event) => {
      const enabled = event.target.checked; setBusy(true); setError(null);
      try {await api("set_skill_enabled", {name: skill.name, enabled}); await skills.refetch();}
      catch (failure) {setError(failure);} finally {setBusy(false);}
    }}/></td></tr>)}</tbody></table></div>
    {selected && <Modal title={selected.skill.name} onClose={() => {if (!busy) setSelected(null);}} actions={<>{selected.skill.kind === "standalone" && <IconButton title={`预览卸载 Skill ${selected.skill.name}`} disabled={!backendAvailable || busy || !selected.skill.valid} onClick={() => {void uninstall(selected.skill);}}><Trash2 size={16}/></IconButton>}<Button disabled={busy} onClick={() => setSelected(null)}>关闭</Button></>}><pre className="json-preview">{selected.markdown}</pre><ErrorState error={error}/></Modal>}
    {importing && <Modal title="导入声明式 Skill" onClose={() => {if (!busy) setImporting(false);}} actions={<><Button disabled={busy} onClick={() => setImporting(false)}>取消</Button><Button className="primary" disabled={busy || !name.trim() || !markdown.trim()} onClick={() => {void install();}}>预览文件计划</Button></>}>
      <label className="field">名称<input aria-label="Skill 名称" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-game-guide"/></label><label className="field">SKILL.md<textarea aria-label="SKILL.md" rows={12} value={markdown} onChange={(event) => setMarkdown(event.target.value)}/></label><ErrorState error={error}/>
    </Modal>}
  </section>;
}
