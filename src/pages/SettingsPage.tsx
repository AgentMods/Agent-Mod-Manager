import { tr } from "@/lib/i18n";
import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { BoxIcon, DownloadIcon, FileTextIcon, MoonIcon, PlusIcon, RotateCcwIcon, SearchIcon, SunIcon, Trash2Icon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { useUI } from "@/lib/store";
import type { FilePlan } from "@/lib/types";
import { AgentConfiguration } from "@/components/AgentConfiguration";
import { Badge, EmptyState, ErrorState, IconButton, Modal, Page, PageHeader } from "@/components/ui";
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

const sections = [["appearance", "外观与存储"], ["model", "AI 与模型"], ["mcp", "工具"], ["skills", "Skills"]] as const;

export default function SettingsPage() {
  const [tab, setTab] = useState("appearance");
  return <Page>
    <PageHeader title={tr("设置")} subtitle={tr("模型、工具、Skills 与外观")} />
    <Tabs value={tab} onValueChange={setTab} className="gap-6">
      <TabsList aria-label={tr("设置分类")} className="max-w-full flex-wrap justify-start group-data-[orientation=horizontal]/tabs:h-auto">
        {sections.map(([id, name]) => <TabsTrigger key={id} value={id} className="h-7.5 flex-none">{tr(name)}</TabsTrigger>)}
      </TabsList>
      <TabsContent value="model"><ModelSettings /></TabsContent>
      <TabsContent value="mcp"><McpTools /></TabsContent>
      <TabsContent value="skills"><SkillsSettings /></TabsContent>
      <TabsContent value="appearance"><AppearanceSettings /></TabsContent>
    </Tabs>
  </Page>;
}

function ModelSettings() {
  return <Card>
    <CardHeader><CardTitle>{tr("模型服务")}</CardTitle><CardDescription>{tr("配置 Agent 使用的模型服务、密钥与默认权限策略。")}</CardDescription></CardHeader>
    <CardContent><AgentConfiguration /></CardContent>
  </Card>;
}

function AppearanceSettings() {
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const navigate = useUI((s) => s.navigate);
  return <div className="flex flex-col gap-6">
    <Card>
      <CardHeader><CardTitle>{tr("外观")}</CardTitle><CardDescription>{tr("设置本机的主题和界面语言。")}</CardDescription></CardHeader>
      <CardContent>
        <ToggleGroup type="single" variant="outline" aria-label={tr("主题")} value={theme} onValueChange={(value) => { if (value && value !== theme) toggleTheme(); }}>
          <ToggleGroupItem value="light" aria-label={tr("亮色")}><SunIcon />{tr("亮色")}</ToggleGroupItem>
          <ToggleGroupItem value="dark" aria-label={tr("暗色")}><MoonIcon />{tr("暗色")}</ToggleGroupItem>
        </ToggleGroup>
        <AppLanguage />
      </CardContent>
    </Card>
    <Card>
      <CardHeader><CardTitle>{tr("存储")}</CardTitle><CardDescription>{tr("管理器数据（数据库、快照、回收站、下载）所在目录。")}</CardDescription></CardHeader>
      <CardContent>
        <StorageDirectory renderExtraActions={(disabled) => <Button variant="outline" disabled={disabled} onClick={() => navigate("onboarding")}><RotateCcwIcon data-icon="inline-start" />{tr("重新查看新手引导")}</Button>} />
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
  return <div className="rounded-md border">
    <Table>
      <TableHeader>
        <TableRow className="bg-muted/50">
          <TableHead className="w-full">{tr("名称")}</TableHead>
          <TableHead>{tr("来源")}</TableHead>
          <TableHead>{tr("状态")}</TableHead>
          <TableHead>{tr("启用")}</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>{skills.map((skill) => {
        const { title, icon: Icon } = skillPresentation(skill);
        return <TableRow key={skill.name}>
          <TableCell className="whitespace-normal">
            <div className="flex items-start gap-2.5">
              <Icon aria-hidden="true" className="mt-0.5 hidden size-4 shrink-0 text-muted-foreground sm:block" />
              <div className="min-w-0 wrap-break-word">
                <Button variant="link" className="h-auto justify-start p-0 text-left font-medium whitespace-normal" onClick={() => onInspect(skill.name)} disabled={busy || !skill.valid}>{title}</Button>
                <p className="text-xs text-muted-foreground">{skill.kind === "system" ? tr(skill.description) : skill.description}</p>
                {skill.issues.map((issue) => <p className="text-xs text-warning" key={issue}>{issue}</p>)}
              </div>
            </div>
          </TableCell>
          <TableCell>{source}</TableCell>
          <TableCell><Badge tone={skill.valid ? "success" : "warning"} className="whitespace-nowrap">{skill.valid ? tr("已校验") : tr("不可用")}</Badge></TableCell>
          <TableCell><Checkbox aria-label={tr("启用 {title}", { title })} checked={skill.enabled} disabled={busy || !backendAvailable || (!skill.valid && !skill.enabled)} onCheckedChange={(checked) => onToggle(skill.name, checked === true)} /></TableCell>
        </TableRow>;
      })}</TableBody>
    </Table>
  </div>;
}
function SkillsSettings() {
  const skills = useBackendQuery<SkillInfo[]>(["skills"], "list_skills");
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

  return <div className="flex flex-col gap-4">
    <ErrorState error={skills.error || (selected || importing ? null : error)} />
    <Card role="region" aria-labelledby="system-skills-title">
      <CardHeader className="px-3 sm:px-6">
        <CardTitle id="system-skills-title">{tr("系统 Skills")}</CardTitle>
        <CardDescription>{tr("管理 Agent 搜索、安装和卸载 Mod 等操作使用的内置指引。")}</CardDescription>
      </CardHeader>
      <CardContent className="px-3 sm:px-6">
        {skills.isLoading && <Spinner />}
        {systemSkills && (systemSkills.length > 0
          ? <SkillTable skills={systemSkills} source={tr("系统内置")} busy={busy} onInspect={inspect} onToggle={toggle} />
          : <EmptyState title={tr("暂无系统 Skills")} />)}
      </CardContent>
    </Card>
    <Card role="region" aria-labelledby="standalone-skills-title">
      <CardHeader className="px-3 sm:px-6">
        <CardTitle id="standalone-skills-title">{tr("手动安装的 Skills")}</CardTitle>
        <CardDescription>{tr("导入声明式 SKILL.md，为 Agent 添加操作说明。")}</CardDescription>
        <CardAction><Button variant="outline" disabled={!backendAvailable || busy} onClick={() => { setError(null); setImporting(true); }}><PlusIcon data-icon="inline-start" />{tr("导入 Skill")}</Button></CardAction>
      </CardHeader>
      <CardContent className="px-3 sm:px-6">
        {standaloneSkills && (standaloneSkills.length > 0
          ? <SkillTable skills={standaloneSkills} source={tr("手动安装")} busy={busy} onInspect={inspect} onToggle={toggle} />
          : <EmptyState title={tr("暂无手动安装的 Skills")} />)}
      </CardContent>
    </Card>
    {selected && <Modal title={skillPresentation(selected.skill).title} onClose={() => { if (!busy) setSelected(null); }} actions={<>
      {selected.skill.kind === "standalone" && <IconButton title={tr("预览卸载 Skill {name}", { name: selected.skill.name })} className="mr-auto" disabled={!backendAvailable || busy || !selected.skill.valid} onClick={() => { void uninstall(selected.skill); }}><Trash2Icon /></IconButton>}
      <Button variant="outline" disabled={busy} onClick={() => setSelected(null)}>{tr("关闭")}</Button>
    </>}>
      <pre className="max-h-96 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-words whitespace-pre-wrap">{selected.markdown}</pre>
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
