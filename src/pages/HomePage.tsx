import { useState, type PointerEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowRightIcon, ArrowUpRightIcon, CompassIcon, Gamepad2Icon, HeartIcon, Layers3Icon, LibraryIcon, SlidersHorizontalIcon, SparklesIcon, UsersIcon } from "lucide-react";
import { SponsorLink } from "@/components/SponsorLink";
import { ErrorState, Page } from "@/components/ui";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardTitle } from "@/components/ui/card";
import { useBackendQuery } from "@/lib/api";
import { useDashboardStats } from "@/lib/dashboard-stats";
import { tr, useLocale } from "@/lib/i18n";
import { stagger } from "@/lib/motion";
import { loadSponsors, sponsorHost, type Sponsor } from "@/lib/sponsors";
import { pendingOnboardingMigration, useUI } from "@/lib/store";
import type { BootstrapState, Game } from "@/lib/types";
import { cn } from "@/lib/utils";

function sponsorMonogram(name: string) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return [...name.trim()].slice(0, 2).join("").toUpperCase();
}

function updateCardSpotlight(event: PointerEvent<HTMLElement>) {
  const element = event.currentTarget;
  const rect = element.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const x = event.clientX - rect.left;
  const y = event.clientY - rect.top;
  element.style.setProperty("--spot-x", `${x.toFixed(1)}px`);
  element.style.setProperty("--spot-y", `${y.toFixed(1)}px`);
  if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
    const ratioX = (x / rect.width - 0.5) * 2;
    const ratioY = (y / rect.height - 0.5) * 2;
    element.style.setProperty("--tilt-x", `${(-ratioY * 3.5).toFixed(2)}deg`);
    element.style.setProperty("--tilt-y", `${(ratioX * 3.5).toFixed(2)}deg`);
  } else {
    element.style.setProperty("--tilt-x", "0deg");
    element.style.setProperty("--tilt-y", "0deg");
  }
}

function resetCardSpotlight(event: PointerEvent<HTMLElement>) {
  const element = event.currentTarget;
  element.style.setProperty("--tilt-x", "0deg");
  element.style.setProperty("--tilt-y", "0deg");
}

function SponsorItemCard({ sponsor, index }: { sponsor: Sponsor; index: number }) {
  const [failedLogo, setFailedLogo] = useState<string | null>(null);
  const [loadedLogo, setLoadedLogo] = useState<string | null>(null);
  const logoSrc = sponsor.logo && sponsor.logo !== failedLogo ? sponsor.logo : null;
  const host = sponsorHost(sponsor.url);
  const monogram = sponsorMonogram(sponsor.name);
  const subtitle = host ?? tr("特别鸣谢");
  const cardBody = <>
    <div aria-hidden="true" className="relative flex size-12 shrink-0 items-center justify-center">
      <div className="relative flex size-12 items-center justify-center overflow-hidden rounded-xl border bg-muted/50 p-1.5 shadow-2xs">
        {logoSrc ? <img
          key={logoSrc}
          src={logoSrc}
          alt=""
          loading="lazy"
          decoding="async"
          referrerPolicy="no-referrer"
          onLoad={() => setLoadedLogo(logoSrc)}
          onError={() => setFailedLogo(logoSrc)}
          className={cn(
            "size-full rounded-lg object-contain motion-safe:transition-[opacity,scale] motion-safe:duration-300 motion-safe:ease-out",
            loadedLogo === logoSrc ? "scale-100 opacity-100 motion-safe:group-hover/sponsor:scale-110" : "motion-safe:scale-95 motion-safe:opacity-0",
          )}
        /> : <span
          data-monogram={monogram}
          className="text-xs font-bold tracking-wider text-foreground/85 after:content-[attr(data-monogram)] motion-safe:transition-transform motion-safe:duration-300 group-hover/sponsor:scale-110"
        />}
      </div>
    </div>
    <div className="min-w-0 flex-1">
      <span className="block truncate text-sm font-semibold tracking-tight text-card-foreground" dir="auto">{sponsor.name}</span>
      <span
        aria-hidden="true"
        data-subtitle={subtitle}
        className={cn(
          "mt-0.5 block truncate text-xs text-muted-foreground after:content-[attr(data-subtitle)] motion-safe:transition-colors group-hover/sponsor:text-foreground",
          host && "font-mono text-[11px]",
        )}
      />
    </div>
    <span
      aria-hidden="true"
      className={cn(
        "flex size-8 shrink-0 items-center justify-center rounded-full border bg-background/75 text-muted-foreground transition-colors duration-200",
        sponsor.url
          ? "group-hover/sponsor:border-foreground/30 group-hover/sponsor:bg-foreground group-hover/sponsor:text-background"
          : "group-hover/sponsor:border-foreground/25 group-hover/sponsor:bg-muted group-hover/sponsor:text-foreground",
      )}
    >
      {sponsor.url
        ? <ArrowUpRightIcon className="size-4 motion-safe:transition-transform motion-safe:duration-200 group-hover/sponsor:translate-x-0.5 group-hover/sponsor:-translate-y-0.5" />
        : <SparklesIcon className="size-3.5 motion-safe:transition-transform motion-safe:duration-300 group-hover/sponsor:scale-110 group-hover/sponsor:rotate-12" />}
    </span>
  </>;

  const sharedClass = "sponsor-card-surface group/sponsor flex h-full w-full items-center gap-3.5 rounded-xl border p-3.5 sm:p-4";
  return <li className="min-w-0 motion-safe:animate-soft-fade" style={stagger(index, 55, 12)}>
    {sponsor.url ? <SponsorLink
      url={sponsor.url}
      data-slot="sponsor-card"
      title={host ? `${sponsor.name} · ${host}` : sponsor.name}
      onPointerMove={updateCardSpotlight}
      onPointerLeave={resetCardSpotlight}
      className={cn(sharedClass, "outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50")}
    >
      {cardBody}
    </SponsorLink> : <div
      data-slot="sponsor-card"
      title={sponsor.name}
      onPointerMove={updateCardSpotlight}
      onPointerLeave={resetCardSpotlight}
      className={sharedClass}
    >
      {cardBody}
    </div>}
  </li>;
}

