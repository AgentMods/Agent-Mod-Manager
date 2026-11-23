//! Recover the ownership order of installations created before overlapping
//! manifests were rejected. Renames record the actual file object even when
//! two successive versions deployed identical bytes.
use super::*;
use crate::install::{checked_manifest, path_key, InstallFileInspection, InstalledMod};

#[derive(PartialEq, Eq)]
struct Payload {
    object: FileIdentity,
    content: Fingerprint,
}

struct CopyLayer {
    published: Payload,
    overwritten: Option<Payload>,
}

fn incomplete() -> AppError {
    AppError::Conflict("安装清单、复制计划与事务日志无法对应".into())
}

impl JournalEntry {
    fn copy_layer(&self, dst: &Path, source: &Fingerprint) -> AppResult<CopyLayer> {
        let renames = self.renames.as_deref().ok_or_else(incomplete)?;
        // Implicit parent directories precede the backup and publish renames.
        let mut writes = renames
            .iter()
            .skip_while(|r| r.empty_dir_only && r.retain_at.is_none());
        let overwritten = if let Some(before) = &self.entry.before {
            let backup = writes.next().ok_or_else(incomplete)?;
            if backup.from != dst
                || backup.retain_at.is_some()
                || backup.empty_dir_only
                || (backup.to != self.backup && Some(&backup.to) != self.overwrite_park.as_ref())
                || backup.expected_content.as_ref() != Some(before)
            {
                return Err(incomplete());
            }
            Some(Payload {
                object: backup.identity.clone(),
                content: before.clone(),
            })
        } else {
            None
        };
        let publish = writes.next().ok_or_else(incomplete)?;
        if publish.to != dst
            || publish.retain_at.as_ref() != Some(&self.undo)
            || publish.empty_dir_only
            || writes.next().is_some()
        {
            return Err(incomplete());
        }
        Ok(CopyLayer {
            published: Payload {
                object: publish.identity.clone(),
                content: source.clone(),
            },
            overwritten,
        })
    }
}

impl FileEngine {
    fn install_copy_layers(
        &self,
        install: &InstalledMod,
        current: bool,
    ) -> AppResult<(String, BTreeMap<String, CopyLayer>)> {
        let (id, plan, journal) = {
            let conn = self.db.conn();
            let (id, plan, journal): (String, String, String) = conn
                .query_row(
                    "SELECT f.id,f.plan_json,f.journal_json FROM install_transactions a
                 JOIN file_transactions f ON f.id=a.transaction_id
                 WHERE a.install_id=?1 AND a.action='install' GROUP BY a.install_id
                 HAVING count(*)=1 AND f.game_id=?2 AND f.status='committed'",
                    params![install.id, install.game_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or_else(incomplete)?;
            let journal = Self::decode_journal(&conn, &id, &journal)?;
            (id, serde_json::from_str::<FilePlan>(&plan)?, journal)
        };
        if plan.id != id
            || plan.entries.is_empty()
            || journal.len() != plan.entries.len()
            || journal.len() != install.files.len()
        {
            return Err(incomplete());
        }
        let mut manifest = checked_manifest(self, install)?;
        let mut layers = BTreeMap::new();
        for (entry, step) in plan.entries.iter().zip(&journal) {
            let FileOp::Copy { dst, .. } = &entry.operation else {
                return Err(incomplete());
            };
            let file = manifest.remove(dst.as_path()).ok_or_else(incomplete)?;
            let source = entry.source.as_ref().ok_or_else(incomplete)?;
            if step.state != "done"
                || step.entry != *entry
                || source.kind != "file"
                || source.sha256 != file.sha256
                || source.size != file.size
                || entry.before != file.before
            {
                return Err(incomplete());
            }
            layers.insert(path_key(dst), step.copy_layer(dst, source)?);
        }
        if current {
            self.validate_install_rollback_payloads(&journal)?;
            self.validate_committed_rollback(&journal)?;
        }
        Ok((id, layers))
    }

    pub(crate) fn shared_install_rollback(
        &self,
        install: &InstalledMod,
        files: &[InstallFileInspection],
        owners: &[&InstalledMod],
    ) -> AppResult<String> {
        if let Some(file) = files.iter().find(|f| f.state != "owned") {
            return Err(AppError::Conflict(format!(
                "当前文件与此安装的输出不符：{}",
                paths::display_path(&file.path)
            )));
        }
        let (transaction, target) = self.install_copy_layers(install, true)?;
        let mut others = BTreeMap::new();
        for owner in owners {
            if owner.game_id != install.game_id || owner.status != "enabled" {
                return Err(AppError::Conflict(
                    "共享路径涉及其他游戏或未完整部署的安装".into(),
                ));
            }
            others.insert(&owner.id, self.install_copy_layers(owner, false)?.1);
        }
        for file in files.iter().filter(|f| !f.other_install_ids.is_empty()) {
            let key = path_key(&file.path);
            let mut layer = target.get(&key).ok_or_else(incomplete)?;
            let mut remaining = file
                .other_install_ids
                .iter()
                .map(|id| {
                    others
                        .get(id)
                        .and_then(|layers| layers.get(&key))
                        .ok_or_else(incomplete)
                })
                .collect::<AppResult<Vec<_>>>()?;
            while !remaining.is_empty() {
                // Walk from the current physical object down through every owner.
                let mut previous = remaining
                    .iter()
                    .enumerate()
                    .filter(|(_, old)| Some(&old.published) == layer.overwritten.as_ref());
                let index = previous
                    .next()
                    .map(|(i, _)| i)
                    .filter(|_| previous.next().is_none());
                let index = index.ok_or_else(|| {
                    AppError::Conflict(format!(
                        "无法核实共享文件的覆盖顺序：{}",
                        paths::display_path(&file.path)
                    ))
                })?;
                layer = remaining.swap_remove(index);
            }
        }
        Ok(transaction)
    }
}
