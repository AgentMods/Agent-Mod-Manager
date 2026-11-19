import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { CheckCircle2, FileCode2, Package, Plus, RefreshCw, Search, ShieldCheck, Upload } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '../lib/api';
import { useUI } from '../lib/store';
import type { FilePlan } from '../lib/types';
import { Badge, Button, date, EmptyState, ErrorState, IconButton, Modal, PageHeader } from '../components/ui';
import { PluginPublications } from '../components/PluginPublications';

type Plugin = { slug: string; name: string; gameName: string; version: string; description: string; source: string; enabled: boolean; validated: boolean; installPath: string };
type Draft = { id: string; slug: string; path: string; writeTransactionId: string; createdAt: string };
type Validation = { draft: Draft; transactionStatus: string; valid: boolean; package: { adapter: { name: string; version: string; platforms: string[]; modTypes: { id: string; name: string }[] }; skill: { description: string } } | null; hashes: { adapterSha256: string; skillSha256: string } | null; issues: { code: string; message: string }[]; warnings: string[] };

export default function PluginsPage() {
  const client = useQueryClient();
  const plugins = useBackendQuery<Plugin[]>(['plugins'], 'list_plugins');
  const drafts = useBackendQuery<Draft[]>(['plugin-drafts'], 'list_plugin_drafts');
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const reviewPlan = useUI(s => s.reviewPlan);
  const [tab, setTab] = useState('installed');
  const [search, setSearch] = useState('');
  const [selectedSlug, setSelectedSlug] = useState<string | null>(null);
  const [draftOpen, setDraftOpen] = useState(false);
  const [publishDraft, setPublishDraft] = useState<Draft | null>(null);
  const [slug, setSlug] = useState('');
  const [adapterJson, setAdapterJson] = useState('');
  const [skillMarkdown, setSkillMarkdown] = useState('');
  const [validation, setValidation] = useState<Validation | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const selected = plugins.data?.find(plugin => plugin.slug === selectedSlug);
  const filtered = (plugins.data ?? []).filter(plugin => `${plugin.name} ${plugin.gameName} ${plugin.slug}`.toLowerCase().includes(search.toLowerCase()));
  async function setEnabled(plugin: Plugin, enabled: boolean) {
    if (!writable || busy) return;
    setBusy(true); setError(null);
    try {
      const updated = await api<Plugin>('set_plugin_enabled', { slug: plugin.slug, enabled });
      client.setQueryData<Plugin[]>(['plugins'], current => current?.map(item => item.slug === updated.slug ? updated : item));
      await Promise.all([
        client.invalidateQueries({ queryKey: ['plugins'] }),
        client.invalidateQueries({ queryKey: ['skills'] }),
      ]);
    } catch (e) { setError(e); } finally { setBusy(false); }
  }
  async function writeDraft() {
    if (!writable) return;
    setBusy(true); setError(null);
    try {
      const result = await api<{ draft: Draft; plan: FilePlan }>('write_plugin_draft', { slug: slug.trim(), adapterJson, skillMarkdown });
      setDraftOpen(false);
      reviewPlan(result.plan, () => { void drafts.refetch(); setTab('drafts'); });
    } catch (e) { setError(e); } finally { setBusy(false); }
  }
  async function validate(draft: Draft) {
    setBusy(true); setError(null); setValidation(null);
    try { setValidation(await api<Validation>('validate_plugin', { draftId: draft.id })); }
    catch (e) { setError(e); } finally { setBusy(false); }
  }
  async function install() {
    if (!validation?.valid || !writable) return;
    setBusy(true); setError(null);
    try {
      const result = await api<{ plan: FilePlan }>('install_plugin', { draftId: validation.draft.id });
      setValidation(null);
      reviewPlan(result.plan, () => { void plugins.refetch(); void drafts.refetch(); });
    } catch (e) { setError(e); } finally { setBusy(false); }
  }
  return <div className="page plugins-page" data-testid="plugins-page">
    <PageHeader title="适配包" subtitle={backendAvailable ? `${plugins.data?.length ?? 0} 个已登记适配包` : '桌面能力未连接'} actions={<><IconButton title="刷新适配包" disabled={!backendAvailable || busy} onClick={() => { void plugins.refetch(); void drafts.refetch(); }}><RefreshCw size={16}/></IconButton><Button className="primary" disabled={!writable || busy} onClick={() => { setDraftOpen(true); setError(null); }}><Plus size={16}/>新建草稿</Button></>}/>
    <div className="tabs" role="tablist"><button role="tab" aria-selected={tab === 'installed'} className={tab === 'installed' ? 'active' : ''} onClick={() => setTab('installed')}>已安装</button><button role="tab" aria-selected={tab === 'drafts'} className={tab === 'drafts' ? 'active' : ''} onClick={() => setTab('drafts')}>草稿 {drafts.data?.length ?? 0}</button><button role="tab" aria-selected={tab === 'publications'} className={tab === 'publications' ? 'active' : ''} onClick={() => setTab('publications')}>发布记录</button></div>
    <ErrorState error={plugins.error}/><ErrorState error={drafts.error}/><ErrorState error={error}/>
    {tab === 'installed' && <>
      <div className="toolbar"><label className="search-field"><Search size={16}/><input placeholder="搜索适配包…" aria-label="搜索适配包" value={search} onChange={e => setSearch(e.target.value)}/></label></div>
      <div className="split-layout"><section><div className="table-wrap"><table><thead><tr><th>适配包</th><th>版本</th><th>来源</th><th>验证</th><th>状态</th></tr></thead><tbody>{filtered.map(plugin => <tr key={plugin.slug} className={`row-clickable ${selectedSlug === plugin.slug ? 'selected' : ''}`} onClick={() => setSelectedSlug(plugin.slug)}>
        <td><button className="table-select" onClick={() => setSelectedSlug(plugin.slug)}><Package size={19}/><span><strong>{plugin.name}</strong><small>{plugin.gameName}</small></span></button></td><td>{plugin.version}</td><td>{plugin.source === 'builtin' ? '内置' : plugin.source === 'external' ? '本地导入' : plugin.source}</td><td><Badge tone={plugin.validated ? 'success' : 'warning'}>{plugin.validated ? '已验证' : '未验证'}</Badge></td><td><Badge tone={plugin.enabled && plugin.validated ? 'success' : 'neutral'}>{plugin.enabled ? '已启用' : '已停用'}</Badge></td>
      </tr>)}</tbody></table></div>{!filtered.length && <EmptyState title={plugins.isLoading ? '正在读取适配包…' : search ? '没有匹配的适配包' : '暂无已登记适配包'}/>}</section>
      <aside className="inspector"><h2>{selected?.name ?? '适配包详情'}</h2>{selected ? <><p>{selected.description}</p><dl className="metadata-list" style={{ marginTop: 20 }}><dt>标识</dt><dd>{selected.slug}</dd><dt>游戏</dt><dd>{selected.gameName}</dd><dt>版本</dt><dd>{selected.version}</dd><dt>来源</dt><dd>{selected.source}</dd><dt>安装目录</dt><dd className="path-text">{selected.installPath}</dd></dl><div className="toolbar" style={{ marginTop: 24 }}><label className="checkbox-row"><input type="checkbox" data-testid="plugin-enabled-toggle" aria-label={`启用适配包 ${selected.name}`} checked={selected.enabled} disabled={busy || !writable} onChange={e => { void setEnabled(selected, e.target.checked); }}/><span>启用适配包</span></label></div></> : <p>未选择适配包</p>}</aside></div>
    </>}
    {tab === 'drafts' && <><div className="table-wrap"><table><thead><tr><th>草稿</th><th>创建时间</th><th>事务</th><th/></tr></thead><tbody>{(drafts.data ?? []).map(draft => <tr key={draft.id}>
      <td><strong>{draft.slug}</strong><small className="path-text">{draft.path}</small></td><td>{date(draft.createdAt)}</td><td className="path-text">{draft.writeTransactionId}</td>
      <td><div className="page-actions"><Button disabled={!backendAvailable || busy} onClick={() => { void validate(draft); }}><ShieldCheck size={15}/>验证</Button><Button disabled={!writable || busy} aria-label={`准备发布 ${draft.slug}`} onClick={() => setPublishDraft(draft)}><Upload size={15}/>准备发布</Button></div></td>
    </tr>)}</tbody></table></div>{!drafts.data?.length && <EmptyState title="暂无草稿" action={<Button disabled={!writable} onClick={() => setDraftOpen(true)}><Plus size={15}/>新建草稿</Button>}/>}</>}
    <PluginPublications draft={publishDraft} showHistory={tab === 'publications'} writable={writable} onDraftClose={() => setPublishDraft(null)}/>
    {draftOpen && <Modal title="声明式适配包草稿" onClose={() => { if (!busy) setDraftOpen(false); }} actions={<><Button disabled={busy} onClick={() => setDraftOpen(false)}>取消</Button><Button className="primary" disabled={!writable || busy || !slug.trim() || !adapterJson.trim() || !skillMarkdown.trim()} onClick={() => { void writeDraft(); }}><FileCode2 size={15}/>预览保存草稿</Button></>}><ErrorState error={error}/><label className="field">Slug<input value={slug} onChange={e => setSlug(e.target.value)} maxLength={100}/></label><label className="field">adapter.json<textarea className="code-editor" rows={12} spellCheck={false} value={adapterJson} onChange={e => setAdapterJson(e.target.value)}/></label><label className="field">SKILL.md<textarea className="code-editor" rows={10} spellCheck={false} value={skillMarkdown} onChange={e => setSkillMarkdown(e.target.value)}/></label></Modal>}
    {validation && <Modal title={`验证：${validation.draft.slug}`} onClose={() => { if (!busy) setValidation(null); }} actions={<><Button disabled={busy} onClick={() => setValidation(null)}>关闭</Button><Button className="primary" disabled={!writable || busy || !validation.valid} onClick={() => { void install(); }}><Package size={15}/>安装预览</Button></>}><ErrorState error={error}/><Badge tone={validation.valid ? 'success' : 'error'}>{validation.valid ? <><CheckCircle2 size={13}/>验证通过</> : '验证未通过'}</Badge><dl className="metadata-list" style={{ marginTop: 18 }}><dt>草稿事务</dt><dd>{validation.transactionStatus}</dd>{validation.package && <><dt>名称</dt><dd>{validation.package.adapter.name}</dd><dt>版本</dt><dd>{validation.package.adapter.version}</dd><dt>平台</dt><dd>{validation.package.adapter.platforms.join('、')}</dd><dt>Mod 类型</dt><dd>{validation.package.adapter.modTypes.map(type => `${type.name} (${type.id})`).join('、')}</dd></>}{validation.hashes && <><dt>Adapter SHA-256</dt><dd className="path-text">{validation.hashes.adapterSha256}</dd><dt>Skill SHA-256</dt><dd className="path-text">{validation.hashes.skillSha256}</dd></>}</dl>{validation.issues.map((issue, i) => <div className="error-state" key={i}>{issue.code}: {issue.message}</div>)}{validation.warnings.map((warning, i) => <p className="warning-text" key={i}>{warning}</p>)}</Modal>}
  </div>;
}
