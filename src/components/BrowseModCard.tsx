import { useState } from "react";
import { DownloadIcon, ExternalLinkIcon, ImageIcon, LanguagesIcon, UserRoundIcon } from "lucide-react";
import { browseTypes, type BrowseItem } from "@/lib/browse";
import { tr } from "@/lib/i18n";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Item, ItemContent, ItemFooter } from "./ui/item";
import { Tooltip, TooltipContent, TooltipTrigger } from "./ui/tooltip";

interface BrowseModCardProps {
  item: BrowseItem;
  source: string;
  gameNames: string;
  onOpen: () => void;
  onInstall: () => void;
}

export function BrowseModCard({ item, source, gameNames, onOpen, onInstall }: BrowseModCardProps) {
  const [failedCover, setFailedCover] = useState<string | null>(null);
  const [showOriginal, setShowOriginal] = useState(false);
  const translated = item.title !== item.originalTitle;
  const openLabel = tr("在浏览器中查看《{title}》", { title: item.title });
  const unavailable = tr("来源未提供可核验的作品网页");
  const coverClass = "flex aspect-[2/1] w-full shrink-0 items-center justify-center overflow-hidden bg-muted";
  const cover =
    item.coverUrl && item.coverUrl !== failedCover ? (
      <img
        alt=""
        src={item.coverUrl}
        loading="lazy"
        decoding="async"
        referrerPolicy="no-referrer"
        onError={() => setFailedCover(item.coverUrl)}
        className="size-full object-cover"
      />
    ) : (
      <ImageIcon aria-hidden="true" className="size-8 text-muted-foreground/60" />
    );
  const title = item.canOpenProject ? (
    <button
      type="button"
      onClick={onOpen}
      aria-label={openLabel}
      className="text-start hover:underline focus-visible:underline"
    >
      {item.title}{" "}
      <ExternalLinkIcon aria-hidden="true" className="inline size-3.5 align-baseline text-muted-foreground" />
    </button>
  ) : (
    <span tabIndex={translated ? 0 : undefined}>{item.title}</span>
  );

  return (
    <Item
      asChild
      variant="outline"
      className="min-w-0 flex-col flex-nowrap items-stretch gap-0 overflow-hidden rounded-xl bg-card p-0 text-card-foreground"
    >
      <article aria-label={item.title}>
        {item.canOpenProject ? (
          <button type="button" tabIndex={-1} aria-label={openLabel} onClick={onOpen} className={coverClass}>
            {cover}
          </button>
        ) : (
          <div className={coverClass}>{cover}</div>
        )}
        <ItemContent className="min-w-0 gap-1.5 px-4 py-3">
          <div className="flex items-start gap-1.5">
            <h2 className="min-w-0 text-base leading-snug font-semibold wrap-anywhere">
              {translated ? (
                <Tooltip>
                  <TooltipTrigger asChild>{title}</TooltipTrigger>
                  <TooltipContent className="max-w-72 wrap-anywhere">{item.originalTitle}</TooltipContent>
                </Tooltip>
              ) : (
                title
              )}
            </h2>
            {translated && (
              <button
                type="button"
                aria-label={tr("查看原名")}
                title={tr("查看原名")}
                aria-expanded={showOriginal}
                onClick={() => setShowOriginal((value) => !value)}
                className="shrink-0 rounded-sm p-1 text-muted-foreground hover:bg-accent"
              >
                <LanguagesIcon aria-hidden="true" className="size-3.5" />
              </button>
            )}
          </div>
          {translated && showOriginal && (
            <p className="text-xs wrap-anywhere text-muted-foreground">{item.originalTitle}</p>
          )}
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <UserRoundIcon aria-hidden="true" className="size-3.5 shrink-0" />
            <span className="truncate" title={item.author ?? undefined}>
              {item.author ? tr("作者：{name}", { name: item.author }) : tr("作者未提供")}
            </span>
          </div>
          <p className="line-clamp-2 text-sm leading-relaxed wrap-anywhere">{item.summary || tr("暂无简介")}</p>
          <div className="flex min-w-0 flex-wrap items-center gap-x-1.5 text-xs text-muted-foreground">
            <span className="wrap-anywhere">{source}</span>
            {gameNames && (
              <>
                <span aria-hidden="true">·</span>
                <span className="wrap-anywhere">{gameNames}</span>
              </>
            )}
          </div>
          {!!item.tags.length && (
            <div className="flex flex-wrap gap-1 pt-0.5" aria-label={tr("标签")}>
              {item.tags.map((tag) => (
                <Badge key={tag.key} variant="secondary" className="max-w-full">
                  <span className="truncate" title={tag.label}>
                    {tag.label}
                  </span>
                </Badge>
              ))}
            </div>
          )}
          {!item.canOpenProject && <p className="text-xs text-muted-foreground">{unavailable}</p>}
        </ItemContent>
        <ItemFooter className="basis-auto flex-wrap gap-2 border-t px-3 py-2.5">
          <Badge variant="outline">{browseTypes()[item.typeKey]}</Badge>
          <Button type="button" size="sm" className="ms-auto" disabled={!item.gameIds.length} onClick={onInstall}>
            <DownloadIcon data-icon="inline-start" />
            {tr("安装")}
          </Button>
        </ItemFooter>
      </article>
    </Item>
  );
}