function SponsorsCard() {
  const locale = useLocale();
  const sponsors = useQuery({
    queryKey: ["dashboard-sponsors"], queryFn: loadSponsors,
    staleTime: 15 * 60 * 1000, retry: false,
  });
  return <Card className="items-stretch gap-4 p-5 shadow-xs sm:gap-5 sm:p-6">
    <div className="flex flex-wrap items-center justify-between gap-3">
      <div className="flex items-center gap-3">
        <span aria-hidden="true" className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/85">
          <UsersIcon className="size-5" />
        </span>
        <div className="flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-2">
            <CardTitle className="text-lg">{tr("赞助商")}</CardTitle>
            {!!sponsors.data?.length && <Badge variant="secondary" className="rounded-full px-2 py-0 text-xs tabular-nums">{sponsors.data.length.toLocaleString(locale)}</Badge>}
          </div>
          <p className="text-xs text-muted-foreground">{tr("感谢每一位赞助商与支持者让项目持续进化。")}</p>
        </div>
      </div>
    </div>

    {sponsors.data?.length ? <ul className="grid w-full grid-cols-[repeat(auto-fill,minmax(min(100%,16.5rem),1fr))] gap-3.5 sm:gap-4" aria-label={tr("赞助商")}>
      {sponsors.data.map((sponsor, index) => <SponsorItemCard key={sponsor.name} sponsor={sponsor} index={index} />)}
    </ul> : sponsors.isLoading ? <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground" role="status">{tr("正在加载赞助商名单…")}</p>
      <div aria-hidden="true" className="grid w-full grid-cols-[repeat(auto-fill,minmax(min(100%,16.5rem),1fr))] gap-3.5 sm:gap-4">
        {[0, 1, 2].map(item => <div key={item} className="flex items-center gap-3.5 rounded-xl border bg-muted/30 p-3.5 sm:p-4">
          <div className="size-12 shrink-0 rounded-xl bg-muted motion-safe:animate-pulse" />
          <div className="flex flex-1 flex-col gap-2">
            <div className="h-3.5 w-24 rounded-md bg-muted motion-safe:animate-pulse" />
            <div className="h-2.5 w-32 rounded-md bg-muted/70 motion-safe:animate-pulse" />
          </div>
        </div>)}
      </div>
    </div> : <div className="rounded-xl border border-dashed bg-muted/20 px-4 py-6 text-center">
      <p className="text-sm text-muted-foreground" role="status">{sponsors.data ? tr("暂无公开的赞助者。") : tr("暂时无法加载赞助商名单。")}</p>
    </div>}

    <div className="group/cta-bar flex flex-wrap items-center justify-between gap-3 rounded-xl border border-dashed bg-muted/25 px-4 py-3 transition-colors duration-200 hover:border-foreground/20 hover:bg-muted/45">
      <div className="flex items-center gap-2.5 text-sm text-muted-foreground">
        <HeartIcon aria-hidden="true" className="size-4 shrink-0 text-destructive motion-safe:transition-transform motion-safe:duration-300 group-hover/cta-bar:scale-110" />
        <span>{tr("支持管理器的开发与维护。")}</span>
      </div>
      <Button asChild size="sm" variant="outline" className="group/donate bg-background/80 hover:bg-muted">
        <SponsorLink>
          {tr("去赞助")}
          <ArrowUpRightIcon aria-hidden="true" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/donate:translate-x-0.5 group-hover/donate:-translate-y-0.5" />
        </SponsorLink>
      </Button>
    </div>
  </Card>;
}

