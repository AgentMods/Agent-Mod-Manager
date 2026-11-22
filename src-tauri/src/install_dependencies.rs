//! Compare Modrinth declarations with the target game's install journal.
//! Graph review also checks current files of selected exact matches, not loader state.
use crate::{
    artifacts::{self, DownloadOrigin},
    error::{AppError, AppResult},
    games::Game,
    install::INSTALL_STATE,
    install_file_evidence::{FileEvidence, FileEvidencePass},
    sources::{
        modrinth_id, valid_modrinth_dependencies, DependencyGraph, DependencyGraphRequest,
        DependencyType, ModDependency, Sources,
    },
    transaction::FileEngine,
};
use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

use uuid::Uuid;
const MAX_MATCHES_PER_DECLARATION: usize = 4;

/// Tauri IPC calls do not have a request cancellation channel. A caller can
/// provide a fresh UUID and cancel the running read with a second IPC call.
#[derive(Default)]
pub(crate) struct DesktopDependencyOperations(Mutex<HashMap<Uuid, CancellationToken>>);

pub(crate) struct DesktopDependencyOperation<'a> {
    registry: &'a DesktopDependencyOperations,
    id: Option<Uuid>,
    pub(crate) cancel: CancellationToken,
}

impl DesktopDependencyOperations {
    pub(crate) fn start(&self, id: Option<Uuid>) -> AppResult<DesktopDependencyOperation<'_>> {
        let cancel = CancellationToken::new();
        if let Some(id) = id {
            use std::collections::hash_map::Entry;
            let mut active = self.0.lock().unwrap_or_else(|e| e.into_inner());
            match active.entry(id) {
                Entry::Vacant(entry) => {
                    entry.insert(cancel.clone());
                }
                Entry::Occupied(_) => {
                    return Err(AppError::Conflict("依赖核对请求 ID 正在使用".into()))
                }
            }
        }
        Ok(DesktopDependencyOperation {
            registry: self,
            id,
            cancel,
        })
    }

    pub(crate) fn cancel(&self, id: Uuid) -> bool {
        let cancel = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&id)
            .cloned();
        if let Some(cancel) = cancel {
            cancel.cancel();
            true
        } else {
            false
        }
    }
}

