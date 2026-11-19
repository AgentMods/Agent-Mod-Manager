//! Read-only game discovery and explicit registration. Discovery never writes
//! game/launcher files and never adds a filesystem root to the manager's DB.
//! Format references: ValveSoftware/source-sdk-2013 src/tier1/KeyValues.cpp;
//! legendary-gl/legendary legendary/{lfs,models}/egl.py (checked 2026-11-17).
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::plugins::{self, GameAdapter, MarkerKind, Platform};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_KV_DEPTH: usize = 32;
const MAX_KV_TOKENS: usize = 65_536;
const MAX_KV_STRING: usize = 65_536;
const MAX_LIBRARIES: usize = 64;
pub const HIGH_CONFIDENCE: f64 = 0.85;

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub root_path: String,
    /// Detected payload layout, not a claim that the game runs natively here.
    pub platform: String,
    pub adapter_slug: Option<String>,
    pub adapter_version: Option<String>,
    pub confidence: f64,
    pub cover_path: Option<String>,
    pub steam_app_id: Option<String>,
    pub last_scanned_at: Option<String>,
    pub installed_mod_count: i64,
    pub enabled_mod_count: i64,
    pub metadata: Value,
    pub created_at: String,
    pub updated_at: String,
}

const GAME_COLUMNS: &str = "id,name,slug,root_path,platform,adapter_slug,adapter_version,confidence,cover_path,steam_app_id,last_scanned_at,installed_mod_count,enabled_mod_count,metadata_json,created_at,updated_at";

fn game_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Game> {
    let metadata: String = row.get(13)?;
    let metadata = serde_json::from_str(&metadata).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(13, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Game {
        id: row.get(0)?,
        name: row.get(1)?,
        slug: row.get(2)?,
        root_path: row.get(3)?,
        platform: row.get(4)?,
        adapter_slug: row.get(5)?,
        adapter_version: row.get(6)?,
        confidence: row.get(7)?,
        cover_path: row.get(8)?,
        steam_app_id: row.get(9)?,
        last_scanned_at: row.get(10)?,
        installed_mod_count: row.get(11)?,
        enabled_mod_count: row.get(12)?,
        metadata,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

pub fn list_games(db: &Db) -> AppResult<Vec<Game>> {
    let conn = db.conn();
    let mut statement = conn.prepare(&format!(
        "SELECT {GAME_COLUMNS} FROM games ORDER BY name COLLATE NOCASE,id"
    ))?;
    let rows = statement.query_map([], game_from_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn get_game(db: &Db, id: &str) -> AppResult<Game> {
    let conn = db.conn();
    conn.query_row(
        &format!("SELECT {GAME_COLUMNS} FROM games WHERE id=?1"),
        [id],
        game_from_row,
    )
    .optional()?
    .ok_or_else(|| AppError::NotFound(format!("游戏: {id}")))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentificationStatus {
    HighConfidence,
    Candidate,
    Unidentified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterMatch {
    pub slug: String,
    pub name: String,
    pub version: String,
    pub platform: Platform,
    pub confidence: f64,
    pub matched_markers: Vec<String>,
    pub missing_required_markers: Vec<String>,
    pub distinctive_file_matched: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryKind {
    User,
    Steam,
    Epic,
    Common,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverySource {
    pub kind: DiscoveryKind,
    pub metadata_path: Option<String>,
    pub display_name: Option<String>,
    pub steam_app_id: Option<String>,
    /// Launcher metadata is advisory, including when this flag is true.
    pub complete_install: Option<bool>,
}

impl DiscoverySource {
    fn simple(kind: DiscoveryKind) -> Self {
        Self {
            kind,
            metadata_path: None,
            display_name: None,
            steam_app_id: None,
            complete_install: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameCandidate {
    pub root_path: String,
    pub name: String,
    pub confidence: f64,
    pub status: IdentificationStatus,
    pub suggested_adapter_slug: Option<String>,
    pub platform: String,
    pub matches: Vec<AdapterMatch>,
    pub sources: Vec<DiscoverySource>,
    /// Discovery is never equivalent to registering this directory.
    pub requires_confirmation: bool,
    pub registered_game_id: Option<String>,
    pub warnings: Vec<String>,
}

fn host_platform() -> Platform {
    if cfg!(target_os = "windows") {
        Platform::Windows
    } else if cfg!(target_os = "macos") {
        Platform::Macos
    } else {
        Platform::Linux
    }
}

fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "windows",
        Platform::Macos => "macos",
        Platform::Linux => "linux",
    }
}

fn path_text(path: &Path) -> AppResult<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("目录名无法无损表示为 UTF-8"))
}

fn canonical_directory(path: &Path) -> AppResult<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("请选择绝对目录路径"));
    }
    let root = path.canonicalize()?;
    if !fs::metadata(&root)?.is_dir() {
        return Err(invalid("游戏根必须是已存在的目录"));
    }
    path_text(&root)?;
    Ok(root)
}

/// Read only enabled, validated adapters. An entirely unseeded database can
/// discover embedded packages; disabling every seeded package stays disabled.
fn active_adapters(db: &Db) -> AppResult<Vec<GameAdapter>> {
    let packages = plugins::list_plugins(db)?;
    if packages.is_empty() {
        return Ok(plugins::builtin_packages()?
            .into_iter()
            .map(|p| p.adapter)
            .collect());
    }
    packages
        .into_iter()
        .filter(|p| p.enabled && p.validated)
        .map(|p| plugins::adapter_for(db, &p.slug))
        .collect()
}

fn distinctive_file(path: &str, kind: MarkerKind) -> bool {
    matches!(kind, MarkerKind::File)
        && matches!(
            Path::new(path).extension().and_then(|s| s.to_str()),
            Some("exe" | "dll" | "esm")
        )
}

fn score_adapter(
    root: &Path,
    adapter: &GameAdapter,
    platform: Platform,
    sources: &[DiscoverySource],
) -> Option<AdapterMatch> {
    let mut matched = Vec::new();
    let mut missing = Vec::new();
    let mut required = 0.0;
    let mut required_hit = 0.0;
    let mut optional = 0.0;
    let mut optional_hit = 0.0;
    let mut distinctive = false;
    let mut reasons = Vec::new();
    for marker in adapter
        .markers
        .iter()
        .filter(|m| m.platforms.contains(&platform))
    {
        let specific = distinctive_file(&marker.path, marker.kind);
        let weight = if specific { 4.0 } else { 1.0 };
        let target = root.join(&marker.path);
        let hit = target
            .canonicalize()
            .ok()
            .filter(|p| p.starts_with(root))
            .and_then(|p| fs::metadata(p).ok())
            .is_some_and(|m| match marker.kind {
                MarkerKind::File => m.is_file(),
                MarkerKind::Directory => m.is_dir(),
            });
        if marker.required {
            required += weight;
        } else {
            optional += weight;
        }
        if hit {
            matched.push(marker.path.clone());
            distinctive |= specific;
            if marker.required {
                required_hit += weight;
            } else {
                optional_hit += weight;
            }
        } else if marker.required {
            missing.push(marker.path.clone());
        }
    }
    let steam_ids: BTreeSet<_> = sources
        .iter()
        .filter_map(|s| s.steam_app_id.as_deref())
        .collect();
    let steam_match = adapter
        .steam_app_id
        .as_deref()
        .is_some_and(|id| steam_ids.contains(id));
    if matched.is_empty() && !steam_match {
        return None;
    }
    let total = required + optional;
    let mut score: f64 = if matched.is_empty() {
        0.15
    } else if required > 0.0 && missing.is_empty() && distinctive {
        0.9 + 0.1
            * if optional == 0.0 {
                1.0
            } else {
                optional_hit / optional
            }
    } else {
        0.7 * (required_hit + optional_hit) / total.max(1.0)
    };
    if !distinctive {
        score = score.min(0.6);
        reasons.push("通用目录或配置文件只能作为候选信号".into());
    }
    if !missing.is_empty() {
        reasons.push("缺少必需标记，不能确认完整布局".into());
    }
    if steam_match {
        reasons.push("Steam App ID 与适配包一致；仍需核对磁盘标记".into());
    }
    if !steam_ids.is_empty() && !steam_match {
        score = score.min(0.5);
        reasons.push("Steam 元数据与标记推测的 App ID 不一致".into());
    }
    if sources.iter().any(|s| s.complete_install == Some(false)) {
        score = score.min(0.6);
        reasons.push("启动器报告安装未完成，保留为候选".into());
    }
    Some(AdapterMatch {
        slug: adapter.slug.clone(),
        name: adapter.name.clone(),
        version: adapter.version.clone(),
        platform,
        confidence: score.clamp(0.0, 1.0),
        matched_markers: matched,
        missing_required_markers: missing,
        distinctive_file_matched: distinctive,
        reasons,
    })
}

fn identify_at(
    root: &Path,
    adapters: &[GameAdapter],
    sources: Vec<DiscoverySource>,
) -> AppResult<GameCandidate> {
    let mut matches = Vec::new();
    for adapter in adapters {
        let mut layouts: Vec<_> = adapter
            .platforms
            .iter()
            .filter_map(|p| score_adapter(root, adapter, *p, &sources))
            .collect();
        layouts.sort_by(|a, b| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| (b.platform == host_platform()).cmp(&(a.platform == host_platform())))
        });
        if let Some(best) = layouts.into_iter().next() {
            matches.push(best);
        }
    }
    matches.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.slug.cmp(&b.slug))
    });
    let best = matches.first();
    let confidence = best.map_or(0.0, |m| m.confidence);
    let ambiguous = matches
        .get(1)
        .is_some_and(|other| confidence - other.confidence < 0.15);
    let status = if best.is_none() {
        IdentificationStatus::Unidentified
    } else if confidence >= HIGH_CONFIDENCE && !ambiguous {
        IdentificationStatus::HighConfidence
    } else {
        IdentificationStatus::Candidate
    };
    let mut warnings = Vec::new();
    if ambiguous {
        warnings.push("多个适配包具有接近的得分，需要人工选择".into());
    }
    if let Some(best) = best {
        if best.platform != host_platform() {
            warnings.push(format!(
                "标记显示 {} 布局；尚未确认当前主机的运行环境",
                platform_name(best.platform)
            ));
        }
    }
    let name = best
        .map(|m| m.name.clone())
        .or_else(|| sources.iter().find_map(|s| s.display_name.clone()))
        .unwrap_or_else(|| {
            root.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| root.display().to_string())
        });
    Ok(GameCandidate {
        root_path: path_text(root)?,
        name,
        confidence,
        status,
        suggested_adapter_slug: best.map(|m| m.slug.clone()),
        platform: best.map_or("unknown", |m| platform_name(m.platform)).into(),
        matches,
        sources,
        requires_confirmation: true,
        registered_game_id: None,
        warnings,
    })
}

