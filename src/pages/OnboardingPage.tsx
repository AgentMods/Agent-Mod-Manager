import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ArrowLeftIcon, ArrowRightIcon, BotIcon, CheckIcon, CircleAlertIcon, Gamepad2Icon, MoonIcon, RadarIcon, ScanSearchIcon, SunIcon, TriangleAlertIcon } from "lucide-react";
import { api, errorText } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { BootstrapState, GameCandidate, ScanReport } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { PathText } from "@/components/ui";
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
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

const steps = ["添加", "配置 Agent", "功能导览"];

const features = [
  {id: "library", title: "游戏库", details: "点击「扫描游戏」发现候选，或输入路径并点击「检查目录」。核对路径和适配包后，逐项添加游戏。扫描候选没有写入授权。"},
  {id: "detail", title: "游戏详情", details: "从游戏库打开游戏，查看目录文件和已装 Mod，创建或还原快照。诊断会列出日志和文件证据，实际修复效果与游戏启动结果需另外验证。"},
  {id: "agent", title: "Agent 对话", details: "用自然语言搜索、下载 Mod，或检查游戏的 Mod 加载日志。可查看工具调用和文件计划。预览展示拟议变更，实际写入须经过当前权限策略审批。"},
  {id: "plugins", title: "插件中心", details: "查看游戏适配包、版本和验证状态，按需启用或安装。适配包为受支持的游戏提供目录识别和 Mod 安装规则。"},
  {id: "trash", title: "回收站", details: "选择条目并审阅恢复计划，可将文件还原到原路径。永久删除需要单独确认，该操作无法撤回。"},
  {id: "history", title: "历史对话", details: "打开过去的 Agent 会话查看消息和任务进度。可以归档会话，也可以从归档中恢复。"},
  {id: "settings", title: "设置", details: "在「AI 与模型」配置服务和权限，在「Mod 来源」管理来源凭据，在「MCP 服务」管理连接，在「Skills」管理指引，在「外观与存储」切换主题并查看数据目录。外部 stdio MCP 进程拥有当前用户的系统权限，运行在本地文件事务沙盒之外。"},
];

