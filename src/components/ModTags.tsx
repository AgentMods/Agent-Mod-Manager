import { tr } from "@/lib/i18n";
import { useState, type CSSProperties, type FormEvent } from 'react';
import { PencilIcon, PlusIcon, TagsIcon, Trash2Icon } from 'lucide-react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { ErrorState, IconButton, Modal } from '@/components/ui';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group';
import { api } from '@/lib/api';
import { modMetadataTarget, type ModRow } from '@/lib/mod-management';

export type ModTag = { id: string; name: string; color: string };

function tagColor(color: string): CSSProperties {
  return { '--mod-tag-color': color } as CSSProperties;
}

export function ModTagBadge({ tag }: { tag: ModTag }) {
  return <Badge variant="outline" className="mod-tag-color max-w-full rounded-md whitespace-normal wrap-anywhere" style={tagColor(tag.color)}>{tag.name}</Badge>;
}

export function ModTagFilters({ tags, value, onChange, onManage, disabled, busy }: {
  tags: ModTag[]; value: string; onChange: (value: string) => void; onManage: () => void; disabled: boolean; busy: boolean;
}) {
  return <div className="flex min-w-0 flex-wrap items-start justify-between gap-2">
    <ToggleGroup type="single" size="sm" variant="outline" spacing={1} value={value || 'all'} disabled={busy}
      aria-label={tr("按标签筛选")} onValueChange={id => { if (id) onChange(id === 'all' ? '' : id); }} className="max-w-full flex-wrap justify-start">
      <ToggleGroupItem value="all">{tr("全部标签")}</ToggleGroupItem>
      {tags.map(tag => <ToggleGroupItem key={tag.id} value={tag.id} className="mod-tag-color max-w-full" style={tagColor(tag.color)}>
        <span className="truncate">{tag.name}</span>
      </ToggleGroupItem>)}
    </ToggleGroup>
    <Button size="sm" variant="outline" disabled={disabled} onClick={onManage}><TagsIcon />{tr("管理标签")}</Button>
  </div>;
}

const newTag = { id: null as string | null, name: '', color: '#719eff' };
export function ManageModTags({ gameId, tags, onClose }: { gameId: string; tags: ModTag[]; onClose: () => void }) {
  const client = useQueryClient();
  const [draft, setDraft] = useState(newTag);
  const [deleting, setDeleting] = useState<ModTag | null>(null);
  const mutation = useMutation({
    mutationFn: async (action: 'save' | 'delete') => {
      if (action === 'delete' && deleting) await api('delete_mod_tag', { gameId, id: deleting.id });
      else await api('save_mod_tag', { gameId, ...draft, name: draft.name.trim() });
    },
    onSuccess: async (_, action) => {
      if (action === 'save' || draft.id === deleting?.id) setDraft(newTag);
      setDeleting(null);
      await Promise.all([
        client.invalidateQueries({ queryKey: ['mod-tags', gameId] }),
        ...(action === 'delete' ? [client.invalidateQueries({ queryKey: ['installs', gameId] }),
          client.invalidateQueries({ queryKey: ['mod-library', gameId] })] : []),
      ]);
    },
  });
  const pending = mutation.isPending;
  function submit(event: FormEvent) {
    event.preventDefault();
    if (!pending && draft.name.trim()) mutation.mutate('save');
  }
  return <Modal title={tr("管理标签")} className="sm:max-w-lg" onClose={() => { if (!pending) onClose(); }}>
    <form onSubmit={submit}>
      <FieldGroup className="flex-row flex-wrap items-end gap-3">
        <Field className="min-w-28 flex-1"><FieldLabel htmlFor="mod-tag-name">{tr("标签名称")}</FieldLabel>
          <Input id="mod-tag-name" value={draft.name} maxLength={48} required disabled={pending} onChange={e => setDraft({ ...draft, name: e.target.value })} />
        </Field>
        <Field className="w-fit"><FieldLabel htmlFor="mod-tag-color">{tr("标签颜色")}</FieldLabel>
          <Input type="color" id="mod-tag-color" className="w-16 cursor-pointer p-1" value={draft.color} disabled={pending} onChange={e => setDraft({ ...draft, color: e.target.value })} />
        </Field>
        <Button type="submit" disabled={pending || !draft.name.trim() || deleting !== null}>{draft.id ? <PencilIcon /> : <PlusIcon />}{draft.id ? tr("保存修改") : tr("创建标签")}</Button>
        {draft.id && <Button type="button" size="sm" variant="ghost" disabled={pending} onClick={() => { setDraft(newTag); mutation.reset(); }}>{tr("取消编辑")}</Button>}
      </FieldGroup>
    </form>
    <ErrorState error={mutation.error} />
    {!!tags.length && <ul className="divide-y border-t" aria-label={tr("已有标签")}>{tags.map(tag => <li key={tag.id} className="flex flex-wrap items-center gap-2 py-2">
      <span className="min-w-0 flex-1"><ModTagBadge tag={tag} /></span>
      <IconButton title={tr("编辑 {name}", { name: tag.name })} disabled={pending} onClick={() => { setDraft(tag); setDeleting(null); mutation.reset(); }}><PencilIcon /></IconButton>
      <IconButton title={tr("删除 {name}", { name: tag.name })} disabled={pending} onClick={() => { setDeleting(tag); mutation.reset(); }}><Trash2Icon /></IconButton>
      {deleting?.id === tag.id && <div className="flex w-full flex-wrap items-center gap-2 rounded-md bg-muted p-2">
        <span className="flex-1 text-sm">{tr("从此游戏的所有 Mod 移除此标签？")}</span>
        <Button size="sm" variant="outline" disabled={pending} onClick={() => setDeleting(null)}>{tr("取消")}</Button>
        <Button size="sm" variant="destructive" disabled={pending} onClick={() => mutation.mutate('delete')}>{tr("确认删除")}</Button>
      </div>}
    </li>)}</ul>}
  </Modal>;
}

