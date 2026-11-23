use super::{get_install, InstalledFile, InstalledMod};
use crate::{
    artifacts::DownloadOrigin,
    error::{AppError, AppResult},
    transaction::{fingerprint, FileEngine, FileOp, FilePlan, Fingerprint},
    trash::{self, TrashDraft, TrashRequest},
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub(super) struct Replacement {
    pub previous: InstalledMod,
    next: DownloadOrigin,
}

fn conflict(message: impl Into<String>) -> AppError {
    AppError::Conflict(message.into())
}

fn same_project(a: &DownloadOrigin, b: &DownloadOrigin) -> bool {
    a.source_id == b.source_id && a.project_id == b.project_id
}

fn scoped_project(origin: &DownloadOrigin) -> bool {
    origin.source_id != "nexus"
        || origin
            .project_id
            .split_once('/')
            .is_some_and(|(domain, id)| {
                !domain.is_empty()
                    && !id.starts_with('0')
                    && id.bytes().all(|c| c.is_ascii_digit())
                    && id.parse::<u64>().is_ok_and(|id| id > 0)
            })
}

fn shares_project_slot(existing: &DownloadOrigin, target: &DownloadOrigin) -> bool {
    same_project(existing, target)
        || (existing.source_id == "nexus"
            && target.source_id == "nexus"
            && !scoped_project(existing)
            && target.project_id.split_once('/').is_some_and(|(_, id)| {
                existing
                    .project_id
                    .parse::<u64>()
                    .ok()
                    .zip(id.parse::<u64>().ok())
                    .is_some_and(|(old, new)| old == new)
            }))
}

fn expected(file: &InstalledFile) -> Fingerprint {
    Fingerprint {
        kind: "file".into(),
        size: file.size,
        sha256: file.sha256.clone(),
    }
}

// Compare path components conservatively on every platform so previews remain
// safe on case-insensitive Steam libraries and external volumes.
pub(super) fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

fn overlaps(paths: &BTreeSet<String>, path: &str) -> bool {
    if paths.contains(path)
        || path
            .match_indices('/')
            .any(|(i, _)| paths.contains(&path[..i]))
    {
        return true;
    }
    let prefix = format!("{path}/");
    paths
        .range(prefix.clone()..)
        .next()
        .is_some_and(|p| p.starts_with(&prefix))
}

pub(super) fn verify_ownership(
    engine: &FileEngine,
    paths: &BTreeSet<String>,
    ignored: (&str, Option<&str>),
    project: Option<(&str, &DownloadOrigin)>,
) -> AppResult<()> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT game_id,files_json,metadata_json FROM installs
        WHERE id<>?1 AND id<>COALESCE(?2,'') AND ({}) IN ('enabled','installed','partially_restored')",
        super::effective_install_state(),
    ))?;
    let mut rows = stmt.query(rusqlite::params![ignored.0, ignored.1])?;
    while let Some(row) = rows.next()? {
        let files: Vec<InstalledFile> = serde_json::from_str(&row.get::<_, String>(1)?)?;
        if files
            .iter()
            .any(|file| overlaps(paths, &path_key(&file.path)))
        {
            return Err(conflict(
                "涉及其他已安装或部分恢复 Mod 的文件，不能替换或移除",
            ));
        }
        if let Some((game, origin)) = project {
            if row.get::<_, String>(0)? == game {
                let metadata: serde_json::Value = serde_json::from_str(&row.get::<_, String>(2)?)?;
                if metadata
                    .get("downloadOrigin")
                    .and_then(|v| DownloadOrigin::deserialize(v).ok())
                    .is_some_and(|other| shares_project_slot(&other, origin))
                {
                    return Err(conflict("同一游戏还有此项目的其他安装，请先解决重复版本"));
                }
            }
        }
    }
    Ok(())
}

