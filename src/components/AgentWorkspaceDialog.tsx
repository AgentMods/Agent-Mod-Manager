import { tr } from "@/lib/i18n";
import { useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { FieldLegend, FieldSet } from "@/components/ui/field";
import { Spinner } from "@/components/ui/spinner";
import { ErrorState, Modal } from "./ui";
import type { Game, GameWorkspace } from "@/lib/types";

type Props = {
  workspace: GameWorkspace;
  games: Game[] | undefined;
  gamesError: unknown;
  onRetry: () => void;
  disabled: boolean;
  onApply: (workspace: GameWorkspace) => Promise<void>;
  onClose: () => void;
  opener: HTMLButtonElement;
};

export function AgentWorkspaceDialog({ workspace, games, gamesError, onRetry, disabled, onApply, onClose, opener }: Props) {
  const [draft, setDraft] = useState(workspace);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const inFlight = useRef(false);
  const prefix = useId();
  const selected = new Set(draft ?? games?.map(game => game.id));
  const selectedIds = games?.filter(game => selected.has(game.id)).map(game => game.id) ?? [];
  const unavailable = !games || !!gamesError || disabled || saving;

  function toggle(id: string, checked: boolean) {
    if (!games || unavailable) return;
    setDraft(games.filter(game => game.id === id ? checked : selected.has(game.id)).map(game => game.id));
  }

  async function apply() {
    if (inFlight.current || unavailable) return;
    inFlight.current = true;
    setSaving(true);
    setError(null);
    try {
      await onApply(draft === null ? null : selectedIds);
      onClose();
    } catch (failure) { setError(failure); }
    finally { inFlight.current = false; setSaving(false); }
  }

  return <Modal title={tr("选择工作区游戏")} description={tr("选择当前对话关联的游戏。清空后，仍可与 Agent 对话。")}
    className="sm:max-w-md" onClose={() => { if (!inFlight.current) onClose(); }}
    onCloseAutoFocus={event => { if (opener.isConnected) { event.preventDefault(); opener.focus(); } }}
    actions={<><Button variant="outline" disabled={saving} onClick={onClose}>{tr("取消")}</Button>
      <Button disabled={unavailable} onClick={() => void apply()}>{saving && <Spinner />}{tr("应用")}</Button></>}>
    {gamesError ? <><ErrorState error={gamesError} /><Button variant="outline" onClick={onRetry}>{tr("重试加载游戏")}</Button></>
      : !games ? <p role="status" className="text-sm text-muted-foreground">{tr("正在加载游戏…")}</p> : <>
        <div className="flex items-center gap-1">
          <span aria-live="polite" className="me-auto text-xs text-muted-foreground tabular-nums">
            {draft === null ? tr("全部游戏") : tr("已选 {length} / {value2}", { length: selectedIds.length, value2: games.length })}
          </span>
          <Button type="button" variant="ghost" size="sm" disabled={unavailable} onClick={() => setDraft(null)}>{tr("全选")}</Button>
          <Button type="button" variant="ghost" size="sm" disabled={unavailable} onClick={() => setDraft([])}>{tr("清空")}</Button>
        </div>
        {games.length ? <FieldSet disabled={unavailable} className="block max-h-[42dvh] shrink-0 overflow-y-auto rounded-lg border">
          <FieldLegend className="sr-only">{tr("所属游戏")}</FieldLegend>
          {games.map(game => <label key={game.id} htmlFor={`${prefix}-${game.id}`}
            className="flex cursor-pointer items-center gap-3 border-b px-3 py-2.5 text-sm last:border-b-0 hover:bg-accent/50 has-focus-visible:bg-accent/50">
            <Checkbox id={`${prefix}-${game.id}`} checked={selected.has(game.id)} onCheckedChange={next => toggle(game.id, next === true)} />
            <span className="min-w-0 break-words">{game.name}</span>
          </label>)}
        </FieldSet> : <p className="text-sm text-muted-foreground">{tr("游戏库中还没有游戏。")}</p>}
        {draft === null && <p className="text-xs text-muted-foreground">{tr("全选会自动包含以后添加的游戏。")}</p>}
      </>}
    <ErrorState error={error} />
  </Modal>;
}
