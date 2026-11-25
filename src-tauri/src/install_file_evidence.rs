//! Bounded observations of current files from an exact installation manifest.
use crate::{
    error::{AppError, AppResult},
    game_roots,
    games::Game,
    install::{InstalledFile, INSTALL_STATE},
    safe_fs,
    transaction::FileEngine,
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    io::Read,
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const MAX_FILES: usize = 256;
const MAX_HASH_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 512 * 1024;
const MAX_ISSUES: usize = 4;
const SOFT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEvidence {
    status: EvidenceStatus,
    matched: usize,
    modified: usize,
    missing: usize,
    unverified: usize,
    limit_reached: bool,
    issues: Vec<FileIssue>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedFileEvidence {
    game_id: String,
    install_id: String,
    evidence: FileEvidence,
}

pub(crate) fn verify_managed_install(
    engine: &FileEngine,
    game_id: &str,
    install_id: &str,
    cancel: &CancellationToken,
) -> AppResult<ManagedFileEvidence> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    for id in [game_id, install_id] {
        if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
            return Err(AppError::Invalid("游戏或安装记录 ID 无效".into()));
        }
    }
    let _guard = crate::transaction::write_lock();
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let game = crate::games::get_game(&engine.db, game_id)?;
    {
        let conn = engine.db.conn();
        let pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))",
            [], |row| row.get(0),
        )?;
        if pending {
            return Err(AppError::Conflict(
                "有待处理的文件事务，暂不能核验安装文件".into(),
            ));
        }
        let status: Option<String> = conn.query_row(
            &format!("SELECT {INSTALL_STATE} FROM installs WHERE id=?1 AND game_id=?2
              AND EXISTS (SELECT 1 FROM install_transactions a WHERE a.install_id=installs.id AND a.action='install')"),
            params![install_id, game_id], |row| row.get(0),
        ).optional()?;
        match status.as_deref() {
            Some("enabled") => (),
            Some(_) => return Err(AppError::Conflict("安装记录不是完整部署状态".into())),
            None => return Err(AppError::NotFound("目标游戏没有此安装事务记录".into())),
        }
    }
    let mut pass = FileEvidencePass::new(engine, &game)?.with_cancel(cancel);
    let evidence = pass.verify(install_id)?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    Ok(ManagedFileEvidence {
        game_id: game_id.into(),
        install_id: install_id.into(),
        evidence,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum EvidenceStatus {
    Matched,
    Mismatch,
    Unverified,
}

#[derive(Debug, Serialize)]
struct FileIssue {
    path: String,
    state: FileState,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum FileState {
    Matched,
    Modified,
    Missing,
    Unverified,
}

#[derive(Default)]
struct ManifestEvidence {
    files: Option<(PathBuf, Vec<InstalledFile>)>,
    limit_reached: bool,
}

impl FileEvidence {
    fn new(limit_reached: bool) -> Self {
        Self {
            status: EvidenceStatus::Unverified,
            matched: 0,
            modified: 0,
            missing: 0,
            unverified: 0,
            limit_reached,
            issues: Vec::new(),
        }
    }

    fn observe(&mut self, state: FileState, path: Option<&str>) {
        match state {
            FileState::Matched => self.matched += 1,
            FileState::Modified => self.modified += 1,
            FileState::Missing => self.missing += 1,
            FileState::Unverified => self.unverified += 1,
        }
        if !matches!(state, FileState::Matched) && self.issues.len() < MAX_ISSUES {
            if let Some(path) = path.filter(|p| p.len() <= 512 && !p.chars().any(char::is_control))
            {
                self.issues.push(FileIssue {
                    path: path.into(),
                    state,
                });
            }
        }
    }
}

pub(crate) struct FileEvidencePass<'a> {
    engine: &'a FileEngine,
    game_id: &'a str,
    roots: Vec<PathBuf>,
    files_left: usize,
    bytes_left: u64,
    deadline: Instant,
    cancel: Option<&'a CancellationToken>,
}

impl<'a> FileEvidencePass<'a> {
    pub(crate) fn new(engine: &'a FileEngine, game: &'a Game) -> AppResult<Self> {
        let mut roots = vec![PathBuf::from(&game.root_path)];
        roots.extend(game_roots::registered_user_roots(&engine.db, &game.id)?.into_values());
        Ok(Self {
            engine,
            game_id: &game.id,
            roots,
            files_left: MAX_FILES,
            bytes_left: MAX_HASH_BYTES,
            deadline: Instant::now() + SOFT_TIMEOUT,
            cancel: None,
        })
    }

    pub(crate) fn with_cancel(mut self, cancel: &'a CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    fn manifest(&self, id: &str) -> AppResult<ManifestEvidence> {
        let row = self.engine.db.conn().query_row(
            "SELECT install_root, CASE WHEN length(CAST(files_json AS BLOB)) <= ?3 THEN files_json END, coalesce(json_extract(metadata_json,'$.gtaRpf'),0)
             FROM installs WHERE id=?1 AND game_id=?2",
            params![id, self.game_id, MAX_MANIFEST_BYTES],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, bool>(2)?)),
        ).optional()?;
        let Some((Some(root), json, gta_rpf)) = row else {
            return Ok(ManifestEvidence::default());
        };
        if gta_rpf {
            let files = crate::install::gta_rpf::active_files(self.engine, self.game_id, id)?;
            return Ok(ManifestEvidence {
                files: (!files.is_empty()).then(|| (PathBuf::from(root), files)),
                limit_reached: false,
            });
        }
        let limit_reached = json.is_none();
        let files = json
            .and_then(|json| serde_json::from_str::<Vec<InstalledFile>>(&json).ok())
            .filter(|files| !files.is_empty())
            .map(|files| (PathBuf::from(root), files));
        Ok(ManifestEvidence {
            files,
            limit_reached,
        })
    }

    pub(crate) fn verify(&mut self, id: &str) -> AppResult<FileEvidence> {
        let ManifestEvidence {
            files,
            limit_reached,
        } = self.manifest(id)?;
        let mut evidence = FileEvidence::new(limit_reached);
        let Some((root, files)) = files else {
            return Ok(evidence);
        };
        let Some(registered) = self.roots.iter().find(|r| root.starts_with(r)).cloned() else {
            evidence.unverified = files.len();
            return Ok(evidence);
        };
        let mut unique = HashSet::new();
        for (index, file) in files.iter().enumerate() {
            if self.cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(AppError::Cancelled);
            }
            if self.files_left == 0 || Instant::now() >= self.deadline {
                evidence.limit_reached = true;
                evidence.unverified += files.len() - index;
                break;
            }
            self.files_left -= 1;
            let relative = manifest_relative(&file.path, &root, &registered);
            let state = if relative.is_some() && unique.insert(&file.path) {
                self.observe_file(file, &registered, &mut evidence.limit_reached)
            } else {
                FileState::Unverified
            };
            evidence.observe(state, relative.as_deref());
        }
        evidence.status = if evidence.modified != 0 || evidence.missing != 0 {
            EvidenceStatus::Mismatch
        } else if evidence.unverified == 0
            && !evidence.limit_reached
            && evidence.matched == files.len()
        {
            EvidenceStatus::Matched
        } else {
            EvidenceStatus::Unverified
        };
        Ok(evidence)
    }

    fn observe_file(
        &mut self,
        expected: &InstalledFile,
        root: &PathBuf,
        limited: &mut bool,
    ) -> FileState {
        if expected.sha256.len() != 64 || !expected.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return FileState::Unverified;
        }
        let path = &expected.path;
        let paths = &self.engine.paths;
        if path.starts_with(&paths.root)
            || paths.root.starts_with(root)
            || !matches!(paths.guard(path, std::slice::from_ref(root)), Ok(guarded) if guarded == *path)
        {
            return FileState::Unverified;
        }
        let file = match safe_fs::open_regular(path) {
            Ok(file) => file,
            Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return FileState::Missing
            }
            Err(_) => return FileState::Unverified,
        };
        self.digest_regular(file, expected, limited)
            .unwrap_or(FileState::Unverified)
    }

    fn digest_regular(
        &mut self,
        mut file: std::fs::File,
        expected: &InstalledFile,
        limited: &mut bool,
    ) -> AppResult<FileState> {
        safe_fs::verify_regular_at(&file, &expected.path)?;
        let before = file.metadata()?;
        if before.len() != expected.size {
            return Ok(FileState::Modified);
        }
        if expected.size > self.bytes_left {
            *limited = true;
            return Ok(FileState::Unverified);
        }
        self.bytes_left -= expected.size;
        let mut remaining = expected.size;
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        while remaining != 0 {
            if self.cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(AppError::Cancelled);
            }
            if Instant::now() >= self.deadline {
                *limited = true;
                return Ok(FileState::Unverified);
            }
            let limit = remaining.min(buffer.len() as u64) as usize;
            let count = file.read(&mut buffer[..limit])?;
            if count == 0 {
                return Ok(FileState::Unverified);
            }
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let after = file.metadata()?;
        safe_fs::verify_regular_at(&file, &expected.path)?;
        if after.len() != before.len() || after.modified()? != before.modified()? {
            return Ok(FileState::Unverified);
        }
        Ok(
            if hex::encode(digest.finalize()).eq_ignore_ascii_case(&expected.sha256) {
                FileState::Matched
            } else {
                FileState::Modified
            },
        )
    }
}

