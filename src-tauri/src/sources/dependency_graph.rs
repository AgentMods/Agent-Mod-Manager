//! Advisory closure of required Modrinth version declarations with bounded work.
use super::{array, http::graph_api_client, modrinth_matches, Provider};
use crate::{
    agent::sse::response_payload_capacity,
    error::{AppError, AppResult},
    sources::{
        modrinth_id, valid_modrinth_dependencies, DependencyType, ModDependency,
        MAX_MODRINTH_DEPENDENCIES,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

// These limit representative evidence, not the graph traversal.
const MAX_CHOICES: usize = 5;
const MAX_INCOMPATIBLE_WITNESSES: usize = 5;
// The decoded version metadata is only part of live graph memory: traversal
// also retains indices, edges, selected IDs, and a serializable result.
const PROJECTION_OVERHEAD: usize = 4;

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
        match_count: usize,
    },
}

struct VersionMetadata {
    project_id: String,
    version_id: String,
    compatible: bool,
    declarations: Option<Vec<ModDependency>>,
}

impl VersionMetadata {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.project_id.len()
            + self.version_id.len()
            + self.declarations.as_ref().map_or(0, |rows| {
                rows.capacity() * std::mem::size_of::<ModDependency>()
                    + rows
                        .iter()
                        .map(|row| {
                            [&row.project_id, &row.version_id, &row.file_name]
                                .into_iter()
                                .flatten()
                                .map(String::capacity)
                                .sum::<usize>()
                        })
                        .sum::<usize>()
            })
    }
}

#[derive(Clone)]
struct ProjectChoices {
    candidates: Vec<String>,
    unique_release: bool,
}

struct GraphBuilder<'a> {
    provider: &'a Provider,
    request: &'a DependencyGraphRequest,
    client: reqwest::Client,
    cancel: &'a CancellationToken,
    metadata_bytes_left: usize,
    truncated: bool,
    exact_cache: HashMap<String, Result<Arc<VersionMetadata>, EdgeState>>,
    choice_cache: HashMap<String, Result<ProjectChoices, EdgeState>>,
    selected: Vec<Arc<VersionMetadata>>,
    index: HashMap<String, usize>,
    edges: Vec<DependencyEdge>,
}

impl Provider {
    pub(super) async fn modrinth_graph(
        &self,
        request: &DependencyGraphRequest,
        cancel: &CancellationToken,
    ) -> AppResult<DependencyGraph> {
        self.modrinth_graph_with_budget(
            request,
            cancel,
            response_payload_capacity() / PROJECTION_OVERHEAD,
        )
        .await
    }

    pub(in crate::sources) async fn modrinth_graph_with_budget(
        &self,
        request: &DependencyGraphRequest,
        cancel: &CancellationToken,
        metadata_budget: usize,
    ) -> AppResult<DependencyGraph> {
        let client = graph_api_client()?;
        let root = cancellable(
            cancel,
            self.graph_version(&client, request, &request.version_id),
        )
        .await?;
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
        let mut root = root;
        let root_bytes = root.retained_bytes();
        let truncated = root_bytes > metadata_budget;
        if truncated {
            root.declarations = None;
        }
        let metadata_bytes_left = metadata_budget.saturating_sub(root_bytes);
        let root_id = root.version_id.clone();
        let root = Arc::new(root);
        let mut builder = GraphBuilder {
            provider: self,
            request,
            client,
            cancel,
            metadata_bytes_left,
            truncated,
            exact_cache: HashMap::from([(root_id.clone(), Ok(root.clone()))]),
            choice_cache: HashMap::new(),
            selected: vec![root],
            index: HashMap::from([(root_id, 0)]),
            edges: Vec::new(),
        };
        builder.walk().await?;
        builder.finish().await
    }

