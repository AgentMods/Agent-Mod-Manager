export type DependencyType = 'required' | 'optional' | 'incompatible' | 'embedded';
export type Dependency = {
  dependency_type: DependencyType; project_id: string | null; version_id: string | null; file_name: string | null;
};
export type DependencyCheck = {
  dependency: Dependency;
  status: 'recorded_exact' | 'recorded_project' | 'different_version' | 'not_recorded' | 'unconfirmed' | 'embedded';
  reason: 'file_reference' | 'ambiguous_records' | 'partial_install' | 'inconsistent_ids' | null;
  matchCount: number;
  matches: { installId: string; projectId: string; versionId: string; status: string }[];
};

export const dependencyLabels: Record<DependencyType, string> = { required: '必需', optional: '可选', incompatible: '不兼容', embedded: '已内嵌' };
export const dependencyReasons: Record<NonNullable<DependencyCheck['reason']>, string> = {
  file_reference: '仅有文件名，无法比对安装记录', ambiguous_records: '有多条相关安装记录',
  partial_install: '相关安装记录为部分恢复', inconsistent_ids: '相关记录的项目 ID 与版本 ID 不一致',
};

export function dependencyOutcome({ dependency, status }: DependencyCheck): { label: string; tone?: 'success' | 'warning' } {
  const required = dependency.dependency_type === 'required';
  const incompatible = dependency.dependency_type === 'incompatible';
  switch (status) {
    case 'recorded_exact': return incompatible ? { label: '已记录不兼容版本', tone: 'warning' } : { label: '已记录确切版本', tone: 'success' };
    case 'recorded_project': return incompatible ? { label: '已记录不兼容项目', tone: 'warning' } : { label: '已记录项目', tone: 'success' };
    case 'different_version': return incompatible ? { label: '记录版本不同，未确认冲突', tone: 'warning' }
      : required ? { label: '必需依赖的记录版本不符', tone: 'warning' } : { label: '记录版本不同' };
    case 'not_recorded': return required ? { label: '缺少必需依赖的安装记录', tone: 'warning' } : { label: '未找到匹配的安装记录' };
    case 'unconfirmed': return { label: incompatible ? '无法确认是否冲突' : '无法确认安装记录', tone: required || incompatible ? 'warning' : undefined };
    case 'embedded': return { label: '来源声明已内嵌' };
  }
}

type EdgeState = 'selected' | 'not_followed' | 'reference_only' | 'choice_required' | 'no_compatible_version'
  | 'metadata_unavailable' | 'project_mismatch' | 'incompatible_version' | 'limit_reached';

export const edgeLabels: Record<EdgeState, string> = {
  selected: '已确定版本', not_followed: '未展开', reference_only: '仅文件引用', choice_required: '需选择版本',
  no_compatible_version: '没有匹配版本', metadata_unavailable: '来源声明不可用', project_mismatch: '项目 ID 不匹配',
  incompatible_version: '不匹配游戏版本或加载器', limit_reached: '超出核对范围',
};

export type GraphEdge = {
  from_version_id: string; declaration: Dependency; state: EdgeState; selected_version_id: string | null; candidates: string[];
};
export type GraphIssue =
  | { kind: 'cycle'; version_ids: string[] }
  | { kind: 'conflicting_versions'; project_id: string; version_ids: string[] }
  | { kind: 'declared_incompatible'; from_version_id: string; version_ids: string[] };

export type FileEvidence = {
  status: 'matched' | 'mismatch' | 'unverified';
  matched: number; modified: number; missing: number; unverified: number; limitReached: boolean;
  issues: { path: string; state: 'modified' | 'missing' | 'unverified' }[];
};

export type GraphReview = {
  artifactId: string; gameId: string; excludedInstallId: string | null; gameVersion: string; loader: string;
  graph: {
    root_version_id: string; complete: boolean; truncated: boolean;
    nodes: { project_id: string; version_id: string; declarations_known: boolean }[];
    edges: GraphEdge[]; issues: GraphIssue[];
  };
  checks: DependencyCheck[];
  fileEvidence: Record<string, FileEvidence>;
};
