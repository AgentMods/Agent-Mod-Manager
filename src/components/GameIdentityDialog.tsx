import { useLayoutEffect, useState } from 'react';
import { Check, Globe, Search } from 'lucide-react';
import { api, useBackendQuery } from '../lib/api';
import { Badge, Button, EmptyState, ErrorState, Modal } from './ui';

type IdentityPreview = {
  rootPath: string; fingerprint: string; signals: string[]; suggestedQuery: string; warnings: string[];
};
type IdentityCandidate = { name: string; title: string; url: string; snippet: string };
type IdentitySearch = {
  id: string; rootPath: string; fingerprint: string; query: string;
  status: 'available' | 'needs_configuration' | 'unavailable'; message: string;
  candidates: IdentityCandidate[]; createdAt: string;
};

export function GameIdentityDialog({ directory, writable, onClose, onConfirmed }: {
  directory: string; writable: boolean; onClose: () => void; onConfirmed: () => Promise<void>;
}) {
  const preview = useBackendQuery<IdentityPreview>(['identity-preview', directory], 'preview_game_identity', { directory });
  const [query, setQuery] = useState('');
  const [searchConsent, setSearchConsent] = useState(false);
  const [registrationConsent, setRegistrationConsent] = useState(false);
  const [result, setResult] = useState<IdentitySearch | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [name, setName] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);

  useLayoutEffect(() => {
    if (preview.data) {
      setQuery(preview.data.suggestedQuery); setSearchConsent(false); setRegistrationConsent(false);
      setResult(null); setSelected(null); setName(''); setError(null);
    }
  }, [preview.data?.fingerprint]);

  function changeQuery(value: string) {
    setQuery(value); setSearchConsent(false); setRegistrationConsent(false);
    setResult(null); setSelected(null); setName(''); setError(null);
  }
  async function search() {
    if (!preview.data || busy || !searchConsent || !query.trim()) return;
    setBusy(true); setError(null); setResult(null); setSelected(null); setRegistrationConsent(false);
    try {
      setResult(await api<IdentitySearch>('search_game_identity', {
        directory: preview.data.rootPath, expectedFingerprint: preview.data.fingerprint,
        publicQuery: query.trim(), confirmation: 'SEARCH_GAME_IDENTITY',
      }));
    } catch (failure) { setError(failure); }
    finally { setSearchConsent(false); setBusy(false); }
  }
  async function confirm() {
    if (!result || result.status !== 'available' || selected === null || !registrationConsent || !name.trim() || busy || !writable) return;
    setBusy(true); setError(null);
    try {
      await api('confirm_game_identity', {
        searchId: result.id, candidateIndex: selected, name: name.trim(), confirmation: 'CONFIRM_GAME_IDENTITY',
      });
      await onConfirmed(); onClose();
    } catch (failure) { setError(failure); }
    finally { setRegistrationConsent(false); setBusy(false); }
  }
  const canConfirm = writable && !busy && result?.status === 'available' && selected !== null && registrationConsent && !!name.trim();
  return <Modal title="联网识别游戏" onClose={() => { if (!busy) onClose(); }} actions={<>
    <Button disabled={busy} onClick={onClose}>取消</Button>
    <Button className="primary" disabled={!canConfirm} onClick={() => { void confirm(); }}><Check size={15}/>确认登记</Button>
  </>}>
    <ErrorState error={preview.error || error}/>
    {!preview.data ? <EmptyState title={preview.isLoading ? '正在检查本地目录…' : '无法检查目录'}/> : <div className="identity-review">
      <dl className="metadata-list"><dt>本地目录</dt><dd className="path-text">{preview.data.rootPath}</dd><dt>搜索目的地</dt><dd>Brave Search API</dd><dt>发送内容</dt><dd>仅下方搜索词</dd></dl>
      <details className="identity-evidence"><summary>本地名称信号（{preview.data.signals.length}）</summary><ul>{preview.data.signals.map(signal => <li className="path-text" key={signal}>{signal}</li>)}</ul></details>
      {preview.data.warnings.map((warning, i) => <p className="warning-text" key={i}>{warning}</p>)}
      <label className="field">公开搜索词<input aria-label="公开搜索词" value={query} maxLength={512} disabled={busy} onChange={event => changeQuery(event.target.value)}/></label>
      <label className="checkbox-row"><input type="checkbox" aria-label="同意发送公开搜索词" checked={searchConsent} disabled={busy} onChange={event => setSearchConsent(event.target.checked)}/><span>我已核对搜索词不含本机路径、密钥或私人信息，同意将其发送给 Brave。</span></label>
      <div className="toolbar"><Button disabled={busy || !searchConsent || !query.trim()} onClick={() => { void search(); }}><Search size={15}/>{busy ? '正在处理…' : '搜索候选'}</Button></div>
      {result && <section className="identity-results" aria-label="联网识别候选">
        <div className="section-heading"><h3><Globe size={15}/>网络候选</h3><Badge tone="warning">未验证</Badge></div>
        {result.status === 'needs_configuration' ? <p role="status" className="warning-text">请先在设置中配置 Brave Search API Key，未执行网络搜索。</p> : result.status === 'unavailable' ? <p role="status" className="warning-text">联网搜索暂不可用，未确认任何游戏身份。</p> : <>
          {result.candidates.length ? <fieldset className="identity-candidates"><legend className="sr-only">选择游戏候选</legend>{result.candidates.map((candidate, index) => <label className="identity-candidate" key={`${index}:${candidate.url}`}>
            <input type="radio" name="game-identity-candidate" aria-label={`选择候选 ${index + 1}：${candidate.title}`} checked={selected === index} disabled={busy} onChange={() => { setSelected(index); setName(candidate.name); setRegistrationConsent(false); }}/>
            <span><strong>{candidate.title}</strong><small className="path-text">{candidate.url}</small><p>{candidate.snippet}</p></span>
          </label>)}</fieldset> : <EmptyState title="没有找到可确认的候选"/>}
          {selected !== null && <><label className="field">确认的游戏名称<input aria-label="确认的游戏名称" maxLength={128} value={name} disabled={busy} onChange={event => { setName(event.target.value); setRegistrationConsent(false); }}/></label>
            <label className="checkbox-row"><input type="checkbox" aria-label="确认游戏目录登记" checked={registrationConsent} disabled={busy || !writable} onChange={event => setRegistrationConsent(event.target.checked)}/><span>我已核对候选，确认将此目录登记为游戏。不自动关联适配包或执行安装。</span></label></>}
        </>}
      </section>}
    </div>}
  </Modal>;
}