    async fn graph_version(
        &self,
        client: &reqwest::Client,
        request: &DependencyGraphRequest,
        version_id: &str,
    ) -> AppResult<VersionMetadata> {
        let version_id = modrinth_id(version_id)?;
        let version = self
            .fetch_json(client.get(format!("https://api.modrinth.com/v2/version/{version_id}")))
            .await?;
        if version["id"].as_str() != Some(version_id) {
            return Err(AppError::Other("Modrinth 返回的版本 ID 与查询不符".into()));
        }
        let project_id = modrinth_id(version["project_id"].as_str().unwrap_or_default())?;
        Ok(VersionMetadata {
            project_id: project_id.into(),
            version_id: version_id.into(),
            compatible: graph_compatibility(&version, request)?,
            declarations: version.get("dependencies").and_then(|value| {
                let rows = Vec::<ModDependency>::deserialize(value).ok()?;
                // Reuse direct-dependency item validation without imposing the
                // direct tool's per-input list quota on a provider response.
                rows.chunks(MAX_MODRINTH_DEPENDENCIES)
                    .all(valid_modrinth_dependencies)
                    .then_some(rows)
            }),
        })
    }

    async fn graph_choices(
        &self,
        client: &reqwest::Client,
        request: &DependencyGraphRequest,
        project: &str,
    ) -> AppResult<ProjectChoices> {
        let version_filters = [
            (
                "game_versions",
                serde_json::to_string(&[&request.game_version])?,
            ),
            ("loaders", serde_json::to_string(&[&request.loader])?),
        ];
        let list = self
            .fetch_json(
                client
                    .get(format!(
                        "https://api.modrinth.com/v2/project/{project}/version"
                    ))
                    .query(&version_filters),
            )
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

fn check_cancel(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

async fn checkpoint(cancel: &CancellationToken, step: usize) -> AppResult<()> {
    if step.is_multiple_of(256) {
        tokio::task::yield_now().await;
        check_cancel(cancel)?;
    }
    Ok(())
}

async fn cancellable<T>(
    cancel: &CancellationToken,
    work: impl Future<Output = AppResult<T>>,
) -> AppResult<T> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AppError::Cancelled),
        result = work => {
            check_cancel(cancel)?;
            result
        },
    }
}

