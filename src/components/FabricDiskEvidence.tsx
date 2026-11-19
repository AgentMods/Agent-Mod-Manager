import { Badge } from './ui';

const scanLabels = {
  scanned: null,
  missing: '未找到 mods 目录',
  partial: '仅检查了部分证据',
  unavailable: '无法核实 mods 目录',
};
const metadataLabels = {
  present: null,
  absent: '未找到 Fabric 元数据',
  invalid: '归档或元数据不受支持',
  limited: '超过读取上限',
  unreadable: '无法安全读取',
};
const ownershipLabels = {
  recorded: '有启用记录',
  unrecorded: '无对应启用记录',
  ambiguous: '多条启用记录',
  unverified: '管理记录未核实',
};
const logLabels = {
  exact: 'ID 与版本一致',
  version_differs: '日志版本不同',
  ambiguous: '多个 JAR 声明相同 ID',
  not_listed: '日志清单未列出该 ID',
  unverified: '无法对照日志',
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
  return <section className="fabric-evidence fabric-disk" aria-labelledby="fabric-disk-heading">
    <div className="fabric-evidence-heading">
      <h2 id="fabric-disk-heading">Fabric 磁盘证据</h2>
      {label && <Badge tone={report.status === 'partial' || report.status === 'unavailable' ? 'warning' : undefined}>{label}</Badge>}
    </div>
    <p className="path-text">{report.directory}</p>
    {report.jars.length > 0 ? <details open>
      <summary>磁盘 JAR 对照（{report.jars.length}）</summary>
      <ul className="fabric-jar-list" aria-label="Fabric JAR 对照清单" role="list">
        {report.jars.map(jar => <li key={jar.file} className="fabric-jar-row">
          <strong className="path-text fabric-jar-name">{jar.file}</strong>
          {jar.metadataStatus === 'present'
            ? <span className="path-text">{jar.id} · {jar.version}</span>
            : <span className="muted">{metadataLabels[jar.metadataStatus]}</span>}
          <span>{ownershipLabels[jar.ownership]}</span>
          {jar.metadataStatus === 'present' && <div>
            <span>{logLabels[jar.logRelation.status]}</span>
            {jar.logRelation.line !== null && <small className="path-text fabric-jar-source">第 {jar.logRelation.line} 行
              {jar.logRelation.status === 'version_differs' && jar.logRelation.version && <> · {jar.logRelation.version}</>}
            </small>}
          </div>}
        </li>)}
      </ul>
      <p className="muted">仅对照声明的 ID 与版本，不证明日志来自此 JAR，也不代表已加载。管理记录仅按路径对照。</p>
    </details> : report.status === 'scanned' && <p className="muted">未发现可检查的直属 JAR。</p>}
  </section>;
}