impl Drop for DesktopDependencyOperation<'_> {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(id) = self.id {
            self.registry
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckRequest {
    pub game_id: String,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub dependencies: Option<Vec<ModDependency>>,
    pub replace_install_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyReport {
    pub game_id: String,
    pub excluded_install_id: Option<String>,
    pub checks: Option<Vec<DependencyCheck>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphReviewRequest {
    pub artifact_id: String,
    pub game_id: String,
    pub replace_install_id: Option<String>,
    pub game_version: String,
    pub loader: String,
    #[serde(default)]
    pub project_pins: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphReview {
    pub artifact_id: String,
    pub game_id: String,
    pub excluded_install_id: Option<String>,
    pub game_version: String,
    pub loader: String,
    pub graph: DependencyGraph,
    /// In the same order as graph.edges. Selected edges use the exact resolved ID.
    pub checks: Vec<DependencyCheck>,
    pub file_evidence: BTreeMap<String, FileEvidence>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallMatch {
    pub install_id: String,
    pub project_id: String,
    pub version_id: String,
    pub status: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyStatus {
    RecordedExact,
    RecordedProject,
    DifferentVersion,
    NotRecorded,
    Unconfirmed,
    Embedded,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnconfirmedReason {
    FileReference,
    AmbiguousRecords,
    PartialInstall,
    InconsistentIds,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyCheck {
    pub dependency: ModDependency,
    pub status: DependencyStatus,
    pub reason: Option<UnconfirmedReason>,
    pub matches: Vec<InstallMatch>,
    pub match_count: usize,
}

fn valid_local_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
}

pub fn check(engine: &FileEngine, request: &CheckRequest) -> AppResult<DependencyReport> {
    check_with_cancel(engine, request, &CancellationToken::new())
}

pub(crate) fn check_with_cancel(
    engine: &FileEngine,
    request: &CheckRequest,
    cancel: &CancellationToken,
) -> AppResult<DependencyReport> {
    if !valid_local_id(&request.game_id)
        || request
            .replace_install_id
            .as_deref()
            .is_some_and(|id| !valid_local_id(id))
        || request
            .dependencies
            .as_deref()
            .is_some_and(|deps| !valid_modrinth_dependencies(deps))
    {
        return Err(AppError::Invalid("游戏、替换项或依赖声明格式无效".into()));
    }

    journal_checkpoint(cancel)?;
    let conn = engine.db.conn();
    let tx = conn.unchecked_transaction()?;
    let dependencies = request.dependencies.as_deref();
    let include_records = dependencies.is_some_and(|rows| !rows.is_empty());
    validate_journal(&tx, &request.game_id, include_records)?;
    let mut excluded_install_id = None;
    let checks = match dependencies {
        Some(rows) if include_records => {
            let replacement = request.replace_install_id.as_deref();
            validate_replacement(&tx, &request.game_id, replacement, None)?;
            excluded_install_id = request.replace_install_id.clone();
            Some(scan_checks(
                &tx,
                &request.game_id,
                rows,
                replacement,
                cancel,
            )?)
        }
        Some(_) => Some(Vec::new()),
        None => None,
    };
    journal_checkpoint(cancel)?;
    Ok(DependencyReport {
        game_id: request.game_id.clone(),
        excluded_install_id,
        checks,
    })
}

fn validate_journal(conn: &Connection, game_id: &str, include_records: bool) -> AppResult<()> {
    let game_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM games WHERE id=?1)",
        [game_id],
        |row| row.get(0),
    )?;
    if !game_exists {
        return Err(AppError::NotFound("目标游戏未登记".into()));
    }
    if include_records {
        let pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))",
            [],
            |row| row.get(0),
        )?;
        if pending {
            return Err(AppError::Conflict(
                "有待处理的文件事务，暂不能核对依赖".into(),
            ));
        }
    }
    Ok(())
}

fn journal_query(id_predicate: &str) -> String {
    format!(
        "WITH records AS (
           SELECT id, CASE WHEN source='modrinth' THEN {INSTALL_STATE} END AS deployment_state,
             CASE WHEN source='modrinth' AND json_valid(metadata_json) THEN metadata_json ELSE '{{}}' END AS metadata
           FROM installs WHERE game_id=?1 {id_predicate}
         )
         SELECT id, deployment_state,
           CASE WHEN json_extract(metadata, '$.downloadOrigin.sourceId')='modrinth'
             AND json_type(metadata, '$.downloadOrigin.projectId')='text'
             AND instr(json_extract(metadata, '$.downloadOrigin.projectId'), char(0))=0
             THEN substr(json_extract(metadata, '$.downloadOrigin.projectId'), 1, 33) END,
           CASE WHEN json_type(metadata, '$.downloadOrigin.versionId')='text'
             AND instr(json_extract(metadata, '$.downloadOrigin.versionId'), char(0))=0
             THEN substr(json_extract(metadata, '$.downloadOrigin.versionId'), 1, 33) END
         FROM records"
    )
}

fn read_record(row: &Row<'_>) -> rusqlite::Result<Option<InstallMatch>> {
    let Some(status) = row.get::<_, Option<String>>(1)? else {
        return Ok(None);
    };
    if !matches!(status.as_str(), "enabled" | "partially_restored") {
        return Ok(None);
    }
    let (Some(project_id), Some(version_id)) = (
        row.get::<_, Option<String>>(2)?,
        row.get::<_, Option<String>>(3)?,
    ) else {
        return Ok(None);
    };
    if modrinth_id(&project_id).is_err() || modrinth_id(&version_id).is_err() {
        return Ok(None);
    }
    Ok(Some(InstallMatch {
        install_id: row.get(0)?,
        status,
        project_id,
        version_id,
    }))
}

fn validate_replacement(
    conn: &Connection,
    game_id: &str,
    id: Option<&str>,
    root: Option<&DownloadOrigin>,
) -> AppResult<()> {
    let Some(id) = id else { return Ok(()) };
    let mut statement = conn.prepare(&journal_query("AND id=?2"))?;
    let mut rows = statement.query(params![game_id, id])?;
    if let Some(row) = rows.next()? {
        if read_record(row)?.is_some_and(|record| {
            record.status == "enabled"
                && root.is_none_or(|root| {
                    record.project_id == root.project_id && record.version_id != root.version_id
                })
        }) {
            return Ok(());
        }
    }
    Err(AppError::Conflict("待替换的版本已变化，需重新选择".into()))
}

