import { useState } from 'react';
import { FolderOpen, FolderPlus, RefreshCw, Search, Gamepad2, ArrowRight, Globe } from 'lucide-react';
import { open } from '@tauri-apps/plugin-dialog';
import { api, backendAvailable, useBackendQuery } from '../lib/api';
import { useUI } from '../lib/store';
import { Button, EmptyState, ErrorState, PageHeader, Badge, IconButton } from '../components/ui';
import { GameIdentityDialog } from '../components/GameIdentityDialog';

type Game = { id: string; name: string; rootPath: string; platform: string; installedModCount: number; enabledModCount: number };
type Candidate = { rootPath: string; name: string; confidence: number; suggestedAdapterSlug: string | null; registeredGameId: string | null; warnings: string[] };
type ScanReport = { candidates: Candidate[]; warnings: { path: string; message: string }[]; directoriesChecked: number; entriesChecked: number; truncated: boolean };
export default function LibraryPage() {
  const games = useBackendQuery<Game[]>(['games'], 'list_games');
  const bootstrap = useBackendQuery<{ recoveryError: string | null }>(['bootstrap'], 'get_bootstrap_state');
  const writable = backendAvailable && !!bootstrap.data && !bootstrap.data.recoveryError;
  const selectGame = useUI(s => s.selectGame);
  const notify = useUI(s => s.notify);
  const [search, setSearch] = useState('');
  const [directory, setDirectory] = useState('');
  const [candidates, setCandidates] = useState<Candidate[]>([]);
  const [report, setReport] = useState<ScanReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [identityRoot, setIdentityRoot] = useState<string | null>(null);
  async function scan() {
    setBusy(true); setError(null);
    try {
      const report = await api<ScanReport>('scan_games', { options: { userRoots: [], includeCommonRoots: true } });
      setReport(report);
      setCandidates(report.candidates);
      notify(`${report.truncated ? '扫描达到上限' : '扫描完成'}，发现 ${report.candidates.length} 个候选目录`);
    } catch (e) { setError(e); } finally { setBusy(false); }
  }
  async function identify() {
    if (!backendAvailable || !directory.trim() || busy) return;
    setBusy(true); setError(null); setReport(null);
    try { setCandidates([await api<Candidate>('identify_game', { directory: directory.trim() })]); }
    catch (e) { setError(e); } finally { setBusy(false); }
  }
  async function chooseDirectory() {
    if (!backendAvailable || busy) return;
    setBusy(true); setError(null);
    try {
      const selected = await open({directory: true, multiple: false, title: '选择游戏目录'});
      if (typeof selected === 'string') setDirectory(selected);
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function register(candidate: Candidate) {
    if (!writable || busy) return;
    setBusy(true); setError(null);
    try {
      const game = await api<Game>('register_game', { directory: candidate.rootPath, adapterSlug: candidate.suggestedAdapterSlug });
      await games.refetch(); setCandidates(items => items.filter(c => c.rootPath !== candidate.rootPath)); selectGame(game.id);
    } catch (e) { setError(e); } finally { setBusy(false); }
  }
  const rows = (games.data ?? []).filter(game => game.name.toLowerCase().includes(search.toLowerCase()));
  return <div className="page library-page" data-testid="library-page">
    <PageHeader title="游戏库" subtitle={backendAvailable ? `${games.data?.length ?? 0} 个已登记游戏` : '桌面能力未连接'} actions={<Button disabled={!backendAvailable || busy} onClick={scan}><RefreshCw size={16} className={busy ? 'spinning' : ''}/>扫描游戏</Button>}/>
    <div className="toolbar"><label className="search-field"><Search size={17}/><input placeholder="搜索游戏…" value={search} onChange={e => setSearch(e.target.value)}/></label><span className="toolbar-spacer"/><Badge>{backendAvailable ? '本机游戏' : '浏览器预览'}</Badge></div>
    {games.error && <ErrorState error={games.error}/>} {error != null && <ErrorState error={error}/>}
    <div className="game-grid">{rows.map(game => <button className="game-tile" key={game.id} onClick={() => selectGame(game.id)}><div className="game-cover"><Gamepad2 size={44}/><span>{game.name.slice(0, 2)}</span></div><div className="game-tile-info"><h2>{game.name}</h2><p>{game.platform} · {game.installedModCount} 个 Mod</p><small className="path-text">{game.rootPath}</small></div><ArrowRight size={18}/></button>)}</div>
    {!rows.length && !games.error && <EmptyState title={!backendAvailable ? '桌面能力未连接' : games.isLoading ? '正在读取游戏库…' : search ? '没有匹配的游戏' : '尚未登记游戏'}/>}
    <section className="page-section"><h2><FolderPlus size={18}/>添加游戏目录</h2><div className="toolbar"><input className="grow" aria-label="游戏目录" placeholder="输入本机游戏目录的完整路径" value={directory} onChange={e => setDirectory(e.target.value)}/><IconButton title="选择游戏目录" disabled={!backendAvailable || busy} onClick={() => {void chooseDirectory();}}><FolderOpen size={16}/></IconButton><Button disabled={!backendAvailable || !directory.trim() || busy} onClick={() => { void identify(); }}>检查目录</Button></div>
      {report && <><p className="muted">已检查 {report.directoriesChecked} 个目录、{report.entriesChecked} 个条目{report.truncated ? ' · 已达到扫描上限' : ''}</p>{report.warnings.map((warning, i) => <p className="warning-text path-text" key={i}>{warning.path}：{warning.message}</p>)}</>}
      {candidates.map(candidate => <div className="list-row" key={candidate.rootPath}><Gamepad2 size={24}/><div className="grow"><strong>{candidate.name}</strong><p className="path-text">{candidate.rootPath}</p>{candidate.warnings.map(w => <small key={w} className="warning-text">{w}</small>)}</div><Badge>{Math.round(candidate.confidence * 100)}% 匹配</Badge><div className="page-actions">{candidate.confidence < 0.85 && !candidate.registeredGameId && <Button disabled={!backendAvailable || busy} onClick={() => setIdentityRoot(candidate.rootPath)}><Globe size={15}/>联网识别</Button>}<Button disabled={!writable || busy || !!candidate.registeredGameId} onClick={() => register(candidate)}>{candidate.registeredGameId ? '已登记' : '确认登记'}</Button></div></div>)}
    </section>
    {identityRoot && <GameIdentityDialog key={identityRoot} directory={identityRoot} writable={writable} onClose={() => setIdentityRoot(null)} onConfirmed={async () => {
      await games.refetch(); setCandidates(items => items.filter(candidate => candidate.rootPath !== identityRoot)); notify('已确认游戏身份并登记目录');
    }}/>}
  </div>;
}
