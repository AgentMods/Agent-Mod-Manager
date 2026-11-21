//! Opt-in public search evidence for unknown games. No game bytes are read.
use crate::{
    error::{AppError, AppResult},
    games::{self, Game, GameCandidate},
    settings::Settings,
    sources::web_search::{self, WebSearchQuery, WebSearchResult, WebSearchStatus},
    transaction::FileEngine,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use tokio_util::sync::CancellationToken;

const MAX_ENTRIES: usize = 512;
const MAX_NAME_BYTES: usize = 512;
const MAX_PATH_BYTES: usize = 4096;
const MAX_EVIDENCE_BYTES: usize = 32 * 1024;
const TTL_SECONDS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameIdentityPreview {
    pub root_path: String,
    pub fingerprint: String,
    pub local_candidate: GameCandidate,
    pub signals: Vec<String>,
    pub suggested_query: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameIdentityStatus {
    Available,
    NeedsConfiguration,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameIdentityCandidate {
    pub name: String,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameIdentitySearch {
    pub id: String,
    pub root_path: String,
    pub fingerprint: String,
    pub query: String,
    pub status: GameIdentityStatus,
    pub message: String,
    pub candidates: Vec<GameIdentityCandidate>,
    pub created_at: String,
}

impl GameIdentitySearch {
    pub(crate) fn into_user_paths(mut self) -> Self {
        crate::paths::set_user_path_text(&mut self.root_path);
        self
    }
}

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}

// Exceptions are exact known game roots, not grants to the containing data tree.
fn within(root: &Path, container: &Path) -> bool {
    if !cfg!(windows) {
        return root.starts_with(container);
    }
    let mut parts = root.components();
    container.components().all(|expected| {
        parts.next().is_some_and(|actual| {
            actual
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.as_os_str().to_string_lossy())
        })
    })
}

fn known_user_game(root: &Path, container: &Path, mac_library: bool) -> bool {
    if !within(root, container) {
        return false;
    }
    let Some(parts) = root
        .components()
        .skip(container.components().count())
        .map(|p| p.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let matches = |expected: &[&str]| {
        parts.iter().zip(expected).all(|(a, b)| {
            if cfg!(windows) {
                a.eq_ignore_ascii_case(b)
            } else {
                a == b
            }
        })
    };
    if mac_library {
        (parts.len() == 5 && matches(&["Application Support", "Steam", "steamapps", "common"]))
            || (parts.len() == 2 && matches(&["Application Support", "minecraft"]))
    } else {
        (parts.len() == 1 && matches(&[".minecraft"]))
            || (parts.len() == 4 && matches(&["Steam", "steamapps", "common"]))
    }
}

fn sensitive_user_path(root: &Path, home: &Path) -> bool {
    (within(root, home) && within(home, root))
        || [
            ".ssh",
            ".gnupg",
            ".aws",
            ".config",
            ".local/share/keyrings",
            "Library",
        ]
        .iter()
        .any(|name| {
            let sensitive = home.join(name);
            within(root, &sensitive)
                && !(*name == "Library" && known_user_game(root, &sensitive, true))
        })
}

pub fn migrate(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS game_identity_searches (
            id TEXT PRIMARY KEY,
            root_path TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            query TEXT NOT NULL,
            evidence_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            invalidated INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_game_identity_cache
        ON game_identity_searches(root_path,fingerprint,query,created_at);",
    )?;
    Ok(())
}

fn safe_root(engine: &FileEngine, directory: &Path) -> AppResult<PathBuf> {
    if !directory.is_absolute() || directory.as_os_str().len() > MAX_PATH_BYTES {
        return Err(invalid("Select a bounded absolute game directory"));
    }
    // Do not accept a root which has itself been replaced by a symlink.
    if !fs::symlink_metadata(directory)?.file_type().is_dir() {
        return Err(invalid("Game root must be a real directory, not a link"));
    }
    let root = directory.canonicalize()?;
    let text = root.to_str().ok_or_else(|| invalid("Root must be UTF-8"))?;
    if text.len() > MAX_PATH_BYTES || root.parent().is_none() {
        return Err(invalid("Filesystem roots are not game directories"));
    }
    engine.paths.stable_directory(&engine.paths.root)?;
    if engine.paths.overlaps_storage(&root) {
        return Err(invalid("Game directory overlaps manager data"));
    }
    // Descendants of these locations must never become identity scan roots.
    for sensitive in [
        "/System",
        "/Library",
        "/etc",
        "/private/etc",
        "/proc",
        "/sys",
        "/dev",
        "/bin",
        "/sbin",
        "/usr",
        "/boot",
        "/root",
        "/run",
        "/var/lib",
        "/var/log",
        "/var/run",
        "/private/var/db",
        "/private/var/log",
        "/private/var/run",
    ] {
        let p = Path::new(sensitive);
        if root.starts_with(p) {
            return Err(invalid("Sensitive system directory"));
        }
    }
    if root.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(".ssh" | ".gnupg" | ".aws" | ".azure" | ".kube")
        )
    }) {
        return Err(invalid("Sensitive credential directory"));
    }
    // Also reject broad containers and user credential/configuration locations.
    for container in [
        "/Users",
        "/home",
        "/Volumes",
        "/Applications",
        "/private",
        "/var",
        "/tmp",
        "/private/tmp",
        "/private/var",
        "/private/var/folders",
    ] {
        if root == Path::new(container) {
            return Err(invalid("Select a specific game directory"));
        }
    }
    if let Some(home) = dirs::home_dir().and_then(|p| p.canonicalize().ok()) {
        if sensitive_user_path(&root, &home) {
            return Err(invalid("Sensitive user directory"));
        }
    }
    // Windows paths are checked case-insensitively, including redirected system locations.
    if cfg!(windows) {
        let lower = text.to_lowercase();
        if lower.split(['/', '\\']).any(|part| {
            matches!(
                part,
                "windows"
                    | "system32"
                    | "programdata"
                    | "$recycle.bin"
                    | "system volume information"
            )
        }) {
            return Err(invalid("Sensitive system directory"));
        }
        for var in [
            "WINDIR",
            "SYSTEMROOT",
            "PROGRAMDATA",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            if let Some(p) =
                std::env::var_os(var).and_then(|p| PathBuf::from(p).canonicalize().ok())
            {
                let game_exception =
                    matches!(var, "APPDATA" | "LOCALAPPDATA") && known_user_game(&root, &p, false);
                if within(&root, &p) && !game_exception {
                    return Err(invalid("Sensitive system directory"));
                }
            }
        }
    }
    Ok(root)
}

