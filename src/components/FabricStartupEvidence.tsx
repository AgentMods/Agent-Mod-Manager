import { tr } from "@/lib/i18n";
import { displayPath } from "@/lib/paths";
import { Badge, PathText, date } from '@/components/ui';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';

const labels = {
  get unavailable() { return tr("启动日志不可用"); },
  get unrecognized() { return tr("未识别到 Fabric 启动记录"); },
  get startup_observed() { return tr("日志记录了启动版本"); },
  get selection_reported() { return tr("日志记录了 Mod 选择清单"); },
  get partial_selection() { return tr("清单不完整"); },
  get resolution_failed() { return tr("日志记录了解析失败"); },
  get ambiguous() { return tr("无法区分启动记录"); },
};

export type FabricStartup = {
  status: keyof typeof labels;
  file: string;
  modifiedAt: string | null;
  gameVersion: string | null;
  loaderVersion: string | null;
  startupLine: number | null;
  selectionLine: number | null;
  failureLine: number | null;
  declaredCount: number | null;
  mods: { id: string; version: string; line: number; nested: boolean }[];
  omittedCount: number;
};

export function FabricStartupEvidence({ report }: { report: FabricStartup }) {
  const warning = report.status === 'partial_selection' || report.status === 'resolution_failed' || report.status === 'ambiguous';
  return <section className="min-w-0" aria-labelledby="fabric-evidence-heading">
    <Card>
      <CardHeader className="flex flex-wrap items-center gap-2">
        <CardTitle id="fabric-evidence-heading">{tr("Fabric 启动证据")}</CardTitle>
        <Badge tone={warning ? 'warning' : undefined}>{labels[report.status]}</Badge>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-3">
        {(report.gameVersion || report.loaderVersion) && <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
          {report.gameVersion && <><dt>Minecraft</dt><dd><PathText>{report.gameVersion}</PathText></dd></>}
          {report.loaderVersion && <><dt>Fabric Loader</dt><dd><PathText>{report.loaderVersion}</PathText></dd></>}
        </dl>}
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 text-sm text-muted-foreground">
          <PathText>{displayPath(report.file)}{report.startupLine !== null && `:${report.startupLine}`}</PathText>
          {report.selectionLine !== null && <span>{tr("清单第 {selectionLine} 行", { selectionLine: report.selectionLine })}</span>}
          {report.failureLine !== null && <span>{tr("失败第 {failureLine} 行", { failureLine: report.failureLine })}</span>}
          {report.modifiedAt && <span>{tr("日志更新时间")} <time dateTime={report.modifiedAt}>{date(report.modifiedAt)}</time></span>}
        </div>
        {report.mods.length > 0 && <details className="min-w-0 rounded-md border p-3">
          <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring">{report.status === 'partial_selection' ? tr("日志中可读的 Mod") : tr("启动时选中的 Mod")}（{report.mods.length}）</summary>
          <div className="mt-3 min-w-0">
            <Table aria-label={tr("Fabric Mod 选择清单")}>
              <TableHeader><TableRow><TableHead scope="col">Mod ID</TableHead><TableHead scope="col">{tr("版本")}</TableHead><TableHead scope="col">{tr("行号")}</TableHead></TableRow></TableHeader>
              <TableBody>{report.mods.map(mod => <TableRow key={mod.line}>
                <TableCell className="whitespace-normal"><PathText>{mod.id}</PathText>{mod.nested && <> <Badge>{tr("内嵌")}</Badge></>}</TableCell>
                <TableCell className="whitespace-normal"><PathText>{mod.version}</PathText></TableCell>
                <TableCell>{mod.line}</TableCell>
              </TableRow>)}</TableBody>
            </Table>
          </div>
        </details>}
        {report.declaredCount !== null && <p className="text-sm text-muted-foreground">{tr("日志声明 {declaredCount} 项", { declaredCount: report.declaredCount })}{report.omittedCount > 0 && <> · <span>{tr("已省略 {omittedCount} 项", { omittedCount: report.omittedCount })}</span></>}</p>}
        <p className="text-sm text-muted-foreground">{tr("这是日志中的启动记录，不代表当前运行状态或初始化成功。")}</p>
      </CardContent>
    </Card>
  </section>;
}
