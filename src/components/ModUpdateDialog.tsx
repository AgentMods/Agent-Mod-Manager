import { useEffect, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { openUrl } from '@tauri-apps/plugin-opener';
import { AlertTriangleIcon, CheckIcon, DownloadIcon, ExternalLinkIcon, RefreshCwIcon } from 'lucide-react';
import { ErrorState, Modal } from '@/components/ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Spinner } from '@/components/ui/spinner';
import { api, errorText } from '@/lib/api';
import { dashboardStatsKey } from '@/lib/dashboard-stats';
import { tr } from '@/lib/i18n';
import { modSource, modVersion, type LibraryMod, type ModRow } from '@/lib/mod-management';
import { bindModSwitch, findPendingModSwitch, releaseModSwitch, reserveModSwitch, useModSwitch } from '@/lib/mod-switch-store';
import {
  canAgentCheckUpdate, checkModUpdate, downloadModUpdate, reserveCatalogUpdate, updateCatalog, updateProject, updateProjectPage, verifyUpdateTarget,
  type UpdateCheck,
} from '@/lib/mod-update';
import { trackAgentTurn } from '@/lib/agent-store';
import { useUI } from '@/lib/store';
import type { AppSettings, Conversation } from '@/lib/types';

async function ensureAgentConfigured() {
  const settings = await api<AppSettings>('get_settings');
  if (!settings.model.trim() || !settings.base_url.trim()) throw new Error(tr('请先在「设置 → 模型服务」中配置 Agent 模型。'));
}

type Props = { gameId: string; row: ModRow; disabled: boolean; onClose: () => void };

export function ModUpdateDialog(props: Props) {
  const { gameId, row } = props;
  const project = updateProject(row);
  const page = updateProjectPage(row);
  // A refreshed row cannot inherit a candidate checked for a different target.
  return <UpdateDialog key={JSON.stringify([gameId, row.key, 'id' in row.item ? row.item.status : null, row.installed, modVersion(row), project, page])}
    {...props} project={project} page={page} />;
}

function UpdateDialog({ gameId, row, disabled, onClose, project, page }: Props & {
  project: ReturnType<typeof updateProject>; page: string | null;
}) {
  const client = useQueryClient();
  const [result, setResult] = useState<UpdateCheck | null>(null);
  const [phase, setPhase] = useState<'checking' | 'downloading' | 'review' | 'agent-check' | null>(null);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  const busy = useRef(false);
  const alive = useRef(true);
  const draft = useRef<{ artifactId: string; conversationId: string } | null>(null);
  const catalog = updateCatalog(project, page);
  const version = modVersion(row);
  const candidate = result?.kind === 'available' ? result.version : null;
  const replaceInstallId = row.installed && 'id' in row.item && row.item.status === 'enabled' ? row.item.id : undefined;
  const mutating = phase === 'downloading' || phase === 'review' || phase === 'agent-check';
  const canReplace = !row.installed || (replaceInstallId != null && 'id' in row.item && !!row.item.downloadOrigin);
  const agentCheck = (!catalog || result?.kind === 'unverified' || !!error) && canAgentCheckUpdate(project, page);

  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  async function check() {
    if (!catalog || disabled || busy.current) return;
    busy.current = true;
    setPhase('checking');
    setResult(null);
    setError('');
    try { setResult(await checkModUpdate(catalog, version)); }
    catch (failure) { setError(errorText(failure) || tr('检查更新失败，请稍后重试。')); }
    finally { busy.current = false; setPhase(null); }
  }

  async function checkWithAgent() {
    if (!agentCheck || disabled || busy.current) return;
    busy.current = true;
    setPhase('agent-check');
    setError('');
    let release: (() => void) | undefined;
    try {
      release = reserveCatalogUpdate(gameId, project, page);
      await ensureAgentConfigured();
      if (!alive.current) return;
      let conversationId = draft.current?.artifactId === '' ? draft.current.conversationId : undefined;
      if (!conversationId) {
        const conversation = await api<Conversation>('create_conversation', {
          gameId, title: tr('核对 Mod 来源：{name}', { name: row.item.name.slice(0, 72) }),
        });
        conversationId = conversation.id;
        draft.current = { artifactId: '', conversationId };
        void client.invalidateQueries({ queryKey: ['conversations'] });
      }
      if (!alive.current) return;
      const content = tr('只读核对以下 Mod 的原发布渠道：{target}。只使用记录的来源项目 ID 或项目页，不得按 Mod 名称搜索或改用其他来源。请报告适用版本、发布日期及原发布页证据；无法确认时说明原因。不要获取下载链接或下载文件，不要导入、安装、卸载、替换、编辑文件或申请任何写入审批。', {
        target: JSON.stringify({ project, projectPage: page, currentVersion: version }),
      });
      const turnId = await api<string>('start_turn', { request: {
        conversationId, content, model: null, permissionMode: 'request_approval', reasoningEffort: null,
      } });
      trackAgentTurn(conversationId, turnId);
      if (alive.current) {
        onClose();
        useUI.getState().openConversation(conversationId, gameId);
      }
    } catch (failure) { if (alive.current) setError(errorText(failure)); }
    finally {
      release?.();
      busy.current = false;
      if (alive.current) setPhase(null);
    }
  }

  async function update() {
    if (!catalog || !candidate || disabled || busy.current || saved || !canReplace || (row.installed && !replaceInstallId)) return;
    busy.current = true;
    setError('');
    let release: (() => void) | undefined;
    let switchId: string | null = null;
    try {
      release = reserveCatalogUpdate(gameId, catalog);
      setPhase('downloading');
      if (findPendingModSwitch(useModSwitch.getState().pending, row.target)) throw new Error(tr('此项目的更新任务正在处理。'));
      const library = await api<LibraryMod[]>('list_mod_library', { gameId });
      await verifyUpdateTarget(gameId, row, catalog, library);
      if (replaceInstallId) await ensureAgentConfigured();
      if (!alive.current) return;
      const downloaded = await downloadModUpdate(gameId, catalog, candidate, library);
      void client.invalidateQueries({ queryKey: ['mod-library', gameId] });
      void client.invalidateQueries({ queryKey: dashboardStatsKey });
      if (!alive.current) return;
      if (!replaceInstallId) {
        setSaved(true);
        return;
      }
      setPhase('review');
      await verifyUpdateTarget(gameId, row, catalog, downloaded.library);
      if (!alive.current) return;
      const artifactId = downloaded.archive.artifactId;
      switchId = reserveModSwitch({ kind: 'install', gameId, artifactId, installId: replaceInstallId, replaceInstallId, name: row.item.name });
      if (!switchId) throw new Error(tr('此项目的更新任务正在处理。'));
      let conversationId = draft.current?.artifactId === artifactId ? draft.current.conversationId : undefined;
      if (!conversationId) {
        const conversation = await api<Conversation>('create_conversation', {
          gameId, category: 'install', title: tr('更新 Mod：{name}', { name: row.item.name.slice(0, 72) }),
        });
        conversationId = conversation.id;
        draft.current = { artifactId, conversationId };
        void client.invalidateQueries({ queryKey: ['conversations'] });
      }
      if (!alive.current) return;
      const requestTarget = JSON.stringify({ gameId, artifactId, replaceInstallId });
      const content = tr('请审核以下 Mod 更新。只使用这个新版归档替换指定的旧安装记录：{target}。先用 {match} 核对归档和适配规则，再用 {inspect} 核对旧安装。确认兼容后，用 {install} 提交审核，必须在 request 中原样传入以上三个字段。不要另行卸载、安装其他归档或省略替换目标；无法确认时请停止并说明原因。', {
        target: requestTarget, match: 'match_mod_install', inspect: 'inspect_install', install: 'install_library_mod',
      });
      // Replacement must be reviewed even when the global composer uses Full Trust.
      const turnId = await api<string>('start_turn', { request: {
        conversationId, content, model: null, permissionMode: 'request_approval', reasoningEffort: null,
      } });
      if (!bindModSwitch(switchId, conversationId, turnId)) trackAgentTurn(conversationId, turnId);
      if (alive.current) {
        onClose();
        useUI.getState().openConversation(conversationId, gameId);
      }
    } catch (failure) { if (alive.current) setError(errorText(failure)); }
    finally {
      if (switchId) releaseModSwitch(switchId);
      release?.();
      busy.current = false;
      if (alive.current) setPhase(null);
    }
  }

  const manual = tr('请从原发布渠道获取新版，再通过此游戏的「导入本地归档」将其加入库。');
  const status = !project && !agentCheck ? tr('无法确认此 Mod 的来源项目')
    : !catalog ? tr('此来源不支持自动核对版本。')
    : result?.kind === 'unverified' ? tr('无法确认有可安全升级的版本。')
    : result?.kind === 'current' ? tr('未找到更新的匹配正式版。')
    : candidate ? tr('找到可更新的正式版。')
    : tr('检查更新不会下载文件或修改游戏。');
  const showManual = !catalog || result?.kind === 'unverified' || !!error || !canReplace;
  return <Modal title={tr('更新 Mod')} description={row.item.name} className="sm:max-w-lg [&_[data-slot=dialog-description]]:wrap-anywhere"
    onClose={() => { if (!mutating) onClose(); }} actions={
      <>
        <Button variant="outline" disabled={mutating} onClick={onClose}>{tr('关闭')}</Button>
        {agentCheck && <Button variant={catalog ? 'secondary' : 'default'} disabled={disabled || !!phase}
          className="h-auto min-h-9 whitespace-normal" onClick={() => { void checkWithAgent(); }}>
          {phase === 'agent-check' && <Spinner data-icon="inline-start" className="motion-reduce:animate-none" />}
          {tr('用 Agent 核对来源')}
        </Button>}
        {catalog && !saved && (candidate
          ? <Button disabled={disabled || !!phase || !canReplace} className="h-auto min-h-9 whitespace-normal" onClick={() => { void update(); }}>
            {mutating ? <Spinner data-icon="inline-start" className="motion-reduce:animate-none" /> : <DownloadIcon data-icon="inline-start" />}
            {phase === 'downloading' ? tr('正在准备新版归档…') : phase === 'review' ? tr('正在启动 Agent 审核…')
              : replaceInstallId ? tr('下载并审核替换') : tr('下载到库')}
          </Button>
          : <Button disabled={disabled || !!phase} onClick={() => { void check(); }}>
            {phase === 'checking' ? <Spinner data-icon="inline-start" className="motion-reduce:animate-none" /> : <RefreshCwIcon data-icon="inline-start" />}
            {phase === 'checking' ? tr('正在检查更新…') : tr('检查更新')}
          </Button>)}
      </>
    }>
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-x-4 gap-y-2 text-sm">
      <div className="flex min-w-0 items-baseline gap-2">
        <span className="shrink-0 text-muted-foreground">{tr('来源')}</span>
        <span className="min-w-0 font-medium wrap-anywhere">{modSource(row) || tr('未提供')}</span>
      </div>
      {page && <a className="inline-flex items-center gap-1.5 text-primary underline underline-offset-4 focus-visible:outline-2 focus-visible:outline-ring"
        href={page} onClick={event => {
          event.preventDefault();
          void openUrl(page).catch(failure => useUI.getState().notify(errorText(failure)));
        }}>{tr('打开原发布页')}<ExternalLinkIcon className="size-3.5" aria-hidden="true" /></a>}
    </div>
    <dl className="grid min-w-0 grid-cols-2 gap-2 sm:gap-3">
      <div className="min-w-0 rounded-lg border bg-muted/40 px-3 py-2.5">
        <dt className="text-sm text-muted-foreground">{tr('当前版本')}</dt>
        <dd className="mt-1 font-semibold wrap-anywhere">{version ?? result?.current?.version ?? tr('未提供')}</dd>
      </div>
      <div className="min-w-0 rounded-lg border bg-muted/40 px-3 py-2.5">
        <dt className="text-sm text-muted-foreground">{tr('可更新版本')}</dt>
        <dd className="mt-1 font-semibold wrap-anywhere">{candidate?.version ?? '—'}</dd>
      </div>
    </dl>
    <Alert role="status" variant={showManual ? 'warning' : saved ? 'success' : 'default'}>
      {showManual ? <AlertTriangleIcon aria-hidden="true" /> : saved || candidate ? <CheckIcon aria-hidden="true" /> : null}
      <AlertDescription className="min-w-0 wrap-anywhere">
        <p className="text-foreground">{saved ? tr('新版已加入此游戏的库，未安装。') : phase === 'checking' ? tr('正在检查更新…')
          : phase === 'downloading' ? tr('正在准备新版归档…')
          : phase === 'review' || phase === 'agent-check' ? tr('正在启动 Agent 审核…') : status}</p>
        {!canReplace && <p>{tr('此安装记录缺少可验证的替换依据，不能自动更新。')}</p>}
        {agentCheck && <p>{tr('Agent 仅核对原发布渠道并报告证据，不下载或替换。')}</p>}
        {showManual && <p>{manual}</p>}
        {canReplace && candidate && !saved && <p>{replaceInstallId
          ? tr('新版下载后，Agent 将审核是否替换此安装记录。')
          : tr('新版仅加入此游戏的库，不会安装。')}</p>}
      </AlertDescription>
    </Alert>
    <ErrorState error={error} />
  </Modal>;
}