fn metadata_stamp(meta: &fs::Metadata) -> AppResult<String> {
    let modified = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("Unsupported directory timestamp"))?
        .as_nanos();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(format!(
            "{}:{modified}:{}:{}:{}:{}:{}",
            meta.len(),
            meta.dev(),
            meta.ino(),
            meta.ctime(),
            meta.ctime_nsec(),
            meta.mode()
        ))
    }
    #[cfg(not(unix))]
    {
        Ok(format!(
            "{}:{modified}:{:?}:{}",
            meta.len(),
            meta.created().ok(),
            meta.permissions().readonly()
        ))
    }
}

fn field(hash: &mut Sha256, value: &str) {
    hash.update((value.len() as u64).to_le_bytes());
    hash.update(value.as_bytes());
}

struct Snapshot {
    root: PathBuf,
    fingerprint: String,
    signals: Vec<String>,
}

fn snapshot(engine: &FileEngine, directory: &Path) -> AppResult<Snapshot> {
    let root = safe_root(engine, directory)?;
    let before = metadata_stamp(&fs::symlink_metadata(&root)?)?;
    let mut entries = Vec::new();
    for (index, entry) in fs::read_dir(&root)?.enumerate() {
        if index >= MAX_ENTRIES {
            return Err(invalid("Directory entry limit exceeded"));
        }
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if !meta.is_file() && !meta.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("Entry name must be UTF-8"))?;
        if name.len() > MAX_NAME_BYTES || name.chars().any(unsafe_character) {
            return Err(invalid("Directory entry name exceeds safe limits"));
        }
        entries.push((
            name,
            if meta.is_dir() { "directory" } else { "file" },
            metadata_stamp(&meta)?,
        ));
    }
    entries.sort();
    let mut hash = Sha256::new();
    field(&mut hash, "game-identity-v1");
    field(
        &mut hash,
        root.to_str().ok_or_else(|| invalid("Root must be UTF-8"))?,
    );
    field(&mut hash, &before);
    for (name, kind, stamp) in &entries {
        field(&mut hash, name);
        field(&mut hash, kind);
        field(&mut hash, stamp);
    }
    if before != metadata_stamp(&fs::symlink_metadata(&root)?)?
        || safe_root(engine, directory)? != root
    {
        return Err(invalid("Directory changed during identity scan"));
    }
    let signals = entries
        .into_iter()
        .take(32)
        .map(|(name, kind, _)| format!("{kind}: {name}"))
        .collect();
    Ok(Snapshot {
        root,
        fingerprint: hex::encode(hash.finalize()),
        signals,
    })
}

