import { tr } from "@/lib/i18n";
import { AlertTriangleIcon, CheckCircle2Icon, FileWarningIcon } from 'lucide-react';
import { Badge, EmptyState, PathText } from '@/components/ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Item, ItemActions, ItemContent, ItemGroup, ItemTitle } from '@/components/ui/item';

const scanLabels = {
  get scanned() { return tr("已完成有界检查"); },
  get partial() { return tr("证据不完整"); },
  get unavailable() { return tr("无法核实"); },
} as const;

const headerLabels = {
  get parsed() { return tr("已解析"); },
  get missing() { return tr("文件缺失"); },
  get unreadable() { return tr("无法安全读取"); },
  get invalid() { return tr("头格式无效"); },
  get limited() { return tr("达到读取上限"); },
} as const;

export type BethesdaPlugin = {
  name: string;
  path: string;
  enabled: boolean;
  listLine: number;
  loadOrder?: number;
  headerStatus: keyof typeof headerLabels;
  masters?: string[];
  missingMasters?: string[];
  unlistedMasters?: string[];
  disabledMasters?: string[];
  mastersAfterPlugin?: string[];
  notes?: string[];
};

export type BethesdaEvidence = {
  status: keyof typeof scanLabels;
  plugins: BethesdaPlugin[];
  notes: string[];
};

function PluginStatus({ plugin }: { plugin: BethesdaPlugin }) {
  if (Array.isArray(plugin.missingMasters) && plugin.missingMasters.length > 0) {
    return <Badge tone="warning"><AlertTriangleIcon data-icon="inline-start"/>{tr("缺少主文件")}</Badge>;
  }
  if ((Array.isArray(plugin.disabledMasters) && plugin.disabledMasters.length > 0) || (Array.isArray(plugin.mastersAfterPlugin) && plugin.mastersAfterPlugin.length > 0)) {
    return <Badge tone="warning"><AlertTriangleIcon data-icon="inline-start"/>{tr("加载顺序需核对")}</Badge>;
  }
  if (plugin.headerStatus === 'parsed') {
    return <Badge tone="success"><CheckCircle2Icon data-icon="inline-start"/>{tr("已解析")}</Badge>;
  }
  return <Badge tone="warning"><FileWarningIcon data-icon="inline-start"/>{headerLabels[plugin.headerStatus]}</Badge>;
}