export default function HomePage({ bootstrap, version }: { bootstrap?: BootstrapState; version: string }) {
  const navigate = useUI(state => state.navigate);
  const locale = useLocale();
  const selectGame = useUI(state => state.selectGame);
  const stats = useDashboardStats();
  const data = stats.data;
  const games = useBackendQuery<Game[]>(["games"], "list_games", undefined, !!data?.managedGameCount);
  const resume = bootstrap && (bootstrap.resumeOnboarding || !!pendingOnboardingMigration());
  const setup = resume || bootstrap?.isFirstRun;
  const number = (count?: number) => count?.toLocaleString(locale) ?? "—";
  const installRatio = data?.totalModCount ? Math.min(100, Math.max(0, (100 * data.installedModCount) / data.totalModCount)) : 0;

  return <Page data-testid="home-page" className="max-w-6xl gap-4 lg:gap-6">
    <header className="flex flex-col gap-5 rounded-2xl border bg-card p-5 text-card-foreground shadow-xs sm:gap-6 sm:p-7">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-3">
        <div className="flex flex-col gap-1.5">
          <h1 className="text-3xl leading-tight font-semibold tracking-tight text-balance sm:text-4xl">{tr("欢迎来到 Agent Mod Manager")}</h1>
          <p className="text-sm text-muted-foreground">{data?.managedGameCount === 0 ? tr("打开游戏库，添加你想管理的游戏。") : tr("继续管理游戏，或为它们寻找新的 Mod。")}</p>
        </div>
        <p className="inline-flex items-center gap-2 rounded-full border bg-muted/40 px-3 py-1 text-xs text-muted-foreground">
          <span aria-hidden="true" className="relative flex size-2">
            <span className="absolute inline-flex size-full rounded-full bg-success motion-safe:animate-soft-ping" />
            <span className="relative inline-flex size-2 rounded-full bg-success" />
          </span>
          {tr("当前版本")} <span className="font-mono font-medium text-foreground">{version}</span>
        </p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2" aria-busy={stats.isLoading}>
        <div className="group/stat flex min-w-0 flex-col gap-3 rounded-xl border bg-muted/25 p-4 transition-colors duration-200 hover:border-foreground/20 hover:bg-muted/45 sm:p-5">
          <CardTitle className="flex items-center gap-2.5 text-sm font-medium text-muted-foreground">
            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-background/80 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/stat:scale-110 group-hover/stat:-rotate-3">
              <Gamepad2Icon aria-hidden="true" className="size-4" />
            </span>
            {tr("管理的游戏")}
          </CardTitle>
          <p className="text-4xl font-semibold tracking-tight tabular-nums sm:text-5xl">{number(data?.managedGameCount)}</p>
          {!!data?.managedGameCount && !!games.data?.length && <ul className="mt-auto flex flex-wrap items-center gap-1.5 pt-1 text-sm">
            {games.data.slice(0, 3).map(game => <li key={game.id} className="min-w-0 max-w-full">
              <button
                type="button"
                onClick={() => selectGame(game.id)}
                className="inline-flex max-w-full items-center rounded-full border bg-background/85 px-2.5 py-0.5 text-xs font-medium text-foreground transition-colors duration-200 hover:border-foreground/25 hover:bg-muted focus-visible:outline-ring motion-safe:transition-[transform,background-color,border-color] motion-safe:hover:-translate-y-0.5"
                title={game.name}
              >
                <span className="truncate">{game.name}</span>
              </button>
            </li>)}
            {games.data.length > 3 && <li>
              <button type="button" onClick={() => navigate("library")} className="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground">
                {tr("全部游戏")} →
              </button>
            </li>}
          </ul>}
        </div>

        <div className="group/stat flex min-w-0 flex-col gap-3 rounded-xl border bg-muted/25 p-4 transition-colors duration-200 hover:border-foreground/20 hover:bg-muted/45 sm:p-5">
          <div className="flex items-center justify-between gap-2">
            <CardTitle className="flex items-center gap-2.5 text-sm font-medium text-muted-foreground">
              <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-background/80 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/stat:scale-110 group-hover/stat:rotate-3">
                <Layers3Icon aria-hidden="true" className="size-4" />
              </span>
              {tr("已安装的 Mod")} / {tr("全部 Mod")}
            </CardTitle>
            {data && data.totalModCount > 0 && <span aria-hidden="true" className="rounded-full border bg-background/80 px-2 py-0.5 font-mono text-xs font-medium text-muted-foreground tabular-nums">{Math.round(installRatio)}%</span>}
          </div>
          <p className="flex flex-wrap items-baseline gap-x-2 text-4xl font-semibold tracking-tight tabular-nums sm:text-5xl">
            <span>{number(data?.installedModCount)}</span> <span className="text-xl font-normal text-muted-foreground">/</span> <span className="text-2xl text-muted-foreground">{number(data?.totalModCount)}</span>
          </p>
          {data && <div aria-hidden="true" className="mt-auto pt-1">
            <div className="h-2 overflow-hidden rounded-full bg-muted p-0.5">
              <div className="h-full rounded-full bg-primary motion-safe:transition-[width] motion-safe:duration-700 motion-safe:ease-out" style={{ width: `${installRatio}%` }} />
            </div>
          </div>}
        </div>
      </div>

      <ErrorState error={stats.error} />

      <div className="flex flex-wrap items-center gap-2.5">
        <Button onClick={() => navigate("agent")} className="group/cta">
          <SparklesIcon aria-hidden="true" className="motion-safe:transition-transform motion-safe:duration-300 group-hover/cta:scale-110 group-hover/cta:rotate-12" />
          {tr("开始使用")}
          <ArrowRightIcon aria-hidden="true" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/cta:translate-x-1" />
        </Button>
        <Button variant="outline" aria-label={tr("前往游戏库")} onClick={() => navigate("library")} className="group/nav bg-background/65 hover:bg-muted/60">
          <LibraryIcon aria-hidden="true" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/nav:-translate-y-0.5" />
          {tr("游戏库")}
        </Button>
        <Button variant="outline" aria-label={tr("前往游览")} onClick={() => navigate("browse")} className="group/nav bg-background/65 hover:bg-muted/60">
          <CompassIcon aria-hidden="true" className="motion-safe:transition-transform motion-safe:duration-300 group-hover/nav:rotate-45" />
          {tr("游览")}
        </Button>
        <Button variant="outline" aria-label={tr("前往设置")} onClick={() => navigate("settings")} className="group/nav bg-background/65 hover:bg-muted/60">
          <SlidersHorizontalIcon aria-hidden="true" className="motion-safe:transition-transform motion-safe:duration-300 group-hover/nav:scale-110" />
          {tr("设置")}
        </Button>
      </div>

      {setup && <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border bg-muted/35 px-4 py-3 text-sm text-muted-foreground">
        <p>{tr("先设置存储目录，再扫描要管理的游戏。")}</p>
        <Button size="sm" variant="outline" className="bg-background/80 hover:bg-background" onClick={() => navigate("onboarding")}>{resume ? tr("继续设置") : tr("开始设置")}</Button>
      </div>}
    </header>
    <SponsorsCard />
  </Page>;
}