fn graph_compatibility(
    version: &serde_json::Value,
    request: &DependencyGraphRequest,
) -> AppResult<bool> {
    for key in ["game_versions", "loaders"] {
        let filters = array(&version[key])?;
        if filters.iter().any(|value| !value.is_string()) {
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
    async fn walk(&mut self) -> AppResult<()> {
        // The append-only node list also acts as a FIFO. Each exact version
        // enters it once, regardless of diamonds or cycles in declarations.
        let mut cursor = 0;
        while let Some(node) = self.selected.get(cursor).cloned() {
            cursor += 1;
            let mut seen = HashSet::new();
            for declaration in node.declarations.iter().flatten() {
                // Keep the cached/CPU-only path cooperatively cancellable too.
                tokio::task::yield_now().await;
                check_cancel(self.cancel)?;
                if !seen.insert((
                    declaration.dependency_type as u8,
                    &declaration.project_id,
                    &declaration.version_id,
                    &declaration.file_name,
                )) {
                    continue;
                }
                let mut edge = DependencyEdge {
                    from_version_id: node.version_id.clone(),
                    declaration: declaration.clone(),
                    state: EdgeState::NotFollowed,
                    selected_version_id: None,
                    candidates: Vec::new(),
                };
                if declaration.dependency_type == DependencyType::Required {
                    edge.state = match self.follow(&mut edge).await {
                        Ok(()) => EdgeState::Selected,
                        Err(state) => {
                            check_cancel(self.cancel)?;
                            state
                        }
                    };
                }
                self.edges.push(edge);
                if self.truncated {
                    return Ok(());
                }
            }
        }
        check_cancel(self.cancel)
    }

    async fn follow(&mut self, edge: &mut DependencyEdge) -> Result<(), EdgeState> {
        let project = edge.declaration.project_id.as_deref();
        let pin = edge
            .declaration
            .version_id
            .as_ref()
            .or_else(|| project.and_then(|id| self.request.project_pins.get(id)));
        let version = if let Some(version) = pin {
            version.clone()
        } else if let Some(project) = project {
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

        let version_metadata = self.lookup(&version).await?;
        if project.is_some_and(|id| id != version_metadata.project_id) {
            return Err(EdgeState::ProjectMismatch);
        }
        if !version_metadata.compatible {
            return Err(EdgeState::IncompatibleVersion);
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = self.index.entry(version.clone())
        {
            entry.insert(self.selected.len());
            self.selected.push(version_metadata);
        }
        edge.selected_version_id = Some(version);
        Ok(())
    }

    async fn lookup(&mut self, version: &str) -> Result<Arc<VersionMetadata>, EdgeState> {
        if let Some(cached) = self.exact_cache.get(version) {
            return cached.clone();
        }
        let result = match cancellable(
            self.cancel,
            self.provider
                .graph_version(&self.client, self.request, version),
        )
        .await
        {
            Ok(metadata) => {
                if let Some(left) = self
                    .metadata_bytes_left
                    .checked_sub(metadata.retained_bytes())
                {
                    self.metadata_bytes_left = left;
                    Ok(Arc::new(metadata))
                } else {
                    self.truncated = true;
                    Err(EdgeState::LimitReached)
                }
            }
            Err(_) => Err(EdgeState::MetadataUnavailable),
        };
        self.exact_cache.insert(version.into(), result.clone());
        result
    }

    async fn choices(&mut self, project: &str) -> Result<ProjectChoices, EdgeState> {
        if let Some(cached) = self.choice_cache.get(project) {
            return cached.clone();
        }
        let result = cancellable(
            self.cancel,
            self.provider
                .graph_choices(&self.client, self.request, project),
        )
        .await
        .map_err(|_| EdgeState::MetadataUnavailable);
        self.choice_cache.insert(project.into(), result.clone());
        result
    }

    async fn finish(self) -> AppResult<DependencyGraph> {
        let mut nodes = Vec::with_capacity(self.selected.len());
        for (step, version) in self.selected.iter().enumerate() {
            checkpoint(self.cancel, step).await?;
            nodes.push(DependencyNode {
                project_id: version.project_id.clone(),
                version_id: version.version_id.clone(),
                declarations_known: version.declarations.is_some(),
            });
        }
        let issues = graph_issues(&nodes, &self.edges, &self.index, self.cancel).await?;
        check_cancel(self.cancel)?;
        let complete = !self.truncated
            && issues.is_empty()
            && nodes.iter().all(|node| node.declarations_known)
            && self.edges.iter().all(|edge| {
                edge.declaration.dependency_type != DependencyType::Required
                    || edge.state == EdgeState::Selected
            });
        Ok(DependencyGraph {
            root_version_id: self.request.version_id.clone(),
            complete,
            truncated: self.truncated,
            nodes,
            edges: self.edges,
            issues,
        })
    }
}

async fn graph_issues(
    nodes: &[DependencyNode],
    edges: &[DependencyEdge],
    index: &HashMap<String, usize>,
    cancel: &CancellationToken,
) -> AppResult<Vec<GraphIssue>> {
    let mut issues = Vec::new();
    let mut by_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (step, node) in nodes.iter().enumerate() {
        checkpoint(cancel, step).await?;
        by_project
            .entry(&node.project_id)
            .or_default()
            .push(node.version_id.clone());
    }
    for (step, (&project_id, version_ids)) in by_project.iter().enumerate() {
        checkpoint(cancel, step).await?;
        if version_ids.len() > 1 {
            issues.push(GraphIssue::ConflictingVersions {
                project_id: project_id.into(),
                version_ids: version_ids.clone(),
            });
        }
    }

    let mut adjacency = vec![Vec::new(); nodes.len()];
    let mut distinct = HashSet::new();
    for (step, edge) in edges.iter().enumerate() {
        checkpoint(cancel, step).await?;
        if let Some(to) = &edge.selected_version_id {
            let pair = (index[&edge.from_version_id], index[to]);
            if distinct.insert(pair) {
                adjacency[pair.0].push(pair.1);
            }
        } else if edge.declaration.dependency_type == DependencyType::Incompatible {
            let project = edge.declaration.project_id.as_deref();
            let version = edge.declaration.version_id.as_deref();
            let (version_ids, match_count) = if let Some(id) = version {
                let Some(&node) = index.get(id) else { continue };
                if project.is_some_and(|project| project != nodes[node].project_id) {
                    continue;
                }
                (vec![id.into()], 1)
            } else if let Some(versions) = project.and_then(|id| by_project.get(id)) {
                (
                    versions
                        .iter()
                        .take(MAX_INCOMPATIBLE_WITNESSES)
                        .cloned()
                        .collect(),
                    versions.len(),
                )
            } else {
                continue;
            };
            issues.push(GraphIssue::DeclaredIncompatible {
                from_version_id: edge.from_version_id.clone(),
                version_ids,
                match_count,
            });
        }
    }

    // Enumerating closed paths in a dense cyclic graph would amplify the
    // result. A closed witness is sufficient; all observed edges remain in
    // the graph. Mark the active path separately from already explored nodes
    // so a shared child in a DAG is not a cycle.
    if let Some(cycle) = cycle_witness(&adjacency, cancel).await? {
        issues.push(GraphIssue::Cycle {
            version_ids: cycle
                .into_iter()
                .map(|id| nodes[id].version_id.clone())
                .collect(),
        });
    }
    Ok(issues)
}

async fn cycle_witness(
    adjacency: &[Vec<usize>],
    cancel: &CancellationToken,
) -> AppResult<Option<Vec<usize>>> {
    let mut visited = vec![false; adjacency.len()];
    let mut active = vec![None; adjacency.len()];
    let mut stack = Vec::new();
    let mut step = 0;
    for start in 0..adjacency.len() {
        checkpoint(cancel, start).await?;
        if visited[start] {
            continue;
        }
        visited[start] = true;
        active[start] = Some(0);
        stack.push((start, adjacency[start].iter()));
        while let Some((node, neighbors)) = stack.last_mut() {
            checkpoint(cancel, step).await?;
            step += 1;
            if let Some(&next) = neighbors.next() {
                if let Some(position) = active[next] {
                    return Ok(Some(
                        stack[position..]
                            .iter()
                            .map(|(node, _)| *node)
                            .chain(std::iter::once(next))
                            .collect(),
                    ));
                }
                if !visited[next] {
                    visited[next] = true;
                    active[next] = Some(stack.len());
                    stack.push((next, adjacency[next].iter()));
                }
            } else {
                active[*node] = None;
                stack.pop();
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_stalled_lookup_is_dropped_when_the_user_cancels() {
        struct OnDrop<'a>(&'a std::cell::Cell<bool>);
        impl Drop for OnDrop<'_> {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        let dropped = std::cell::Cell::new(false);
        let guard = OnDrop(&dropped);
        let cancel = CancellationToken::new();
        let stalled = async {
            let _guard = guard;
            let sent = cancel.clone();
            tokio::spawn(async move { sent.cancel() });
            std::future::pending::<AppResult<()>>().await
        };
        assert!(matches!(
            cancellable(&cancel, stalled).await,
            Err(AppError::Cancelled)
        ));
        assert!(dropped.get());
    }

    #[tokio::test]
    async fn cycle_analysis_is_iterative_even_for_long_paths() {
        let cancel = CancellationToken::new();
        let count = 30_000;
        let mut chain: Vec<Vec<usize>> = (1..count).map(|next| vec![next]).collect();
        chain.push(vec![]);
        assert!(cycle_witness(&chain, &cancel).await.unwrap().is_none());
        chain[count - 1].push(0);
        let witness = cycle_witness(&chain, &cancel).await.unwrap().unwrap();
        assert_eq!(witness.len(), count + 1);
        assert_eq!(witness.first(), witness.last());
    }
}
