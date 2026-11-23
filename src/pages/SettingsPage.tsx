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
import { ModOrganizationLanguage } from "@/components/ModOrganizationLanguage";

const sections = [["appearance", "外观与存储"], ["model", "AI 与模型"], ["mcp", "工具"], ["skills", "Skills"]] as const;

export default function SettingsPage() {
  const [tab, setTab] = useState("appearance");
  return <Page>
    <PageHeader title="设置" subtitle="模型、工具、Skills 与外观" />
    <Tabs value={tab} onValueChange={setTab} className="gap-6">
      <TabsList aria-label="设置分类" className="max-w-full flex-wrap justify-start group-data-[orientation=horizontal]/tabs:h-auto">
        {sections.map(([id, name]) => <TabsTrigger key={id} value={id} className="h-7.5 flex-none">{name}</TabsTrigger>)}
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
    <CardHeader><CardTitle>模型服务</CardTitle><CardDescription>配置 Agent 使用的模型服务、密钥与默认权限策略。</CardDescription></CardHeader>
    <CardContent><AgentConfiguration /></CardContent>
  </Card>;
}

function AppearanceSettings() {
  const theme = useUI((s) => s.theme);
  const toggleTheme = useUI((s) => s.toggleTheme);
  const navigate = useUI((s) => s.navigate);
  return <div className="flex flex-col gap-6">
    <Card>
      <CardHeader><CardTitle>外观</CardTitle><CardDescription>选择界面主题，设置会保存在本机。</CardDescription></CardHeader>
      <CardContent>
        <ToggleGroup type="single" variant="outline" aria-label="主题" value={theme} onValueChange={(value) => { if (value && value !== theme) toggleTheme(); }}>
          <ToggleGroupItem value="light" aria-label="亮色"><SunIcon />亮色</ToggleGroupItem>
          <ToggleGroupItem value="dark" aria-label="暗色"><MoonIcon />暗色</ToggleGroupItem>
        </ToggleGroup>
        <ModOrganizationLanguage />
      </CardContent>
    </Card>
    <Card>
      <CardHeader><CardTitle>存储</CardTitle><CardDescription>管理器数据（数据库、快照、回收站、下载）所在目录。</CardDescription></CardHeader>
      <CardContent>
        <StorageDirectory renderExtraActions={(disabled) => <Button variant="outline" disabled={disabled} onClick={() => navigate("onboarding")}><RotateCcwIcon data-icon="inline-start" />重新查看新手引导</Button>} />
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
  ["system-adapter-authoring", { title: "创建与修改适配包", icon: BoxIcon }],
  ["system-web-search", { title: "联网搜索", icon: SearchIcon }],
  ["system-mod-download", { title: "从网站下载 Mod", icon: DownloadIcon }],
  ["system-mod-commission", { title: "定制 Mod 委托", icon: FileTextIcon }],
  ["system-mod-organize", { title: "安装后整理 Mod", icon: FileTextIcon }],
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
          <TableHead className="w-full">名称</TableHead>
          <TableHead>来源</TableHead>
          <TableHead>状态</TableHead>
          <TableHead>启用</TableHead>
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
                <p className="text-xs text-muted-foreground">{skill.description}</p>
                {skill.issues.map((issue) => <p className="text-xs text-warning" key={issue}>{issue}</p>)}
              </div>
            </div>
          </TableCell>
          <TableCell>{source}</TableCell>
          <TableCell><Badge tone={skill.valid ? "success" : "warning"} className="whitespace-nowrap">{skill.valid ? "已校验" : "不可用"}</Badge></TableCell>
          <TableCell><Checkbox aria-label={`启用 ${title}`} checked={skill.enabled} disabled={busy || !backendAvailable || (!skill.valid && !skill.enabled)} onCheckedChange={(checked) => onToggle(skill.name, checked === true)} /></TableCell>
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
        <CardTitle id="system-skills-title">系统 Skills</CardTitle>
        <CardDescription>管理 Agent 用于创建适配包、联网搜索和下载 Mod 的内置指引。</CardDescription>
      </CardHeader>
      <CardContent className="px-3 sm:px-6">
        {skills.isLoading && <Spinner />}
        {systemSkills && (systemSkills.length > 0
          ? <SkillTable skills={systemSkills} source="系统内置" busy={busy} onInspect={inspect} onToggle={toggle} />
          : <EmptyState title="暂无系统 Skills" />)}
      </CardContent>
    </Card>
    <Card role="region" aria-labelledby="standalone-skills-title">
      <CardHeader className="px-3 sm:px-6">
        <CardTitle id="standalone-skills-title">手动安装的 Skills</CardTitle>
        <CardDescription>导入声明式 SKILL.md，为 Agent 添加操作说明。</CardDescription>
        <CardAction><Button variant="outline" disabled={!backendAvailable || busy} onClick={() => { setError(null); setImporting(true); }}><PlusIcon data-icon="inline-start" />导入 Skill</Button></CardAction>
      </CardHeader>
      <CardContent className="px-3 sm:px-6">
        {standaloneSkills && (standaloneSkills.length > 0
          ? <SkillTable skills={standaloneSkills} source="手动安装" busy={busy} onInspect={inspect} onToggle={toggle} />
          : <EmptyState title="暂无手动安装的 Skills" />)}
      </CardContent>
    </Card>
    {selected && <Modal title={skillPresentation(selected.skill).title} onClose={() => { if (!busy) setSelected(null); }} actions={<>
      {selected.skill.kind === "standalone" && <IconButton title={`预览卸载 Skill ${selected.skill.name}`} className="mr-auto" disabled={!backendAvailable || busy || !selected.skill.valid} onClick={() => { void uninstall(selected.skill); }}><Trash2Icon /></IconButton>}
      <Button variant="outline" disabled={busy} onClick={() => setSelected(null)}>关闭</Button>
    </>}>
      <pre className="max-h-96 overflow-auto rounded-md border bg-muted p-3 font-mono text-xs break-words whitespace-pre-wrap">{selected.markdown}</pre>
      <ErrorState error={error} />
    </Modal>}
    {importing && <Modal title="导入声明式 Skill" onClose={() => { if (!busy) setImporting(false); }} actions={<>
      <Button variant="outline" disabled={busy} onClick={() => setImporting(false)}>取消</Button>
      <Button disabled={busy || !name.trim() || !markdown.trim()} onClick={() => { void install(); }}>预览文件计划</Button>
    </>}>
      <FieldGroup>
        <Field><FieldLabel htmlFor="skill-name">名称</FieldLabel><Input id="skill-name" aria-label="Skill 名称" value={name} onChange={(event) => setName(event.target.value)} placeholder="my-game-guide" /></Field>
        <Field><FieldLabel htmlFor="skill-md">SKILL.md</FieldLabel><Textarea id="skill-md" aria-label="SKILL.md" rows={12} className="font-mono text-xs" value={markdown} onChange={(event) => setMarkdown(event.target.value)} /><FieldDescription>粘贴完整的 SKILL.md（含 frontmatter）。</FieldDescription></Field>
      </FieldGroup>
      <ErrorState error={error} />
    </Modal>}
  </div>;
}
