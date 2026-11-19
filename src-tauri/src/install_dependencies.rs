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
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_RECORDS: usize = 4096;
const MAX_MATCHES_PER_DECLARATION: usize = 4;

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

    let mut report = DependencyReport {
        game_id: request.game_id.clone(),
        excluded_install_id: None,
        checks: request.dependencies.as_ref().map(|_| Vec::new()),
    };
    let mut records = read_records(
        engine,
        &request.game_id,
        request
            .dependencies
            .as_ref()
            .is_some_and(|rows| !rows.is_empty()),
    )?;
    let Some(dependencies) = request.dependencies.as_deref().filter(|d| !d.is_empty()) else {
        return Ok(report);
    };
    if let Some(id) = &request.replace_install_id {
        exclude_replacement(&mut records, id, None)?;
        report.excluded_install_id = Some(id.clone());
    }
    report.checks = Some(dependencies.iter().map(|d| compare(d, &records)).collect());
    Ok(report)
}

fn read_records(
    engine: &FileEngine,
    game_id: &str,
    include_records: bool,
) -> AppResult<Vec<InstallMatch>> {
    let conn = engine.db.conn();
    let game_exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM games WHERE id=?1)",
        [game_id],
        |row| row.get(0),
    )?;
    if !game_exists {
        return Err(AppError::NotFound("目标游戏未登记".into()));
    }
    if !include_records {
        return Ok(Vec::new());
    }

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
    let mut statement = conn.prepare(&format!(
        "WITH records AS (
           SELECT id, {INSTALL_STATE} AS deployment_state,
             CASE WHEN json_valid(metadata_json) THEN metadata_json ELSE '{{}}' END AS metadata
           FROM installs
           WHERE game_id=?1 AND source='modrinth' AND EXISTS (
             SELECT 1 FROM install_transactions a WHERE a.install_id=installs.id AND a.action='install'
           )
         )
         SELECT id, deployment_state,
           substr(json_extract(metadata, '$.downloadOrigin.projectId'), 1, 33),
           substr(json_extract(metadata, '$.downloadOrigin.versionId'), 1, 33)
         FROM records
         WHERE deployment_state IN ('enabled','partially_restored')
           AND json_extract(metadata, '$.downloadOrigin.sourceId')='modrinth'
           AND json_type(metadata, '$.downloadOrigin.projectId')='text'
           AND json_type(metadata, '$.downloadOrigin.versionId')='text'
         ORDER BY id LIMIT ?2"
    ))?;
    let mut records = statement
        .query_map(params![game_id, MAX_RECORDS + 1], |row| {
            Ok(InstallMatch {
                install_id: row.get(0)?,
                status: row.get(1)?,
                project_id: row.get(2)?,
                version_id: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if records.len() > MAX_RECORDS {
        return Err(AppError::Invalid("已装版本记录超出本次核对上限".into()));
    }
    records
        .retain(|row| modrinth_id(&row.project_id).is_ok() && modrinth_id(&row.version_id).is_ok());
    Ok(records)
}

fn exclude_replacement(
    records: &mut Vec<InstallMatch>,
    id: &str,
    root: Option<&DownloadOrigin>,
) -> AppResult<()> {
    let index = records
        .iter()
        .position(|record| {
            record.install_id == id
                && record.status == "enabled"
                && root.is_none_or(|root| {
                    record.project_id == root.project_id && record.version_id != root.version_id
                })
        })
        .ok_or_else(|| AppError::Conflict("待替换的版本已变化，需重新选择".into()))?;
    records.remove(index);
    Ok(())
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
    graph_records(engine, request, &root)?;
    Ok(root)
}

fn graph_records(
    engine: &FileEngine,
    request: &GraphReviewRequest,
    root: &DownloadOrigin,
) -> AppResult<(Game, Vec<InstallMatch>)> {
    let game = crate::games::get_game(&engine.db, &request.game_id)?;
    if game.adapter_slug.as_deref() != Some("minecraft-java") {
        return Err(AppError::Unsupported(
            "Modrinth 传递依赖核对需要已确认 Minecraft Java 适配包的游戏".into(),
        ));
    }
    let mut records = read_records(engine, &request.game_id, true)?;
    if let Some(id) = &request.replace_install_id {
        exclude_replacement(&mut records, id, Some(root))?;
    }
    Ok((game, records))
}

pub(crate) async fn review_graph(
    engine: FileEngine,
    sources: &Sources,
    request: GraphReviewRequest,
) -> AppResult<GraphReview> {
    let preflight = engine.clone();
    let (request, root) = tokio::task::spawn_blocking(move || {
        let root = graph_origin(&preflight, &request)?;
        Ok::<_, AppError>((request, root))
    })
    .await
    .map_err(|_| AppError::Other("读取依赖核对范围失败".into()))??;
    let graph = sources
        .modrinth_dependency_graph(&DependencyGraphRequest {
            project_id: Some(root.project_id.clone()),
            version_id: root.version_id.clone(),
            game_version: request.game_version.clone(),
            loader: request.loader.clone(),
            project_pins: request.project_pins.clone(),
        })
        .await?;
    tokio::task::spawn_blocking(move || {
        // Do not use an install snapshot taken before an asynchronous provider lookup.
        let _guard = crate::transaction::write_lock();
        let (game, records) = graph_records(&engine, &request, &root)?;
        let checks: Vec<_> = graph
            .journal_declarations()
            .iter()
            .map(|dep| compare(dep, &records))
            .collect();
        let mut file_evidence = BTreeMap::new();
        let mut pass = FileEvidencePass::new(&engine, &game)?;
        for (selected, check) in graph.selected_edges().zip(&checks) {
            if selected && check.status == DependencyStatus::RecordedExact {
                let id = &check.matches[0].install_id;
                if let std::collections::btree_map::Entry::Vacant(entry) =
                    file_evidence.entry(id.clone())
                {
                    entry.insert(pass.verify(id)?);
                }
            }
        }
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

fn compare(dependency: &ModDependency, records: &[InstallMatch]) -> DependencyCheck {
    use DependencyStatus::*;
    let project = dependency.project_id.as_deref();
    let version = dependency.version_id.as_deref();
    let mut matches = Vec::new();
    let mut match_count = 0;
    let mut inconsistent = false;
    let (status, reason) = if dependency.dependency_type == DependencyType::Embedded {
        (Embedded, None)
    } else if project.is_none() && version.is_none() {
        (Unconfirmed, Some(UnconfirmedReason::FileReference))
    } else {
        for record in records {
            let same_project = project == Some(record.project_id.as_str());
            let same_version = version == Some(record.version_id.as_str());
            if same_version && project.is_some() && !same_project {
                inconsistent = true;
            }
            if same_project || same_version {
                match_count += 1;
                if matches.len() < MAX_MATCHES_PER_DECLARATION {
                    matches.push(record.clone());
                }
            }
        }
        match (match_count, matches.first()) {
            _ if inconsistent => (Unconfirmed, Some(UnconfirmedReason::InconsistentIds)),
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

#[cfg(test)]
#[path = "install_dependency_tests.rs"]
mod tests;