/// Probe this directory only. Names alone, generic markers, missing required
/// markers and near-tied matches never produce a high-confidence identity.
pub fn identify_game(db: &Db, directory: &Path) -> AppResult<GameCandidate> {
    let root = canonical_directory(directory)?;
    identify_at(
        &root,
        &active_adapters(db)?,
        vec![DiscoverySource::simple(DiscoveryKind::User)],
    )
}

/// Explicit user-selected root registration. Passing an adapter slug also
/// explicitly selects that identity; otherwise only a unique high-confidence
/// match is attached. Low-confidence roots can be registered without an adapter.
/// Existing install counts, cover, creation time, metadata and identity survive
/// repeated root registration unless a different adapter is explicitly selected.
pub fn register_game(db: &Db, directory: &Path, adapter_slug: Option<&str>) -> AppResult<Game> {
    let root = canonical_directory(directory)?;
    if root.parent().is_none() {
        return Err(invalid("不能将整个文件系统根登记为游戏目录"));
    }
    let adapters = active_adapters(db)?;
    let candidate = identify_at(
        &root,
        &adapters,
        vec![DiscoverySource::simple(DiscoveryKind::User)],
    )?;
    let selected = if let Some(slug) = adapter_slug {
        plugins::validate_slug(slug)?;
        adapters
            .iter()
            .find(|a| a.slug == slug)
            .ok_or_else(|| AppError::NotFound(format!("未启用的适配包: {slug}")))?;
        Some(
            candidate
                .matches
                .iter()
                .find(|m| m.slug == slug && !m.matched_markers.is_empty())
                .ok_or_else(|| invalid("所选适配包没有任何实际匹配的磁盘标记"))?,
        )
    } else if candidate.status == IdentificationStatus::HighConfidence {
        candidate.matches.first()
    } else {
        None
    };
    let root_text = path_text(&root)?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let existing = tx
        .query_row(
            &format!("SELECT {GAME_COLUMNS} FROM games WHERE root_path=?1"),
            [&root_text],
            game_from_row,
        )
        .optional()?;
    let retain_identity =
        adapter_slug.is_none() && existing.as_ref().is_some_and(|g| g.adapter_slug.is_some());
    let old = existing.as_ref();
    let adapter = selected.and_then(|m| adapters.iter().find(|a| a.slug == m.slug));
    let slug = if retain_identity {
        old.unwrap().slug.clone()
    } else {
        adapter.map_or_else(|| "unknown".into(), |a| a.slug.clone())
    };
    let name = old.map(|g| g.name.clone()).unwrap_or_else(|| {
        adapter.map_or_else(
            || root.file_name().unwrap().to_string_lossy().to_string(),
            |a| a.name.clone(),
        )
    });
    let id = old
        .map(|g| g.id.clone())
        .unwrap_or_else(|| format!("game_{}", uuid::Uuid::new_v4().simple()));
    let adapter_slug = if retain_identity {
        old.unwrap().adapter_slug.clone()
    } else {
        adapter.map(|a| a.slug.clone())
    };
    let version = if retain_identity {
        old.unwrap().adapter_version.clone()
    } else {
        adapter.map(|a| a.version.clone())
    };
    let platform = if retain_identity {
        old.unwrap().platform.clone()
    } else {
        selected
            .map_or("unknown", |m| platform_name(m.platform))
            .into()
    };
    let confidence = if retain_identity {
        old.unwrap().confidence
    } else {
        selected.map_or(0.0, |m| m.confidence)
    };
    let app_id = if retain_identity {
        old.unwrap().steam_app_id.clone()
    } else {
        adapter.and_then(|a| a.steam_app_id.clone())
    };
    let mut metadata = old.map_or_else(|| json!({}), |g| g.metadata.clone());
    if !metadata.is_object() {
        return Err(invalid("现有游戏 metadata 必须是 JSON 对象"));
    }
    metadata["registration"] = json!({"kind":"explicit", "at":now});
    metadata["lastIdentification"] = serde_json::to_value(&candidate)?;
    tx.execute(
        "INSERT INTO games (id,name,slug,root_path,platform,adapter_slug,adapter_version,confidence,steam_app_id,last_scanned_at,metadata_json,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?10,?10)
         ON CONFLICT(root_path) DO UPDATE SET name=excluded.name,slug=excluded.slug,platform=excluded.platform,adapter_slug=excluded.adapter_slug,adapter_version=excluded.adapter_version,confidence=excluded.confidence,steam_app_id=excluded.steam_app_id,last_scanned_at=excluded.last_scanned_at,metadata_json=excluded.metadata_json,updated_at=excluded.updated_at",
        params![id,name,slug,root_text,platform,adapter_slug,version,confidence,app_id,now,metadata.to_string()],
    )?;
    let game = tx.query_row(
        &format!("SELECT {GAME_COLUMNS} FROM games WHERE root_path=?1"),
        [&root_text],
        game_from_row,
    )?;
    tx.commit()?;
    Ok(game)
}

