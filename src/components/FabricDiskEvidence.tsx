import { tr } from "@/lib/i18n";
import { displayPath } from "@/lib/paths";
import { Badge, EmptyState, PathText } from '@/components/ui';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Item, ItemActions, ItemContent, ItemGroup, ItemTitle } from '@/components/ui/item';

const scanLabels = {
  scanned: null,
  get missing() { return tr("未找到 mods 目录"); },
  get partial() { return tr("仅检查了部分证据"); },
  get unavailable() { return tr("无法核实 mods 目录"); },
};
const metadataLabels = {
  present: null,
  get absent() { return tr("未找到 Fabric 元数据"); },
  get invalid() { return tr("归档或元数据不受支持"); },
  get limited() { return tr("超过读取上限"); },
  get unreadable() { return tr("无法安全读取"); },
};
const ownershipLabels = {
  get recorded() { return tr("有启用记录"); },
  get unrecorded() { return tr("无对应启用记录"); },
  get ambiguous() { return tr("多条启用记录"); },
  get unverified() { return tr("管理记录未核实"); },
};
const logLabels = {
  get exact() { return tr("ID 与版本一致"); },
  get version_differs() { return tr("日志版本不同"); },
  get ambiguous() { return tr("多个 JAR 声明相同 ID"); },
  get not_listed() { return tr("日志清单未列出该 ID"); },
  get unverified() { return tr("无法对照日志"); },
};

export type FabricDisk = {
  directory: string;
  status: keyof typeof scanLabels;
  jars: {
    file: string;
    metadataStatus: keyof typeof metadataLabels;
    id: string | null;
    version: string | null;
    ownership: keyof typeof ownershipLabels;
    logRelation: { status: keyof typeof logLabels; line: number | null; version: string | null };
  }[];
};

export function FabricDiskEvidence({ report }: { report: FabricDisk }) {
  const label = scanLabels[report.status];
  return <section className="min-w-0" aria-labelledby="fabric-disk-heading">
    <Card>
      <CardHeader className="flex flex-wrap items-center gap-2">
        <CardTitle id="fabric-disk-heading">{tr("Fabric 磁盘证据")}</CardTitle>
        {label && <Badge tone={report.status === 'partial' || report.status === 'unavailable' ? 'warning' : undefined}>{label}</Badge>}
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-3">
        <p><PathText>{displayPath(report.directory)}</PathText></p>
        {report.jars.length > 0 ? <details open className="min-w-0 rounded-md border p-3">
          <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-2 focus-visible:outline-ring">{tr("磁盘 JAR 对照（{length}）", { length: report.jars.length })}</summary>
          <ItemGroup aria-label={tr("Fabric JAR 对照清单")} className="mt-3 gap-2">
            {report.jars.map(jar => <Item key={jar.file} role="listitem" variant="outline" size="sm" className="items-start">
              <ItemContent className="min-w-0 basis-44">
                <ItemTitle className="max-w-full break-all">{jar.file}</ItemTitle>
                {jar.metadataStatus === 'present'
                  ? <PathText>{jar.id} · {jar.version}</PathText>
                  : <span className="text-muted-foreground">{metadataLabels[jar.metadataStatus]}</span>}
              </ItemContent>
              <ItemActions className="min-w-0 flex-wrap">
                <Badge>{ownershipLabels[jar.ownership]}</Badge>
                {jar.metadataStatus === 'present' && <div className="flex min-w-0 flex-col gap-1">
                  <Badge>{logLabels[jar.logRelation.status]}</Badge>
                  {jar.logRelation.line !== null && <PathText>{tr("第 {line} 行", { line: jar.logRelation.line })}{jar.logRelation.status === 'version_differs' && jar.logRelation.version && <> · {jar.logRelation.version}</>}
                  </PathText>}
                </div>}
              </ItemActions>
            </Item>)}
          </ItemGroup>
          <p className="mt-3 text-sm text-muted-foreground">{tr("仅对照声明的 ID 与版本，不证明日志来自此 JAR，也不代表已加载。管理记录仅按路径对照。")}</p>
        </details> : report.status === 'scanned' && <EmptyState title={tr("未发现可检查的直属 JAR。")}/>}
      </CardContent>
    </Card>
  </section>;
}
