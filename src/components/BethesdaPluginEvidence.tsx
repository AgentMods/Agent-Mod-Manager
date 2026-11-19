import { AlertTriangle, CheckCircle2, FileWarning } from 'lucide-react';
import { Badge } from './ui';

const scanLabels = {
  scanned: '已完成有界检查',
  partial: '证据不完整',
  unavailable: '无法核实',
} as const;

const headerLabels = {
  parsed: '已解析',
  missing: '文件缺失',
  unreadable: '无法安全读取',
  invalid: '头格式无效',
  limited: '达到读取上限',
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
    return <Badge tone="warning"><AlertTriangle size={13}/>缺少主文件</Badge>;
  }
  if ((Array.isArray(plugin.disabledMasters) && plugin.disabledMasters.length > 0) || (Array.isArray(plugin.mastersAfterPlugin) && plugin.mastersAfterPlugin.length > 0)) {
    return <Badge tone="warning"><AlertTriangle size={13}/>加载顺序需核对</Badge>;
  }
  if (plugin.headerStatus === 'parsed') {
    return <Badge tone="success"><CheckCircle2 size={13}/>已解析</Badge>;
  }
  return <Badge tone="warning"><FileWarning size={13}/>{headerLabels[plugin.headerStatus]}</Badge>;
}

export function BethesdaPluginEvidence({ report }: { report: BethesdaEvidence }) {
  const plugins = Array.isArray(report.plugins) ? report.plugins : [];
  const notes = Array.isArray(report.notes) ? report.notes : [];
  const missingCount = plugins.reduce((total, plugin) => total + (Array.isArray(plugin.missingMasters) ? plugin.missingMasters.length : 0), 0);
  const disabledCount = plugins.reduce((total, plugin) => total + (Array.isArray(plugin.disabledMasters) ? plugin.disabledMasters.length : 0), 0);
  const lateCount = plugins.reduce((total, plugin) => total + (Array.isArray(plugin.mastersAfterPlugin) ? plugin.mastersAfterPlugin.length : 0), 0);
  const hasIncompleteRows = plugins.some(plugin => plugin.headerStatus !== 'parsed' || (Array.isArray(plugin.unlistedMasters) && plugin.unlistedMasters.length > 0) || (Array.isArray(plugin.disabledMasters) && plugin.disabledMasters.length > 0) || (Array.isArray(plugin.mastersAfterPlugin) && plugin.mastersAfterPlugin.length > 0));
  const incomplete = report.status === 'partial' || hasIncompleteRows;
  const statusLabel = report.status === 'unavailable' ? scanLabels.unavailable : incomplete ? '证据不完整' : scanLabels.scanned;
  const statusTone = report.status === 'scanned' && !incomplete && missingCount === 0 ? 'success' : 'warning';
  return <section className="fabric-evidence bethesda-evidence" aria-labelledby="bethesda-evidence-heading">
    <div className="fabric-evidence-heading">
      <h2 id="bethesda-evidence-heading">Bethesda 插件头证据</h2>
      <Badge tone={statusTone}>{statusLabel}</Badge>
    </div>
    <div className="bethesda-summary">
      <span>已检查 {plugins.length} 个启用插件</span>
      <span>{missingCount ? `发现 ${missingCount} 个缺失主文件` : '未发现已确认的缺失主文件'}</span>
      {disabledCount > 0 && <span>未启用主文件 {disabledCount} 个</span>}
      {lateCount > 0 && <span>主文件顺序异常 {lateCount} 个</span>}
    </div>
    {plugins.length > 0 ? <details open>
      <summary>TES4 主文件依赖（{plugins.length}）</summary>
      <ul className="bethesda-plugin-list" aria-label="Bethesda 插件头依赖清单" role="list">
        {plugins.map(plugin => {
          const masters = Array.isArray(plugin.masters) ? plugin.masters : [];
          const missingMasters = Array.isArray(plugin.missingMasters) ? plugin.missingMasters : [];
          const unlistedMasters = Array.isArray(plugin.unlistedMasters) ? plugin.unlistedMasters : [];
          const disabledMasters = Array.isArray(plugin.disabledMasters) ? plugin.disabledMasters : [];
          const mastersAfterPlugin = Array.isArray(plugin.mastersAfterPlugin) ? plugin.mastersAfterPlugin : [];
          const notesForPlugin = Array.isArray(plugin.notes) ? plugin.notes : [];
          return <li className={`bethesda-plugin-row${missingMasters.length || unlistedMasters.length || disabledMasters.length || mastersAfterPlugin.length ? ' has-issue' : ''}`} key={`${plugin.name}:${plugin.listLine}`}>
          <div className="bethesda-plugin-name">
            <strong>{plugin.name}</strong>
            <small className="path-text">Plugins.txt 第 {plugin.listLine} 行</small>
            {plugin.loadOrder ? <small className="path-text">启用顺序 #{plugin.loadOrder}</small> : null}
          </div>
          <div className="bethesda-masters">
            {masters.length > 0 ? masters.map(master => {
              const missing = missingMasters.includes(master);
              const unlisted = unlistedMasters.includes(master);
              const disabled = disabledMasters.includes(master);
              const late = mastersAfterPlugin.includes(master);
              return <span className={`bethesda-master${missing ? ' missing' : ''}${unlisted || disabled || late ? ' unlisted' : ''}`} key={master} title={unlisted ? '主文件未出现在当前 Plugins.txt' : disabled ? '主文件已列出但未启用' : late ? '主文件排在当前插件之后' : undefined}>
                {master}{unlisted && <small>未列出</small>}{disabled && <small>未启用</small>}{late && <small>顺序异常</small>}
              </span>;
            }) : <span className="muted">未声明 MAST</span>}
          </div>
          <PluginStatus plugin={plugin}/>
          {notesForPlugin.map((note, index) => <p className="bethesda-note" key={index}>{note}</p>)}
        </li>;
        })}
      </ul>
    </details> : <p className="muted">没有可读取的启用插件头。</p>}
    {notes.map((note, index) => <p className="warning-text bethesda-note" key={index}>{note}</p>)}
    <p className="muted">只读取 Plugins.txt 声明的 loose 插件；不代表 MO2/VFS、归档内容或游戏当前加载状态。</p>
  </section>;
}