export function BethesdaPluginEvidence({ report }: { report: BethesdaEvidence }) {
  const plugins = Array.isArray(report.plugins) ? report.plugins : [];
  const notes = Array.isArray(report.notes) ? report.notes : [];
  const missingCount = plugins.reduce((total, plugin) => total + (Array.isArray(plugin.missingMasters) ? plugin.missingMasters.length : 0), 0);
  const disabledCount = plugins.reduce((total, plugin) => total + (Array.isArray(plugin.disabledMasters) ? plugin.disabledMasters.length : 0), 0);
  const lateCount = plugins.reduce((total, plugin) => total + (Array.isArray(plugin.mastersAfterPlugin) ? plugin.mastersAfterPlugin.length : 0), 0);
  const hasIncompleteRows = plugins.some(plugin => plugin.headerStatus !== 'parsed' || (Array.isArray(plugin.unlistedMasters) && plugin.unlistedMasters.length > 0) || (Array.isArray(plugin.disabledMasters) && plugin.disabledMasters.length > 0) || (Array.isArray(plugin.mastersAfterPlugin) && plugin.mastersAfterPlugin.length > 0));
  const incomplete = report.status === 'partial' || hasIncompleteRows;
  const statusLabel = report.status === 'unavailable' ? scanLabels.unavailable : incomplete ? tr("证据不完整") : scanLabels.scanned;
  const statusTone = report.status === 'scanned' && !incomplete && missingCount === 0 ? 'success' : 'warning';
  return <section className="min-w-0" aria-labelledby="bethesda-evidence-heading">
    <Card>
      <CardHeader className="flex flex-wrap items-center gap-2">
        <CardTitle id="bethesda-evidence-heading">{tr("Bethesda 插件头证据")}</CardTitle>
        <Badge tone={statusTone}>{statusLabel}</Badge>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-3">
        <div className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-muted-foreground">
          <span>{tr("已检查 {length} 个启用插件", { length: plugins.length })}</span>
          <span>{missingCount ? tr("发现 {missingCount} 个缺失主文件", { missingCount }) : tr("未发现已确认的缺失主文件")}</span>
          {disabledCount > 0 && <span>{tr("未启用主文件 {disabledCount} 个", { disabledCount })}</span>}
          {lateCount > 0 && <span>{tr("主文件顺序异常 {lateCount} 个", { lateCount })}</span>}
        </div>
        {plugins.length > 0 ? <details open className="min-w-0 rounded-md border p-3">
          <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring">{tr("TES4 主文件依赖（{length}）", { length: plugins.length })}</summary>
          <ItemGroup aria-label={tr("Bethesda 插件头依赖清单")} className="mt-3 gap-2">
            {plugins.map(plugin => {
              const masters = Array.isArray(plugin.masters) ? plugin.masters : [];
              const missingMasters = Array.isArray(plugin.missingMasters) ? plugin.missingMasters : [];
              const unlistedMasters = Array.isArray(plugin.unlistedMasters) ? plugin.unlistedMasters : [];
              const disabledMasters = Array.isArray(plugin.disabledMasters) ? plugin.disabledMasters : [];
              const mastersAfterPlugin = Array.isArray(plugin.mastersAfterPlugin) ? plugin.mastersAfterPlugin : [];
              const notesForPlugin = Array.isArray(plugin.notes) ? plugin.notes : [];
              return <Item key={`${plugin.name}:${plugin.listLine}`} role="listitem" variant="outline" size="sm" className="items-start">
                <ItemContent className="min-w-0 basis-36">
                  <ItemTitle className="max-w-full break-all">{plugin.name}</ItemTitle>
                  <PathText>{tr("Plugins.txt 第 {listLine} 行", { listLine: plugin.listLine })}</PathText>
                  {plugin.loadOrder ? <PathText>{tr("启用顺序 #{loadOrder}", { loadOrder: plugin.loadOrder })}</PathText> : null}
                </ItemContent>
                <ItemActions className="min-w-0 flex-wrap">
                  <PluginStatus plugin={plugin}/>
                </ItemActions>
                <div className="flex min-w-0 basis-full flex-wrap gap-1.5">
                  {masters.length > 0 ? masters.map(master => {
                    const missing = missingMasters.includes(master);
                    const unlisted = unlistedMasters.includes(master);
                    const disabled = disabledMasters.includes(master);
                    const late = mastersAfterPlugin.includes(master);
                    const title = unlisted ? tr("主文件未出现在当前 Plugins.txt") : disabled ? tr("主文件已列出但未启用") : late ? tr("主文件排在当前插件之后") : undefined;
                    return <span className="min-w-0 max-w-full" key={master} title={title}>
                      <Badge tone={missing || unlisted || disabled || late ? 'warning' : undefined} className="max-w-full flex-wrap break-all">
                        {master}{unlisted && <small>{tr("未列出")}</small>}{disabled && <small>{tr("未启用")}</small>}{late && <small>{tr("顺序异常")}</small>}
                      </Badge>
                    </span>;
                  }) : <span className="text-muted-foreground">{tr("未声明 MAST")}</span>}
                </div>
                {notesForPlugin.map((note, index) => <p className="min-w-0 basis-full wrap-anywhere text-muted-foreground" key={index}>{tr(note)}</p>)}
              </Item>;
            })}
          </ItemGroup>
        </details> : <EmptyState title={tr("没有可读取的启用插件头。")}/>}
        {notes.map((note, index) => <Alert role="note" variant="warning" key={index}>
          <AlertTriangleIcon/>
          <AlertDescription className="wrap-anywhere">{tr(note)}</AlertDescription>
        </Alert>)}
        <p className="text-sm text-muted-foreground">{tr("只读取 Plugins.txt 声明的 loose 插件；不代表 MO2/VFS、归档内容或游戏当前加载状态。")}</p>
      </CardContent>
    </Card>
  </section>;
}