fn manifest_relative(path: &Path, install_root: &Path, registered: &Path) -> Option<String> {
    if !path.is_absolute() || path == install_root || !path.starts_with(install_root) {
        return None;
    }
    let parts: Vec<_> = path
        .strip_prefix(registered)
        .ok()?
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str().filter(|name| {
                let lower = name.to_ascii_lowercase();
                lower != ".env" && !lower.starts_with(".env.") && lower != ".amm-retained"
            }),
            _ => None,
        })
        .collect::<Option<_>>()?;
    let relative = parts.join("/");
    crate::plugins::validate_relative_path(&relative, false).ok()?;
    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_fixture(engine: &FileEngine, game: &Game, files: &str) {
        engine.db.conn().execute(
            "INSERT INTO installs(id,game_id,name,install_root,files_json,installed_at,updated_at)
             VALUES ('fixture',?1,'Fixture',?2,?3,'now','now')",
            params![game.id, game.root_path, files],
        ).unwrap();
    }

    #[test]
    fn file_evidence_shares_work_limits_without_claiming_skipped_files_match() {
        let (_temp, engine, root) = crate::transaction::tests::setup();
        let game = crate::games::register_game(&engine.db, &root, None).unwrap();
        let file = root.join("matches.jar");
        std::fs::write(&file, b"bytes").unwrap();
        let manifest = serde_json::to_string(&[
            InstalledFile {
                path: file,
                size: 5,
                sha256: hex::encode(Sha256::digest(b"bytes")),
                before: None,
            },
            InstalledFile {
                path: root.join("not-checked.jar"),
                size: 5,
                sha256: "0".repeat(64),
                before: None,
            },
        ])
        .unwrap();
        manifest_fixture(&engine, &game, &manifest);

        let mut pass = FileEvidencePass::new(&engine, &game).unwrap();
        pass.files_left = 1;
        let evidence = pass.verify("fixture").unwrap();
        assert!(matches!(evidence.status, EvidenceStatus::Unverified));
        assert!(evidence.limit_reached);
        assert_eq!(
            (evidence.matched, evidence.missing, evidence.unverified),
            (1, 0, 1)
        );

        let mut pass = FileEvidencePass::new(&engine, &game).unwrap();
        pass.bytes_left = 4;
        let evidence = pass.verify("fixture").unwrap();
        assert!(matches!(evidence.status, EvidenceStatus::Mismatch));
        assert!(evidence.limit_reached);
        assert_eq!(
            (evidence.matched, evidence.missing, evidence.unverified),
            (0, 1, 1)
        );
        assert_eq!(pass.bytes_left, 4);
    }

    #[test]
    fn file_evidence_omits_oversized_manifests() {
        let (_temp, engine, root) = crate::transaction::tests::setup();
        let game = crate::games::register_game(&engine.db, &root, None).unwrap();
        manifest_fixture(&engine, &game, &" ".repeat(MAX_MANIFEST_BYTES + 1));
        let mut pass = FileEvidencePass::new(&engine, &game).unwrap();
        let evidence = pass.verify("fixture").unwrap();
        assert!(matches!(evidence.status, EvidenceStatus::Unverified));
        assert!(evidence.limit_reached);
        assert_eq!(pass.files_left, MAX_FILES);
    }
}
