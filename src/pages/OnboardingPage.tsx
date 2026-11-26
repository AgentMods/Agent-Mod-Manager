import { tr, useLocale } from "@/lib/i18n";
import { useCloudGameMatch } from '@/lib/cloud-game-match';
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeftIcon, ArrowRightIcon, BotIcon, CheckIcon, CircleAlertIcon, CircleCheckIcon, FolderOpenIcon, Gamepad2Icon, MoonIcon, PlugIcon, RadarIcon, ScanSearchIcon, SunIcon, TriangleAlertIcon } from "lucide-react";
import { api, errorText, useBackendQuery } from "@/lib/api";
import { stagger } from "@/lib/motion";
import { pendingOnboardingMigration, rememberOnboardingMigration, useUI } from "@/lib/store";
import type { AppSettings, BootstrapState, Game, GameCandidate, ScanReport } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { AppLogo } from "@/components/AppLogo";
import { StorageDirectory } from "@/components/StorageDirectory";
import { McpTools } from "@/components/McpTools";
import { ScanGameCard, ScanGameGrid } from "@/components/ScanGameCard";
import { PathText } from "@/components/ui";
import { displayPath, gameDirectoryKey } from "@/lib/paths";
import { reconcileCandidates } from "@/lib/game-discovery";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

const steps = [
  { id: "storage", icon: FolderOpenIcon, get title() { return tr("设置数据存储目录"); }, get description() { return tr("选择存放 Mod 下载、数据库和快照的位置，也可以直接使用当前目录。"); } },
  { id: "agent", icon: BotIcon, get title() { return tr("配置 AI 模型服务"); }, get description() { return tr("为 Agent 设置服务地址、默认模型和权限。你也可以稍后在设置中配置。"); } },
  { id: "mcp", icon: PlugIcon, get title() { return tr("启用可用工具"); }, get description() { return tr("选择 Agent 可以调用的 MCP 工具，或直接继续。"); } },
  { id: "scan", icon: RadarIcon, get title() { return tr("扫描游戏列表"); }, get description() { return tr("在 Steam、Epic 等常见位置查找游戏。确认游戏目录后，点击「添加」即可加入游戏库。"); } },
  { id: "done", icon: CircleCheckIcon, get title() { return tr("完成"); }, get description() { return tr("确认当前设置后即可进入游戏库。"); } },
] as const;

