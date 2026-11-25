import { useRef } from "react";
import { CopyIcon, EllipsisIcon, FolderIcon, GlobeIcon, PlusIcon } from "lucide-react";
import type { CloudGameMatch } from "@/lib/cloud-game-match";
import { candidateWarnings } from "@/lib/game-discovery";
import { tr } from "@/lib/i18n";
import { stagger } from "@/lib/motion";
import { displayPath } from "@/lib/paths";
import { useUI } from "@/lib/store";
import type { GameCandidate } from "@/lib/types";
import { CatalogGameCover } from "@/components/CatalogGameCover";
import { GameCover } from "@/components/GameCover";
import { GameMatchBadge } from "@/components/GameMatchBadge";
import { PathText } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Item, ItemActions, ItemFooter } from "@/components/ui/item";

type Props = {
  candidate: GameCandidate; index: number; coverUrl?: string | null; matched?: CloudGameMatch;
  waiting: boolean; failed: boolean; disableRegister: boolean; disableIdentify: boolean;
  onRegister: (localOnly?: boolean) => void; onIdentify: (trigger: HTMLButtonElement) => void;
};

export function ScanGameCard({ candidate, index, coverUrl, matched, waiting, failed, disableRegister, disableIdentify, onRegister, onIdentify }: Props) {
  const notify = useUI(state => state.notify);
  const more = useRef<HTMLButtonElement>(null);
  const openingIdentity = useRef(false);
  const name = matched?.gameName ?? candidate.name;
  const path = displayPath(candidate.rootPath);
  const managed = !!candidate.registeredGameId;
  async function copyDirectory() {
    try { await navigator.clipboard.writeText(path); notify(tr("已复制")); }
    catch { notify(tr("复制失败，请重试")); }
  }
  return <Item role="listitem" variant="outline" className="min-w-0 flex-col flex-nowrap items-stretch gap-0 overflow-hidden rounded-xl bg-card p-0 text-card-foreground motion-safe:animate-rise-in" style={stagger(index)}>
    {matched ? <CatalogGameCover name={name} releaseId={matched.releaseId} className="aspect-[2/1] rounded-none" /> : <GameCover name={name} url={coverUrl} />}
    <div className="flex min-w-0 flex-1 flex-col gap-1.5 px-4 py-3">
      <h3 className="text-base leading-snug font-semibold wrap-anywhere">{name}</h3>
      <div className="flex min-w-0 items-start gap-1.5 text-muted-foreground">
        <FolderIcon aria-hidden="true" className="mt-0.5 size-3.5 shrink-0" />
        <PathText dir="ltr" title={path} className="min-w-0 wrap-anywhere break-normal">{path}</PathText>
      </div>
      {matched && <p className="text-xs leading-relaxed wrap-anywhere text-muted-foreground">{matched.pluginName} · {matched.reusable ? tr("将校验并复用已安装适配包") : tr("适配包将随添加安装")}</p>}
      {candidateWarnings(candidate).map(warning => <p key={warning} className="text-xs wrap-anywhere text-warning">{warning}</p>)}
    </div>
    <ItemFooter className="basis-auto flex-wrap gap-2 border-t px-3 py-2.5">
      <GameMatchBadge candidate={candidate} loading={waiting} cloudMatched={!!matched} />
      <ItemActions className="ml-auto gap-1.5">
        <Button type="button" size="sm" variant={managed ? "outline" : "default"} disabled={disableRegister || managed || waiting} onClick={() => onRegister()}>
          {!managed && !waiting && <PlusIcon data-icon="inline-start" />}{managed ? tr("管理中") : waiting ? tr("正在匹配…") : tr("添加")}
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild><Button ref={more} type="button" size="icon-sm" variant="outline" aria-label={tr("更多操作")} title={tr("更多操作")}><EllipsisIcon /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end" onCloseAutoFocus={event => {
            if (openingIdentity.current) { event.preventDefault(); openingIdentity.current = false; }
          }}>
            <DropdownMenuGroup>
              <DropdownMenuItem onSelect={() => void copyDirectory()}><CopyIcon />{tr("复制游戏目录")}</DropdownMenuItem>
              {!managed && !matched && !waiting && !candidate.suggestedAdapterSlug && <DropdownMenuItem disabled={disableIdentify} onSelect={() => {
                if (more.current) { openingIdentity.current = true; onIdentify(more.current); }
              }}><GlobeIcon />{tr("联网识别")}</DropdownMenuItem>}
              {!managed && (waiting || failed) && <DropdownMenuItem disabled={disableRegister} onSelect={() => onRegister(true)}><PlusIcon />{tr("仅添加本地")}</DropdownMenuItem>}
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </ItemActions>
    </ItemFooter>
  </Item>;
}
