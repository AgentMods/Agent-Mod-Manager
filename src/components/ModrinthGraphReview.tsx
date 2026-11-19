import { useId, useLayoutEffect, useRef, useState } from 'react';
import { ArrowRightIcon, CircleCheckIcon, GitForkIcon, TriangleAlertIcon } from 'lucide-react';
import { Badge, ErrorState, PathText } from '@/components/ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Field, FieldGroup, FieldLabel } from '@/components/ui/field';
import { InputGroup, InputGroupInput } from '@/components/ui/input-group';
import { Item, ItemContent, ItemTitle } from '@/components/ui/item';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Spinner } from '@/components/ui/spinner';
import { api } from '@/lib/api';
import { dependencyLabels, dependencyOutcome, dependencyReasons, edgeLabels, type FileEvidence, type GraphEdge, type GraphIssue, type GraphReview } from '@/lib/mod-dependencies';
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
  return <div className="graph-file-result flex min-w-0 flex-col items-start gap-1">
    <Badge tone={matched ? 'success' : 'warning'}>
      {matched ? <CircleCheckIcon aria-hidden="true"/> : <TriangleAlertIcon aria-hidden="true"/>}
      {matched ? '文件与清单一致' : result.status === 'mismatch' ? '文件与清单不符' : '文件未能核验'}
    </Badge>
    {counts.length > 0 && <p className="text-muted-foreground">{counts.join(' · ')}</p>}
    {result.limitReached && <p className="text-muted-foreground">达到文件核验上限</p>}
    {result.issues.map((issue, index) => <PathText key={index}>{fileIssueLabels[issue.state]}：{issue.path}</PathText>)}
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
  const id = useId();
  return <FieldGroup className="gap-3 md:col-span-2">
    <Field className="min-w-0 gap-1.5 [&>[data-slot=native-select-wrapper]]:w-full">
      <FieldLabel htmlFor={`${id}-version`}>依赖版本</FieldLabel>
      <NativeSelect id={`${id}-version`} className="w-full min-w-0" aria-label={`依赖版本 ${project}`} value={choice}
        disabled={disabled} onChange={e => setChoice(e.target.value)}>
        <NativeSelectOption value="">选择精确版本</NativeSelectOption>
        {edge.candidates.map(id => <NativeSelectOption key={id} value={id}>{id}</NativeSelectOption>)}
        <NativeSelectOption value={manualSelection}>其他版本 ID</NativeSelectOption>
      </NativeSelect>
    </Field>
    {choice === manualSelection && <Field className="gap-1.5">
      <FieldLabel htmlFor={`${id}-custom`}>其他版本 ID</FieldLabel>
      <InputGroup>
        <InputGroupInput id={`${id}-custom`} aria-label={`其他版本 ID ${project}`} value={custom} disabled={disabled}
          maxLength={32} onChange={e => setCustom(e.target.value)}/>
      </InputGroup>
    </Field>}
    <Button variant="outline" className="self-start" disabled={disabled || !exactId.test(version)} onClick={() => onConfirm(version)}>确认版本</Button>
  </FieldGroup>;
}
export function ModrinthGraphReview({ scope, disabled }: { scope: Scope; disabled: boolean }) {
  const id = useId();
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

  return <details className="source-dependencies min-w-0 rounded-lg border p-3 text-sm">
    <summary className="cursor-pointer font-medium">
      <span className="inline-flex items-center gap-2 align-middle"><GitForkIcon className="size-4" aria-hidden="true"/>传递依赖核对</span>
    </summary>
    <div className="mt-3 flex min-w-0 flex-col gap-3">
      <FieldGroup className="grid items-end gap-3 sm:grid-cols-2 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
        <Field className="gap-1.5">
          <FieldLabel htmlFor={`${id}-game-version`}>Minecraft 版本</FieldLabel>
          <InputGroup>
            <InputGroupInput id={`${id}-game-version`} value={gameVersion} maxLength={64} spellCheck={false} disabled={disabled}
              onChange={e => changeEnvironment(e.target.value, gameVersion, setGameVersion)}/>
          </InputGroup>
        </Field>
        <Field className="gap-1.5">
          <FieldLabel htmlFor={`${id}-loader`}>加载器</FieldLabel>
          <InputGroup>
            <InputGroupInput id={`${id}-loader`} value={loader} maxLength={64} spellCheck={false} disabled={disabled}
              onChange={e => changeEnvironment(e.target.value, loader, setLoader)}/>
          </InputGroup>
        </Field>
        <Button variant="outline" className="justify-self-start sm:col-span-2 lg:col-span-1"
          disabled={disabled || pending || !validEnvironment} aria-busy={pending} onClick={() => { void check(pins); }}>
          {pending ? <Spinner data-icon="inline-start"/> : <GitForkIcon data-icon="inline-start" aria-hidden="true"/>}
          核对传递依赖
        </Button>
      </FieldGroup>
      {Object.keys(pins).length > 0 && <Button variant="outline" className="self-start" disabled={disabled || pending}
        onClick={() => { void check({}); }}>清除所选版本</Button>}
      <div className="flex flex-wrap items-center gap-2 empty:hidden" role="status">
        {pending && '正在核对来源声明、安装记录及本地文件…'}
        {graph && <>
          <Badge tone={graph.complete ? 'success' : 'warning'}>{graphStatus}</Badge>
          <span className="text-muted-foreground">{graph.edges.length} 条依赖声明 · {graph.nodes.length - 1} 个已解析依赖版本</span>
        </>}
      </div>
      <ErrorState error={state.status === 'error' ? state.error : null}/>
      {graph && report && <>
        <p className="text-muted-foreground">文件核验不代表加载器已启用。{report.excludedInstallId && ' 已排除待替换记录。'}</p>
        {unknownNodes.length > 0 && <Alert variant="warning" role="note">
          <TriangleAlertIcon aria-hidden="true"/>
          <AlertDescription className="min-w-0 [overflow-wrap:anywhere]">依赖声明未知：{unknownNodes.map(node => node.version_id).join('、')}</AlertDescription>
        </Alert>}
        {graph.issues.length > 0 && <ul role="list" aria-label="依赖冲突" className="flex min-w-0 list-none flex-col gap-2">
          {graph.issues.map((issue, i) => <Item asChild key={i} variant="outline" size="sm">
            <li><ItemContent className="min-w-0">
              <ItemTitle className="max-w-full [overflow-wrap:anywhere]">{issueTitle(issue)}</ItemTitle>
              <PathText>{issue.version_ids.join(' → ')}</PathText>
            </ItemContent></li>
          </Item>)}
        </ul>}
        {graph.edges.length > 0 && <ul role="list" aria-label="传递依赖核对结果" className="flex min-w-0 list-none flex-col gap-2">
          {graph.edges.map((edge, i) => {
            const local = report.checks[i];
            const outcome = dependencyOutcome(local);
            const dependency = edge.declaration;
            const selected = edge.selected_version_id ?? dependency.version_id;
            const project = local.dependency.project_id ?? dependency.project_id;
            const installId = local.status === 'recorded_exact' && edge.state === 'selected' ? local.matches[0]?.installId : null;
            const fileEvidence = installId && report.fileEvidence?.[installId];
            return <Item asChild variant="outline" size="sm" key={i}
              className="grid min-w-0 items-start md:grid-cols-2">
              <li>
                <ItemContent className="min-w-0">
                  <div className="flex flex-wrap gap-1.5">
                    <Badge>{dependencyLabels[dependency.dependency_type]}</Badge>
                    {edge.state !== 'selected' && <Badge tone={dependency.dependency_type === 'required' ? 'warning' : undefined}>{edgeLabels[edge.state]}</Badge>}
                  </div>
                  <p className="flex flex-wrap items-center gap-1.5">
                    <PathText>{edge.from_version_id}</PathText><ArrowRightIcon className="size-3 shrink-0" aria-label="依赖"/>
                    <PathText>{selected ?? project ?? dependency.file_name}</PathText>
                  </p>
                  {selected && project && <PathText>项目 {project}</PathText>}
                </ItemContent>
                <ItemContent className="min-w-0 items-start">
                  <Badge tone={outcome.tone}>{outcome.label}</Badge>
                  {local.reason && <p className="text-muted-foreground">{dependencyReasons[local.reason]}</p>}
                  {local.matches.length === 1 && <PathText>记录版本 {local.matches[0].versionId}</PathText>}
                  {fileEvidence && <FileEvidenceResult result={fileEvidence}/>}
                </ItemContent>
                {project && choiceIndices.get(project) === i && <GraphChoice project={project} edge={edge} disabled={disabled}
                  onConfirm={version => { void check({ ...pins, [project]: version }); }}/>}
              </li>
            </Item>;
          })}
        </ul>}
      </>}
    </div>
  </details>;
}
