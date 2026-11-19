import { Badge, date } from './ui';

const labels = {
  unavailable: '启动日志不可用',
  unrecognized: '未识别到 Fabric 启动记录',
  startup_observed: '日志记录了启动版本',
  selection_reported: '日志记录了 Mod 选择清单',
  partial_selection: '清单不完整',
  resolution_failed: '日志记录了解析失败',
  ambiguous: '无法区分启动记录',
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
  return <section className="fabric-evidence" aria-labelledby="fabric-evidence-heading">
    <div className="fabric-evidence-heading">
      <h2 id="fabric-evidence-heading">Fabric 启动证据</h2>
      <Badge tone={warning ? 'warning' : undefined}>{labels[report.status]}</Badge>
    </div>
    {(report.gameVersion || report.loaderVersion) && <dl>
      {report.gameVersion && <><dt>Minecraft</dt><dd className="path-text">{report.gameVersion}</dd></>}
      {report.loaderVersion && <><dt>Fabric Loader</dt><dd className="path-text">{report.loaderVersion}</dd></>}
    </dl>}
    <div className="fabric-evidence-source">
      <span className="path-text">{report.file}{report.startupLine !== null && `:${report.startupLine}`}</span>
      {report.selectionLine !== null && <span>清单第 {report.selectionLine} 行</span>}
      {report.failureLine !== null && <span>失败第 {report.failureLine} 行</span>}
      {report.modifiedAt && <span>日志更新时间 <time dateTime={report.modifiedAt}>{date(report.modifiedAt)}</time></span>}
    </div>
    {report.mods.length > 0 && <details>
      <summary>{report.status === 'partial_selection' ? '日志中可读的 Mod' : '启动时选中的 Mod'}（{report.mods.length}）</summary>
      <table aria-label="Fabric Mod 选择清单">
        <thead><tr><th scope="col">Mod ID</th><th scope="col">版本</th><th scope="col">行号</th></tr></thead>
        <tbody>{report.mods.map(mod => <tr key={mod.line}>
          <td className="path-text">{mod.id}{mod.nested && <small className="fabric-nested">内嵌</small>}</td>
          <td className="path-text">{mod.version}</td>
          <td>{mod.line}</td>
        </tr>)}</tbody>
      </table>
    </details>}
    {report.declaredCount !== null && <p className="muted">日志声明 {report.declaredCount} 项{report.omittedCount > 0 && <> · <span>已省略 {report.omittedCount} 项</span></>}</p>}
    <p className="muted">这是日志中的启动记录，不代表当前运行状态或初始化成功。</p>
  </section>;
}