export default function OnboardingPage({automatic = false, recoveryError = null}: {automatic?: boolean; recoveryError?: string | null}) {
  const navigate = useUI((state) => state.navigate);
  const theme = useUI((state) => state.theme);
  const toggleTheme = useUI((state) => state.toggleTheme);
  const client = useQueryClient();
  const [step, setStep] = useState(0);
  const [report, setReport] = useState<ScanReport | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [candidateErrors, setCandidateErrors] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<"scan" | "register" | "settings" | "finish" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const resumeRecorded = useRef(false);

  function moveTo(next: number) {
    setError(null);
    setStep(next);
    heading.current?.focus();
  }

  async function scan() {
    setBusy("scan");
    setError(null);
    try {
      setReport(await api<ScanReport>("scan_games", {options: {}}));
      setSelected([]);
      setCandidateErrors({});
    } catch (failure) {
      setError(errorText(failure));
    } finally {
      setBusy(null);
    }
  }

  async function register(candidate: GameCandidate) {
    const path = candidate.rootPath;
    if (busy || recoveryError || !selected.includes(path) || candidate.registeredGameId) return;
    setBusy("register");
    setCandidateErrors((previous) => ({...previous, [path]: ""}));
    try {
      if (automatic && !resumeRecorded.current) {
        await api("start_onboarding");
        resumeRecorded.current = true;
      }
      const game = await api<{id: string}>("register_game", {directory: path, adapterSlug: candidate.suggestedAdapterSlug});
      setReport((previous) => previous && ({...previous, candidates: previous.candidates.map((item) =>
        item.rootPath === path ? {...item, registeredGameId: game.id} : item,
      )}));
      setSelected((previous) => previous.filter((item) => item !== path));
      void client.invalidateQueries({queryKey: ["games"]});
    } catch (failure) {
      setCandidateErrors((previous) => ({...previous, [path]: errorText(failure)}));
    } finally {
      setBusy(null);
    }
  }

  async function finish() {
    if (busy) return;
    setBusy("finish");
    setError(null);
    try {
      await api("set_onboarding_completed", {completed: true});
      client.setQueryData<BootstrapState>(["bootstrap"], (previous) => previous && ({
        ...previous, onboardingCompleted: true, isFirstRun: false, resumeOnboarding: false,
      }));
      void client.invalidateQueries({queryKey: ["settings"]});
      navigate("library");
    } catch (failure) {
      setError(errorText(failure));
    } finally {
      setBusy(null);
    }
  }

  const stepTitle = ["先找到你想管理的游戏", "配置你的 AI Agent", "认识 Agent Mod Manager"][step];
  const StepIcon = step === 0 ? RadarIcon : step === 1 ? BotIcon : Gamepad2Icon;
  return <div className="min-h-dvh bg-[radial-gradient(ellipse_at_top_right,var(--accent),transparent_55%)] bg-muted/40 text-foreground">
    <header data-tauri-drag-region className="flex items-center justify-between gap-4 border-b bg-background px-4 py-3 sm:px-8">
      <div className="flex items-center gap-2.5"><img src="/favicon.png" width={32} height={32} alt="" className="size-8 rounded-lg"/><strong>Agent Mod Manager</strong></div>
      <Button variant="ghost" size="icon" onClick={toggleTheme} aria-label={theme === "light" ? "切换暗色主题" : "切换亮色主题"}>
        {theme === "light" ? <MoonIcon/> : <SunIcon/>}
      </Button>
    </header>
    <main className="mx-auto grid max-w-5xl gap-8 px-4 py-10 sm:px-8 md:grid-cols-[11rem_minmax(0,1fr)] md:py-16">
      <header className="flex flex-col items-center gap-2 text-center md:col-span-2">
        <h1 ref={heading} tabIndex={-1} className="text-3xl font-semibold tracking-tight outline-none sm:text-4xl">欢迎使用 Agent Mod Manager</h1>
        <p className="text-muted-foreground">设置游戏库和 AI Agent，了解如何管理游戏 Mod。</p>
      </header>
      <div className="flex flex-col gap-2 md:hidden">
        <span className="text-sm text-muted-foreground">步骤 {step + 1} / {steps.length}</span>
        <Progress aria-label="引导进度" value={step + 1} max={steps.length}/>
      </div>
      <ol className="hidden flex-col gap-5 py-2 md:flex" aria-label="引导步骤">
        {steps.map((title, index) => <li key={title} aria-current={index === step ? "step" : undefined} className={cn("flex items-center gap-3 text-sm font-medium text-muted-foreground", index === step && "text-primary")}>
          <span aria-hidden="true" className={cn("grid size-8 shrink-0 place-items-center rounded-full border bg-background text-xs", index === step && "border-primary", index < step && "border-primary bg-primary text-primary-foreground")}>{index < step ? <CheckIcon className="size-4"/> : index + 1}</span>{title}
        </li>)}
      </ol>
      <Card className="min-w-0 gap-0">
        <CardHeader className="flex-row items-start gap-4 pb-6">
          <span aria-hidden="true" className="grid size-10 shrink-0 place-items-center rounded-lg bg-accent text-accent-foreground"><StepIcon className="size-5"/></span>
          <div className="flex min-w-0 flex-col gap-1.5">
            <CardTitle>{stepTitle}</CardTitle>
            <CardDescription>{[
              "检查 Steam、Epic 等常见游戏目录。请核对候选路径，然后逐项添加。",
              "设置模型服务和默认权限。也可以稍后在设置中配置。",
              "选择功能分类，查看使用方法和操作边界。",
            ][step]}</CardDescription>
          </div>
        </CardHeader>
        {step === 1 ? <AgentConfiguration
          navigation={{onBack: () => moveTo(0), onSkip: () => moveTo(2)}}
          onSaved={() => moveTo(2)}
          onSavingChange={(saving) => setBusy(saving ? "settings" : null)}
        /> : <>
        <CardContent className="flex flex-col gap-4">
          {error && <OnboardingAlert warning={false}>{error}</OnboardingAlert>}
          {step === 0 && <>
            <div className="flex flex-wrap items-center gap-3">
              <Button size="lg" onClick={() => void scan()} disabled={!!busy}>
                {busy === "scan" ? <Spinner data-icon="inline-start"/> : <ScanSearchIcon data-icon="inline-start"/>}
                {busy === "scan" ? "正在扫描…" : report ? "重新扫描" : "扫描已安装游戏"}
              </Button>
              <span role="status" className="text-sm text-muted-foreground">{busy === "scan" ? "正在检查常见游戏目录" : report ? `已检查 ${report.directoriesChecked} 个目录、${report.entriesChecked} 个条目` : "扫描仅用于发现候选目录"}</span>
            </div>
            {recoveryError && <OnboardingAlert warning>文件事务恢复需要处理，暂时无法添加游戏。{recoveryError}</OnboardingAlert>}
            {report?.truncated && <OnboardingAlert warning>已达到扫描上限。可在游戏库中手动检查其他目录。</OnboardingAlert>}
            {!!report?.warnings.length && <details className="rounded-md border px-3 py-2 text-sm">
              <summary className="cursor-pointer font-medium">扫描警告（{report.warnings.length}）</summary>
              <ul className="mt-2 flex list-disc flex-col gap-1 pl-5 break-all text-muted-foreground">{report.warnings.map(({path, message}, index) => <li key={index}>{path}：{message}</li>)}</ul>
            </details>}
            {report && (report.candidates.length ? <ItemGroup className="gap-2" aria-label="扫描候选游戏">
              {report.candidates.map((candidate) => <CandidateRow key={candidate.rootPath} candidate={candidate}
                selected={selected.includes(candidate.rootPath)} disabled={!!busy || !!recoveryError}
                error={candidateErrors[candidate.rootPath]} onSelect={(checked) => setSelected((previous) =>
                  checked ? [...previous, candidate.rootPath] : previous.filter((path) => path !== candidate.rootPath),
                )} onRegister={() => void register(candidate)}/>)}
            </ItemGroup> : <Empty className="border"><EmptyHeader><EmptyTitle>未发现候选目录</EmptyTitle><EmptyDescription>未发现候选目录。可在游戏库中手动添加游戏。</EmptyDescription></EmptyHeader></Empty>)}
          </>}
          {step === 2 && <Tabs defaultValue="library" className="gap-4">
            <div className="max-w-full overflow-x-auto"><TabsList aria-label="功能分类">
              {features.map(({id, title}) => <TabsTrigger key={id} value={id}>{title}</TabsTrigger>)}
            </TabsList></div>
            {features.map(({id, title, details}) => <TabsContent key={id} value={id}>
              <Card className="gap-2 bg-muted/40 p-5 shadow-none"><h3 className="font-medium">{title}</h3><p className="text-sm leading-relaxed text-muted-foreground">{details}</p></Card>
            </TabsContent>)}
          </Tabs>}
        </CardContent>
        <CardFooter className="mt-6 justify-between gap-2 border-t pt-6">
          {step > 0 ? <Button variant="ghost" disabled={!!busy} onClick={() => moveTo(step - 1)}><ArrowLeftIcon data-icon="inline-start"/>上一步</Button> : <span/>}
          <div className="flex items-center gap-2">
            {step === 0 && <Button disabled={!!busy} onClick={() => moveTo(1)}>{report ? "继续配置 Agent" : "稍后再扫"}<ArrowRightIcon data-icon="inline-end"/></Button>}
            {step === 2 && <Button disabled={!!busy} onClick={() => void finish()}>完成并进入游戏库<CheckIcon data-icon="inline-end"/></Button>}
          </div>
        </CardFooter>
        </>}
      </Card>
      <footer className="flex justify-center md:col-span-2"><Button variant="outline" disabled={!!busy} onClick={() => void finish()}>退出引导</Button></footer>
    </main>
  </div>;
}

