import { useLayoutEffect, useRef, useState } from 'react';
import { ArrowRight, CircleCheck, GitFork, LoaderCircle, TriangleAlert } from 'lucide-react';
import { api } from '../lib/api';
import { dependencyLabels, dependencyOutcome, dependencyReasons, edgeLabels, type FileEvidence, type GraphEdge, type GraphIssue, type GraphReview } from '../lib/mod-dependencies';
import { Badge, Button, ErrorState } from './ui';

type Scope = { artifactId: string; projectId: string; versionId: string; gameId: string; replaceInstallId: string | null };
type ProjectPins = Record<string, string>;
type ReviewState = { status: 'idle' | 'loading' } | { status: 'ready'; report: GraphReview } | { status: 'error'; error: unknown };
const versionFilter = /^[A-Za-z0-9._+-]{1,64}$/;
const exactId = /^[A-Za-z0-9]{1,32}$/;
const manualSelection = '!manual';
const fileIssueLabels = { modified: '已修改', missing: '缺失', unverified: '未核验' };

function FileEvidenceResult({ result }: { result: FileEvidence }) {
  const matched = result.status === 'matched';
  const counts = [result.matched && `一致 ${result.matched}`, result.modified && `已修改 ${result.modified}`,
    result.missing && `缺失 ${result.missing}`, result.unverified && `未核验 ${result.unverified}`].filter(Boolean);
  return <div className="graph-file-result">
    <Badge tone={matched ? 'success' : 'warning'}>
      {matched ? <CircleCheck size={13} aria-hidden="true"/> : <TriangleAlert size={13} aria-hidden="true"/>}
      {matched ? '文件与清单一致' : result.status === 'mismatch' ? '文件与清单不符' : '文件未能核验'}
    </Badge>
    {counts.length > 0 && <p className="muted">{counts.join(' · ')}</p>}
    {result.limitReached && <p className="muted">达到文件核验上限</p>}
    {result.issues.map((issue, index) => <p className="path-text" key={index}>{fileIssueLabels[issue.state]}：{issue.path}</p>)}
  </div>;
}

function issueTitle(issue: GraphIssue) {
  switch (issue.kind) {
    case 'cycle': return '依赖循环';
    case 'conflicting_versions': return `项目 ${issue.project_id} 要求不同版本`;
    case 'declared_incompatible': return `版本 ${issue.from_version_id} 声明不兼容`;
  }
}

function GraphChoice({ project, edge, disabled, onConfirm }: {
  project: string; edge: GraphEdge; disabled: boolean; onConfirm: (version: string) => void;
}) {
  const [choice, setChoice] = useState('');
  const [custom, setCustom] = useState('');
  const version = choice === manualSelection ? custom.trim() : choice;
  return <div className="graph-choice">
    <label className="field">依赖版本<select aria-label={`依赖版本 ${project}`} value={choice} disabled={disabled} onChange={e => setChoice(e.target.value)}>
      <option value="">选择精确版本</option>
      {edge.candidates.map(id => <option key={id} value={id}>{id}</option>)}
      <option value={manualSelection}>其他版本 ID</option>
    </select></label>
    {choice === manualSelection && <label className="field">其他版本 ID<input aria-label={`其他版本 ID ${project}`} value={custom} disabled={disabled} maxLength={32} onChange={e => setCustom(e.target.value)}/></label>}
    <Button disabled={disabled || !exactId.test(version)} onClick={() => onConfirm(version)}>确认版本</Button>
  </div>;
}

