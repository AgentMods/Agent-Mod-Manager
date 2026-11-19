//! Bounded, advisory closure of required Modrinth version declarations.
use super::{array, modrinth_dependencies, modrinth_matches, Provider};
use crate::{
    error::{AppError, AppResult},
    sources::{modrinth_id, DependencyType, ModDependency},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};
use tokio::time::{timeout_at, Instant};

const MAX_NODES: usize = 32;
const MAX_DEPTH: usize = 8;
const MAX_EDGES: usize = 256;
const MAX_LOOKUPS: usize = 48;
const MAX_CHOICES: usize = 5;
const DEADLINE: Duration = Duration::from_secs(25);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyGraphRequest {
    pub project_id: Option<String>,
    pub version_id: String,
    pub game_version: String,
    pub loader: String,
    /// User-selected exact versions for project-only required declarations.
    #[serde(default)]
    pub project_pins: BTreeMap<String, String>,
}

impl DependencyGraphRequest {
    pub(in crate::sources) fn validate_pins(&self) -> AppResult<()> {
        if self.project_pins.len() > MAX_NODES {
            return Err(AppError::Invalid("依赖图指定版本数量超出上限".into()));
        }
        for (project, version) in &self.project_pins {
            modrinth_id(project)?;
            modrinth_id(version)?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct DependencyGraph {
    root_version_id: String,
    complete: bool,
    truncated: bool,
    nodes: Vec<DependencyNode>,
    edges: Vec<DependencyEdge>,
    issues: Vec<GraphIssue>,
}

impl DependencyGraph {
    pub(crate) fn selected_edges(&self) -> impl Iterator<Item = bool> + '_ {
        self.edges
            .iter()
            .map(|edge| edge.state == EdgeState::Selected)
    }

    pub(crate) fn journal_declarations(&self) -> Vec<ModDependency> {
        let projects: HashMap<_, _> = self
            .nodes
            .iter()
            .map(|node| (&node.version_id, &node.project_id))
            .collect();
        self.edges
            .iter()
            .map(|edge| {
                let mut declaration = edge.declaration.clone();
                if let Some(version) = &edge.selected_version_id {
                    declaration.version_id = Some(version.clone());
                    declaration.project_id =
                        projects.get(version).map(|project| (*project).clone());
                }
                declaration
            })
            .collect()
    }
}

#[derive(Debug, Serialize)]
struct DependencyNode {
    project_id: String,
    version_id: String,
    declarations_known: bool,
}

#[derive(Debug, Serialize)]
struct DependencyEdge {
    from_version_id: String,
    declaration: ModDependency,
    state: EdgeState,
    selected_version_id: Option<String>,
    candidates: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EdgeState {
    Selected,
    NotFollowed,
    ReferenceOnly,
    ChoiceRequired,
    NoCompatibleVersion,
    MetadataUnavailable,
    ProjectMismatch,
    IncompatibleVersion,
    LimitReached,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum GraphIssue {
    Cycle {
        version_ids: Vec<String>,
    },
    ConflictingVersions {
        project_id: String,
        version_ids: Vec<String>,
    },
    DeclaredIncompatible {
        from_version_id: String,
        version_ids: Vec<String>,
    },
}

struct VersionMetadata {
    project_id: String,
    version_id: String,
    compatible: bool,
    declarations: Option<Vec<ModDependency>>,
}

#[derive(Clone)]
struct ProjectChoices {
    candidates: Vec<String>,
    unique_release: bool,
}

struct GraphBuilder<'a> {
    provider: &'a Provider,
    request: &'a DependencyGraphRequest,
    deadline: Instant,
    lookups: usize,
    truncated: bool,
    exact_cache: HashMap<String, Result<Arc<VersionMetadata>, EdgeState>>,
    choice_cache: HashMap<String, Result<ProjectChoices, EdgeState>>,
    selected: Vec<(Arc<VersionMetadata>, usize)>,
    index: HashMap<String, usize>,
    edges: Vec<DependencyEdge>,
}

impl Provider {
    pub(super) async fn modrinth_graph(
        &self,
        request: &DependencyGraphRequest,
    ) -> AppResult<DependencyGraph> {
        let deadline = Instant::now() + DEADLINE;
        let root = timeout_at(deadline, self.graph_version(request, &request.version_id))
            .await
            .map_err(|_| AppError::Other("获取根版本依赖元数据超时".into()))??;
        if !root.compatible
            || request
                .project_id
                .as_ref()
                .is_some_and(|id| id != &root.project_id)
        {
            return Err(AppError::Conflict(
                "根版本与项目 ID 或所选 Minecraft 版本和加载器不符".into(),
            ));
        }
        let root_id = root.version_id.clone();
        let root = Arc::new(root);
        let mut builder = GraphBuilder {
            provider: self,
            request,
            deadline,
            lookups: 1,
            truncated: false,
            exact_cache: HashMap::from([(root_id.clone(), Ok(root.clone()))]),
            choice_cache: HashMap::new(),
            selected: vec![(root, 0)],
            index: HashMap::from([(root_id, 0)]),
            edges: Vec::new(),
        };
        builder.walk().await;
        Ok(builder.finish())
    }

    async fn graph_version(
        &self,
        request: &DependencyGraphRequest,
        version_id: &str,
    ) -> AppResult<VersionMetadata> {
        let version = self.modrinth_exact_version(version_id).await?;
        if version["id"].as_str() != Some(version_id) {
            return Err(AppError::Other("Modrinth 返回的版本 ID 与查询不符".into()));
        }
        let project_id = modrinth_id(version["project_id"].as_str().unwrap_or_default())?;
        Ok(VersionMetadata {
            project_id: project_id.into(),
            version_id: version_id.into(),
            compatible: graph_compatibility(&version, request)?,
            declarations: modrinth_dependencies(&version),
        })
    }

    async fn graph_choices(
        &self,
        request: &DependencyGraphRequest,
        project: &str,
    ) -> AppResult<ProjectChoices> {
        let list = self
            .modrinth_project_versions(project, Some(&request.game_version), Some(&request.loader))
            .await?;
        let mut seen = HashMap::new();
        let mut candidates = Vec::new();
        for row in array(&list)? {
            if row["project_id"].as_str() != Some(project) {
                return Err(AppError::Other(
                    "Modrinth 项目版本列表包含不匹配的项目".into(),
                ));
            }
            let id = modrinth_id(row["id"].as_str().unwrap_or_default())?;
            let compatible = graph_compatibility(row, request)?;
            let release = match row["version_type"].as_str() {
                Some("release") => true,
                Some("beta" | "alpha") => false,
                _ => return Err(AppError::Other("Modrinth 发布类型无效".into())),
            };
            if let Some(previous) = seen.insert(id, (compatible, release)) {
                if previous != (compatible, release) {
                    return Err(AppError::Other("Modrinth 版本列表声明冲突".into()));
                }
            } else if compatible && candidates.len() < MAX_CHOICES {
                candidates.push(id.to_owned());
            }
        }
        let unique_release = matches!(&candidates[..], [only] if seen[only.as_str()].1);
        Ok(ProjectChoices {
            candidates,
            unique_release,
        })
    }
}

fn graph_compatibility(
    version: &serde_json::Value,
    request: &DependencyGraphRequest,
) -> AppResult<bool> {
    for key in ["game_versions", "loaders"] {
        let filters = array(&version[key])?;
        if filters.len() > 1024 || filters.iter().any(|value| !value.is_string()) {
            return Err(AppError::Other("Modrinth 版本的兼容声明无效".into()));
        }
    }
    Ok(modrinth_matches(
        version,
        Some(&request.game_version),
        Some(&request.loader),
    ))
}

impl GraphBuilder<'_> {
    async fn walk(&mut self) {
        // Nodes are appended in discovery order, so a shared node is visited at its shortest depth.
        let mut cursor = 0;
        while let Some((node, depth)) = self.selected.get(cursor).cloned() {
            cursor += 1;
            for declaration in node.declarations.iter().flatten() {
                if self.edges.len() == MAX_EDGES {
                    self.truncated = true;
                    return;
                }
                let mut edge = DependencyEdge {
                    from_version_id: node.version_id.clone(),
                    declaration: declaration.clone(),
                    state: EdgeState::NotFollowed,
                    selected_version_id: None,
                    candidates: Vec::new(),
                };
                if declaration.dependency_type == DependencyType::Required {
                    edge.state = match self.follow(&mut edge, depth + 1).await {
                        Ok(()) => EdgeState::Selected,
                        Err(state) => state,
                    };
                }
                self.edges.push(edge);
            }
        }
    }

    async fn follow(&mut self, edge: &mut DependencyEdge, depth: usize) -> Result<(), EdgeState> {
        let project = edge.declaration.project_id.as_deref();
        let pin = edge
            .declaration
            .version_id
            .as_ref()
            .or_else(|| project.and_then(|id| self.request.project_pins.get(id)));
        let version = if let Some(version) = pin {
            version.clone()
        } else if let Some(project) = project {
            if !self.choice_cache.contains_key(project) {
                self.check_room(depth)?;
            }
            let choices = self.choices(project).await?;
            edge.candidates = choices.candidates;
            if edge.candidates.is_empty() {
                return Err(EdgeState::NoCompatibleVersion);
            }
            if !choices.unique_release {
                return Err(EdgeState::ChoiceRequired);
            }
            edge.candidates[0].clone()
        } else {
            return Err(EdgeState::ReferenceOnly);
        };

        let version_metadata = if let Some(index) = self.index.get(&version) {
            self.selected[*index].0.clone()
        } else {
            self.check_room(depth)?;
            self.lookup(&version).await?
        };
        if project.is_some_and(|id| id != version_metadata.project_id) {
            return Err(EdgeState::ProjectMismatch);
        }
        if !version_metadata.compatible {
            return Err(EdgeState::IncompatibleVersion);
        }
        if !self.index.contains_key(&version) {
            self.index.insert(version.clone(), self.selected.len());
            self.selected.push((version_metadata, depth));
        }
        edge.selected_version_id = Some(version);
        Ok(())
    }

    fn check_room(&mut self, depth: usize) -> Result<(), EdgeState> {
        if depth > MAX_DEPTH || self.selected.len() == MAX_NODES {
            self.truncated = true;
            return Err(EdgeState::LimitReached);
        }
        Ok(())
    }

    fn begin_lookup(&mut self) -> Result<(), EdgeState> {
        if self.lookups == MAX_LOOKUPS || Instant::now() >= self.deadline {
            self.truncated = true;
            return Err(EdgeState::LimitReached);
        }
        self.lookups += 1;
        Ok(())
    }

    async fn lookup(&mut self, version: &str) -> Result<Arc<VersionMetadata>, EdgeState> {
        if let Some(cached) = self.exact_cache.get(version) {
            return cached.clone();
        }
        self.begin_lookup()?;
        let result = match timeout_at(
            self.deadline,
            self.provider.graph_version(self.request, version),
        )
        .await
        {
            Ok(Ok(metadata)) => Ok(Arc::new(metadata)),
            Ok(Err(_)) => Err(EdgeState::MetadataUnavailable),
            Err(_) => {
                self.truncated = true;
                Err(EdgeState::LimitReached)
            }
        };
        self.exact_cache.insert(version.into(), result.clone());
        result
    }

    async fn choices(&mut self, project: &str) -> Result<ProjectChoices, EdgeState> {
        if let Some(cached) = self.choice_cache.get(project) {
            return cached.clone();
        }
        self.begin_lookup()?;
        let result = match timeout_at(
            self.deadline,
            self.provider.graph_choices(self.request, project),
        )
        .await
        {
            Ok(Ok(choices)) => Ok(choices),
            Ok(Err(_)) => Err(EdgeState::MetadataUnavailable),
            Err(_) => {
                self.truncated = true;
                Err(EdgeState::LimitReached)
            }
        };
        self.choice_cache.insert(project.into(), result.clone());
        result
    }

    fn finish(self) -> DependencyGraph {
        let nodes: Vec<_> = self
            .selected
            .iter()
            .map(|(version, _)| DependencyNode {
                project_id: version.project_id.clone(),
                version_id: version.version_id.clone(),
                declarations_known: version.declarations.is_some(),
            })
            .collect();
        let issues = graph_issues(&nodes, &self.edges, &self.index);
        let complete = !self.truncated
            && issues.is_empty()
            && nodes.iter().all(|node| node.declarations_known)
            && self.edges.iter().all(|edge| {
                edge.declaration.dependency_type != DependencyType::Required
                    || edge.state == EdgeState::Selected
            });
        DependencyGraph {
            root_version_id: self.request.version_id.clone(),
            complete,
            truncated: self.truncated,
            nodes,
            edges: self.edges,
            issues,
        }
    }
}

fn graph_issues(
    nodes: &[DependencyNode],
    edges: &[DependencyEdge],
    index: &HashMap<String, usize>,
) -> Vec<GraphIssue> {
    let mut issues = Vec::new();
    let mut by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for node in nodes {
        by_project
            .entry(&node.project_id)
            .or_default()
            .push(node.version_id.clone());
    }
    for (project_id, version_ids) in by_project {
        if version_ids.len() > 1 {
            issues.push(GraphIssue::ConflictingVersions {
                project_id: project_id.into(),
                version_ids,
            });
        }
    }

    let mut adjacency = vec![Vec::new(); nodes.len()];
    for edge in edges {
        if let Some(to) = &edge.selected_version_id {
            let destinations = &mut adjacency[index[&edge.from_version_id]];
            if !destinations.contains(&index[to]) {
                destinations.push(index[to]);
            }
        } else if edge.declaration.dependency_type == DependencyType::Incompatible {
            let project = edge.declaration.project_id.as_deref();
            let version = edge.declaration.version_id.as_deref();
            if project.is_none() && version.is_none() {
                continue;
            }
            let version_ids: Vec<_> = nodes
                .iter()
                .filter(|node| {
                    project.is_none_or(|id| id == node.project_id)
                        && version.is_none_or(|id| id == node.version_id)
                })
                .map(|node| node.version_id.clone())
                .collect();
            if !version_ids.is_empty() {
                issues.push(GraphIssue::DeclaredIncompatible {
                    from_version_id: edge.from_version_id.clone(),
                    version_ids,
                });
            }
        }
    }

    // DFS on at most MAX_NODES; a back edge points into the active path, not merely a shared child.
    fn visit(
        node: usize,
        adjacency: &[Vec<usize>],
        active: &mut Vec<usize>,
        visited: &mut [bool],
        nodes: &[DependencyNode],
        issues: &mut Vec<GraphIssue>,
    ) {
        visited[node] = true;
        active.push(node);
        for &next in &adjacency[node] {
            if let Some(start) = active.iter().position(|&ancestor| ancestor == next) {
                let version_ids = active[start..]
                    .iter()
                    .chain(std::iter::once(&next))
                    .map(|&id| nodes[id].version_id.clone())
                    .collect();
                issues.push(GraphIssue::Cycle { version_ids });
            } else if !visited[next] {
                visit(next, adjacency, active, visited, nodes, issues);
            }
        }
        active.pop();
    }
    visit(
        0,
        &adjacency,
        &mut Vec::new(),
        &mut vec![false; nodes.len()],
        nodes,
        &mut issues,
    );
    issues
}
