import { useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { CheckCircle2Icon, FileCode2Icon, PackageIcon, PlusIcon, RefreshCwIcon, SearchIcon, ShieldCheckIcon, SparklesIcon, Trash2Icon, UploadIcon } from 'lucide-react';
import { api, backendAvailable, useBackendQuery } from '@/lib/api';
import { useUI } from '@/lib/store';
import type { FilePlan, Plugin } from '@/lib/types';
import { Badge, date, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader, PathText } from '@/components/ui';
import { GameCover } from '@/components/GameCover';
import { PluginPublications } from '@/components/PluginPublications';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { AlertDialog, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from '@/components/ui/alert-dialog';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group';
import { Label } from '@/components/ui/label';
import { Spinner } from '@/components/ui/spinner';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Textarea } from '@/components/ui/textarea';

type Draft = { id: string; slug: string; path: string; writeTransactionId: string; createdAt: string };
type Validation = { draft: Draft; transactionStatus: string; valid: boolean; package: { adapter: { name: string; version: string; platforms: string[]; modTypes: { id: string; name: string }[] }; skill: { description: string } } | null; hashes: { adapterSha256: string; skillSha256: string } | null; issues: { code: string; message: string }[]; warnings: string[] };

export default function PluginsPage() {
  const client = useQueryClient();
  const plugins = useBackendQuery<Plugin[]>(['plugins'], 'list_plugins');
  const drafts = useBackendQuery<Draft[]>(['plugin-drafts'], 'list_plugin_drafts');
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const reviewPlan = useUI(s => s.reviewPlan);
  const startAdapterChat = useUI(s => s.startAdapterChat);
  const [tab, setTab] = useState('installed');
  const [search, setSearch] = useState('');
  const [selectedSlug, setSelectedSlug] = useState<string | null>(null);
  const [skillSlug, setSkillSlug] = useState<string | null>(null);
  const [skillError, setSkillError] = useState<unknown>(null);
  const skill = useBackendQuery<{ skill: { enabled: boolean; valid: boolean }; markdown: string }>(['skills', 'package', skillSlug], 'get_skill', { name: skillSlug }, !!skillSlug);
  const [draftOpen, setDraftOpen] = useState(false);
  const [publishDraft, setPublishDraft] = useState<Draft | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Draft | null>(null);
  const deleteButton = useRef<HTMLButtonElement>(null);
  const draftsTab = useRef<HTMLButtonElement>(null);
  const [deleteError, setDeleteError] = useState<unknown>(null);
  const notify = useUI(s => s.notify);
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
  async function setGameSkillEnabled(enabled: boolean) {
    if (!skillSlug || !writable || busy) return;
    setBusy(true); setSkillError(null);
    try {
      await api('set_skill_enabled', { name: skillSlug, enabled });
      await client.invalidateQueries({ queryKey: ['skills'] });
    } catch (e) { setSkillError(e); } finally { setBusy(false); }
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
  async function deleteDraft() {
    if (!deleteTarget || !writable || busy) return;
    setBusy(true); setDeleteError(null);
    try {
      await api('delete_plugin_draft', { draftId: deleteTarget.id });
      client.setQueryData<Draft[]>(['plugin-drafts'], current => current?.filter(draft => draft.id !== deleteTarget.id));
      setDeleteTarget(null);
      notify('草稿已移入回收站');
      void client.invalidateQueries({ queryKey: ['trash'] });
      void client.invalidateQueries({ queryKey: ['plugin-drafts'] });
    } catch (e) { setDeleteError(e); } finally { setBusy(false); }
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

  const plural = (n: number) => `${n}`;
  return <Page data-testid="plugins-page">
    <PageHeader title="适配包" subtitle={backendAvailable ? `${plugins.data?.length ?? 0} 个已安装适配包` : '桌面能力未连接'} actions={<>
      <IconButton title="刷新适配包" variant="outline" size="icon" disabled={!backendAvailable || busy} onClick={() => { void plugins.refetch(); void drafts.refetch(); }}><RefreshCwIcon /></IconButton>
      <Button disabled={!writable || busy} onClick={() => startAdapterChat()}><SparklesIcon data-icon="inline-start" />用 Agent 创建</Button>
      <Button variant="outline" disabled={!writable || busy} onClick={() => { setDraftOpen(true); setError(null); }}><PlusIcon data-icon="inline-start" />新建草稿</Button>
    </>} />
    <Tabs value={tab} onValueChange={setTab} className="gap-4">
      <div className="max-w-full "><TabsList>
        <TabsTrigger value="installed">已安装</TabsTrigger>
        <TabsTrigger value="drafts" ref={draftsTab}>草稿 {plural(drafts.data?.length ?? 0)}</TabsTrigger>
        <TabsTrigger value="publications">发布记录</TabsTrigger>
      </TabsList></div>
    </Tabs>
    <ErrorState error={plugins.error} /><ErrorState error={drafts.error} /><ErrorState error={error} />
    {tab === 'installed' && <>
      <InputGroup className="max-w-sm"><InputGroupAddon><SearchIcon /></InputGroupAddon><InputGroupInput placeholder="搜索适配包…" aria-label="搜索适配包" value={search} onChange={e => setSearch(e.target.value)} /></InputGroup>
      <div className="grid min-w-0 gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <section className="min-w-0">
          <div className="rounded-lg border">
            <Table>
              <TableHeader><TableRow><TableHead>适配包</TableHead><TableHead>版本</TableHead><TableHead>来源</TableHead><TableHead>验证</TableHead><TableHead>状态</TableHead></TableRow></TableHeader>
              <TableBody>{filtered.map(plugin => <TableRow key={plugin.slug} data-state={selectedSlug === plugin.slug ? 'selected' : undefined} className="cursor-pointer" onClick={() => setSelectedSlug(plugin.slug)}>
                <TableCell>
                  <button type="button" className="flex items-center gap-3 rounded-md text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50" onClick={() => setSelectedSlug(plugin.slug)}>
                    <PackageIcon className="size-5 shrink-0 text-muted-foreground" />
                    <span className="flex flex-col"><strong className="font-medium">{plugin.name}</strong><span className="text-xs text-muted-foreground">{plugin.gameName}</span></span>
                  </button>
                </TableCell>
                <TableCell>{plugin.version}</TableCell>
                <TableCell>{plugin.source === 'builtin' ? '内置' : plugin.source === 'external' ? '本地导入' : plugin.source}</TableCell>
                <TableCell><Badge tone={plugin.validated ? 'success' : 'warning'}>{plugin.validated ? '已验证' : '未验证'}</Badge></TableCell>
                <TableCell><Badge tone={plugin.enabled && plugin.validated ? 'success' : 'neutral'}>{plugin.enabled ? '已启用' : '已停用'}</Badge></TableCell>
              </TableRow>)}</TableBody>
            </Table>
          </div>
          {!filtered.length && <div className="mt-4"><EmptyState title={plugins.isLoading ? '正在读取适配包…' : search ? '没有匹配的适配包' : '暂无已安装适配包'} /></div>}
        </section>
        <aside className="min-w-0">
          <Card>
            <CardHeader><CardTitle>{selected?.name ?? '适配包详情'}</CardTitle></CardHeader>
            <CardContent className="flex flex-col gap-4">
              {selected ? <>
                <div className="overflow-hidden rounded-md"><GameCover name={selected.name} url={selected.coverUrl} /></div>
                <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
                  <dt className="text-muted-foreground">标识</dt><dd className="break-all">{selected.slug}</dd>
                  <dt className="text-muted-foreground">游戏</dt><dd>{selected.gameName}</dd>
                  <dt className="text-muted-foreground">版本</dt><dd>{selected.version}</dd>
                  <dt className="text-muted-foreground">来源</dt><dd>{selected.source === "builtin" ? "内置" : selected.source === "external" ? "本地导入" : selected.source}</dd>
                  <dt className="text-muted-foreground">安装目录</dt><dd><PathText>{selected.installPath}</PathText></dd>
                </dl>
                <Label className="font-normal">
                  <Checkbox data-testid="plugin-enabled-toggle" aria-label={`启用适配包 ${selected.name}`} checked={selected.enabled} disabled={busy || !writable} onCheckedChange={checked => { void setEnabled(selected, checked === true); }} />
                  <span>启用适配包</span>
                </Label>
                <div className="flex flex-col gap-2 border-t pt-4">
                  <h3 className="text-sm font-semibold">游戏 Skill</h3>
                  <CardDescription>{selected.description}</CardDescription>
                  <Button variant="outline" size="sm" className="self-start" disabled={!selected.validated} onClick={() => { setSkillError(null); setSkillSlug(selected.slug); }}><FileCode2Icon data-icon="inline-start" />查看游戏 Skill</Button>
                </div>
                <Button disabled={!writable || busy || !selected.validated} onClick={() => startAdapterChat({ name: selected.name, slug: selected.slug })}><SparklesIcon data-icon="inline-start" />用 Agent 修改</Button>
              </> : <p className="text-sm text-muted-foreground">未选择适配包</p>}
            </CardContent>
          </Card>
        </aside>
      </div>
    </>}
    {tab === 'drafts' && <>
      {!!drafts.data?.length && <div className="rounded-lg border">
        <Table>
          <TableHeader><TableRow><TableHead>草稿</TableHead><TableHead>创建时间</TableHead><TableHead>事务</TableHead><TableHead className="text-right">操作</TableHead></TableRow></TableHeader>
          <TableBody>{drafts.data.map(draft => <TableRow key={draft.id}>
            <TableCell className="whitespace-normal"><strong className="font-medium">{draft.slug}</strong><PathText className="block">{draft.path}</PathText></TableCell>
            <TableCell>{date(draft.createdAt)}</TableCell>
            <TableCell><PathText>{draft.writeTransactionId}</PathText></TableCell>
            <TableCell><div className="flex flex-wrap justify-end gap-2 xl:flex-nowrap">
              <Button variant="outline" size="sm" disabled={!backendAvailable || busy} onClick={() => { void validate(draft); }}><ShieldCheckIcon data-icon="inline-start" />验证</Button>
              <Button variant="outline" size="sm" disabled={!writable || busy} aria-label={`准备发布 ${draft.slug}`} onClick={() => setPublishDraft(draft)}><UploadIcon data-icon="inline-start" />准备发布</Button>
              <Button variant="outline" size="sm" disabled={!writable || busy} aria-label={`删除草稿 ${draft.slug}`} onClick={event => { deleteButton.current = event.currentTarget; setDeleteError(null); setDeleteTarget(draft); }}><Trash2Icon data-icon="inline-start" />删除</Button>
            </div></TableCell>
          </TableRow>)}</TableBody>
        </Table>
      </div>}
      {!drafts.data?.length && <EmptyState title="暂无草稿" action={<Button disabled={!writable} onClick={() => setDraftOpen(true)}><PlusIcon data-icon="inline-start" />新建草稿</Button>} />}
    </>}
    <PluginPublications draft={publishDraft} showHistory={tab === 'publications'} writable={writable} onDraftClose={() => setPublishDraft(null)} />
    {deleteTarget && <AlertDialog open onOpenChange={open => { if (!open && !busy) setDeleteTarget(null); }}>
      <AlertDialogContent onCloseAutoFocus={event => {
        event.preventDefault();
        (deleteButton.current?.isConnected ? deleteButton.current : draftsTab.current)?.focus();
      }}>
        <AlertDialogHeader>
          <AlertDialogTitle>删除草稿？</AlertDialogTitle>
          <AlertDialogDescription className="wrap-anywhere">将「{deleteTarget.slug}」的这份草稿移入回收站，可在回收站恢复。其他草稿、已安装的适配包和发布记录会保留。</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex min-w-0 flex-col gap-1"><span className="text-sm">创建于 {date(deleteTarget.createdAt)}</span><PathText>{deleteTarget.path}</PathText></div>
        <ErrorState error={deleteError} />
        <AlertDialogFooter>
          <AlertDialogCancel disabled={busy}>取消</AlertDialogCancel>
          <Button variant="destructive" disabled={!writable || busy} onClick={() => { void deleteDraft(); }}>{busy && <Spinner data-icon="inline-start" />}删除草稿</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>}
    {skillSlug && <Modal title={`游戏 Skill · ${skillSlug}`} onClose={() => { if (!busy) setSkillSlug(null); }}>
      <ErrorState error={skillError || skill.error} />
      {skill.data && <Label className="font-normal">
        <Checkbox aria-label="启用游戏 Skill" checked={skill.data.skill.enabled} disabled={!writable || busy || !selected?.enabled || !skill.data.skill.valid} onCheckedChange={checked => { void setGameSkillEnabled(checked === true); }} />
        <span>启用游戏 Skill</span>
      </Label>}
      {skill.isLoading && <Spinner />}
      {skill.data && <pre className="max-h-[55vh] overflow-auto whitespace-pre-wrap break-words rounded-md border bg-muted/40 p-3 text-xs">{skill.data.markdown}</pre>}
    </Modal>}
    {draftOpen && <Modal title="声明式适配包草稿" onClose={() => { if (!busy) setDraftOpen(false); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setDraftOpen(false)}>取消</Button>
      <Button disabled={!writable || busy || !slug.trim() || !adapterJson.trim() || !skillMarkdown.trim()} onClick={() => { void writeDraft(); }}>{busy ? <Spinner data-icon="inline-start" /> : <FileCode2Icon data-icon="inline-start" />}预览保存草稿</Button>
    </>}>
      <ErrorState error={error} />
      <Field><FieldLabel htmlFor="draft-slug">Slug</FieldLabel><Input id="draft-slug" value={slug} onChange={e => setSlug(e.target.value)} maxLength={100} /></Field>
      <Field><FieldLabel htmlFor="draft-adapter">adapter.json</FieldLabel><Textarea id="draft-adapter" className="font-mono text-xs" rows={12} spellCheck={false} value={adapterJson} onChange={e => setAdapterJson(e.target.value)} /></Field>
      <Field><FieldLabel htmlFor="draft-skill">SKILL.md</FieldLabel><Textarea id="draft-skill" className="font-mono text-xs" rows={10} spellCheck={false} value={skillMarkdown} onChange={e => setSkillMarkdown(e.target.value)} /></Field>
    </Modal>}
    {validation && <Modal title={`验证：${validation.draft.slug}`} onClose={() => { if (!busy) setValidation(null); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setValidation(null)}>关闭</Button>
      <Button disabled={!writable || busy || !validation.valid} onClick={() => { void install(); }}><PackageIcon data-icon="inline-start" />安装预览</Button>
    </>}>
      <ErrorState error={error} />
      <div><Badge tone={validation.valid ? 'success' : 'error'}>{validation.valid ? <><CheckCircle2Icon className="size-3" />验证通过</> : '验证未通过'}</Badge></div>
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
        <dt className="text-muted-foreground">草稿事务</dt><dd>{validation.transactionStatus}</dd>
        {validation.package && <>
          <dt className="text-muted-foreground">名称</dt><dd>{validation.package.adapter.name}</dd>
          <dt className="text-muted-foreground">版本</dt><dd>{validation.package.adapter.version}</dd>
          <dt className="text-muted-foreground">平台</dt><dd>{validation.package.adapter.platforms.join('、')}</dd>
          <dt className="text-muted-foreground">Mod 类型</dt><dd>{validation.package.adapter.modTypes.map(type => `${type.name} (${type.id})`).join('、')}</dd>
        </>}
        {validation.hashes && <>
          <dt className="text-muted-foreground">Adapter SHA-256</dt><dd><PathText>{validation.hashes.adapterSha256}</PathText></dd>
          <dt className="text-muted-foreground">Skill SHA-256</dt><dd><PathText>{validation.hashes.skillSha256}</PathText></dd>
        </>}
      </dl>
      {validation.issues.map((issue, i) => <Alert variant="destructive" key={i}><AlertDescription>{issue.code}: {issue.message}</AlertDescription></Alert>)}
      {validation.warnings.map((warning, i) => <Alert variant="warning" key={i}><AlertDescription>{warning}</AlertDescription></Alert>)}
    </Modal>}
  </Page>;
}