/// Called inside the identity evidence transaction while holding write_lock.
/// Online evidence can name an unknown game, never select an adapter.
pub(crate) fn register_online_identity(
    tx: &rusqlite::Transaction<'_>,
    root: &str,
    name: &str,
    evidence: Value,
) -> AppResult<Game> {
    let old = tx
        .query_row(
            &format!("SELECT {GAME_COLUMNS} FROM games WHERE root_path=?1"),
            [root],
            game_from_row,
        )
        .optional()?;
    if old
        .as_ref()
        .is_some_and(|g| g.adapter_slug.is_some() || g.adapter_version.is_some())
    {
        return Err(invalid(
            "Online identity cannot replace an adapter-confirmed game",
        ));
    }
    let mut metadata = old
        .as_ref()
        .map_or_else(|| json!({}), |g| g.metadata.clone());
    if !metadata.is_object() {
        return Err(invalid("Existing game metadata must be an object"));
    }
    let now = chrono::Utc::now().to_rfc3339();
    metadata["onlineIdentity"] = evidence;
    let id = old
        .as_ref()
        .map(|g| g.id.clone())
        .unwrap_or_else(|| format!("game_{}", uuid::Uuid::new_v4().simple()));
    tx.execute(
        "INSERT INTO games(id,name,slug,root_path,confidence,metadata_json,created_at,updated_at)
         VALUES(?1,?2,'unknown',?3,0,?4,?5,?5)
         ON CONFLICT(root_path) DO UPDATE SET name=excluded.name,
         confidence=MIN(games.confidence,0.6),metadata_json=excluded.metadata_json,updated_at=excluded.updated_at",
        params![id,name,root,metadata.to_string(),now],
    )?;
    Ok(tx.query_row(
        &format!("SELECT {GAME_COLUMNS} FROM games WHERE root_path=?1"),
        [root],
        game_from_row,
    )?)
}

// Bounded text KeyValues parser. No regex, includes, condition evaluation,
// filesystem access or binary/KV3 support. Duplicate case-insensitive keys fail
// closed because choosing first/last would make install-root interpretation
// ambiguous. Scalars stay strings; syntax is separated from Steam semantics.
#[derive(Debug, PartialEq)]
enum Kv {
    Text(String),
    Object(BTreeMap<String, Kv>),
}
#[derive(Debug)]
enum Token {
    Text(String),
    Open,
    Close,
}

struct KvParser<'a> {
    bytes: &'a [u8],
    offset: usize,
    tokens: usize,
}
impl<'a> KvParser<'a> {
    fn error(&self, message: &str) -> AppError {
        invalid(format!("KeyValues 在字节 {}: {message}", self.offset))
    }
    fn token(&mut self) -> AppResult<Option<Token>> {
        loop {
            while self
                .bytes
                .get(self.offset)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.offset += 1;
            }
            if self.bytes.get(self.offset..self.offset + 2) == Some(b"//") {
                while self.bytes.get(self.offset).is_some_and(|b| *b != b'\n') {
                    self.offset += 1;
                }
            } else if self.bytes.get(self.offset..self.offset + 2) == Some(b"/*") {
                self.offset += 2;
                while self.bytes.get(self.offset..self.offset + 2) != Some(b"*/") {
                    if self.offset >= self.bytes.len() {
                        return Err(self.error("未闭合注释"));
                    }
                    self.offset += 1;
                }
                self.offset += 2;
            } else {
                break;
            }
        }
        let Some(&first) = self.bytes.get(self.offset) else {
            return Ok(None);
        };
        self.tokens += 1;
        if self.tokens > MAX_KV_TOKENS {
            return Err(self.error("超出 token 数量限制"));
        }
        self.offset += 1;
        if first == b'{' {
            return Ok(Some(Token::Open));
        }
        if first == b'}' {
            return Ok(Some(Token::Close));
        }
        let mut value = Vec::new();
        if first == b'"' {
            loop {
                let Some(&b) = self.bytes.get(self.offset) else {
                    return Err(self.error("未闭合字符串"));
                };
                self.offset += 1;
                match b {
                    b'"' => break,
                    b'\\' => {
                        let Some(&next) = self.bytes.get(self.offset) else {
                            return Err(self.error("截断的转义序列"));
                        };
                        self.offset += 1;
                        match next {
                            b'"' | b'\\' => value.push(next),
                            b'n' => value.push(b'\n'),
                            b'r' => value.push(b'\r'),
                            b't' => value.push(b'\t'),
                            _ => {
                                value.push(b'\\');
                                value.push(next);
                            }
                        }
                    }
                    0 => return Err(self.error("不允许空字节")),
                    _ => value.push(b),
                }
                if value.len() > MAX_KV_STRING {
                    return Err(self.error("字符串超出大小限制"));
                }
            }
        } else {
            value.push(first);
            while let Some(&b) = self.bytes.get(self.offset) {
                if b.is_ascii_whitespace() || matches!(b, b'{' | b'}' | b'"') {
                    break;
                }
                if b == 0 {
                    return Err(self.error("不允许空字节"));
                }
                value.push(b);
                self.offset += 1;
                if value.len() > MAX_KV_STRING {
                    return Err(self.error("字符串超出大小限制"));
                }
            }
        }
        let value = String::from_utf8(value).map_err(|_| self.error("无效 UTF-8"))?;
        if value.starts_with(['#', '[', ']']) {
            return Err(self.error("不支持 includes 或平台条件"));
        }
        Ok(Some(Token::Text(value)))
    }
    fn object(&mut self, depth: usize, nested: bool) -> AppResult<BTreeMap<String, Kv>> {
        if depth > MAX_KV_DEPTH {
            return Err(self.error("嵌套深度超限"));
        }
        let mut entries = BTreeMap::new();
        loop {
            let key = match self.token()? {
                None if !nested => break,
                Some(Token::Close) if nested => break,
                Some(Token::Text(k)) if !k.is_empty() => k.to_ascii_lowercase(),
                _ => return Err(self.error("缺少键或对象未闭合")),
            };
            let value = match self.token()? {
                Some(Token::Text(value)) => Kv::Text(value),
                Some(Token::Open) => Kv::Object(self.object(depth + 1, true)?),
                _ => return Err(self.error("键后缺少值")),
            };
            if entries.insert(key, value).is_some() {
                return Err(self.error("重复的键"));
            }
        }
        Ok(entries)
    }
}

