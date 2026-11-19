use crate::error::{AppError, AppResult};
use std::path::{Path, PathBuf};

/// Root of every path the manager owns. Nothing outside this (or a registered
/// game root) may ever be written by the file engine.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
}

impl AppPaths {
    pub fn new(root: PathBuf) -> AppResult<Self> {
        let root = crate::paths::lexical_normalize(&root);
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let p = AppPaths { root };
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

    /// Roots that the file engine is allowed to write into. Game roots are
    /// appended by the caller from the `games` table.
    pub fn managed_roots(&self, game_roots: &[PathBuf]) -> Vec<PathBuf> {
        let mut roots = vec![self.root.clone()];
        roots.extend(game_roots.iter().cloned());
        roots
    }

    /// Resolve a user/agent supplied path and assert it lands in a managed root.
    pub fn guard(&self, target: &Path, game_roots: &[PathBuf]) -> AppResult<PathBuf> {
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
        let resolved = crate::paths::ensure_within(target, &roots)?;
        if resolved.starts_with(self.root.join("private"))
            || resolved
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == ".env" || n.starts_with(".env."))
        {
            return Err(AppError::Denied(
                "凭据目录和 .env 不可通过文件或 Agent 工具访问".into(),
            ));
        }
        Ok(resolved)
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
