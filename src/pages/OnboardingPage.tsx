import { type CloudGameMatch, useCloudGameMatch } from '@/lib/cloud-game-match';
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeftIcon, ArrowRightIcon, BotIcon, CheckIcon, CircleAlertIcon, CircleCheckIcon, FolderOpenIcon, MoonIcon, PlugIcon, RadarIcon, ScanSearchIcon, SunIcon, TriangleAlertIcon } from "lucide-react";
import { api, errorText, useBackendQuery } from "@/lib/api";
import { pendingOnboardingMigration, rememberOnboardingMigration, useUI } from "@/lib/store";
import type { AppSettings, BootstrapState, Game, GameCandidate, ScanReport } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { StorageDirectory } from "@/components/StorageDirectory";
import { McpTools } from "@/components/McpTools";
import { GameMatchBadge } from "@/components/GameMatchBadge";
import { PathText } from "@/components/ui";
import { displayPath, gameDirectoryKey } from "@/lib/paths";
import { reconcileCandidates } from "@/lib/game-discovery";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

const steps = [
  { id: "storage", icon: FolderOpenIcon, title: "设置数据存储目录", description: "选择存放 Mod 下载、数据库和快照的位置，也可以直接使用当前目录。" },
  { id: "agent", icon: BotIcon, title: "配置 AI 模型服务", description: "为 Agent 设置服务地址、默认模型和权限。你也可以稍后在设置中配置。" },
  { id: "mcp", icon: PlugIcon, title: "启用可用工具", description: "选择 Agent 可以调用的 MCP 工具，或直接继续。" },
  { id: "scan", icon: RadarIcon, title: "扫描游戏列表", description: "在 Steam、Epic 等常见位置查找游戏。核对候选目录后，选择要添加的游戏。" },
  { id: "done", icon: CircleCheckIcon, title: "完成", description: "确认当前设置后即可进入游戏库。" },
] as const;

