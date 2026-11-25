import { useId, useState } from "react";
import { Checkbox } from "@/components/ui/checkbox";
import { Button } from "@/components/ui/button";
import { Modal } from "@/components/ui";
import { tr } from "@/lib/i18n";
import { MAX_BROWSE_GAMES } from "@/lib/browse";

/** Changes stay local until Apply so checking several games never fans out partial queries. */
export function BrowseGamePicker({
  games,
  selected,
  apply,
  close,
}: {
  games: { id: string; name: string }[];
  selected: string[];
  apply: (ids: string[]) => void;
  close: () => void;
}) {
  const id = useId();
  const [draft, setDraft] = useState(new Set(selected));
  return (
    <Modal
      title={tr("选择游览的游戏")}
      description={tr("只查看所选游戏的 Mod。下次打开时会记住你的选择。")}
      onClose={close}
      className="sm:max-w-md"
      actions={
        <>
          <Button variant="outline" onClick={close}>
            {tr("取消")}
          </Button>
          <Button onClick={() => apply(games.filter((g) => draft.has(g.id)).map((g) => g.id))}>
            {tr("确定")}
          </Button>
        </>
      }
    >
      <div className="flex items-center gap-1">
        <span className="me-auto text-xs text-muted-foreground">
          {tr("已选 {length} / {value2}", { length: draft.size, value2: games.length })}
        </span>
        {games.length <= MAX_BROWSE_GAMES && (
          <Button size="sm" variant="ghost" onClick={() => setDraft(new Set(games.map((g) => g.id)))}>
            {tr("全选")}
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={() => setDraft(new Set())}>
          {tr("清空")}
        </Button>
      </div>
      {games.length > MAX_BROWSE_GAMES && (
        <p className="text-xs text-muted-foreground">
          {tr("每次最多可选择 {count} 款游戏。", { count: MAX_BROWSE_GAMES })}
        </p>
      )}
      <div className="flex max-h-[45dvh] flex-col overflow-auto rounded-lg border">
        {games.map((game) => (
          <label
            key={game.id}
            htmlFor={`${id}-${game.id}`}
            className="flex cursor-pointer items-center gap-3 border-b px-3 py-2.5 text-sm last:border-0 hover:bg-accent"
          >
            <Checkbox
              id={`${id}-${game.id}`}
              disabled={!draft.has(game.id) && draft.size >= MAX_BROWSE_GAMES}
              checked={draft.has(game.id)}
              onCheckedChange={(checked) =>
                setDraft((old) => {
                  const next = new Set(old);
                  if (checked === true) next.add(game.id);
                  else next.delete(game.id);
                  return next;
                })
              }
            />
            <span className="min-w-0 break-words">{game.name}</span>
          </label>
        ))}
      </div>
    </Modal>
  );
}
