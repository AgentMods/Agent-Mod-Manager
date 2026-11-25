import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDownIcon, FolderOpenIcon, PlayIcon, PuzzleIcon, Settings2Icon } from "lucide-react";
import { ErrorState, Modal } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { invalidateGameQueries } from "@/lib/game-cache";
import { chooseGameProgram } from "@/lib/game-launch";
import { tr } from "@/lib/i18n";
import { displayPath, pathBasename } from "@/lib/paths";
import { useUI } from "@/lib/store";
import type { Game } from "@/lib/types";

export type LaunchMethod = "steam" | "direct" | "smapi" | "mod-engine-2" | "custom";
interface LaunchOption { id: LaunchMethod; label: string | null }
interface ModLauncher { name: string; path: string; args: string[] }

function optionName(option: LaunchOption) {
  switch (option.id) {
    case "steam": return tr("Steam 启动");
    case "direct": return tr("直接启动");
    case "smapi": return tr("SMAPI 启动");
    case "mod-engine-2": return tr("Mod Engine 2 启动");
    case "custom": return option.label || tr("Mod 启动器");
  }
}

function ModLauncherDialog({ game, onClose, restoreFocus }: { game: Game; onClose: () => void; restoreFocus: () => void }) {
  const saved = game.metadata?.modLauncher as Partial<ModLauncher> | undefined;
  const [name, setName] = useState(typeof saved?.name === "string" ? saved.name : "");
  const [path, setPath] = useState(typeof saved?.path === "string" ? saved.path : "");
  const [argumentsText, setArgumentsText] = useState(Array.isArray(saved?.args) ? saved.args.join("\n") : "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const client = useQueryClient();
  const notify = useUI(state => state.notify);

  async function choose() {
    setError(null);
    try {
      const selected = await chooseGameProgram(game.rootPath);
      if (typeof selected === "string") {
        setPath(selected);
        setName(previous => previous.trim() ? previous : pathBasename(selected).replace(/\.exe$/i, ""));
      }
    } catch (failure) { setError(failure); }
  }

  async function save(launcher: ModLauncher | null) {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await api("set_game_mod_launcher", { id: game.id, launcher });
      await invalidateGameQueries(client, { gameIds: [game.id] });
      notify(tr("已保存游戏设置"));
      onClose();
    } catch (failure) { setError(failure); }
    finally { setBusy(false); }
  }

  return <Modal title={tr("设置 Mod 启动器")} description={game.name} className="sm:max-w-lg"
    onClose={() => { if (!busy) onClose(); }} onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}
    actions={<>
      {saved != null && <Button type="button" variant="ghost" className="sm:mr-auto" disabled={busy} onClick={() => void save(null)}>{tr("移除启动器")}</Button>}
      <Button type="button" variant="outline" disabled={busy} onClick={onClose}>{tr("取消")}</Button>
      <Button type="submit" form="mod-launcher-form" disabled={busy || !name.trim() || !path.trim()}>{busy && <Spinner />}{tr("保存")}</Button>
    </>}>
    <ErrorState error={error} />
    <form id="mod-launcher-form" onSubmit={event => {
      event.preventDefault();
      if (name.trim() && path.trim()) void save({ name: name.trim(), path, args: argumentsText.split(/\r?\n/).filter(arg => arg.trim().length > 0) });
    }}>
      <FieldGroup>
        <Field>
          <FieldLabel htmlFor="mod-launcher-name">{tr("启动器名称")}</FieldLabel>
          <Input id="mod-launcher-name" required disabled={busy} value={name} onChange={event => setName(event.target.value)} />
        </Field>
        <Field>
          <FieldLabel htmlFor="mod-launcher-path">{tr("启动器程序")}</FieldLabel>
          <InputGroup>
            <InputGroupInput id="mod-launcher-path" required disabled={busy} aria-describedby="mod-launcher-path-hint" value={displayPath(path)} onChange={event => setPath(event.target.value)} />
            <InputGroupAddon align="inline-end"><InputGroupButton type="button" disabled={busy} onClick={() => void choose()}><FolderOpenIcon />{tr("选择")}</InputGroupButton></InputGroupAddon>
          </InputGroup>
          <FieldDescription id="mod-launcher-path-hint">{tr("请选择游戏目录内的启动器程序。")}</FieldDescription>
        </Field>
        <Field>
          <FieldLabel htmlFor="mod-launcher-args">{tr("启动参数")}</FieldLabel>
          <Textarea id="mod-launcher-args" className="font-mono" rows={3} spellCheck={false} disabled={busy} aria-describedby="mod-launcher-args-hint" value={argumentsText} onChange={event => setArgumentsText(event.target.value)} />
          <FieldDescription id="mod-launcher-args-hint">{tr("可留空。每行一个参数，含空格的路径也只需一行。")}</FieldDescription>
        </Field>
      </FieldGroup>
    </form>
  </Modal>;
}

export function GameLaunchButton({ game, disabled, working, writable, onLaunch }: {
  game: Game; disabled: boolean; working: boolean; writable: boolean; onLaunch: (method?: LaunchMethod) => void;
}) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const options = useQuery({
    queryKey: ["game-launch-options", game.id],
    queryFn: () => api<LaunchOption[]>("list_game_launch_options", { id: game.id }),
    enabled: open, staleTime: 0, retry: false, networkMode: "always",
  });

  return <div className="flex min-w-0 flex-1">
    <Button size="sm" className="min-w-0 flex-1 rounded-r-none focus-visible:z-10" disabled={disabled || working} onClick={() => onLaunch()}>
      {working ? <Spinner /> : <PlayIcon />}<span className="truncate">{tr("启动游戏")}</span>
    </Button>
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild><Button ref={trigger} size="icon-sm" className="rounded-l-none border-l border-primary-foreground/30" disabled={disabled || working}
        aria-label={tr("选择启动方式")} title={tr("选择启动方式")}><ChevronDownIcon /></Button></DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-44 max-w-[min(20rem,calc(100vw-1rem))]" onCloseAutoFocus={event => { if (editing) event.preventDefault(); }}>
        {options.isFetching ? <p role="status" className="flex items-center gap-2 px-2 py-1.5 text-sm text-muted-foreground"><Spinner />{tr("读取中…")}</p>
          : options.isError ? <ErrorState error={options.error} />
          : options.data?.length ? <DropdownMenuGroup>{options.data.map(option => <DropdownMenuItem key={option.id} onSelect={() => onLaunch(option.id)}>
              {option.id === "steam" || option.id === "direct" ? <PlayIcon /> : <PuzzleIcon />}<span className="truncate">{optionName(option)}</span>
            </DropdownMenuItem>)}</DropdownMenuGroup>
          : <p className="px-2 py-1.5 text-sm text-muted-foreground">{tr("暂无可用的启动方式")}</p>}
        <DropdownMenuSeparator />
        <DropdownMenuGroup><DropdownMenuItem disabled={!writable} onSelect={() => setEditing(true)}><Settings2Icon />{tr("设置 Mod 启动器")}</DropdownMenuItem></DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
    {editing && <ModLauncherDialog game={game} onClose={() => setEditing(false)} restoreFocus={() => trigger.current?.focus()} />}
  </div>;
}
