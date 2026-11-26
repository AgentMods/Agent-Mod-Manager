import { useQuery } from "@tanstack/react-query";
import { ArrowRightIcon, ArrowUpRightIcon, CompassIcon, Gamepad2Icon, LibraryIcon, Layers3Icon, SettingsIcon, UsersIcon } from "lucide-react";
import { SponsorLink } from "@/components/SponsorLink";
import { ErrorState, Page } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Card, CardTitle } from "@/components/ui/card";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useDashboardStats } from "@/lib/dashboard-stats";
import { tr, useLocale } from "@/lib/i18n";
import { pendingOnboardingMigration, useUI } from "@/lib/store";
import type { BootstrapState, Game } from "@/lib/types";

function SponsorsCard() {
  const sponsors = useQuery({
    queryKey: ["dashboard-sponsors"], queryFn: () => api<string[]>("list_dashboard_sponsors"),
    enabled: backendAvailable, staleTime: 15 * 60 * 1000, retry: false,
  });
  return <Card className="items-start gap-3 p-5 shadow-none sm:p-6">
    <CardTitle className="flex items-center gap-2.5 text-lg"><UsersIcon aria-hidden="true" className="size-5 text-muted-foreground" />{tr("赞助商")}</CardTitle>
    {sponsors.data?.length ? <ul className="flex flex-wrap gap-1.5" aria-label={tr("赞助商")}>
      {sponsors.data.map(name => <li key={name} className="max-w-full rounded-md bg-muted px-2.5 py-1 text-xs break-all" dir="auto">{name}</li>)}
    </ul> : <p className="text-sm text-muted-foreground" role="status">{sponsors.data ? tr("暂无公开的赞助者。") : sponsors.isLoading ? tr("正在加载赞助商名单…") : tr("暂时无法加载赞助商名单。")}</p>}
    <Button asChild size="sm" variant="outline"><SponsorLink>{tr("去赞助")}<ArrowUpRightIcon aria-hidden="true" /></SponsorLink></Button>
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
  return <Page data-testid="home-page" className="max-w-6xl gap-4 lg:gap-6">
    <header className="flex flex-col gap-5 rounded-xl border bg-card p-5 text-card-foreground sm:gap-6 sm:p-7">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-2">
        <h1 className="text-3xl leading-tight font-semibold tracking-tight text-balance sm:text-4xl">{tr("欢迎来到 Agent Mod Manager")}</h1>
        <p className="flex items-center gap-2 text-sm text-muted-foreground">{tr("当前版本")} <span className="font-mono">{version}</span></p>
      </div>
      <div className="grid gap-x-6 gap-y-4 border-y py-5 sm:grid-cols-2" aria-busy={stats.isLoading}>
        <div className="flex min-w-0 flex-col gap-2">
          <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground"><Gamepad2Icon aria-hidden="true" className="size-4" />{tr("管理的游戏")}</CardTitle>
          <p className="text-4xl font-semibold tracking-tight tabular-nums sm:text-5xl">{number(data?.managedGameCount)}</p>
          {!!data?.managedGameCount && !!games.data?.length && <ul className="flex flex-wrap gap-x-3 gap-y-1 text-sm">
            {games.data.slice(0, 3).map(game => <li key={game.id} className="min-w-0 max-w-full"><button onClick={() => selectGame(game.id)} className="block max-w-full truncate rounded-sm text-accent-foreground underline-offset-4 hover:underline focus-visible:outline-ring" title={game.name}>{game.name}</button></li>)}
            {games.data.length > 3 && <li><button onClick={() => navigate("library")} className="text-muted-foreground underline-offset-4 hover:underline">{tr("全部游戏")} →</button></li>}
          </ul>}
        </div>
        <div className="flex min-w-0 flex-col gap-2 sm:border-l sm:pl-6">
          <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground"><Layers3Icon aria-hidden="true" className="size-4" />{tr("已安装的 Mod")} / {tr("全部 Mod")}</CardTitle>
          <p className="flex flex-wrap items-baseline gap-x-2 text-4xl font-semibold tracking-tight tabular-nums sm:text-5xl"><span>{number(data?.installedModCount)}</span> <span className="text-xl font-normal text-muted-foreground">/</span> <span className="text-2xl text-muted-foreground">{number(data?.totalModCount)}</span></p>
          {data && <div aria-hidden="true" className="mt-1 h-1.5 overflow-hidden rounded-full bg-muted"><div className="h-full rounded-full bg-primary" style={{ width: `${data.totalModCount ? 100 * data.installedModCount / data.totalModCount : 0}%` }} /></div>}
        </div>
      </div>
      <ErrorState error={stats.error} />
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => navigate("agent")}>{tr("开始使用")}<ArrowRightIcon /></Button>
        <Button variant="outline" aria-label={tr("前往游戏库")} onClick={() => navigate("library")}><LibraryIcon />{tr("游戏库")}</Button>
        <Button variant="outline" aria-label={tr("前往游览")} onClick={() => navigate("browse")}><CompassIcon />{tr("游览")}</Button>
        <Button variant="outline" aria-label={tr("前往设置")} onClick={() => navigate("settings")}><SettingsIcon />{tr("设置")}</Button>
      </div>
      {setup && <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
        <p>{tr("先设置存储目录，再扫描要管理的游戏。")}</p>
        <Button size="sm" variant="outline" onClick={() => navigate("onboarding")}>{resume ? tr("继续设置") : tr("开始设置")}</Button>
      </div>}
    </header>
    <SponsorsCard />
  </Page>;
}