fn unsafe_character(ch: char) -> bool {
    ch.is_control()
        || matches!(ch, '\u{00ad}' | '\u{034f}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}

fn public_query(query: &str) -> AppResult<WebSearchQuery> {
    let query = WebSearchQuery {
        query: query.into(),
        count: 5,
    };
    query.validate()?;
    Ok(query)
}

pub fn preview(engine: &FileEngine, directory: &Path) -> AppResult<GameIdentityPreview> {
    let scan = snapshot(engine, directory)?;
    let candidate = games::identify_game(&engine.db, &scan.root)?;
    if snapshot(engine, directory)?.fingerprint != scan.fingerprint {
        return Err(invalid("Directory changed"));
    }
    let suggestion = format!("{} game", candidate.name);
    let suggested_query = if public_query(&suggestion).is_ok() {
        suggestion
    } else {
        String::new()
    };
    Ok(GameIdentityPreview {
        root_path: crate::paths::display_path(&scan.root), fingerprint: scan.fingerprint,
        local_candidate: candidate, signals: scan.signals, suggested_query,
        warnings: vec!["仅将你确认的公开搜索词发送给 Brave；本地名称信号留在本机。网页候选未经验证，不会自动关联适配包。".into()],
    })
}

fn check_cancel(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

/// The only network request receives the reviewed query, never the snapshot.
pub async fn search(
    engine: &FileEngine,
    settings: &Settings,
    directory: &Path,
    expected_fingerprint: &str,
    public_query: &str,
    confirmation: &str,
    cancel: &CancellationToken,
) -> AppResult<GameIdentitySearch> {
    search_with(
        engine,
        directory,
        expected_fingerprint,
        public_query,
        confirmation,
        cancel,
        || {
            // Deferred until consent, cancellation, query and fingerprint validation.
            // Missing credentials must report their real state even with a cache.
            match settings.read_secret(web_search::SECRET_REF) {
                Ok(Some(key)) if !key.trim().is_empty() && key.len() <= 4096 => {
                    web_search::validate_query_secret(public_query, &key)?;
                    Ok(true)
                }
                _ => Ok(false),
            }
        },
        |query| async move { web_search::search(settings, &query, cancel).await },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn search_with<F, Fut>(
    engine: &FileEngine,
    directory: &Path,
    expected_fingerprint: &str,
    query: &str,
    confirmation: &str,
    cancel: &CancellationToken,
    use_cache: impl FnOnce() -> AppResult<bool>,
    provider: F,
) -> AppResult<GameIdentitySearch>
where
    F: FnOnce(WebSearchQuery) -> Fut,
    Fut: Future<Output = AppResult<WebSearchResult>>,
{
    check_cancel(cancel)?;
    if confirmation != "SEARCH_GAME_IDENTITY" {
        return Err(invalid("SEARCH_GAME_IDENTITY confirmation required"));
    }
    let query = public_query(query)?;
    let scan = snapshot(engine, directory)?;
    if scan.fingerprint != expected_fingerprint {
        return Err(invalid("目录预览已失效，请重新检查目录"));
    }
    let root = scan
        .root
        .to_str()
        .ok_or_else(|| invalid("Root must be UTF-8"))?;
    check_cancel(cancel)?;
    if use_cache()? {
        let conn = engine.db.conn();
        let id: Option<String> = conn.query_row(
            "SELECT id FROM game_identity_searches WHERE root_path=?1 AND fingerprint=?2 AND query=?3 AND invalidated=0 ORDER BY created_at DESC LIMIT 1",
            params![root,expected_fingerprint,query.query], |row| row.get(0)).optional()?;
        if let Some(id) = id {
            if let Ok(cached) = load_evidence(&conn, &id) {
                if cached.status == GameIdentityStatus::Available && !cached.candidates.is_empty() {
                    drop(conn);
                    check_cancel(cancel)?;
                    if snapshot(engine, directory)?.fingerprint != scan.fingerprint {
                        return Err(invalid("Directory changed"));
                    }
                    return Ok(cached);
                }
            }
        }
    }
    check_cancel(cancel)?;
    if snapshot(engine, directory)?.fingerprint != scan.fingerprint {
        return Err(invalid("Directory changed before search"));
    }
    let result = provider(query.clone()).await?;
    check_cancel(cancel)?;
    if snapshot(engine, directory)?.fingerprint != scan.fingerprint {
        return Err(invalid("搜索期间目录内容已变化，请重新检查并搜索"));
    }
    let status = match result.status {
        WebSearchStatus::Available => GameIdentityStatus::Available,
        WebSearchStatus::NeedsConfiguration => GameIdentityStatus::NeedsConfiguration,
        WebSearchStatus::Unavailable => GameIdentityStatus::Unavailable,
    };
    let candidates = if status == GameIdentityStatus::Available {
        result
            .results
            .into_iter()
            .take(5)
            .map(|hit| GameIdentityCandidate {
                name: hit.title.chars().take(128).collect(),
                title: hit.title,
                url: hit.url,
                snippet: hit.snippet,
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut evidence = GameIdentitySearch {
        id: format!("identity_{}", uuid::Uuid::new_v4().simple()),
        root_path: root.into(),
        fingerprint: scan.fingerprint,
        query: query.query,
        status,
        message: result.message.into(),
        candidates,
        created_at: Utc::now().to_rfc3339(),
    };
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(&evidence)?));
    evidence.id = format!("{}_{digest}", evidence.id);
    let _guard = crate::transaction::write_lock();
    check_cancel(cancel)?;
    if snapshot(engine, directory)?.fingerprint != evidence.fingerprint {
        return Err(invalid("Directory changed before evidence storage"));
    }
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE game_identity_searches SET invalidated=1 WHERE root_path=?1",
        [root],
    )?;
    tx.execute("INSERT INTO game_identity_searches(id,root_path,fingerprint,query,evidence_json,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
        params![evidence.id,root,evidence.fingerprint,evidence.query,serde_json::to_string(&evidence)?,evidence.created_at])?;
    check_cancel(cancel)?;
    tx.commit()?;
    Ok(evidence)
}

fn load_evidence(conn: &Connection, id: &str) -> AppResult<GameIdentitySearch> {
    if id.len() > 128 {
        return Err(invalid("Invalid search ID"));
    }
    let row: Option<(String,String,String,String,String)> = conn.query_row(
        "SELECT root_path,fingerprint,query,evidence_json,created_at FROM game_identity_searches WHERE id=?1 AND invalidated=0",
        [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let (root, fingerprint, query, encoded, created) =
        row.ok_or_else(|| invalid("搜索证据不存在或已被替换，请重新搜索"))?;
    if encoded.len() > MAX_EVIDENCE_BYTES {
        return Err(invalid("Oversized evidence"));
    }
    let mut evidence: GameIdentitySearch = serde_json::from_str(&encoded)?;
    if evidence.id != id
        || evidence.root_path != root
        || evidence.fingerprint != fingerprint
        || evidence.query != query
        || evidence.created_at != created
    {
        return Err(invalid("Search evidence binding mismatch"));
    }
    let (prefix, digest) = id
        .rsplit_once('_')
        .ok_or_else(|| invalid("Invalid evidence ID"))?;
    evidence.id = prefix.into();
    if hex::encode(Sha256::digest(serde_json::to_vec(&evidence)?)) != digest {
        return Err(invalid("Search evidence was modified"));
    }
    evidence.id = id.into();
    let timestamp = DateTime::parse_from_rfc3339(&created)
        .map_err(|_| invalid("Invalid evidence timestamp"))?
        .with_timezone(&Utc);
    let age = Utc::now().signed_duration_since(timestamp);
    if age < chrono::Duration::zero() || age >= chrono::Duration::seconds(TTL_SECONDS) {
        return Err(invalid("搜索证据已过期，请重新搜索"));
    }
    public_query(&evidence.query)?;
    if evidence.candidates.len() > 5 {
        return Err(invalid("Too many candidates"));
    }
    Ok(evidence)
}

fn validate_name(name: &str) -> AppResult<()> {
    let drive_path = name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name.as_bytes().get(1) == Some(&b':')
        && name.as_bytes().get(2) != Some(&b' ');
    if !(1..=128).contains(&name.chars().count())
        || name.trim() != name
        || name.trim().is_empty()
        || name.chars().any(|ch| {
            unsafe_character(ch) || (ch.is_whitespace() && ch != ' ') || matches!(ch, '/' | '\\')
        })
        || drive_path
        || matches!(name, "." | "..")
    {
        return Err(invalid(
            "游戏名称须为 1 至 128 个可见字符，不能包含路径或控制字符",
        ));
    }
    Ok(())
}

pub fn confirm(
    engine: &FileEngine,
    search_id: &str,
    candidate_index: usize,
    name: &str,
    confirmation: &str,
) -> AppResult<Game> {
    if confirmation != "CONFIRM_GAME_IDENTITY" {
        return Err(invalid("CONFIRM_GAME_IDENTITY confirmation required"));
    }
    validate_name(name)?;
    let _guard = crate::transaction::write_lock();
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    let evidence = load_evidence(&tx, search_id)?;
    let interrupted: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))",
        [], |row| row.get(0),
    )?;
    if interrupted {
        return Err(invalid(
            "Resolve interrupted file transactions before registering a game",
        ));
    }
    if evidence.status != GameIdentityStatus::Available {
        return Err(invalid("Only available search evidence can be confirmed"));
    }
    let candidate = evidence
        .candidates
        .get(candidate_index)
        .ok_or_else(|| invalid("Invalid candidate index"))?;
    let directory = Path::new(&evidence.root_path);
    if snapshot(engine, directory)?.fingerprint != evidence.fingerprint {
        return Err(invalid("目录内容已变化，请重新检查并搜索"));
    }
    let game = games::register_online_identity(
        &tx,
        &evidence.root_path,
        name,
        json!({
            "searchId":search_id, "candidateIndex":candidate_index, "name":name,
            "source":"brave", "url":candidate.url, "title":candidate.title, "snippet":candidate.snippet,
            "confirmedAt":Utc::now().to_rfc3339(), "query":evidence.query, "fingerprint":evidence.fingerprint,
            "untrusted":true
        }),
    )?;
    if snapshot(engine, directory)?.fingerprint != evidence.fingerprint {
        return Err(invalid("Directory changed during confirmation"));
    }
    tx.execute(
        "UPDATE game_identity_searches SET invalidated=1 WHERE root_path=?1",
        [&evidence.root_path],
    )?;
    tx.commit()?;
    Ok(game)
}

#[cfg(test)]
#[path = "game_identity_tests.rs"]
mod tests;