fn parse_kv(input: &str) -> AppResult<BTreeMap<String, Kv>> {
    if input.len() > MAX_METADATA_BYTES || input.contains('\0') {
        return Err(invalid("KeyValues 超出大小限制或包含空字节"));
    }
    KvParser {
        bytes: input.trim_start_matches('\u{feff}').as_bytes(),
        offset: 0,
        tokens: 0,
    }
    .object(0, false)
}
fn kv_object<'a>(
    entries: &'a BTreeMap<String, Kv>,
    key: &str,
) -> AppResult<&'a BTreeMap<String, Kv>> {
    match entries.get(key) {
        Some(Kv::Object(value)) => Ok(value),
        _ => Err(invalid(format!("KeyValues 缺少对象 {key}"))),
    }
}
fn kv_text<'a>(entries: &'a BTreeMap<String, Kv>, key: &str) -> AppResult<&'a str> {
    match entries.get(key) {
        Some(Kv::Text(value)) => Ok(value),
        _ => Err(invalid(format!("KeyValues 缺少字符串 {key}"))),
    }
}
fn app_id(input: &str) -> AppResult<u32> {
    input
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0 && n.to_string() == input)
        .ok_or_else(|| invalid("App ID 必须为规范的正整数"))
}
fn display_text(input: &str) -> AppResult<String> {
    if input.trim().is_empty() || input.len() > 1024 || input.chars().any(char::is_control) {
        return Err(invalid("启动器显示名称为空、过长或包含控制字符"));
    }
    Ok(input.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteamManifest {
    pub app_id: u32,
    pub name: String,
    pub install_dir: String,
    pub state_flags: Option<u32>,
}

pub fn parse_appmanifest(input: &str) -> AppResult<SteamManifest> {
    let tree = parse_kv(input)?;
    let state = kv_object(&tree, "appstate")?;
    let install_dir = kv_text(state, "installdir")?.to_string();
    plugins::validate_relative_path(&install_dir, false)?;
    if install_dir.contains('/') {
        return Err(invalid("Steam installdir 必须是 common 下的单个目录名"));
    }
    let state_flags = match state.get("stateflags") {
        None => None,
        Some(Kv::Text(flags)) => Some(
            flags
                .parse::<u32>()
                .map_err(|_| invalid("StateFlags 必须为整数"))?,
        ),
        _ => return Err(invalid("StateFlags 必须为标量")),
    };
    Ok(SteamManifest {
        app_id: app_id(kv_text(state, "appid")?)?,
        name: display_text(kv_text(state, "name")?)?,
        install_dir,
        state_flags,
    })
}

/// Both legacy `"1" "D:\\Library"` and modern `"1" { "path" ... }`.
/// Paths remain strings here: Windows paths must also parse on a Unix host.
pub fn parse_library_folders(input: &str) -> AppResult<Vec<String>> {
    let tree = parse_kv(input)?;
    let folders = kv_object(&tree, "libraryfolders")?;
    let mut paths = BTreeSet::new();
    for (index, entry) in folders {
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let path = match entry {
            Kv::Text(path) => path.as_str(),
            Kv::Object(fields) => kv_text(fields, "path")?,
        };
        if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
            return Err(invalid("Steam 库路径无效"));
        }
        paths.insert(path.to_owned());
        if paths.len() > MAX_LIBRARIES {
            return Err(invalid("Steam 库数量超限"));
        }
    }
    Ok(paths.into_iter().collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ScanOptions {
    pub user_roots: Vec<PathBuf>,
    pub steam_roots: Vec<PathBuf>,
    pub epic_roots: Vec<PathBuf>,
    pub epic_manifest_dirs: Vec<PathBuf>,
    pub legendary_installed_files: Vec<PathBuf>,
    pub include_common_roots: bool,
    /// Root itself is depth 0. Maximum accepted depth is 3, never a disk crawl.
    pub max_depth: u8,
    pub max_directories: usize,
    pub max_entries: usize,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            user_roots: vec![],
            steam_roots: vec![],
            epic_roots: vec![],
            epic_manifest_dirs: vec![],
            legendary_installed_files: vec![],
            include_common_roots: true,
            max_depth: 2,
            max_directories: 2048,
            max_entries: 16_384,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommonRoots {
    pub steam: Vec<PathBuf>,
    pub epic: Vec<PathBuf>,
    pub epic_manifests: Vec<PathBuf>,
    pub legendary_installed_files: Vec<PathBuf>,
    pub game_instances: Vec<PathBuf>,
}

/// Candidate locations only. Existence is checked by the scanner; no launchers
/// are executed, no registry changes are made, and no volumes are enumerated.
pub fn common_roots() -> CommonRoots {
    let mut roots = CommonRoots::default();
    let home = dirs::home_dir();
    match host_platform() {
        Platform::Windows => {
            for key in ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"] {
                if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
                    let path = PathBuf::from(value);
                    roots.steam.push(path.join("Steam"));
                    roots.epic.push(path.join("Epic Games"));
                }
            }
            let drive = std::env::var_os("SystemDrive").unwrap_or_else(|| "C:".into());
            let drive = PathBuf::from(format!("{}\\", drive.to_string_lossy()));
            roots.steam.push(drive.join("Program Files (x86)/Steam"));
            roots.epic.push(drive.join("Program Files/Epic Games"));
            let data = std::env::var_os("ProgramData")
                .map(PathBuf::from)
                .unwrap_or_else(|| drive.join("ProgramData"));
            roots
                .epic_manifests
                .push(data.join("Epic/EpicGamesLauncher/Data/Manifests"));
            if let Some(data) = std::env::var_os("APPDATA") {
                roots
                    .game_instances
                    .push(PathBuf::from(data).join(".minecraft"));
            }
        }
        Platform::Macos => {
            if let Some(home) = &home {
                roots
                    .steam
                    .push(home.join("Library/Application Support/Steam"));
                roots
                    .game_instances
                    .push(home.join("Library/Application Support/minecraft"));
            }
            roots.epic.push(PathBuf::from("/Users/Shared/Epic Games"));
        }
        Platform::Linux => {
            if let Some(data) = dirs::data_local_dir() {
                roots.steam.push(data.join("Steam"));
            }
            if let Some(home) = &home {
                for suffix in [
                    ".local/share/Steam",
                    ".steam/steam",
                    ".steam/root",
                    ".steam/debian-installation",
                    ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                    "snap/steam/common/.local/share/Steam",
                ] {
                    roots.steam.push(home.join(suffix));
                }
                roots.game_instances.push(home.join(".minecraft"));
                roots
                    .epic
                    .extend([home.join("Games/Heroic"), home.join("Games/legendary")]);
                roots.legendary_installed_files.push(home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/legendaryConfig/legendary/installed.json"));
            }
        }
    }
    if let Some(config) = dirs::config_dir() {
        roots.legendary_installed_files.extend([
            config.join("legendary/installed.json"),
            config.join("heroic/legendaryConfig/legendary/installed.json"),
        ]);
    }
    roots
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanWarning {
    pub path: String,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub candidates: Vec<GameCandidate>,
    pub warnings: Vec<ScanWarning>,
    pub directories_checked: usize,
    pub entries_checked: usize,
    pub truncated: bool,
}

fn read_metadata(path: &Path) -> AppResult<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_METADATA_BYTES as u64 {
        return Err(invalid("启动器元数据须为大小受限的普通文件"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_METADATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(invalid("启动器元数据大小超限"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("启动器元数据须为 UTF-8"))
}

struct Scanner<'a> {
    adapters: &'a [GameAdapter],
    options: &'a ScanOptions,
    report: ScanReport,
    probes: BTreeMap<PathBuf, Vec<DiscoverySource>>,
}
impl Scanner<'_> {
    fn warn(&mut self, path: &Path, message: impl ToString) {
        // Warnings are also bounded if a directory contains many broken files.
        if self.report.warnings.len() < 256 {
            self.report.warnings.push(ScanWarning {
                path: path.display().to_string(),
                message: message.to_string(),
            });
        }
    }
    fn limit(&mut self, path: &Path) {
        self.report.truncated = true;
        self.warn(path, "达到扫描数量上限，结果不完整");
    }
    fn probe(&mut self, path: &Path, source: DiscoverySource) {
        let root = match canonical_directory(path) {
            Ok(path) => path,
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                if source.metadata_path.is_some() {
                    self.warn(path, "启动器记录的安装目录不存在");
                }
                return;
            }
            Err(e) => {
                self.warn(path, e);
                return;
            }
        };
        if let Some(sources) = self.probes.get_mut(&root) {
            if !sources.contains(&source) {
                sources.push(source);
            }
            return;
        }
        if self.probes.len() >= self.options.max_directories {
            self.limit(path);
            return;
        }
        self.probes.insert(root, vec![source]);
    }
    fn entries(&mut self, path: &Path) -> Vec<fs::DirEntry> {
        let mut result = Vec::new();
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return result,
            Err(e) => {
                self.warn(path, e);
                return result;
            }
        };
        for entry in entries {
            if self.report.entries_checked >= self.options.max_entries {
                self.limit(path);
                break;
            }
            self.report.entries_checked += 1;
            match entry {
                Ok(entry) => result.push(entry),
                Err(e) => self.warn(path, e),
            }
        }
        result.sort_by_key(fs::DirEntry::file_name);
        result
    }
    fn tree(&mut self, path: &Path, kind: DiscoveryKind, depth: u8) {
        let base = match canonical_directory(path) {
            Ok(base) => base,
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.warn(path, e);
                return;
            }
        };
        let mut queue = VecDeque::from([(base, 0)]);
        let mut seen = BTreeSet::new();
        while let Some((path, level)) = queue.pop_front() {
            if !seen.insert(path.clone()) {
                continue;
            }
            if self.probes.len() >= self.options.max_directories && !self.probes.contains_key(&path)
            {
                self.limit(&path);
                break;
            }
            self.probe(&path, DiscoverySource::simple(kind.clone()));
            if level >= depth {
                continue;
            }
            if self.probes.len() >= self.options.max_directories {
                self.limit(&path);
                break;
            }
            for entry in self.entries(&path) {
                // No recursive following of symlinks/reparse points. A root
                // directly chosen by the user may itself be canonicalized.
                if entry
                    .file_type()
                    .is_ok_and(|ft| ft.is_dir() && !ft.is_symlink())
                {
                    let name = entry.file_name();
                    if matches!(
                        name.to_str(),
                        Some(
                            ".git"
                                | ".egstore"
                                | "node_modules"
                                | "compatdata"
                                | "workshop"
                                | "downloading"
                        )
                    ) {
                        continue;
                    }
                    queue.push_back((entry.path(), level + 1));
                }
            }
        }
    }
    fn steam(&mut self, initial: &[PathBuf]) {
        let mut queue: VecDeque<_> = initial.iter().cloned().collect();
        let mut seen = BTreeSet::new();
        while let Some(path) = queue.pop_front() {
            let path = if path.file_name().is_some_and(|n| n == "steamapps") {
                path.parent().unwrap_or(&path).to_path_buf()
            } else {
                path
            };
            let library = match canonical_directory(&path) {
                Ok(path) => path,
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn(&path, e);
                    continue;
                }
            };
            if seen.contains(&library) {
                continue;
            }
            if seen.len() >= MAX_LIBRARIES {
                self.limit(&library);
                break;
            }
            seen.insert(library.clone());
            for suffix in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
                let file = library.join(suffix);
                match read_metadata(&file).and_then(|s| parse_library_folders(&s)) {
                    Ok(paths) => {
                        for path in paths {
                            let path = PathBuf::from(path);
                            if path.is_absolute() {
                                queue.push_back(path);
                            } else {
                                self.warn(&file, "忽略非当前平台绝对路径的 Steam 库");
                            }
                        }
                    }
                    Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => self.warn(&file, e),
                }
            }
            let apps = library.join("steamapps");
            let common = apps.join("common");
            let common = match canonical_directory(&common) {
                Ok(common) if common.starts_with(&library) => common,
                Ok(_) => {
                    self.warn(&common, "Steam common 符号链接越出库根");
                    continue;
                }
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn(&common, e);
                    continue;
                }
            };
            for entry in self.entries(&apps) {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(id) = name
                    .strip_prefix("appmanifest_")
                    .and_then(|s| s.strip_suffix(".acf"))
                else {
                    continue;
                };
                let file = entry.path();
                let parsed = (|| -> AppResult<SteamManifest> {
                    let id = app_id(id)?;
                    let manifest = parse_appmanifest(&read_metadata(&file)?)?;
                    if manifest.app_id != id {
                        return Err(invalid("manifest 文件名与 AppState App ID 不一致"));
                    }
                    Ok(manifest)
                })();
                let manifest = match parsed {
                    Ok(m) => m,
                    Err(e) => {
                        self.warn(&file, e);
                        continue;
                    }
                };
                let game = common.join(&manifest.install_dir);
                match game.canonicalize() {
                    Ok(path) if path.starts_with(&common) && path != common => {
                        let source = DiscoverySource {
                            kind: DiscoveryKind::Steam,
                            metadata_path: Some(file.display().to_string()),
                            display_name: Some(manifest.name),
                            steam_app_id: Some(manifest.app_id.to_string()),
                            complete_install: manifest
                                .state_flags
                                .map(|f| f & 4 != 0 && f & 1 == 0),
                        };
                        // Steam's macOS content may place the runtime here;
                        // prefer that marked runtime over an unmarked wrapper,
                        // rather than reporting the same app twice. This fixed
                        // probe does not enumerate an app bundle recursively.
                        let runtime = path
                            .join("Contents/MacOS")
                            .canonicalize()
                            .ok()
                            .filter(|p| p.starts_with(&path) && p.is_dir())
                            .filter(|p| {
                                identify_at(p, self.adapters, vec![source.clone()]).is_ok_and(|c| {
                                    c.matches.iter().any(|m| m.distinctive_file_matched)
                                })
                            });
                        if let Some(runtime) = runtime {
                            self.probe(&runtime, source);
                        } else {
                            self.probe(&path, source);
                        }
                    }
                    Ok(_) => self.warn(&game, "Steam 安装目录越出 common，忽略"),
                    Err(e) => self.warn(&game, e),
                }
            }
        }
    }
    fn epic_manifests(&mut self, directories: &[PathBuf]) {
        for directory in directories {
            if !directory.is_absolute() {
                self.warn(directory, "Epic 元数据目录必须是绝对路径");
                continue;
            }
            for entry in self.entries(directory) {
                if entry.path().extension().is_none_or(|s| s != "item") {
                    continue;
                }
                let file = entry.path();
                let parsed = read_metadata(&file)
                    .and_then(|s| serde_json::from_str::<EpicManifest>(&s).map_err(AppError::from));
                match parsed {
                    Ok(manifest) => {
                        if let Err(e) = display_text(&manifest.display_name) {
                            self.warn(&file, e);
                            continue;
                        }
                        if !manifest.install_location.is_absolute() {
                            self.warn(&file, "Epic InstallLocation 必须是当前平台绝对路径");
                            continue;
                        }
                        self.probe(
                            &manifest.install_location,
                            DiscoverySource {
                                kind: DiscoveryKind::Epic,
                                metadata_path: Some(file.display().to_string()),
                                display_name: Some(manifest.display_name),
                                steam_app_id: None,
                                complete_install: Some(
                                    !manifest.incomplete && !manifest.needs_validation,
                                ),
                            },
                        );
                    }
                    Err(e) => self.warn(&file, e),
                }
            }
        }
    }
    fn legendary(&mut self, files: &[PathBuf]) {
        for file in files {
            let entries = match read_metadata(file).and_then(|s| {
                serde_json::from_str::<BTreeMap<String, Value>>(&s).map_err(AppError::from)
            }) {
                Ok(entries) => entries,
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn(file, e);
                    continue;
                }
            };
            for entry in entries.into_values() {
                if self.report.entries_checked >= self.options.max_entries {
                    self.limit(file);
                    break;
                }
                self.report.entries_checked += 1;
                let entry: LegendaryInstall = match serde_json::from_value(entry) {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.warn(file, e);
                        continue;
                    }
                };
                if !entry.install_path.is_absolute() {
                    self.warn(file, "忽略非当前平台绝对路径的 Legendary 安装目录");
                    continue;
                }
                if let Err(e) = display_text(&entry.title) {
                    self.warn(file, e);
                    continue;
                }
                self.probe(
                    &entry.install_path,
                    DiscoverySource {
                        kind: DiscoveryKind::Epic,
                        metadata_path: Some(file.display().to_string()),
                        display_name: Some(entry.title),
                        steam_app_id: None,
                        complete_install: entry.needs_verification.then_some(false),
                    },
                );
            }
        }
    }
}