fn journal_checkpoint(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

fn scan_checks(
    conn: &Connection,
    game_id: &str,
    declarations: &[ModDependency],
    excluded: Option<&str>,
    cancel: &CancellationToken,
) -> AppResult<Vec<DependencyCheck>> {
    journal_checkpoint(cancel)?;
    let mut index = MatchIndex::new(declarations);
    if !index.projects.is_empty() || !index.versions.is_empty() {
        // SQLite's cursor progresses through the registered game's rows in a
        // read snapshot. Don't sort or collect the library; keep counts and
        // representative matches only for IDs mentioned in the declarations.
        let mut statement = conn.prepare(&journal_query("AND id IS NOT ?2"))?;
        let mut rows = statement.query(params![game_id, excluded])?;
        loop {
            let row = rows.next()?;
            journal_checkpoint(cancel)?;
            let Some(row) = row else { break };
            if let Some(record) = read_record(row)? {
                index.observe(&record);
            }
        }
    }
    Ok(declarations.iter().map(|dep| index.compare(dep)).collect())
}

fn graph_origin(engine: &FileEngine, request: &GraphReviewRequest) -> AppResult<DownloadOrigin> {
    if !valid_local_id(&request.artifact_id)
        || !valid_local_id(&request.game_id)
        || request
            .replace_install_id
            .as_deref()
            .is_some_and(|id| !valid_local_id(id))
    {
        return Err(AppError::Invalid(
            "归档、目标游戏或待替换记录格式无效".into(),
        ));
    }
    let root = artifacts::for_id(&engine.db, &request.artifact_id)?
        .and_then(|record| record.origin())
        .filter(|root| root.source_id == "modrinth" && root.artifact_id == request.artifact_id)
        .ok_or_else(|| AppError::Invalid("下载归档没有可核对的 Modrinth 项目及版本记录".into()))?;
    graph_game(engine, request)?;
    let conn = engine.db.conn();
    let tx = conn.unchecked_transaction()?;
    validate_journal(&tx, &request.game_id, true)?;
    validate_replacement(
        &tx,
        &request.game_id,
        request.replace_install_id.as_deref(),
        Some(&root),
    )?;
    Ok(root)
}

fn graph_game(engine: &FileEngine, request: &GraphReviewRequest) -> AppResult<Game> {
    let game = crate::games::get_game(&engine.db, &request.game_id)?;
    if game.adapter_slug.as_deref() != Some("minecraft-java") {
        return Err(AppError::Unsupported(
            "Modrinth 传递依赖核对需要已确认 Minecraft Java 适配包的游戏".into(),
        ));
    }
    Ok(game)
}

pub(crate) async fn review_graph(
    engine: FileEngine,
    sources: &Sources,
    request: GraphReviewRequest,
    cancel: &CancellationToken,
) -> AppResult<GraphReview> {
    journal_checkpoint(cancel)?;
    let preflight = engine.clone();
    let preflight_cancel = cancel.clone();
    let (request, root) = tokio::task::spawn_blocking(move || {
        journal_checkpoint(&preflight_cancel)?;
        let root = graph_origin(&preflight, &request)?;
        journal_checkpoint(&preflight_cancel)?;
        Ok::<_, AppError>((request, root))
    })
    .await
    .map_err(|_| AppError::Other("读取依赖核对范围失败".into()))??;
    let graph = sources
        .modrinth_dependency_graph(
            &DependencyGraphRequest {
                project_id: Some(root.project_id.clone()),
                version_id: root.version_id.clone(),
                game_version: request.game_version.clone(),
                loader: request.loader.clone(),
                project_pins: request.project_pins.clone(),
            },
            cancel,
        )
        .await?;
    let cancel = cancel.clone();
    tokio::task::spawn_blocking(move || {
        // Do not use an install snapshot taken before an asynchronous provider lookup.
        let _guard = crate::transaction::write_lock_cancellable(&cancel)?;
        let game = graph_game(&engine, &request)?;
        let checks = {
            let conn = engine.db.conn();
            let tx = conn.unchecked_transaction()?;
            let replacement = request.replace_install_id.as_deref();
            validate_journal(&tx, &request.game_id, true)?;
            validate_replacement(&tx, &request.game_id, replacement, Some(&root))?;
            scan_checks(
                &tx,
                &request.game_id,
                &graph.journal_declarations(),
                replacement,
                &cancel,
            )?
        };
        let mut file_evidence = BTreeMap::new();
        let mut pass = FileEvidencePass::new(&engine, &game)?.with_cancel(&cancel);
        for (selected, check) in graph.selected_edges().zip(&checks) {
            journal_checkpoint(&cancel)?;
            if selected && check.status == DependencyStatus::RecordedExact {
                let id = &check.matches[0].install_id;
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    file_evidence.entry(id.clone())
                {
                    entry.insert(pass.verify(id)?);
                }
            }
        }
        journal_checkpoint(&cancel)?;
        Ok(GraphReview {
            artifact_id: request.artifact_id,
            game_id: request.game_id,
            excluded_install_id: request.replace_install_id,
            game_version: request.game_version,
            loader: request.loader,
            graph,
            checks,
            file_evidence,
        })
    })
    .await
    .map_err(|_| AppError::Other("读取依赖安装记录失败".into()))?
}

#[derive(Default)]
struct MatchSummary {
    count: usize,
    samples: Vec<InstallMatch>,
}

#[derive(Default)]
struct MatchIndex<'a> {
    projects: HashMap<&'a str, MatchSummary>,
    versions: HashMap<&'a str, MatchSummary>,
    pairs: HashMap<&'a str, HashMap<&'a str, usize>>,
}