export function EditModDetails({ gameId, row, tags, disabled, onClose }: {
  gameId: string; row: ModRow; tags: ModTag[]; disabled: boolean; onClose: () => void;
}) {
  const [name, setName] = useState(row.item.name);
  const [selected, setSelected] = useState(() => new Set(row.item.tagIds ?? []));
  const [managing, setManaging] = useState(false);
  const client = useQueryClient();
  const mutation = useMutation({
    mutationFn: () => api('update_mod_metadata', { gameId, target: modMetadataTarget(row),
      patch: { name: name.trim(), tagIds: tags.filter(tag => selected.has(tag.id)).map(tag => tag.id) } }),
    onSuccess: async () => {
      await Promise.all([client.invalidateQueries({ queryKey: ['installs', gameId] }),
        client.invalidateQueries({ queryKey: ['mod-library', gameId] })]);
      onClose();
    },
  });
  const busy = mutation.isPending;
  if (managing) return <ManageModTags gameId={gameId} tags={tags} onClose={() => setManaging(false)} />;
  return <Modal title={tr("编辑 Mod")} className="sm:max-w-md" onClose={() => { if (!busy) onClose(); }} actions={<>
    <Button variant="outline" disabled={busy} onClick={onClose}>{tr("取消")}</Button>
    <Button type="submit" form="edit-mod-details" disabled={disabled || busy || !name.trim()}>{tr("保存")}</Button>
  </>}>
    <form id="edit-mod-details" onSubmit={event => { event.preventDefault(); if (!disabled && !busy && name.trim()) mutation.mutate(); }}>
      <FieldGroup>
        <Field><FieldLabel htmlFor="mod-display-name">{tr("显示名称")}</FieldLabel>
          <Input id="mod-display-name" required maxLength={200} value={name} disabled={busy} onChange={event => setName(event.target.value)} />
        </Field>
        <FieldSet className="gap-2"><FieldLegend>{tr("标签")}</FieldLegend>
          {tags.length ? <div className="flex flex-col gap-1">{tags.map(tag => <Label key={tag.id} className="flex cursor-pointer items-center gap-3 rounded-md px-2 py-2 hover:bg-accent">
            <Checkbox checked={selected.has(tag.id)} disabled={busy} onCheckedChange={value => setSelected(previous => {
              const next = new Set(previous);
              if (value === true) next.add(tag.id); else next.delete(tag.id);
              return next;
            })} /><ModTagBadge tag={tag} />
          </Label>)}</div> : <p className="text-muted-foreground">{tr("这个游戏还没有标签，可在「管理标签」中创建。")}</p>}
          <Button type="button" size="sm" variant="outline" className="self-start" disabled={disabled || busy} onClick={() => setManaging(true)}><TagsIcon />{tr("管理标签")}</Button>
        </FieldSet>
      </FieldGroup>
    </form>
    <ErrorState error={mutation.error} />
  </Modal>;
}
