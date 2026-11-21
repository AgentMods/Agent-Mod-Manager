use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};

/// Manager-owned paths and the protected locations shared by filesystem callers.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    control_root: Option<PathBuf>,
}

impl AppPaths {
    pub fn new(root: PathBuf) -> AppResult<Self> {
        let root = crate::paths::lexical_normalize(&root);
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let p = AppPaths {
            root,
            control_root: None,
        };
        for dir in [
            p.downloads(),
            p.staging(),
            p.backups(),
            p.trash(),
            p.plugins(),
            p.skills(),
            p.logs(),
        ] {
            crate::safe_fs::create_dir_all(&dir)?;
        }
        Ok(p)
    }

    pub(crate) fn with_control_root(mut self, root: &Path) -> Self {
        if root != self.root {
            self.control_root = Some(root.into());
        }
        self
    }

    pub(crate) fn overlaps_storage(&self, path: &Path) -> bool {
        std::iter::once(&self.root)
            .chain(self.control_root.iter())
            .any(|root| crate::paths::overlaps(path, root))
    }

    pub fn db_file(&self) -> PathBuf {
        self.root.join("manager.db")
    }
    pub fn downloads(&self) -> PathBuf {
        self.root.join("downloads")
    }
    pub fn staging(&self) -> PathBuf {
        self.root.join("staging")
    }
    pub fn backups(&self) -> PathBuf {
        self.root.join("backups")
    }
    pub fn trash(&self) -> PathBuf {
        self.root.join("trash")
    }
    pub fn plugins(&self) -> PathBuf {
        self.root.join("plugins")
    }
    pub fn skills(&self) -> PathBuf {
        self.root.join("skills")
    }
    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn backup_dir(&self, txn: &str) -> PathBuf {
        self.backups().join(txn)
    }
    pub fn trash_entry(&self, id: &str) -> PathBuf {
        self.trash().join(id)
    }
    pub fn snapshot_dir(&self, game: &str, id: &str) -> PathBuf {
        self.backups().join("snapshots").join(game).join(id)
    }

    /// Managed roots for scoped callers and for classifying Agent transactions.
    pub fn managed_roots(&self, game_roots: &[PathBuf]) -> Vec<PathBuf> {
        let mut roots = vec![self.root.clone()];
        roots.extend(game_roots.iter().cloned());
        roots
    }

    /// The default path policy, including for inbound MCP, remains managed-only.
    pub fn guard(&self, target: &Path, game_roots: &[PathBuf]) -> AppResult<PathBuf> {
        self.guard_file_path(target, game_roots, false)
    }

    pub(crate) fn guard_file_path(
        &self,
        target: &Path,
        game_roots: &[PathBuf],
        local_agent: bool,
    ) -> AppResult<PathBuf> {
        self.stable_directory(&self.root)?;
        let mut roots = vec![self.root.clone()];
        for root in game_roots {
            if root.canonicalize().ok().as_ref() == Some(root) && root.is_dir() {
                roots.push(root.clone());
            } else if target.starts_with(root) {
                return Err(AppError::Denied(format!(
                    "已登记管理根缺失或已被链接替换: {}",
                    root.display()
                )));
            }
        }
        let resolved = if local_agent {
            crate::paths::canonicalize_lenient(target)?
        } else {
            crate::paths::ensure_within(target, &roots)?
        };
        if self
            .control_root
            .as_ref()
            .is_some_and(|root| crate::paths::contains_path(root, &resolved))
            || crate::paths::contains_path(&self.root.join("private"), &resolved)
            || resolved.components().any(|c| Self::env_name(c.as_os_str()))
        {
            return Err(AppError::Denied(
                "凭据目录和 .env 不可通过文件或 Agent 工具访问".into(),
            ));
        }
        Ok(resolved)
    }

    pub(crate) fn env_name(name: &std::ffi::OsStr) -> bool {
        name.to_str().is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            name == ".env" || name.starts_with(".env.")
        })
    }

    pub fn stable_directory(&self, directory: &Path) -> AppResult<()> {
        let meta = std::fs::symlink_metadata(directory)?;
        if !meta.file_type().is_dir() || directory.canonicalize()? != directory {
            return Err(AppError::Denied(format!(
                "管理器目录已被链接或其他文件替换: {}",
                directory.display()
            )));
        }
        Ok(())
    }

    pub fn require_game_root(&self, target: &Path, game_roots: &[PathBuf]) -> AppResult<PathBuf> {
        let canon = crate::paths::canonicalize_lenient(target)?;
        for root in game_roots {
            let root_canon = crate::paths::canonicalize_lenient(root)?;
            if crate::paths::is_within(&canon, &root_canon) {
                return Ok(canon);
            }
        }
        Err(AppError::PathEscape(format!(
            "{} 不属于任何已登记的游戏目录，只能先加入游戏库",
            canon.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_layout() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().join("data")).unwrap();
        assert!(paths.downloads().is_dir());
        assert!(paths.trash().is_dir());
        assert!(paths.backups().is_dir());
    }

    #[test]
    fn guard_blocks_outside_paths() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().join("data")).unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        let game = game.canonicalize().unwrap();
        assert!(paths
            .guard(&game.join("mods"), std::slice::from_ref(&game))
            .is_ok());
        assert!(paths
            .guard(&dir.path().join("elsewhere"), std::slice::from_ref(&game))
            .is_err());
    }
}