impl<'a> MatchIndex<'a> {
    fn new(declarations: &'a [ModDependency]) -> Self {
        let mut index = Self::default();
        for dep in declarations
            .iter()
            .filter(|dep| dep.dependency_type != DependencyType::Embedded)
        {
            let project = dep.project_id.as_deref();
            let version = dep.version_id.as_deref();
            if let Some(project) = project {
                index.projects.entry(project).or_default();
            }
            if let Some(version) = version {
                index.versions.entry(version).or_default();
            }
            if let (Some(project), Some(version)) = (project, version) {
                index
                    .pairs
                    .entry(project)
                    .or_default()
                    .entry(version)
                    .or_default();
            }
        }
        index
    }

    fn observe(&mut self, record: &InstallMatch) {
        for group in [
            self.projects.get_mut(record.project_id.as_str()),
            self.versions.get_mut(record.version_id.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            group.count += 1;
            let position = group
                .samples
                .partition_point(|seen| seen.install_id < record.install_id);
            if position < MAX_MATCHES_PER_DECLARATION {
                if group.samples.len() == MAX_MATCHES_PER_DECLARATION {
                    group.samples.pop();
                }
                group.samples.insert(position, record.clone());
            }
        }
        // Exact-pair groups are needed for inclusion/exclusion counts only.
        if let Some(count) = self
            .pairs
            .get_mut(record.project_id.as_str())
            .and_then(|versions| versions.get_mut(record.version_id.as_str()))
        {
            *count += 1;
        }
    }

    fn compare(&self, dependency: &ModDependency) -> DependencyCheck {
        use DependencyStatus::*;
        let project = dependency.project_id.as_deref();
        let version = dependency.version_id.as_deref();
        let mut matches = Vec::new();
        let mut match_count = 0;
        let (status, reason) = if dependency.dependency_type == DependencyType::Embedded {
            (Embedded, None)
        } else if project.is_none() && version.is_none() {
            (Unconfirmed, Some(UnconfirmedReason::FileReference))
        } else {
            let p = project.and_then(|id| self.projects.get(id));
            let v = version.and_then(|id| self.versions.get(id));
            let p_count = p.map_or(0, |group| group.count);
            let v_count = v.map_or(0, |group| group.count);
            let both = project
                .zip(version)
                .and_then(|(project, version)| self.pairs.get(project)?.get(version))
                .copied()
                .unwrap_or(0);
            match_count = p_count + (v_count - both);
            // The first k of the union must be within the first k of either
            // group. Sort these small samples by ID, regardless of scan order.
            let mut samples: Vec<_> = p
                .into_iter()
                .chain(v)
                .flat_map(|group| &group.samples)
                .collect();
            samples.sort_unstable_by(|a, b| a.install_id.cmp(&b.install_id));
            samples.dedup_by(|a, b| a.install_id == b.install_id);
            matches = samples
                .into_iter()
                .take(MAX_MATCHES_PER_DECLARATION)
                .cloned()
                .collect();
            match (match_count, matches.first()) {
                _ if project.is_some() && v_count > both => {
                    (Unconfirmed, Some(UnconfirmedReason::InconsistentIds))
                }
                (0, _) => (NotRecorded, None),
                (1, Some(row)) if row.status == "partially_restored" => {
                    (Unconfirmed, Some(UnconfirmedReason::PartialInstall))
                }
                (1, Some(row)) if version == Some(row.version_id.as_str()) => (RecordedExact, None),
                (1, Some(_)) if version.is_none() => (RecordedProject, None),
                (1, Some(_)) => (DifferentVersion, None),
                _ => (Unconfirmed, Some(UnconfirmedReason::AmbiguousRecords)),
            }
        };
        DependencyCheck {
            dependency: dependency.clone(),
            status,
            reason,
            matches,
            match_count,
        }
    }
}

#[cfg(test)]
#[path = "install_dependency_tests.rs"]
mod tests;