export function ModrinthGraphReview({ scope, disabled }: { scope: Scope; disabled: boolean }) {
  const [gameVersion, setGameVersion] = useState('');
  const [loader, setLoader] = useState('');
  const [pins, setPins] = useState<ProjectPins>({});
  const [state, setState] = useState<ReviewState>({ status: 'idle' });
  const sequence = useRef(0);
  const checking = useRef(false);
  useLayoutEffect(() => () => { ++sequence.current; }, []);

  const pending = state.status === 'loading';
  const validEnvironment = versionFilter.test(gameVersion.trim()) && versionFilter.test(loader.trim());

  function changeEnvironment(value: string, previous: string, update: (value: string) => void) {
    update(value);
    if (value.trim() === previous.trim()) return;
    ++sequence.current;
    checking.current = false;
    setPins({});
    setState({ status: 'idle' });
  }

  async function check(projectPins: ProjectPins) {
    if (disabled || checking.current || !validEnvironment) return;
    const revision = ++sequence.current;
    checking.current = true;
    const request = {
      artifactId: scope.artifactId, gameId: scope.gameId, replaceInstallId: scope.replaceInstallId,
      gameVersion: gameVersion.trim(), loader: loader.trim(), projectPins,
    };
    setPins(projectPins);
    setState({ status: 'loading' });
    try {
      const report = await api<GraphReview>('review_modrinth_dependency_graph', { request });
      if (revision !== sequence.current) return;
      if (report.artifactId !== request.artifactId || report.gameId !== request.gameId ||
        report.excludedInstallId !== request.replaceInstallId || report.gameVersion !== request.gameVersion ||
        report.loader !== request.loader || report.graph.root_version_id !== scope.versionId ||
        !report.graph.nodes.some(node => node.version_id === scope.versionId && node.project_id === scope.projectId) ||
        report.graph.edges.length !== report.checks.length) {
        throw new Error('传递依赖结果与所选归档、游戏或环境不一致。');
      }
      setState({ status: 'ready', report });
    } catch (error) {
      if (revision === sequence.current) setState({ status: 'error', error });
    } finally {
      if (revision === sequence.current) checking.current = false;
    }
  }

  const report = state.status === 'ready' ? state.report : null;
  const graph = report?.graph;
  const unknownNodes = graph?.nodes.filter(node => !node.declarations_known) ?? [];
  const choiceIndices = new Map<string, number>();
  graph?.edges.forEach((edge, index) => {
    const project = edge.declaration.project_id;
    if (edge.state === 'choice_required' && project && !choiceIndices.has(project)) choiceIndices.set(project, index);
  });
  const graphStatus = graph?.issues.length ? '发现声明冲突' : graph?.truncated ? '达到核对上限'
    : choiceIndices.size ? '待选择版本' : graph?.complete ? '必需声明已展开' : '部分声明未能展开';

  return <details className="source-dependencies graph-review">
    <summary><span><GitFork size={15} aria-hidden="true"/>传递依赖核对</span></summary>
    <div className="graph-review-controls">
      <label className="field">Minecraft 版本<input value={gameVersion} maxLength={64} spellCheck={false} disabled={disabled}
        onChange={e => changeEnvironment(e.target.value, gameVersion, setGameVersion)}/></label>
      <label className="field">加载器<input value={loader} maxLength={64} spellCheck={false} disabled={disabled}
        onChange={e => changeEnvironment(e.target.value, loader, setLoader)}/></label>
      <Button disabled={disabled || pending || !validEnvironment} aria-busy={pending} onClick={() => { void check(pins); }}>
        {pending ? <LoaderCircle size={15} className="spin" aria-hidden="true"/> : <GitFork size={15} aria-hidden="true"/>}
        核对传递依赖
      </Button>
    </div>
    {Object.keys(pins).length > 0 && <Button disabled={disabled || pending} onClick={() => { void check({}); }}>清除所选版本</Button>}
    <div className="graph-summary" role="status">
      {pending && '正在核对来源声明、安装记录及本地文件…'}
      {graph && <>
        <Badge tone={graph.complete ? 'success' : 'warning'}>{graphStatus}</Badge>
        <span className="muted">{graph.edges.length} 条依赖声明 · {graph.nodes.length - 1} 个已解析依赖版本</span>
      </>}
    </div>
    <ErrorState error={state.status === 'error' ? state.error : null}/>
    {graph && report && <>
      <p className="muted graph-evidence-note">文件核验不代表加载器已启用。{report.excludedInstallId && ' 已排除待替换记录。'}</p>
      {unknownNodes.length > 0 && <p className="warning-text graph-evidence-note">依赖声明未知：{unknownNodes.map(node => node.version_id).join('、')}</p>}
      {graph.issues.length > 0 && <ul aria-label="依赖冲突">
        {graph.issues.map((issue, i) => <li key={i}><div><strong>{issueTitle(issue)}</strong><p className="path-text">{issue.version_ids.join(' → ')}</p></div></li>)}
      </ul>}
      {graph.edges.length > 0 && <ul aria-label="传递依赖核对结果">
        {graph.edges.map((edge, i) => {
          const local = report.checks[i];
          const outcome = dependencyOutcome(local);
          const dependency = edge.declaration;
          const selected = edge.selected_version_id ?? dependency.version_id;
          const project = local.dependency.project_id ?? dependency.project_id;
          const installId = local.status === 'recorded_exact' && edge.state === 'selected' ? local.matches[0]?.installId : null;
          const fileEvidence = installId && report.fileEvidence?.[installId];
          return <li className="graph-review-row" key={i}>
            <div>
              <div className="graph-row-badges"><Badge>{dependencyLabels[dependency.dependency_type]}</Badge>
                {edge.state !== 'selected' && <Badge tone={dependency.dependency_type === 'required' ? 'warning' : undefined}>{edgeLabels[edge.state]}</Badge>}
              </div>
              <p className="graph-edge-route path-text"><span>{edge.from_version_id}</span><ArrowRight size={12} aria-label="依赖"/><span>{selected ?? project ?? dependency.file_name}</span></p>
              {selected && project && <p className="path-text">项目 {project}</p>}
            </div>
            <div className="graph-local-result">
              <Badge tone={outcome.tone}>{outcome.label}</Badge>
              {local.reason && <p className="muted">{dependencyReasons[local.reason]}</p>}
              {local.matches.length === 1 && <p className="path-text">记录版本 {local.matches[0].versionId}</p>}
              {fileEvidence && <FileEvidenceResult result={fileEvidence}/>}
            </div>
            {project && choiceIndices.get(project) === i && <GraphChoice project={project} edge={edge} disabled={disabled}
              onConfirm={version => { void check({ ...pins, [project]: version }); }}/>}
          </li>;
        })}
      </ul>}
    </>}
  </details>;
}