export default function OnboardingPage({ automatic = false, bootstrap }: { automatic?: boolean; bootstrap: BootstrapState }) {
  useLocale();
  const { dataRoot, recoveryError } = bootstrap;
  const navigate = useUI((state) => state.navigate);
  const theme = useUI((state) => state.theme);
  const toggleTheme = useUI((state) => state.toggleTheme);
  const client = useQueryClient();
  const [step, setStep] = useState(() => {
    const previousRoot = pendingOnboardingMigration();
    const migrated = previousRoot && !bootstrap.storageMigrationError && previousRoot !== dataRoot;
    return migrated ? 1 : 0;
  });
  useEffect(() => { rememberOnboardingMigration(null); }, []);
  const { id: panel, icon: StepIcon, title, description } = steps[step];
  const cloud = useCloudGameMatch(panel === 'scan');
  const [configurationBusy, setConfigurationBusy] = useState(false);
  const [report, setReport] = useState<ScanReport | null>(null);
  const games = useBackendQuery<Game[]>(["games"], "list_games", undefined, panel === "scan" && report !== null);
  const [candidateErrors, setCandidateErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<"scan" | "register" | "finish" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const navigationDisabled = (busy !== null && busy !== "scan") || configurationBusy;
  const heading = useRef<HTMLHeadingElement>(null);
  const previousStep = useRef(step);
  useEffect(() => {
    if (previousStep.current !== step) heading.current?.focus();
    previousStep.current = step;
  }, [step]);
  const resumeRecorded = useRef(false);
  const scanRequest = useRef<string | null>(null);
  const inFlightScan = useRef<string | null>(null);
  const autoScanned = useRef(false);
  useEffect(() => () => {
    const requestId = scanRequest.current;
    scanRequest.current = null;
    inFlightScan.current = null;
    if (requestId) void api("scan_games", { options: { page: { requestId, cancel: true } } }).catch(() => {});
  }, []);

  function moveTo(next: number) {
    setError(null);
    if (panel === "scan") stopScan();
    setStep(next);
    // Started here rather than in an effect so StrictMode cannot double-invoke the scan.
    if (steps[next]?.id === "scan" && !report && !autoScanned.current) {
      autoScanned.current = true;
      void scan();
    }
  }

  function stopScan() {
    const requestId = scanRequest.current;
    inFlightScan.current = null;
    scanRequest.current = null;
    setBusy((current) => (current === "scan" ? null : current));
    if (requestId) void api("scan_games", { options: { page: { requestId, cancel: true } } }).catch(() => {});
    setReport((current) => current && { ...current, nextCursor: null });
  }
  async function scan(cursor?: string) {
    if (busy === "register" || busy === "finish") return;
    if (!cursor) stopScan();
    const requestId = cursor ?? crypto.randomUUID();
    scanRequest.current = requestId;
    inFlightScan.current = requestId;
    setBusy("scan");
    setError(null);
    try {
      const result = await api<ScanReport>("scan_games", { options: { page: cursor ? { cursor } : { requestId } } });
      if (scanRequest.current !== requestId) return;
      scanRequest.current = result.nextCursor ?? null;
      setReport({ ...result, candidates: reconcileCandidates(result.candidates) });
      setCandidateErrors({});
    } catch (failure) {
      if (scanRequest.current === requestId) { stopScan(); setError(errorText(failure)); }
    } finally {
      if (inFlightScan.current === requestId) {
        inFlightScan.current = null;
        setBusy((current) => (current === "scan" ? null : current));
      }
    }
  }

  async function register(candidate: GameCandidate, localOnly = false) {
    const key = gameDirectoryKey(candidate.rootPath);
    if (busy || recoveryError || candidate.registeredGameId) return;
    setBusy("register");
    setCandidateErrors((previous) => ({ ...previous, [key]: "" }));
    try {
      if (automatic && !resumeRecorded.current) {
        await api("start_onboarding");
        resumeRecorded.current = true;
      }
      const game = await cloud.register(candidate, localOnly);
      setReport((previous) => previous && ({
        ...previous, candidates: previous.candidates.map((item) =>
          gameDirectoryKey(item.rootPath) === key ? { ...item, name: game.name, registeredGameId: game.id, suggestedAdapterSlug: game.adapterSlug } : item,
        )
      }));
    } catch (failure) {
      setCandidateErrors((previous) => ({ ...previous, [key]: errorText(failure) }));
    } finally {
      setBusy(null);
    }
  }

  async function finish() {
    if (navigationDisabled) return;
    stopScan();
    setBusy("finish");
    setError(null);
    try {
      await api("set_onboarding_completed", { completed: true });
      client.setQueryData<BootstrapState>(["bootstrap"], (previous) => previous && ({
        ...previous, onboardingCompleted: true, isFirstRun: false, resumeOnboarding: false,
      }));
      void client.invalidateQueries({ queryKey: ["settings"] });
      navigate("library");
    } catch (failure) {
      setError(errorText(failure));
    } finally {
      setBusy(null);
    }
  }

  return <div className="flex min-h-dvh flex-col overflow-x-hidden bg-muted/25 text-foreground">
    <header data-tauri-drag-region className="sticky top-0 z-10 flex items-center justify-between gap-4 border-b bg-background/90 px-4 py-3 backdrop-blur-md sm:px-8">
      <div className="flex items-center gap-2.5"><AppLogo /><strong>Agent Mod Manager</strong></div>
      <Button variant="ghost" size="icon" onClick={toggleTheme} aria-label={theme === "light" ? tr("切换暗色主题") : tr("切换亮色主题")}>
        {theme === "light" ? <MoonIcon className="motion-safe:animate-icon-swap" /> : <SunIcon className="motion-safe:animate-icon-swap" />}
      </Button>
    </header>

    <main className={cn(
      "mx-auto grid w-full items-start gap-5 px-4 py-8 sm:px-8 md:grid-cols-[14rem_minmax(0,1fr)] md:gap-7 md:py-12",
      panel === "scan" ? "max-w-7xl" : "max-w-5xl",
    )}>
      <header className="flex flex-col items-center gap-3 text-center md:col-span-2">
        <div aria-hidden="true" className="inline-flex items-center gap-1.5 rounded-full border bg-card px-3 py-1.5 shadow-2xs">
          {steps.map(({ id }, index) => <span key={id} className={cn(
            "h-1.5 rounded-full motion-safe:transition-[width,background-color] motion-safe:duration-300",
            index === step ? "w-6 bg-foreground" : index < step ? "w-2 bg-foreground/45" : "w-1.5 bg-muted-foreground/25",
          )} />)}
        </div>
        <h1 className="text-3xl font-semibold tracking-tight text-balance sm:text-4xl">{tr("欢迎使用 Agent Mod Manager")}</h1>
      </header>

      <div className="flex flex-col gap-2.5 rounded-2xl border bg-card p-4 shadow-2xs md:hidden">
        <span className="text-sm font-medium text-muted-foreground">{tr("步骤 {value1} / {length}", { value1: step + 1, length: steps.length })}</span>
        <Progress aria-label={tr("引导进度")} aria-valuemin={1} aria-valuemax={steps.length} aria-valuenow={step + 1}
          value={100 * (step + 1) / steps.length} aria-valuetext={tr("步骤 {value1} / {length}", { value1: step + 1, length: steps.length })}
          className="motion-reduce:[&_[data-slot=progress-indicator]]:transition-none" />
      </div>

      <ol className="hidden flex-col gap-1.5 rounded-2xl border bg-card p-3 shadow-xs md:flex" aria-label={tr("引导步骤")}>
        {steps.map(({ id, title }, index) => {
          const active = index === step;
          const completed = index < step;
          return <li
            key={id}
            aria-current={active ? "step" : undefined}
            style={stagger(index, 45)}
            className={cn(
              "relative flex items-center gap-3 rounded-xl px-2.5 py-2 text-sm text-muted-foreground motion-safe:animate-slide-in-left motion-safe:transition-colors after:absolute after:top-10 after:left-[1.58rem] after:h-3.5 after:border-l last:after:hidden",
              completed ? "text-foreground/80 after:border-foreground/25" : "after:border-border",
              active && "bg-muted font-semibold text-foreground",
            )}
          >
            <span aria-hidden="true" className={cn(
              "grid size-8 shrink-0 place-items-center rounded-full border bg-background text-xs font-medium motion-safe:transition-[transform,background-color,border-color] motion-safe:duration-200",
              active && "border-primary bg-primary text-primary-foreground",
              completed && "border-border bg-muted text-foreground",
            )}>
              {completed ? <CheckIcon className="size-4 motion-safe:animate-pop-in" /> : index + 1}
            </span>
            <span className="leading-snug">{title}</span>
          </li>;
        })}
      </ol>

      <Card key={panel} inert={busy === "finish"} aria-busy={navigationDisabled} className="min-w-0 gap-0 overflow-hidden rounded-2xl border bg-card shadow-xs motion-safe:animate-soft-fade">
        <CardHeader className="flex items-start gap-4 pb-6">
          <span aria-hidden="true" className="grid size-11 shrink-0 place-items-center rounded-xl border bg-muted/60 text-foreground/85">
            <StepIcon className="size-5" />
          </span>
          <div className="flex min-w-0 flex-col gap-1.5">
            <CardTitle ref={heading} tabIndex={-1} className="text-xl tracking-tight outline-none">{title}</CardTitle>
            <CardDescription>{description}</CardDescription>
          </div>
        </CardHeader>
        {error && <CardContent className="relative pb-4"><OnboardingAlert warning={false}>{error}</OnboardingAlert></CardContent>}
        {panel === "agent" ? <AgentConfiguration
          navigation={{ onBack: () => moveTo(step - 1), onSkip: () => moveTo(step + 1) }}
          onSaved={() => moveTo(step + 1)} onSavingChange={setConfigurationBusy}
        /> : <>
          <CardContent className="relative flex flex-col gap-4">
            {panel === "storage" && <StorageDirectory onBusyChange={setConfigurationBusy} onMigrationRequest={rememberOnboardingMigration} />}
            {panel === "mcp" && <McpTools onboarding onBusyChange={setConfigurationBusy} />}
            {panel === "scan" && <>
              <div className="flex flex-wrap items-center gap-3 rounded-xl border bg-background/55 p-3.5">
                <Button size="lg" className="shadow-xs" onClick={() => void scan()} disabled={!!busy}>
                  {busy === "scan" ? <Spinner data-icon="inline-start" /> : <ScanSearchIcon data-icon="inline-start" />}
                  {busy === "scan" ? tr("正在扫描…") : report ? tr("重新扫描") : tr("扫描已安装游戏")}
                </Button>
                {report?.nextCursor && busy !== "scan" && <Button variant="outline" className="bg-card/80" disabled={!!busy} onClick={() => void scan(report.nextCursor!)}>{tr("下一批候选")}</Button>}
                {(busy === "scan" || report?.nextCursor) && <Button variant="ghost" onClick={stopScan}>{tr("停止扫描")}</Button>}
                <span role="status" className="text-sm text-muted-foreground">{busy === "scan" ? tr("正在检查常见游戏目录") : report ? tr("已检查 {directoriesChecked} 个目录、{entriesChecked} 个条目", { directoriesChecked: report.directoriesChecked, entriesChecked: report.entriesChecked }) : tr("扫描仅用于发现候选目录")}</span>
              </div>
              {recoveryError && <OnboardingAlert warning>{tr("文件事务恢复需要处理，暂时无法添加游戏。{recoveryError}", { recoveryError })}</OnboardingAlert>}
              {report?.truncated && <OnboardingAlert warning>{tr("已达到扫描上限。可在游戏库中手动检查其他目录。")}</OnboardingAlert>}
              {!!report?.warnings.length && <details className="rounded-xl border bg-background/55 px-3.5 py-2.5 text-sm">
                <summary className="cursor-pointer font-medium">{tr("扫描警告（{length}）", { length: report.warnings.length })}</summary>
                <ul className="mt-2 flex list-disc flex-col gap-1 pl-5 break-all text-muted-foreground">{report.warnings.map((warning, index) => <li key={index}>{displayPath(warning.path)}：{errorText(warning)}</li>)}</ul>
              </details>}
              {report && (report.candidates.length ? <ScanGameGrid>
                {reconcileCandidates(report.candidates, games.data).map((candidate, index) => {
                  const key = gameDirectoryKey(candidate.rootPath);
                  const matched = cloud.match(candidate);
                  const error = candidateErrors[key];
                  return <ScanGameCard key={key} candidate={candidate} index={index}
                    coverUrl={candidate.suggestedAdapterSlug ? cloud.coversByAdapter.get(candidate.suggestedAdapterSlug) : null}
                    matched={matched} waiting={cloud.waiting(candidate)} failed={!!error}
                    disableRegister={!!busy || cloud.pending || !!recoveryError || games.isLoading}
                    onRegister={localOnly => void register(candidate, localOnly)}>
                    <p className="text-xs leading-relaxed wrap-anywhere text-muted-foreground">{adapterHint(candidate, !!matched)}</p>
                    {error && <p role="alert" className="text-xs wrap-anywhere text-destructive">{tr("添加失败：{error}", { error })}</p>}
                  </ScanGameCard>;
                })}
              </ScanGameGrid> : !report.nextCursor && <Empty className="rounded-xl border bg-background/55"><EmptyHeader><EmptyTitle>{tr("未发现候选目录")}</EmptyTitle><EmptyDescription>{tr("未发现候选目录。可在游戏库中手动添加游戏。")}</EmptyDescription></EmptyHeader></Empty>)}
            </>}
            {panel === "done" && <SetupSummary dataRoot={dataRoot} />}
          </CardContent>
          <CardFooter className="relative mt-6 flex-wrap gap-2 border-t bg-muted/20 pt-6">
            {step > 0 && <Button variant="ghost" className="group/back" disabled={navigationDisabled} onClick={() => moveTo(step - 1)}>
              <ArrowLeftIcon data-icon="inline-start" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/back:-translate-x-0.5" />
              {tr("上一步")}
            </Button>}
            {panel === "done" ? <Button className="group/done ml-auto shadow-xs motion-safe:transition-shadow hover:shadow-md" disabled={navigationDisabled} onClick={() => void finish()}>
              {tr("完成并进入游戏库")}
              <CheckIcon data-icon="inline-end" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/done:scale-110" />
            </Button> : <Button className="group/next ml-auto shadow-xs motion-safe:transition-shadow hover:shadow-md" disabled={navigationDisabled} onClick={() => moveTo(step + 1)}>
              {panel === "scan" && !report ? tr("稍后再扫") : tr("下一步")}
              <ArrowRightIcon data-icon="inline-end" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/next:translate-x-1" />
            </Button>}
          </CardFooter>
        </>}
      </Card>
      <footer className="flex justify-center md:col-span-2">
        <Button variant="outline" className="rounded-full bg-card px-5 hover:bg-muted" disabled={navigationDisabled} onClick={() => void finish()}>{tr("退出引导")}</Button>
      </footer>
    </main>
  </div>;
}

function describeSetting<T>(query: { data?: T; isError: boolean }, describe: (data: T) => string) {
  if (query.isError) return tr("读取失败");
  return query.data === undefined ? tr("读取中…") : describe(query.data);
}

function SetupSummary({ dataRoot }: { dataRoot: string }) {
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const connections = useBackendQuery<{ serverId: string }[]>(["mcp", "connections"], "mcp_connections");
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const rows = [
    { icon: FolderOpenIcon, title: tr("存储目录"), value: <PathText>{displayPath(dataRoot)}</PathText> },
    { icon: BotIcon, title: tr("默认模型"), value: describeSetting(settings, ({ model, models }) =>
      models?.find((item) => item.id === model)?.display_name || model || tr("尚未配置")) },
    { icon: PlugIcon, title: tr("运行中的工具服务"), value: describeSetting(connections, (items) => tr("{length} 个", { length: items.length })) },
    { icon: Gamepad2Icon, title: tr("游戏库"), value: describeSetting(games, (items) => tr("{length} 款游戏", { length: items.length })) },
  ];
  return <dl className="grid gap-3 sm:grid-cols-2">
    {rows.map(({ icon: Icon, title, value }, index) => <div
      key={title}
      style={stagger(index, 45)}
      className="group/sum flex flex-col gap-2 rounded-xl border bg-muted/25 p-4 transition-colors duration-200 hover:border-foreground/20 hover:bg-muted/45 motion-safe:animate-soft-fade"
    >
      <dt className="flex items-center gap-2 text-xs font-medium text-muted-foreground">
        <span aria-hidden="true" className="flex size-7 shrink-0 items-center justify-center rounded-lg border bg-background/80 text-foreground/85 motion-safe:transition-transform motion-safe:duration-200 group-hover/sum:scale-110">
          <Icon className="size-3.5" />
        </span>
        {title}
      </dt>
      <dd className="min-w-0 text-sm font-medium wrap-anywhere">{value}</dd>
    </div>)}
  </dl>;
}

function adapterHint(candidate: GameCandidate, cloudMatched: boolean) {
  const platformLabels: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux", unknown: tr("未知平台") };
  return tr("{value1} · 建议适配包：{value2}", {
    value1: platformLabels[candidate.platform] ?? candidate.platform,
    value2: candidate.suggestedAdapterSlug ?? (cloudMatched ? tr("云端已匹配") : tr("无")),
  });
}

function OnboardingAlert({ warning, children }: { warning: boolean; children: string | string[] }) {
  const variant = warning ? "warning" : "destructive";
  const Icon = warning ? TriangleAlertIcon : CircleAlertIcon;
  return <Alert variant={variant}><Icon /><AlertDescription>{children}</AlertDescription></Alert>;
}