export default function OnboardingPage({ automatic = false, bootstrap }: { automatic?: boolean; bootstrap: BootstrapState }) {
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
  const [selected, setSelected] = useState<string[]>([]);
  const [candidateErrors, setCandidateErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<"scan" | "register" | "finish" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const navigationDisabled = !!busy || configurationBusy;
  const heading = useRef<HTMLHeadingElement>(null);
  const previousStep = useRef(step);
  useEffect(() => {
    if (previousStep.current !== step) heading.current?.focus();
    previousStep.current = step;
  }, [step]);
  const resumeRecorded = useRef(false);
  const scanRequest = useRef<string | null>(null);
  useEffect(() => () => {
    const requestId = scanRequest.current;
    scanRequest.current = null;
    if (requestId) void api("scan_games", { options: { page: { requestId, cancel: true } } }).catch(() => {});
  }, []);

  function moveTo(next: number) {
    setError(null);
    if (panel === "scan") stopScan();
    setStep(next);
  }

  function stopScan() {
    const requestId = scanRequest.current;
    scanRequest.current = null;
    if (requestId) void api("scan_games", { options: { page: { requestId, cancel: true } } }).catch(() => {});
    setReport((current) => current && { ...current, nextCursor: null });
  }
  async function scan(cursor?: string) {
    if (!cursor) stopScan();
    const requestId = cursor ?? crypto.randomUUID();
    scanRequest.current = requestId;
    setBusy("scan");
    setError(null);
    try {
      const result = await api<ScanReport>("scan_games", { options: { page: cursor ? { cursor } : { requestId } } });
      if (scanRequest.current !== requestId) return;
      scanRequest.current = result.nextCursor ?? null;
      setReport({ ...result, candidates: reconcileCandidates(result.candidates) });
      setSelected([]);
      setCandidateErrors({});
    } catch (failure) {
      if (scanRequest.current === requestId) { stopScan(); setError(errorText(failure)); }
    } finally {
      setBusy(null);
    }
  }

  async function register(candidate: GameCandidate, localOnly = false) {
    const key = gameDirectoryKey(candidate.rootPath);
    if (busy || recoveryError || !selected.includes(key) || candidate.registeredGameId) return;
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
      setSelected((previous) => previous.filter((item) => item !== key));
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

  return <div className="min-h-dvh bg-[radial-gradient(ellipse_at_top_right,var(--accent),transparent_55%)] bg-muted/40 text-foreground">
    <header data-tauri-drag-region className="flex items-center justify-between gap-4 border-b bg-background px-4 py-3 sm:px-8">
      <div className="flex items-center gap-2.5"><img src="/favicon.png" width={32} height={32} alt="" className="size-8 rounded-lg" /><strong>Agent Mod Manager</strong></div>
      <Button variant="ghost" size="icon" onClick={toggleTheme} aria-label={theme === "light" ? "切换暗色主题" : "切换亮色主题"}>
        {theme === "light" ? <MoonIcon /> : <SunIcon />}
      </Button>
    </header>
    <main className="mx-auto grid max-w-5xl items-start gap-5 px-4 py-8 sm:px-8 md:grid-cols-[11rem_minmax(0,1fr)] md:gap-8 md:py-14">
      <header className="flex flex-col items-center gap-2 text-center md:col-span-2">
        <h1 className="text-3xl font-semibold tracking-tight sm:text-4xl">欢迎使用 Agent Mod Manager</h1>
      </header>
      <div className="flex flex-col gap-2 md:hidden">
        <span className="text-sm text-muted-foreground">步骤 {step + 1} / {steps.length}</span>
        <Progress aria-label="引导进度" aria-valuemin={1} aria-valuemax={steps.length} aria-valuenow={step + 1}
          value={100 * (step + 1) / steps.length} aria-valuetext={`步骤 ${step + 1} / ${steps.length}`}
          className="motion-reduce:[&_[data-slot=progress-indicator]]:transition-none" />
      </div>
      <ol className="hidden flex-col gap-5 py-2 md:flex" aria-label="引导步骤">
        {steps.map(({ id, title }, index) => <li key={id} aria-current={index === step ? "step" : undefined} className={cn(
          "relative flex items-center gap-3 text-sm text-muted-foreground after:absolute after:top-8 after:left-4 after:h-5 after:border-l after:border-border last:after:hidden",
          index === step && "font-semibold text-foreground",
        )}>
          <span aria-hidden="true" className={cn("grid size-8 shrink-0 place-items-center rounded-full border bg-background text-xs",
            index === step && "border-primary bg-primary text-primary-foreground", index < step && "bg-accent text-accent-foreground",
          )}>{index < step ? <CheckIcon className="size-4" /> : index + 1}</span>{title}
        </li>)}
      </ol>
      <Card key={panel} inert={busy === "finish"} aria-busy={navigationDisabled} className="min-w-0 gap-0 motion-safe:animate-soft-fade">
        <CardHeader className="flex items-start gap-4 pb-6">
          <span aria-hidden="true" className="grid size-10 shrink-0 place-items-center rounded-lg bg-accent text-accent-foreground"><StepIcon className="size-5" /></span>
          <div className="flex min-w-0 flex-col gap-1.5">
            <CardTitle ref={heading} tabIndex={-1} className="text-xl tracking-tight outline-none">{title}</CardTitle>
            <CardDescription>{description}</CardDescription>
          </div>
        </CardHeader>
        {error && <CardContent className="pb-4"><OnboardingAlert warning={false}>{error}</OnboardingAlert></CardContent>}
        {panel === "agent" ? <AgentConfiguration
          navigation={{ onBack: () => moveTo(step - 1), onSkip: () => moveTo(step + 1) }}
          onSaved={() => moveTo(step + 1)} onSavingChange={setConfigurationBusy}
        /> : <>
          <CardContent className="flex flex-col gap-4">
            {panel === "storage" && <StorageDirectory onBusyChange={setConfigurationBusy} onMigrationRequest={rememberOnboardingMigration} />}
            {panel === "mcp" && <McpTools onboarding onBusyChange={setConfigurationBusy} />}
            {panel === "scan" && <>
              <div className="flex flex-wrap items-center gap-3">
                <Button size="lg" onClick={() => void scan()} disabled={!!busy}>
                  {busy === "scan" ? <Spinner data-icon="inline-start" /> : <ScanSearchIcon data-icon="inline-start" />}
                  {busy === "scan" ? "正在扫描…" : report ? "重新扫描" : "扫描已安装游戏"}
                </Button>
                {report?.nextCursor && busy !== "scan" && <Button variant="outline" disabled={!!busy} onClick={() => void scan(report.nextCursor!)}>下一批候选</Button>}
                {(busy === "scan" || report?.nextCursor) && <Button variant="ghost" onClick={stopScan}>停止扫描</Button>}
                <span role="status" className="text-sm text-muted-foreground">{busy === "scan" ? "正在检查常见游戏目录" : report ? `已检查 ${report.directoriesChecked} 个目录、${report.entriesChecked} 个条目` : "扫描仅用于发现候选目录"}</span>
              </div>
              {recoveryError && <OnboardingAlert warning>文件事务恢复需要处理，暂时无法添加游戏。{recoveryError}</OnboardingAlert>}
              {report?.truncated && <OnboardingAlert warning>已达到扫描上限。可在游戏库中手动检查其他目录。</OnboardingAlert>}
              {!!report?.warnings.length && <details className="rounded-md border px-3 py-2 text-sm">
                <summary className="cursor-pointer font-medium">扫描警告（{report.warnings.length}）</summary>
                <ul className="mt-2 flex list-disc flex-col gap-1 pl-5 break-all text-muted-foreground">{report.warnings.map(({ path, message }, index) => <li key={index}>{displayPath(path)}：{message}</li>)}</ul>
              </details>}
              {report && (report.candidates.length ? <ItemGroup className="gap-2" aria-label="扫描候选游戏">
                {reconcileCandidates(report.candidates, games.data).map((candidate) => {
                  const key = gameDirectoryKey(candidate.rootPath);
                  const matched = cloud.match(candidate);
                  const waiting = cloud.waiting(candidate);
                  return <CandidateRow key={key} candidate={candidate} selected={selected.includes(key)} disabled={!!busy || cloud.pending || !!recoveryError || games.isLoading}
                    cloudLoading={waiting} onLocalRegister={() => void register(candidate, true)} cloudMatch={matched} error={candidateErrors[key]} onSelect={(checked) => setSelected((previous) =>
                      checked ? [...previous, key] : previous.filter((path) => path !== key),
                    )} onRegister={() => void register(candidate)} />;
                })}
              </ItemGroup> : !report.nextCursor && <Empty className="border"><EmptyHeader><EmptyTitle>未发现候选目录</EmptyTitle><EmptyDescription>未发现候选目录。可在游戏库中手动添加游戏。</EmptyDescription></EmptyHeader></Empty>)}
            </>}
            {panel === "done" && <SetupSummary dataRoot={dataRoot} />}
          </CardContent>
          <CardFooter className="mt-6 flex-wrap gap-2 border-t pt-6">
            {step > 0 && <Button variant="ghost" disabled={navigationDisabled} onClick={() => moveTo(step - 1)}><ArrowLeftIcon data-icon="inline-start" />上一步</Button>}
            {panel === "done" ? <Button className="ml-auto" disabled={navigationDisabled} onClick={() => void finish()}>
              完成并进入游戏库<CheckIcon data-icon="inline-end" />
            </Button> : <Button className="ml-auto" disabled={navigationDisabled} onClick={() => moveTo(step + 1)}>
              {panel === "scan" && !report ? "稍后再扫" : "下一步"}<ArrowRightIcon data-icon="inline-end" />
            </Button>}
          </CardFooter>
        </>}
      </Card>
      <footer className="flex justify-center md:col-span-2"><Button variant="outline" disabled={navigationDisabled} onClick={() => void finish()}>退出引导</Button></footer>
    </main>
  </div>;
}

function describeSetting<T>(query: { data?: T; isError: boolean }, describe: (data: T) => string) {
  if (query.isError) return "读取失败";
  return query.data === undefined ? "读取中…" : describe(query.data);
}

function SetupSummary({ dataRoot }: { dataRoot: string }) {
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const connections = useBackendQuery<{ serverId: string }[]>(["mcp", "connections"], "mcp_connections");
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const rows = [
    { title: "存储目录", value: <PathText>{displayPath(dataRoot)}</PathText> },
    { title: "默认模型", value: describeSetting(settings, ({ model, models }) =>
      models?.find((item) => item.id === model)?.display_name || model || "尚未配置") },
    { title: "运行中的工具服务", value: describeSetting(connections, (items) => `${items.length} 个`) },
    { title: "游戏库", value: describeSetting(games, (items) => `${items.length} 款游戏`) },
  ];
  return <dl className="divide-y rounded-lg border bg-muted/30 px-4">
    {rows.map(({ title, value }) => <div key={title} className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 py-3 text-sm">
      <dt className="text-muted-foreground">{title}</dt><dd className="min-w-0 wrap-anywhere font-medium">{value}</dd>
    </div>)}
  </dl>;
}

function CandidateRow({ cloudLoading, onLocalRegister, cloudMatch, candidate, selected, disabled, error, onSelect, onRegister }: {
  cloudMatch?: CloudGameMatch; cloudLoading: boolean; onLocalRegister: () => void; candidate: GameCandidate; selected: boolean; disabled: boolean; error?: string;
  onSelect: (checked: boolean) => void; onRegister: () => void;
}) {
  const registered = !!candidate.registeredGameId;
  const gameName = cloudMatch?.gameName ?? candidate.name;
  const platformLabels: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux", unknown: "未知平台" };
  return <Item asChild variant="outline" className="items-start">
    <li>
      <ItemContent className="min-w-0 gap-1.5">
        <Label className="font-medium">
          <Checkbox aria-label={`选择 ${gameName}`} checked={registered || selected} disabled={disabled || registered} onCheckedChange={(checked) => onSelect(checked === true)} />
          <ItemTitle>{gameName}</ItemTitle>
        </Label>
        <PathText>{displayPath(candidate.rootPath)}</PathText>
        <ItemDescription className="line-clamp-none">{platformLabels[candidate.platform] ?? candidate.platform} · 建议适配包：{candidate.suggestedAdapterSlug ?? (cloudMatch ? "云端已匹配" : "无")}</ItemDescription>
        {cloudMatch && <ItemDescription className="line-clamp-none wrap-anywhere">{cloudMatch.pluginName} · {cloudMatch.reusable ? "将校验并复用已安装适配包" : "适配包将随添加安装"}</ItemDescription>}
        {!!candidate.warnings.length && <ul className="flex list-disc flex-col gap-1 pl-5 text-xs text-warning">
          {candidate.warnings.map((warning, index) => <li key={index}>{warning}</li>)}
        </ul>}
        {error && <p className="text-xs text-destructive" role="alert">添加失败：{error}</p>}
      </ItemContent>
      <ItemActions className="flex-wrap">
        <GameMatchBadge loading={cloudLoading} candidate={candidate} cloudMatched={!!cloudMatch} variant="outline" />
        {registered ? <Badge variant="success">管理中</Badge> : <Button variant="outline" size="sm" disabled={disabled || cloudLoading || !selected} onClick={onRegister}>{cloudLoading ? "正在匹配…" : "添加"}</Button>}
        {!registered && (cloudLoading || !!error) && <Button variant="outline" size="sm" disabled={disabled || !selected} onClick={onLocalRegister}>仅添加本地</Button>}
      </ItemActions>
    </li>
  </Item>;
}

function OnboardingAlert({ warning, children }: { warning: boolean; children: string | string[] }) {
  const variant = warning ? "warning" : "destructive";
  const Icon = warning ? TriangleAlertIcon : CircleAlertIcon;
  return <Alert variant={variant}><Icon /><AlertDescription>{children}</AlertDescription></Alert>;
}