#[derive(Deserialize)]
struct EpicManifest {
    #[serde(rename = "InstallLocation")]
    install_location: PathBuf,
    #[serde(rename = "DisplayName")]
    display_name: String,
    #[serde(rename = "bIsIncompleteInstall", default)]
    incomplete: bool,
    #[serde(rename = "bNeedsValidation", default)]
    needs_validation: bool,
}
#[derive(Deserialize)]
struct LegendaryInstall {
    install_path: PathBuf,
    title: String,
    #[serde(default)]
    needs_verification: bool,
}

/// Returns discoveries only. Caller must present candidates and explicitly call
/// register_game before treating any new directory as a managed game root.
pub fn scan_games(db: &Db, options: &ScanOptions) -> AppResult<ScanReport> {
    if options.max_depth > 3
        || options.max_directories == 0
        || options.max_directories > 10_000
        || options.max_entries == 0
        || options.max_entries > 100_000
    {
        return Err(invalid(
            "扫描深度须为 0..=3，目录上限为 1..=10000，条目上限为 1..=100000",
        ));
    }
    let roots_count = options.user_roots.len()
        + options.steam_roots.len()
        + options.epic_roots.len()
        + options.epic_manifest_dirs.len()
        + options.legendary_installed_files.len();
    if roots_count > 256 {
        return Err(invalid("显式扫描位置数量超限"));
    }
    for path in options
        .user_roots
        .iter()
        .chain(&options.steam_roots)
        .chain(&options.epic_roots)
        .chain(&options.epic_manifest_dirs)
        .chain(&options.legendary_installed_files)
    {
        if !path.is_absolute() {
            return Err(invalid("扫描位置必须为绝对路径"));
        }
    }
    let adapters = active_adapters(db)?;
    let existing: BTreeMap<_, _> = list_games(db)?
        .into_iter()
        .map(|g| (g.root_path, g.id))
        .collect();
    let common = if options.include_common_roots {
        common_roots()
    } else {
        CommonRoots::default()
    };
    let mut scanner = Scanner {
        adapters: &adapters,
        options,
        report: ScanReport::default(),
        probes: BTreeMap::new(),
    };
    scanner.steam(
        &options
            .steam_roots
            .iter()
            .chain(&common.steam)
            .cloned()
            .collect::<Vec<_>>(),
    );
    scanner.epic_manifests(
        &options
            .epic_manifest_dirs
            .iter()
            .chain(&common.epic_manifests)
            .cloned()
            .collect::<Vec<_>>(),
    );
    scanner.legendary(
        &options
            .legendary_installed_files
            .iter()
            .chain(&common.legendary_installed_files)
            .cloned()
            .collect::<Vec<_>>(),
    );
    for root in &options.user_roots {
        scanner.tree(root, DiscoveryKind::User, options.max_depth);
    }
    for root in options.epic_roots.iter().chain(&common.epic) {
        scanner.tree(root, DiscoveryKind::Epic, options.max_depth);
    }
    for root in &common.game_instances {
        scanner.probe(root, DiscoverySource::simple(DiscoveryKind::Common));
    }
    scanner.report.directories_checked = scanner.probes.len();
    for (root, sources) in &scanner.probes {
        let mut candidate = identify_at(root, scanner.adapters, sources.clone())?;
        if candidate.matches.is_empty() && !sources.iter().any(|s| s.metadata_path.is_some()) {
            continue;
        }
        candidate.registered_game_id = existing.get(&candidate.root_path).cloned();
        scanner.report.candidates.push(candidate);
    }
    scanner.report.candidates.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.root_path.cmp(&b.root_path))
    });
    Ok(scanner.report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(temp: &tempfile::TempDir) -> Db {
        Db::open(&temp.path().join("manager/db.sqlite")).unwrap()
    }
    fn make_file(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"fixture").unwrap();
    }
    fn skyrim(root: &Path) {
        make_file(root, "SkyrimSE.exe");
        make_file(root, "Data/Skyrim.esm");
    }
    fn scan_options() -> ScanOptions {
        ScanOptions {
            include_common_roots: false,
            ..Default::default()
        }
    }
    fn acf(id: &str, directory: &str) -> String {
        format!("\"AppState\" {{ \"appid\" \"{id}\" \"name\" \"A game\" \"installdir\" \"{directory}\" \"StateFlags\" \"4\" }}")
    }

    #[test]
    fn parses_nested_acf_comments_unicode_and_escaped_strings() {
        let input = r#"// Header
        "AppState" {
          "appid" "489830" "name" "天际 \"Special\"" "installdir" "Skyrim Special Edition"
          "StateFlags" "4" /* block comment */
          "UserConfig" { "appid" "999" "name" "wrong" "installdir" "wrong" }
          "InstalledDepots" { "489831" { "manifest" "18446744073709551615" } }
        }"#;
        let result = parse_appmanifest(input).unwrap();
        assert_eq!(result.app_id, 489830);
        assert_eq!(result.name, "天际 \"Special\"");
        assert_eq!(result.install_dir, "Skyrim Special Edition");
        assert_eq!(result.state_flags, Some(4));
    }

    #[test]
    fn parses_modern_and_legacy_library_folders_without_nested_path_confusion() {
        let paths = parse_library_folders(r#""libraryfolders" { "TimeNextStatsReport" "1" "0" { "path" "C:\\Games\\Steam" "apps" { "489830" "0" } } "1" "/mnt/Steam Library" }"#).unwrap();
        assert!(paths.contains(&"C:\\Games\\Steam".to_string()));
        assert!(paths.contains(&"/mnt/Steam Library".to_string()));
        assert!(parse_library_folders(r#"libraryfolders { 0 { apps { path /wrong } } }"#).is_err());
    }

    #[test]
    fn kv_parser_rejects_truncation_duplicates_directives_and_resource_exhaustion() {
        for input in [
            "AppState {",
            "AppState }",
            "AppState { x }",
            "AppState { x \"unterminated }",
            "AppState { x a X b }",
            "#include \"other.acf\"",
            "AppState {} [$WINDOWS]",
            "/* unclosed",
            "AppState { x a } }",
        ] {
            assert!(parse_kv(input).is_err(), "{input}");
        }
        assert!(parse_kv(&format!(
            "{}{}",
            "a {".repeat(MAX_KV_DEPTH + 1),
            "}".repeat(MAX_KV_DEPTH + 1)
        ))
        .is_err());
        assert!(parse_kv(&format!("x \"{}\"", "a".repeat(MAX_KV_STRING + 1))).is_err());
        assert!(parse_kv(&" ".repeat(MAX_METADATA_BYTES + 1)).is_err());
        assert!(parse_kv(&format!("x {}", "\0")).is_err());
        let many = (0..MAX_KV_TOKENS / 2 + 1)
            .map(|i| format!("k{i} v "))
            .collect::<String>();
        assert!(parse_kv(&many).is_err());
    }

    #[test]
    fn manifest_install_directory_and_identity_are_validated_structurally() {
        for directory in [
            "../escape",
            "/absolute",
            "C:/escape",
            "Data/Other",
            "Data\\\\Other",
            ".",
            "NUL",
        ] {
            assert!(
                parse_appmanifest(&acf("489830", directory)).is_err(),
                "{directory}"
            );
        }
        for id in ["0", "01", "-1", "not-an-id"] {
            assert!(parse_appmanifest(&acf(id, "Skyrim")).is_err());
        }
        assert!(parse_appmanifest(
            r#"AppState { UserConfig { appid 489830 name Skyrim installdir Skyrim } }"#
        )
        .is_err());
        parse_appmanifest(&format!("\u{feff}{}", acf("489830", "Skyrim"))).unwrap();
    }

    #[test]
    fn distinctive_required_markers_beat_generic_or_partial_layouts() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let game = temp.path().join("unrelated-folder-name");
        skyrim(&game);
        let result = identify_game(&db, &game).unwrap();
        assert_eq!(result.status, IdentificationStatus::HighConfidence);
        assert_eq!(result.suggested_adapter_slug.as_deref(), Some("skyrim-se"));
        assert!(result.requires_confirmation);
        fs::remove_file(game.join("Data/Skyrim.esm")).unwrap();
        assert_eq!(
            identify_game(&db, &game).unwrap().status,
            IdentificationStatus::Candidate
        );
        let generic = temp.path().join("Skyrim Special Edition");
        fs::create_dir_all(generic.join("Data")).unwrap();
        let result = identify_game(&db, &generic).unwrap();
        assert_ne!(result.status, IdentificationStatus::HighConfidence);
        assert!(result
            .matches
            .iter()
            .all(|m| m.confidence < HIGH_CONFIDENCE));
        let empty = temp.path().join("Minecraft");
        fs::create_dir(&empty).unwrap();
        assert_eq!(
            identify_game(&db, &empty).unwrap().status,
            IdentificationStatus::Unidentified
        );
    }

    #[test]
    fn all_six_adapter_layouts_are_identified_without_claiming_native_runtime_support() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        for package in plugins::builtin_packages().unwrap() {
            let adapter = package.adapter;
            let root = temp.path().join(&adapter.slug);
            fs::create_dir_all(&root).unwrap();
            for marker in &adapter.markers {
                match marker.kind {
                    MarkerKind::File => make_file(&root, &marker.path),
                    MarkerKind::Directory => fs::create_dir_all(root.join(&marker.path)).unwrap(),
                }
            }
            let result = identify_game(&db, &root).unwrap();
            assert_eq!(
                result.suggested_adapter_slug.as_deref(),
                Some(adapter.slug.as_str())
            );
            assert!(result.requires_confirmation);
            if adapter.slug == "minecraft-java" {
                assert_eq!(result.status, IdentificationStatus::Candidate);
            } else {
                assert_eq!(result.status, IdentificationStatus::HighConfidence);
            }
        }
    }

    #[test]
    fn tied_distinctive_adapters_remain_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        skyrim(&root);
        let root = root.canonicalize().unwrap();
        let first = plugins::builtin_packages().unwrap().remove(1).adapter;
        let mut second = first.clone();
        second.slug = "other-layout".into();
        let result = identify_at(&root, &[first, second], vec![]).unwrap();
        assert_eq!(result.status, IdentificationStatus::Candidate);
        assert_eq!(result.matches.len(), 2);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn minecraft_generic_markers_stay_candidates_until_explicit_identity_selection() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let game = temp.path().join("instance");
        make_file(&game, "options.txt");
        fs::create_dir_all(game.join("versions")).unwrap();
        fs::create_dir_all(game.join("saves")).unwrap();
        assert_eq!(
            identify_game(&db, &game).unwrap().status,
            IdentificationStatus::Candidate
        );
        let registered = register_game(&db, &game, None).unwrap();
        assert_eq!(registered.adapter_slug, None);
        let selected = register_game(&db, &game, Some("minecraft-java")).unwrap();
        assert_eq!(registered.id, selected.id);
        assert_eq!(selected.adapter_slug.as_deref(), Some("minecraft-java"));
        assert!(selected.confidence < HIGH_CONFIDENCE);
    }

    #[test]
    fn explicit_registration_is_idempotent_and_preserves_install_state() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let game = temp.path().join("game");
        skyrim(&game);
        let first = register_game(&db, &game, None).unwrap();
        db.conn().execute("UPDATE games SET installed_mod_count=7,enabled_mod_count=5,cover_path='cover.png',metadata_json=json_set(metadata_json,'$.custom','keep') WHERE id=?1", [&first.id]).unwrap();
        fs::remove_file(game.join("SkyrimSE.exe")).unwrap();
        let again = register_game(&db, &game.join("Data/.."), None).unwrap();
        assert_eq!(first.id, again.id);
        assert_eq!(first.created_at, again.created_at);
        assert_eq!(again.installed_mod_count, 7);
        assert_eq!(again.enabled_mod_count, 5);
        assert_eq!(again.cover_path.as_deref(), Some("cover.png"));
        assert_eq!(again.adapter_slug.as_deref(), Some("skyrim-se"));
        assert_eq!(again.metadata["custom"], "keep");
        assert_eq!(
            get_game(&db, &first.id).unwrap().root_path,
            game.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(list_games(&db).unwrap().len(), 1);
        assert!(get_game(&db, "missing").is_err());
        assert!(register_game(&db, Path::new("relative"), None).is_err());
        assert!(register_game(&db, &temp.path().join("not-created"), None).is_err());
    }

    #[test]
    fn scan_follows_secondary_steam_library_but_does_not_register_or_write_games() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let second = temp.path().join("Other Library");
        let game = second.join("steamapps/common/Actual Name");
        skyrim(&game);
        make_file(&steam, "steamapps/common/.keep");
        let path = serde_json::to_string(second.to_str().unwrap()).unwrap();
        fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!("libraryfolders {{ 1 {{ path {path} apps {{ 489830 0 }} }} }}"),
        )
        .unwrap();
        fs::write(
            second.join("steamapps/appmanifest_489830.acf"),
            acf("489830", "Actual Name"),
        )
        .unwrap();
        let options = ScanOptions {
            steam_roots: vec![steam],
            ..scan_options()
        };
        let result = scan_games(&db, &options).unwrap();
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].suggested_adapter_slug.as_deref(),
            Some("skyrim-se")
        );
        assert!(result.candidates[0].requires_confirmation);
        assert_eq!(
            result.candidates[0].sources[0].steam_app_id.as_deref(),
            Some("489830")
        );
        assert!(list_games(&db).unwrap().is_empty());
        assert_eq!(fs::read(game.join("SkyrimSE.exe")).unwrap(), b"fixture");
        assert!(!game.join("mods").exists());
    }

    #[test]
    fn corrupt_or_mismatched_steam_manifest_does_not_hide_other_games() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let common = steam.join("steamapps/common");
        skyrim(&common.join("Skyrim"));
        fs::write(
            steam.join("steamapps/appmanifest_489830.acf"),
            acf("489830", "Skyrim"),
        )
        .unwrap();
        fs::write(
            steam.join("steamapps/appmanifest_377160.acf"),
            acf("489830", "Skyrim"),
        )
        .unwrap();
        fs::write(
            steam.join("steamapps/appmanifest_1086940.acf"),
            "AppState {",
        )
        .unwrap();
        let result = scan_games(
            &db,
            &ScanOptions {
                steam_roots: vec![steam],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.warnings.len(), 2);
    }

    #[test]
    fn steam_macos_runtime_is_reported_once_at_the_adapter_root() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let game = steam.join("steamapps/common/Stardew Valley");
        make_file(&game, "Contents/MacOS/Stardew Valley.dll");
        fs::write(
            steam.join("steamapps/appmanifest_413150.acf"),
            acf("413150", "Stardew Valley"),
        )
        .unwrap();
        let report = scan_games(
            &db,
            &ScanOptions {
                steam_roots: vec![steam],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report.candidates[0].root_path,
            game.join("Contents/MacOS")
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
        assert_eq!(
            report.candidates[0].suggested_adapter_slug.as_deref(),
            Some("stardew-valley")
        );
    }

    #[test]
    fn steam_app_id_alone_does_not_confirm_an_empty_install_directory() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let game = steam.join("steamapps/common/Empty");
        fs::create_dir_all(&game).unwrap();
        fs::write(
            steam.join("steamapps/appmanifest_489830.acf"),
            acf("489830", "Empty"),
        )
        .unwrap();
        let report = scan_games(
            &db,
            &ScanOptions {
                steam_roots: vec![steam],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].status, IdentificationStatus::Candidate);
        assert!(report.candidates[0].confidence < HIGH_CONFIDENCE);
        assert!(report.candidates[0].matches[0].matched_markers.is_empty());
        assert!(register_game(&db, &game, None)
            .unwrap()
            .adapter_slug
            .is_none());
    }

    #[test]
    fn epic_item_and_legendary_metadata_are_read_without_running_launch_commands() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let game = temp.path().join("EpicGame");
        make_file(&game, "bin/x64/Cyberpunk2077.exe");
        let manifests = temp.path().join("manifests");
        fs::create_dir(&manifests).unwrap();
        fs::write(manifests.join("game.item"), json!({"InstallLocation":game,"DisplayName":"Cyberpunk 2077","LaunchCommand":"should never run","bIsIncompleteInstall":true}).to_string()).unwrap();
        let legendary = temp.path().join("installed.json");
        fs::write(
            &legendary,
            json!({"app":{"install_path":game,"title":"Cyberpunk 2077"},"broken":{"title":"Missing path"}}).to_string(),
        )
        .unwrap();
        let result = scan_games(
            &db,
            &ScanOptions {
                epic_manifest_dirs: vec![manifests],
                legendary_installed_files: vec![legendary],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(result.candidates[0].sources.len(), 2);
        assert_eq!(result.candidates[0].status, IdentificationStatus::Candidate);
        assert!(result.candidates[0].confidence <= 0.6);
        assert_eq!(result.warnings.len(), 1);
        assert!(list_games(&db).unwrap().is_empty());
    }

    #[test]
    fn user_scan_is_shallow_bounded_deduplicated_and_reports_existing_registrations() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let parent = temp.path().join("games");
        let game = parent.join("Skyrim");
        skyrim(&game);
        skyrim(&parent.join("nested/deeper/hidden"));
        let registered = register_game(&db, &game, None).unwrap();
        let options = ScanOptions {
            user_roots: vec![parent.clone(), game.clone()],
            max_depth: 1,
            ..scan_options()
        };
        let result = scan_games(&db, &options).unwrap();
        assert_eq!(result.candidates.len(), 1);
        assert_eq!(
            result.candidates[0].registered_game_id.as_deref(),
            Some(registered.id.as_str())
        );
        let limited = scan_games(
            &db,
            &ScanOptions {
                max_entries: 1,
                ..options
            },
        )
        .unwrap();
        assert!(limited.truncated);
        assert!(limited.entries_checked <= 1);
        let limited = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![parent.clone()],
                max_directories: 1,
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(limited.directories_checked, 1);
        assert!(limited.truncated);
        assert!(scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![parent],
                max_depth: 4,
                ..scan_options()
            }
        )
        .is_err());
    }

    #[test]
    fn disabled_adapters_do_not_reappear_via_builtin_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let paths = crate::storage::AppPaths::new(temp.path().join("storage")).unwrap();
        plugins::seed_builtins(&db, &paths).unwrap();
        db.conn()
            .execute("UPDATE plugins SET enabled=0", [])
            .unwrap();
        let game = temp.path().join("game");
        skyrim(&game);
        assert_eq!(
            identify_game(&db, &game).unwrap().status,
            IdentificationStatus::Unidentified
        );
        assert!(register_game(&db, &game, Some("skyrim-se")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn marker_and_steam_directory_symlinks_cannot_escape_scan_roots() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let outside = temp.path().join("outside");
        skyrim(&outside);
        let fake = temp.path().join("fake");
        fs::create_dir(&fake).unwrap();
        symlink(outside.join("SkyrimSE.exe"), fake.join("SkyrimSE.exe")).unwrap();
        assert_eq!(
            identify_game(&db, &fake).unwrap().status,
            IdentificationStatus::Unidentified
        );
        let steam = temp.path().join("Steam");
        fs::create_dir_all(steam.join("steamapps/common")).unwrap();
        symlink(&outside, steam.join("steamapps/common/Escape")).unwrap();
        fs::write(
            steam.join("steamapps/appmanifest_489830.acf"),
            acf("489830", "Escape"),
        )
        .unwrap();
        let result = scan_games(
            &db,
            &ScanOptions {
                steam_roots: vec![steam],
                ..scan_options()
            },
        )
        .unwrap();
        assert!(result.candidates.is_empty());
        assert!(!result.warnings.is_empty());
        let alias = temp.path().join("alias");
        symlink(&outside, &alias).unwrap();
        let first = register_game(&db, &alias, None).unwrap();
        assert_eq!(
            first.root_path,
            outside.canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(first.id, register_game(&db, &outside, None).unwrap().id);
    }
}