function CandidateRow({candidate, selected, disabled, error, onSelect, onRegister}: {
  candidate: GameCandidate; selected: boolean; disabled: boolean; error?: string;
  onSelect: (checked: boolean) => void; onRegister: () => void;
}) {
  const registered = !!candidate.registeredGameId;
  const platformLabels: Record<string, string> = {windows: "Windows", macos: "macOS", linux: "Linux", unknown: "未知平台"};
  return <Item asChild variant="outline" className="items-start">
    <li>
      <ItemContent className="min-w-0 gap-1.5">
        <Label className="font-medium">
          <Checkbox aria-label={`选择 ${candidate.name}`} checked={registered || selected} disabled={disabled || registered} onCheckedChange={(checked) => onSelect(checked === true)}/>
          <ItemTitle>{candidate.name}</ItemTitle>
        </Label>
        <PathText>{candidate.rootPath}</PathText>
        <ItemDescription className="line-clamp-none">{platformLabels[candidate.platform] ?? candidate.platform} · 建议适配包：{candidate.suggestedAdapterSlug ?? "无"}</ItemDescription>
        {!!candidate.warnings.length && <ul className="flex list-disc flex-col gap-1 pl-5 text-xs text-warning">
          {candidate.warnings.map((warning, index) => <li key={index}>{warning}</li>)}
        </ul>}
        {error && <p className="text-xs text-destructive" role="alert">添加失败：{error}</p>}
      </ItemContent>
      <ItemActions className="flex-wrap">
        <Badge variant="outline">{Math.round(candidate.confidence * 100)}% 匹配</Badge>
        {registered ? <Badge variant="success">管理中</Badge> : <Button variant="outline" size="sm" disabled={disabled || !selected} onClick={onRegister}>添加</Button>}
      </ItemActions>
    </li>
  </Item>;
}

function OnboardingAlert({warning, children}: {warning: boolean; children: string | string[]}) {
  const variant = warning ? "warning" : "destructive";
  const Icon = warning ? TriangleAlertIcon : CircleAlertIcon;
  return <Alert variant={variant}><Icon/><AlertDescription>{children}</AlertDescription></Alert>;
}