impl Replacement {
    pub fn load(
        engine: &FileEngine,
        previous_id: &str,
        game_id: &str,
        root: &Path,
        mod_type: &str,
        next: Option<&DownloadOrigin>,
    ) -> AppResult<Self> {
        let previous = get_install(engine, previous_id)?;
        let (Some(prior), Some(next)) = (&previous.download_origin, next) else {
            return Err(conflict("版本替换需要新旧归档都有精确下载记录"));
        };
        if previous.status != "enabled"
            || previous.game_id != game_id
            || previous.install_root != root
            || previous.metadata["modType"].as_str() != Some(mod_type)
        {
            return Err(conflict(
                "原版本不再处于可替换状态，或登记游戏、Mod 类型、安装根不一致",
            ));
        }
        if !scoped_project(prior) || !scoped_project(next) {
            return Err(conflict("旧 Nexus 项目记录缺少游戏域名，不能可靠替换"));
        }
        if !same_project(prior, next) || prior.version_id == next.version_id {
            return Err(conflict("只可替换同一来源和项目的不同精确版本"));
        }
        let mut keys = BTreeSet::new();
        if previous.files.is_empty()
            || previous.files.iter().any(|file| {
                file.before.is_some()
                    || !file.path.starts_with(root)
                    || file.path == root
                    || !keys.insert(path_key(&file.path))
            })
        {
            return Err(conflict("原版本未独占安装文件，或其清单与登记安装根不符"));
        }
        for file in &previous.files {
            engine.stable_path(&file.path, false)?;
        }
        Ok(Self {
            previous,
            next: next.clone(),
        })
    }

    pub fn drafts(&self, engine: &FileEngine, copies: &[FileOp]) -> AppResult<Vec<TrashDraft>> {
        let targets: BTreeSet<_> = copies.iter().map(FileOp::dst).collect();
        let paths = self
            .previous
            .files
            .iter()
            .filter(|file| !targets.contains(file.path.as_path()))
            .map(|file| {
                file.path
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| conflict("旧文件路径不是 UTF-8"))
            })
            .collect::<AppResult<_>>()?;
        trash::prepare_trash(
            engine,
            &TrashRequest {
                paths,
                game_id: Some(self.previous.game_id.clone()),
                reason: format!("替换 {} 版本；请通过替换事务回滚还原", self.previous.name),
            },
        )
    }

    pub fn inspect_plan(
        &self,
        engine: &FileEngine,
        plan: &FilePlan,
        next_install_id: Option<&str>,
    ) -> AppResult<()> {
        let mut original: BTreeMap<_, _> = self
            .previous
            .files
            .iter()
            .map(|f| (f.path.as_path(), f))
            .collect();
        let original_keys: BTreeSet<_> = original.keys().map(|p| path_key(p)).collect();
        let mut touched = original_keys.clone();
        for entry in &plan.entries {
            match &entry.operation {
                FileOp::Copy { dst, .. } => {
                    engine.stable_path(dst, false)?;
                    if !dst.starts_with(&self.previous.install_root)
                        || dst == &self.previous.install_root
                    {
                        return Err(conflict("替换目标超出原版本的安装根"));
                    }
                    let prior = original.remove(dst.as_path());
                    let key = path_key(dst);
                    if (prior.is_none() && overlaps(&original_keys, &key))
                        || entry.before != prior.map(expected)
                    {
                        return Err(conflict(format!(
                            "目标已被修改、存在未登记文件或路径别名：{}",
                            crate::paths::display_path(dst)
                        )));
                    }
                    touched.insert(key);
                }
                FileOp::Move { src, dst } => {
                    let prior = original.remove(src.as_path());
                    if prior.is_none()
                        || entry.source != prior.map(expected)
                        || entry.before.is_some()
                        || !dst.starts_with(engine.paths.trash())
                    {
                        return Err(conflict(format!(
                            "旧版本文件已修改或不能移入回收站：{}",
                            crate::paths::display_path(src)
                        )));
                    }
                }
                FileOp::Mkdir { .. }
                | FileOp::WriteText { .. }
                | FileOp::Compress { .. }
                | FileOp::Extract { .. } => return Err(conflict("版本替换含未知变更")),
            }
        }
        if !original.is_empty() {
            return Err(conflict("部分旧版本文件未纳入替换计划"));
        }
        verify_ownership(
            engine,
            &touched,
            (&self.previous.id, next_install_id),
            Some((&self.previous.game_id, &self.next)),
        )
    }
}

