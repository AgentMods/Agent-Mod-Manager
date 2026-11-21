import type { Game } from "@/lib/types";
import { displayPath } from "@/lib/paths";
import { type ComponentProps, useLayoutEffect, useState } from 'react';
import { CheckIcon, GlobeIcon, SearchIcon } from 'lucide-react';
import { api, useBackendQuery } from '@/lib/api';
import { Badge, EmptyState, ErrorState, Modal, PathText } from './ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from '@/components/ui/item';
import { Label } from '@/components/ui/label';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { Separator } from '@/components/ui/separator';
import { Spinner } from '@/components/ui/spinner';

type IdentityPreview = {
  rootPath: string; fingerprint: string; signals: string[]; suggestedQuery: string; warnings: string[];
};
type IdentityCandidate = { name: string; title: string; url: string; snippet: string };
type IdentitySearch = {
  id: string; rootPath: string; fingerprint: string; query: string;
  status: 'available' | 'needs_configuration' | 'unavailable'; message: string;
  candidates: IdentityCandidate[]; createdAt: string;
};

export function GameIdentityDialog({ directory, writable, onClose, onCloseAutoFocus, onConfirmed }: {
  directory: string; writable: boolean; onClose: () => void; onConfirmed: (game: Game) => Promise<void>;
  onCloseAutoFocus: ComponentProps<typeof Modal>["onCloseAutoFocus"];
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
      const game = await api<Game>('confirm_game_identity', {
        searchId: result.id, candidateIndex: selected, name: name.trim(), confirmation: 'CONFIRM_GAME_IDENTITY',
      });
      await onConfirmed(game); onClose();
    } catch (failure) { setError(failure); }
    finally { setRegistrationConsent(false); setBusy(false); }
  }
  const canConfirm = writable && !busy && result?.status === 'available' && selected !== null && registrationConsent && !!name.trim();

  return <Modal title="联网识别游戏" onCloseAutoFocus={onCloseAutoFocus} onClose={() => { if (!busy) onClose(); }} className="sm:max-w-2xl" actions={<>
    <Button variant="outline" disabled={busy} onClick={onClose}>取消</Button>
    <Button disabled={!canConfirm} onClick={() => { void confirm(); }}><CheckIcon data-icon="inline-start"/>添加</Button>
  </>}>
    <ErrorState error={preview.error || error}/>
    {!preview.data ? <EmptyState title={preview.isLoading ? '正在检查本地目录…' : '无法检查目录'}/> : <div className="flex flex-col gap-4 text-sm">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2">
        <dt className="text-muted-foreground">本地目录</dt><dd><PathText>{displayPath(preview.data.rootPath)}</PathText></dd>
        <dt className="text-muted-foreground">搜索目的地</dt><dd>Bing（应用内置搜索）</dd>
        <dt className="text-muted-foreground">发送内容</dt><dd>仅下方搜索词</dd>
      </dl>
      <details className="rounded-md border px-3 py-2">
        <summary className="cursor-pointer font-medium">本地名称信号（{preview.data.signals.length}）</summary>
        <ul className="mt-2 flex max-h-36 list-disc flex-col gap-1 overflow-auto pl-5">{preview.data.signals.map(signal => <li key={signal}><PathText>{signal}</PathText></li>)}</ul>
      </details>
      {preview.data.warnings.map((warning, i) => <Alert variant="warning" key={i}><AlertDescription>{warning}</AlertDescription></Alert>)}
      <FieldGroup>
        <Field><FieldLabel htmlFor="identity-query">公开搜索词</FieldLabel><Input id="identity-query" aria-label="公开搜索词" value={query} maxLength={512} disabled={busy} onChange={event => changeQuery(event.target.value)}/></Field>
        <Label className="items-start font-normal"><Checkbox aria-label="同意发送公开搜索词" checked={searchConsent} disabled={busy} onCheckedChange={checked => setSearchConsent(checked === true)}/><span>我已核对搜索词不含本机路径、密钥或私人信息，同意将其发送给 Bing。</span></Label>
        <div><Button disabled={busy || !searchConsent || !query.trim()} onClick={() => { void search(); }}>{busy ? <Spinner data-icon="inline-start"/> : <SearchIcon data-icon="inline-start"/>}{busy ? '正在处理…' : '搜索候选'}</Button></div>
      </FieldGroup>
      {result && <section className="flex flex-col gap-4" aria-label="联网识别候选">
        <Separator/>
        <div className="flex items-center justify-between gap-2"><h3 className="flex items-center gap-2 font-medium"><GlobeIcon className="size-4"/>网络候选</h3><Badge tone="warning">未验证</Badge></div>
        {result.status !== 'available'
          ? <Alert variant="warning" role="status"><AlertDescription>{result.message}</AlertDescription></Alert>
          : <>
          {result.candidates.length ? <RadioGroup aria-label="选择游戏候选" value={selected === null ? '' : String(selected)} disabled={busy} onValueChange={value => { const index = Number(value); setSelected(index); setName(result.candidates[index].name); setRegistrationConsent(false); }}>
            <ItemGroup className="gap-2">{result.candidates.map((candidate, index) => <Item asChild variant="outline" key={`${index}:${candidate.url}`}>
              <label className="cursor-pointer items-start">
                <RadioGroupItem value={String(index)} aria-label={`选择候选 ${index + 1}：${candidate.title}`} className="mt-1"/>
                <ItemContent className="min-w-0"><ItemTitle className="break-words">{candidate.title}</ItemTitle><PathText>{candidate.url}</PathText><ItemDescription className="line-clamp-none break-words">{candidate.snippet}</ItemDescription></ItemContent>
              </label>
            </Item>)}</ItemGroup>
          </RadioGroup> : <EmptyState title="没有找到可确认的候选"/>}
          {selected !== null && <FieldGroup>
            <Field><FieldLabel htmlFor="identity-name">确认的游戏名称</FieldLabel><Input id="identity-name" aria-label="确认的游戏名称" maxLength={128} value={name} disabled={busy} onChange={event => { setName(event.target.value); setRegistrationConsent(false); }}/></Field>
            <Label className="items-start font-normal"><Checkbox aria-label="确认添加" checked={registrationConsent} disabled={busy || !writable} onCheckedChange={checked => setRegistrationConsent(checked === true)}/><span>我已核对候选，确认将此目录添加到游戏管理器。不自动关联适配包或执行安装。</span></Label>
          </FieldGroup>}
        </>}
      </section>}
    </div>}
  </Modal>;
}
