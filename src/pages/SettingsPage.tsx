import { tr, useLocale } from "@/lib/i18n";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { BotIcon, BoxIcon, DownloadIcon, FileTextIcon, HardDriveIcon, MoonIcon, PaletteIcon, PlugIcon, PlusIcon, RotateCcwIcon, SearchIcon, SlidersHorizontalIcon, SparklesIcon, SunIcon, Trash2Icon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { FilePlan } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { Badge, ErrorState, IconButton, Modal, Page } from "@/components/ui";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { McpTools } from "@/components/McpTools";
import { AppUpdateSettings } from "@/components/AppUpdateSettings";
import { StorageDirectory } from "@/components/StorageDirectory";
import { AppLanguage } from "@/components/AppLanguage";

const sections = [
  ["appearance", "外观与存储", PaletteIcon],
  ["model", "AI 与模型", BotIcon],
  ["mcp", "工具", PlugIcon],
  ["skills", "Skills", SparklesIcon],
] as const;

export default function SettingsPage() {
  useLocale();
  const [tab, setTab] = useState("appearance");
  return <Page className="max-w-6xl gap-3.5 px-4 py-3.5 sm:px-6 sm:py-4 lg:px-7 lg:py-4">
    <Tabs value={tab} onValueChange={setTab} className="gap-3.5">
      <header className="flex flex-wrap items-center justify-between gap-3 border-b pb-3">
        <div className="flex min-w-0 items-center gap-2.5">
          <span aria-hidden="true" className="flex size-9 shrink-0 items-center justify-center rounded-xl border bg-card text-foreground/85 shadow-2xs">
            <SlidersHorizontalIcon className="size-4" />
          </span>
          <div className="flex min-w-0 flex-col">
            <h1 className="text-xl font-semibold tracking-tight sm:text-2xl">{tr("设置")}</h1>
            <p className="text-xs text-muted-foreground">{tr("模型、工具、Skills 与外观")}</p>
          </div>
        </div>
        <TabsList aria-label={tr("设置分类")} className="max-w-full flex-wrap justify-start gap-1 rounded-xl border bg-card p-1 shadow-2xs group-data-[orientation=horizontal]/tabs:h-auto">
          {sections.map(([id, name, Icon]) => <TabsTrigger key={id} value={id} className="group/tab h-8 flex-none gap-1.5 rounded-lg px-3 text-xs sm:text-sm data-[state=active]:bg-muted data-[state=active]:text-foreground">
            <Icon aria-hidden="true" className="size-3.5 motion-safe:transition-transform motion-safe:duration-200 group-hover/tab:scale-110" />
            {tr(name)}
          </TabsTrigger>)}
        </TabsList>
      </header>
      <TabsContent value="model"><ModelSettings /></TabsContent>
      <TabsContent value="mcp"><McpTools /></TabsContent>
      <TabsContent value="skills"><SkillsSettings /></TabsContent>
      <TabsContent value="appearance"><AppearanceSettings /></TabsContent>
    </Tabs>
  </Page>;
}

function ModelSettings() {
  return <Card className="surface-panel-card group/card gap-3 rounded-2xl py-3.5 shadow-xs">
    <CardHeader className="gap-1 px-4 sm:px-5">
      <CardTitle className="flex items-center gap-2 text-sm font-semibold sm:text-base">
        <span aria-hidden="true" className="flex size-7 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/card:scale-110">
          <BotIcon className="size-3.5" />
        </span>
        {tr("模型服务")}
      </CardTitle>
      <CardDescription className="pl-9 text-xs">{tr("配置 Agent 使用的模型服务、密钥与默认权限策略。")}</CardDescription>
    </CardHeader>
    <CardContent className="px-4 sm:px-5"><AgentConfiguration /></CardContent>
  </Card>;
}

function AppearanceSettings() {
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const navigate = useUI((s) => s.navigate);
  return <div className="flex flex-col gap-3.5">
    <Card className="surface-panel-card group/card gap-3 rounded-2xl py-3.5 shadow-xs">
      <CardHeader className="gap-1 px-4 sm:px-5">
        <CardTitle className="flex items-center gap-2 text-sm font-semibold sm:text-base">
          <span aria-hidden="true" className="flex size-7 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/card:scale-110">
            <PaletteIcon className="size-3.5" />
          </span>
          {tr("外观")}
        </CardTitle>
        <CardDescription className="pl-9 text-xs">{tr("设置本机的主题和界面语言。")}</CardDescription>
      </CardHeader>
      <CardContent className="px-4 sm:px-5">
        <div className="grid gap-3 md:grid-cols-2">
          <div className="flex flex-wrap content-center items-center justify-between gap-2.5 rounded-xl border bg-muted/25 px-3.5 py-2.5 transition-colors duration-200 hover:border-foreground/20">
            <span className="flex items-center gap-2 text-sm font-medium">
              {theme === "light"
                ? <SunIcon aria-hidden="true" className="size-4 text-muted-foreground" />
                : <MoonIcon aria-hidden="true" className="size-4 text-muted-foreground" />}
              {tr("主题")}
            </span>
            <ToggleGroup type="single" variant="outline" size="sm" aria-label={tr("主题")} value={theme} onValueChange={(value) => { if (value && value !== theme) toggleTheme(); }}>
              <ToggleGroupItem value="light" aria-label={tr("亮色")} className="group/light gap-1.5">
                <SunIcon className="size-3.5 motion-safe:transition-transform motion-safe:duration-300 group-hover/light:rotate-45" />
                {tr("亮色")}
              </ToggleGroupItem>
              <ToggleGroupItem value="dark" aria-label={tr("暗色")} className="group/dark gap-1.5">
                <MoonIcon className="size-3.5 motion-safe:transition-transform motion-safe:duration-300 group-hover/dark:-rotate-12" />
                {tr("暗色")}
              </ToggleGroupItem>
            </ToggleGroup>
          </div>
          <AppLanguage />
        </div>
      </CardContent>
    </Card>
    <Card className="surface-panel-card group/card gap-3 rounded-2xl py-3.5 shadow-xs">
      <CardHeader className="gap-1 px-4 sm:px-5">
        <CardTitle className="flex items-center gap-2 text-sm font-semibold sm:text-base">
          <span aria-hidden="true" className="flex size-7 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/card:scale-110">
            <HardDriveIcon className="size-3.5" />
          </span>
          {tr("存储")}
        </CardTitle>
        <CardDescription className="pl-9 text-xs">{tr("管理器数据（数据库、快照、回收站、下载）所在目录。")}</CardDescription>
      </CardHeader>
      <CardContent className="px-4 sm:px-5">
        <StorageDirectory compact renderExtraActions={(disabled) => <Button variant="outline" size="sm" className="group/guide bg-background/65 hover:bg-muted/60" disabled={disabled} onClick={() => navigate("onboarding")}>
          <RotateCcwIcon data-icon="inline-start" className="motion-safe:transition-transform motion-safe:duration-300 group-hover/guide:-rotate-45" />
          {tr("重新查看新手引导")}
        </Button>} />
      </CardContent>
    </Card>
    <AppUpdateSettings />
  </div>;
}

type SkillInfo = {
  name: string;
  description: string;
  enabled: boolean;
  kind: "system" | "standalone" | "builtin" | "plugin" | "unknown";
  valid: boolean;
  issues: string[];
};

const systemSkillPresentation = new Map([
  ["system-adapter-authoring", { get title() { return tr("创建与修改适配包"); }, icon: BoxIcon }],
  ["system-web-search", { get title() { return tr("联网搜索"); }, icon: SearchIcon }],
  ["system-mod-download", { get title() { return tr("从网站下载 Mod"); }, icon: DownloadIcon }],
  ["system-mod-commission", { get title() { return tr("定制 Mod 委托"); }, icon: FileTextIcon }],
  ["system-mod-install", { get title() { return tr("安装 Mod"); }, icon: BoxIcon }],
  ["system-mod-organize", { get title() { return tr("安装后整理 Mod"); }, icon: FileTextIcon }],
  ["system-mod-uninstall", { get title() { return tr("卸载 Mod"); }, icon: Trash2Icon }],
]);

function skillPresentation(skill: SkillInfo) {
  return (skill.kind === "system" ? systemSkillPresentation.get(skill.name) : undefined)
    ?? { title: skill.name, icon: FileTextIcon };
}

function SkillTable({ skills, source, busy, onInspect, onToggle }: {
  skills: SkillInfo[];
  source: string;
  busy: boolean;
  onInspect: (name: string) => void;
  onToggle: (name: string, enabled: boolean) => void;
}) {
  return <div className="overflow-hidden rounded-xl border bg-background/60">
    <Table>
      <TableHeader>
        <TableRow className="bg-muted/45 hover:bg-muted/45">
          <TableHead className="h-8 w-full px-2.5 text-xs">{tr("名称")}</TableHead>
          <TableHead className="h-8 px-2.5 text-xs">{tr("来源")}</TableHead>
          <TableHead className="h-8 px-2.5 text-xs">{tr("状态")}</TableHead>
          <TableHead className="h-8 px-2.5 text-xs">{tr("启用")}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>{skills.map((skill) => {
        const { title, icon: Icon } = skillPresentation(skill);
        return <TableRow key={skill.name} className="group/row transition-colors hover:bg-muted/35">
          <TableCell className="px-2.5 py-1.5 whitespace-normal">
            <div className="flex items-center gap-2.5">
              <span aria-hidden="true" className="hidden size-7 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/80 motion-safe:transition-transform motion-safe:duration-200 group-hover/row:scale-105 sm:flex">
                <Icon className="size-3.5" />
              </span>
              <div className="min-w-0 wrap-break-word">
                <Button variant="link" className="h-auto justify-start p-0 text-left text-sm leading-tight font-medium whitespace-normal" onClick={() => onInspect(skill.name)} disabled={busy || !skill.valid}>{title}</Button>
                <p className="text-xs leading-snug text-muted-foreground">{skill.kind === "system" ? tr(skill.description) : skill.description}</p>
                {skill.issues.map((issue) => <p className="text-xs leading-snug text-warning" key={issue}>{issue}</p>)}
              </div>
            </div>
          </TableCell>
          <TableCell className="px-2.5 py-1.5 text-xs text-muted-foreground">{source}</TableCell>
          <TableCell className="px-2.5 py-1.5"><Badge tone={skill.valid ? "success" : "warning"} className="whitespace-nowrap">{skill.valid ? tr("已校验") : tr("不可用")}</Badge></TableCell>
          <TableCell className="px-2.5 py-1.5"><Checkbox aria-label={tr("启用 {title}", { title })} checked={skill.enabled} disabled={busy || !backendAvailable || (!skill.valid && !skill.enabled)} onCheckedChange={(checked) => onToggle(skill.name, checked === true)} /></TableCell>
        </TableRow>;
      })}</TableBody>
    </Table>
  </div>;
}

function SkillEmptyRow({ title }: { title: string }) {
  return <div className="flex items-center justify-center gap-2 rounded-xl border border-dashed bg-muted/20 px-4 py-4 text-xs text-muted-foreground">
    <FileTextIcon aria-hidden="true" className="size-4 shrink-0 opacity-70" />
    <span>{title}</span>
  </div>;
}

function SkillsSettings() {
  const skills = useBackendQuery<SkillInfo[]>(["skills", "settings"], "list_skills", { settingsOnly: true });
  const [importing, setImporting] = useState(false);
  const [name, setName] = useState("");
  const [markdown, setMarkdown] = useState("");
  const [selected, setSelected] = useState<{ skill: SkillInfo; markdown: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const reviewPlan = useUI((s) => s.reviewPlan);
  const client = useQueryClient();
  async function inspect(name: string) {
    setBusy(true); setError(null);
    try { setSelected(await api("get_skill", { name })); }
    catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function install() {
    setBusy(true); setError(null);
    try {
      const preview = await api<{ plan: FilePlan }>("plan_install_skill", { name, markdown });
      setImporting(false);
      reviewPlan(preview.plan, () => { setName(""); setMarkdown(""); void client.invalidateQueries({ queryKey: ["skills"] }); });
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function uninstall(skill: SkillInfo) {
    setBusy(true); setError(null);
    try {
      const preview = await api<{ plan: FilePlan }>("plan_uninstall_skill", { name: skill.name });
      setSelected(null);
      reviewPlan(preview.plan, () => {
        void client.invalidateQueries({ queryKey: ["skills"] });
        void client.invalidateQueries({ queryKey: ["trash"] });
      });
    } catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  async function toggle(name: string, enabled: boolean) {
    setBusy(true); setError(null);
    try { await api("set_skill_enabled", { name, enabled }); await skills.refetch(); }
    catch (failure) { setError(failure); } finally { setBusy(false); }
  }
  const systemSkills = skills.data?.filter((skill) => skill.kind === "system");
  const standaloneSkills = skills.data?.filter((skill) => skill.kind === "standalone");

  return <div className="flex flex-col gap-3">
    <ErrorState error={skills.error || (selected || importing ? null : error)} />
    <Card role="region" aria-labelledby="system-skills-title" className="surface-panel-card group/card gap-2.5 rounded-2xl py-3 shadow-xs">
      <CardHeader className="gap-1 px-3.5 sm:px-5">
        <div className="flex flex-wrap items-center gap-2">
          <span aria-hidden="true" className="flex size-7 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/card:scale-110">
            <SparklesIcon className="size-3.5" />
          </span>
          <CardTitle id="system-skills-title" className="text-sm font-semibold sm:text-base">{tr("系统 Skills")}</CardTitle>
          {systemSkills && <Badge tone="neutral" className="rounded-full px-2 py-0 text-xs tabular-nums">{systemSkills.length}</Badge>}
        </div>
        <CardDescription className="text-xs sm:pl-9">{tr("管理 Agent 搜索、安装和卸载 Mod 等操作使用的内置指引。")}</CardDescription>
      </CardHeader>
      <CardContent className="px-3.5 sm:px-5">
        {skills.isLoading && <Spinner />}
        {systemSkills && (systemSkills.length > 0
          ? <SkillTable skills={systemSkills} source={tr("系统内置")} busy={busy} onInspect={inspect} onToggle={toggle} />
          : <SkillEmptyRow title={tr("暂无系统 Skills")} />)}
      </CardContent>
    </Card>
    <Card role="region" aria-labelledby="standalone-skills-title" className="surface-panel-card group/card gap-2.5 rounded-2xl py-3 shadow-xs">
      <CardHeader className="gap-1 px-3.5 sm:px-5">
        <div className="flex flex-wrap items-center gap-2">
          <span aria-hidden="true" className="flex size-7 shrink-0 items-center justify-center rounded-lg border bg-muted/60 text-foreground/85 motion-safe:transition-transform motion-safe:duration-300 group-hover/card:scale-110">
            <FileTextIcon className="size-3.5" />
          </span>
          <CardTitle id="standalone-skills-title" className="text-sm font-semibold sm:text-base">{tr("手动安装的 Skills")}</CardTitle>
          {standaloneSkills && <Badge tone="neutral" className="rounded-full px-2 py-0 text-xs tabular-nums">{standaloneSkills.length}</Badge>}
        </div>
        <CardDescription className="text-xs sm:pl-9">{tr("导入声明式 SKILL.md，为 Agent 添加操作说明。")}</CardDescription>
        <CardAction>
          <Button variant="outline" size="sm" className="bg-background/65 hover:bg-muted/60" disabled={!backendAvailable || busy} onClick={() => { setError(null); setImporting(true); }}>
            <PlusIcon data-icon="inline-start" />{tr("导入 Skill")}
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="px-3.5 sm:px-5">
        {standaloneSkills && (standaloneSkills.length > 0
          ? <SkillTable skills={standaloneSkills} source={tr("手动安装")} busy={busy} onInspect={inspect} onToggle={toggle} />
          : <SkillEmptyRow title={tr("暂无手动安装的 Skills")} />)}
      </CardContent>
    </Card>
    {selected && <Modal title={skillPresentation(selected.skill).title} onClose={() => { if (!busy) setSelected(null); }} actions={<>
      {selected.skill.kind === "standalone" && <IconButton title={tr("预览卸载 Skill {name}", { name: selected.skill.name })} className="mr-auto" disabled={!backendAvailable || busy || !selected.skill.valid} onClick={() => { void uninstall(selected.skill); }}><Trash2Icon /></IconButton>}
      <Button variant="outline" disabled={busy} onClick={() => setSelected(null)}>{tr("关闭")}</Button>
    </>}>
      <pre className="max-h-96 overflow-auto rounded-xl border bg-muted/50 p-3.5 font-mono text-xs break-words whitespace-pre-wrap">{selected.markdown}</pre>
      <ErrorState error={error} />
    </Modal>}
    {importing && <Modal title={tr("导入声明式 Skill")} onClose={() => { if (!busy) setImporting(false); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setImporting(false)}>{tr("取消")}</Button>
      <Button disabled={busy || !name.trim() || !markdown.trim()} onClick={() => { void install(); }}>{tr("预览文件计划")}</Button>
    </>}>
      <FieldGroup>
        <Field><FieldLabel htmlFor="skill-name">{tr("名称")}</FieldLabel><Input id="skill-name" aria-label={tr("Skill 名称")} value={name} onChange={(event) => setName(event.target.value)} placeholder="my-game-guide" /></Field>
        <Field><FieldLabel htmlFor="skill-md">SKILL.md</FieldLabel><Textarea id="skill-md" aria-label="SKILL.md" rows={12} className="font-mono text-xs" value={markdown} onChange={(event) => setMarkdown(event.target.value)} /><FieldDescription>{tr("粘贴完整的 SKILL.md（含 frontmatter）。")}</FieldDescription></Field>
      </FieldGroup>
      <ErrorState error={error} />
    </Modal>}
  </div>;
}