pub(crate) fn validate_apply(engine: &FileEngine, plan: &FilePlan) -> AppResult<()> {
    let link: Option<(String, String, Option<String>)> = engine.db.conn().query_row(
        "SELECT i.install_id,i.action,r.replaced_install_id FROM install_transactions i
         LEFT JOIN install_replacements r ON r.transaction_id=i.transaction_id WHERE i.transaction_id=?1",
        [&plan.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    if let Some((next_id, _, Some(previous_id))) = link {
        let next = get_install(engine, &next_id)?;
        if next.status != "planned" {
            return Err(conflict("替换计划不再待执行"));
        }
        Replacement::load(
            engine,
            &previous_id,
            &next.game_id,
            &next.install_root,
            next.metadata["modType"].as_str().unwrap_or_default(),
            next.download_origin.as_ref(),
        )?
        .inspect_plan(engine, plan, Some(&next_id))?;
    } else if let Some((id, action, None)) = link {
        if action == "uninstall" {
            super::validate_uninstall_apply(engine, plan, &id)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_rollback(engine: &FileEngine, transaction_id: &str) -> AppResult<bool> {
    let (superseded, replacement): (bool, Option<(String, String)>) = {
        let conn = engine.db.conn();
        let forgotten: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM install_transactions a
            JOIN install_missing_removals m ON m.install_id=a.install_id WHERE a.transaction_id=?1)",
            [transaction_id], |r| r.get(0))?;
        if forgotten {
            return Err(conflict(
                "此安装在文件全部缺失后已从管理列表移除，不能再回滚其文件",
            ));
        }
        let superseded = conn.query_row("SELECT EXISTS(SELECT 1 FROM install_transactions i
            JOIN install_replacements r ON r.replaced_install_id=i.install_id
            JOIN file_transactions f ON f.id=r.transaction_id
            WHERE i.transaction_id=?1 AND f.status IN ('committed','applying','rolling_back','recovery_failed'))",
            [transaction_id], |r| r.get(0))?;
        let replacement = conn
            .query_row(
                "SELECT replaced_install_id,replacement_install_id
            FROM install_replacements WHERE transaction_id=?1",
                [transaction_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        (superseded, replacement)
    };
    if superseded {
        return Err(conflict("先回滚较新的版本替换，再回滚此事务"));
    }
    let Some((previous_id, next_id)) = replacement else {
        return Ok(false);
    };
    let next = get_install(engine, &next_id)?;
    if next.status != "enabled" {
        return Err(conflict(
            "替换版本不再完整部署，请先处理其卸载或部分恢复状态",
        ));
    }
    let mut touched = BTreeSet::new();
    for entry in engine.get_plan(transaction_id)?.entries {
        let dst = entry.operation.dst();
        engine.stable_path(dst, true)?;
        if fingerprint(dst)? != entry.source {
            return Err(conflict(format!(
                "替换后的文件已修改或丢失，不能回滚：{}",
                crate::paths::display_path(dst)
            )));
        }
        match &entry.operation {
            FileOp::Copy { dst, .. } => {
                touched.insert(path_key(dst));
            }
            FileOp::Move { src, .. } => {
                engine.stable_path(src, false)?;
                if fingerprint(src)?.is_some() {
                    return Err(conflict(format!(
                        "被替换文件的原位置已被占用：{}",
                        crate::paths::display_path(src)
                    )));
                }
                touched.insert(path_key(src));
            }
            FileOp::Mkdir { .. }
            | FileOp::WriteText { .. }
            | FileOp::Compress { .. }
            | FileOp::Extract { .. } => return Err(conflict("替换事务含未知变更")),
        }
    }
    verify_ownership(
        engine,
        &touched,
        (&next_id, Some(&previous_id)),
        next.download_origin
            .as_ref()
            .map(|origin| (next.game_id.as_str(), origin)),
    )?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nexus(project_id: &str) -> DownloadOrigin {
        DownloadOrigin {
            artifact_id: "fixture".into(),
            source_id: "nexus".into(),
            project_id: project_id.into(),
            version_id: "700".into(),
        }
    }

    #[test]
    fn nexus_project_slots_never_treat_a_bare_mod_id_as_exact_origin() {
        let legacy = nexus("0123");
        let skyrim = nexus("skyrimspecialedition/123");
        let fallout = nexus("fallout4/123");
        assert!(!scoped_project(&legacy));
        assert!(scoped_project(&skyrim));
        assert!(!same_project(&skyrim, &fallout));
        assert!(!same_project(&legacy, &skyrim));
        assert!(shares_project_slot(&legacy, &skyrim));
        assert!(!shares_project_slot(&fallout, &skyrim));
        assert!(!shares_project_slot(&nexus("124"), &skyrim));
    }
}
