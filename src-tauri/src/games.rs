//! Read-only game discovery and explicit registration. Discovery never writes
//! game/launcher files and never adds a filesystem root to the manager's DB.
//! Format references: ValveSoftware/source-sdk-2013 src/tier1/KeyValues.cpp;
//! legendary-gl/legendary legendary/{lfs,models}/egl.py (checked 2026-11-17).
use crate::db::Db;
use crate::error::{AppError, AppResult, NativeMessage};
use crate::paths::{display_path, user_path};
use crate::plugins::{self, GameAdapter, MarkerKind, Platform};
use crate::util;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[path = "games_scan_pages.rs"]
pub(crate) mod pages;

const MAX_METADATA_BYTES: usize = 2 * 1024 * 1024;
const MAX_KV_DEPTH: usize = 32;
const MAX_KV_TOKENS: usize = 65_536;
const MAX_KV_STRING: usize = 65_536;
// Steamworks Common Redistributables and Ubisoft Connect Services, respectively.
const STEAM_NON_GAME_APP_IDS: [u32; 2] = [228980, 4099820];
pub const HIGH_CONFIDENCE: f64 = 0.80;
pub(crate) const AUTOMATIC_GAME_NAME: &str = "automaticName";

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub id: String,
    pub name: String,
    pub slug: String,
    /// The database spelling is also the file engine's trusted root.
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
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
    #[cfg_attr(windows, serde(serialize_with = "serialize_game_metadata"))]
    pub metadata: Value,
    pub created_at: String,
    pub updated_at: String,
}

impl Game {
    pub(crate) fn automatic_adapter_name<'a>(&self, adapter: &'a GameAdapter) -> Option<&'a str> {
        // A missing or mismatched record cannot establish the origin of the current name.
        let automatic = self
            .metadata
            .get(AUTOMATIC_GAME_NAME)
            .and_then(Value::as_str);
        if automatic != Some(self.name.as_str()) {
            return None;
        }
        adapter.game_name.as_deref()
    }
}

const GAME_COLUMNS: &str = "id,name,slug,root_path,platform,adapter_slug,adapter_version,confidence,cover_path,steam_app_id,last_scanned_at,installed_mod_count,enabled_mod_count,metadata_json,created_at,updated_at";

// Older registrations contain canonical paths in this otherwise opaque JSON.
#[cfg(windows)]
fn serialize_game_metadata<S: serde::Serializer>(
    metadata: &Value,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    fn display_field(value: Option<&mut Value>) {
        if let Some(Value::String(text)) = value {
            crate::paths::set_user_path_text(text);
        }
    }
    let mut metadata = metadata.clone();
    display_field(metadata.get_mut("launchPath"));
    if let Some(candidate) = metadata.get_mut("lastIdentification") {
        display_field(candidate.get_mut("rootPath"));
        if let Some(sources) = candidate.get_mut("sources").and_then(Value::as_array_mut) {
            for source in sources {
                display_field(source.get_mut("metadataPath"));
            }
        }
    }
    metadata.serialize(serializer)
}

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
    get_game_from(&db.conn(), id)
}

pub(crate) fn get_game_from(conn: &rusqlite::Connection, id: &str) -> AppResult<Game> {
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warning_details: Vec<NativeMessage>,
}

impl GameCandidate {
    fn add_warning(&mut self, warning: NativeMessage) {
        self.warnings.push(warning.message.clone());
        self.warning_details.push(warning);
    }
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

/// Discovery only uses installed, enabled and validated adapters.
fn active_adapters(db: &Db) -> AppResult<Vec<GameAdapter>> {
    active_adapters_from(&db.conn())
}

fn active_adapters_from(conn: &rusqlite::Connection) -> AppResult<Vec<GameAdapter>> {
    let mut stmt =
        conn.prepare("SELECT slug FROM plugins WHERE enabled AND validated ORDER BY slug")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    rows.map(|slug| plugins::adapter_for_from(conn, &slug?))
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

fn auto_match(best: Option<&AdapterMatch>, ambiguous: bool) -> Option<&AdapterMatch> {
    best.filter(|m| m.confidence >= HIGH_CONFIDENCE && !ambiguous)
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
    let suggested = auto_match(best, ambiguous);
    let status = if best.is_none() {
        IdentificationStatus::Unidentified
    } else if suggested.is_some() {
        IdentificationStatus::HighConfidence
    } else {
        IdentificationStatus::Candidate
    };
    let mut warnings = Vec::new();
    if ambiguous {
        warnings.push(NativeMessage::new("scan.adapter_ambiguous", []));
    }
    if let Some(matched) = suggested {
        if matched.platform != host_platform() {
            warnings.push(NativeMessage::new(
                "scan.foreign_layout",
                [("platform", platform_name(matched.platform).into())],
            ));
        }
    }
    let name = suggested
        .and_then(|matched| adapters.iter().find(|adapter| adapter.slug == matched.slug))
        .map(|adapter| adapter.display_game_name().to_owned())
        .or_else(|| sources.iter().find_map(|s| s.display_name.clone()))
        .unwrap_or_else(|| {
            root.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| display_path(root))
        });
    Ok(GameCandidate {
        root_path: path_text(&user_path(root))?,
        name,
        confidence,
        status,
        suggested_adapter_slug: suggested.map(|m| m.slug.clone()),
        platform: suggested
            .map_or("unknown", |m| platform_name(m.platform))
            .into(),
        matches,
        sources,
        requires_confirmation: true,
        registered_game_id: None,
        warnings: warnings
            .iter()
            .map(|warning| warning.message.clone())
            .collect(),
        warning_details: warnings,
    })
}

/// Probe this directory only. Names alone, generic markers, missing required
/// markers and near-tied matches never produce a high-confidence identity.
pub fn identify_game(db: &Db, directory: &Path) -> AppResult<GameCandidate> {
    let root = canonical_directory(directory)?;
    let mut candidate = identify_at(
        &root,
        &active_adapters(db)?,
        vec![DiscoverySource::simple(DiscoveryKind::User)],
    )?;
    apply_registered_game_name(&db.conn(), &path_text(&root)?, &mut candidate)?;
    Ok(candidate)
}

fn apply_registered_game_name(
    conn: &rusqlite::Connection,
    root: &str,
    candidate: &mut GameCandidate,
) -> AppResult<()> {
    if let Some((id, name)) = conn
        .query_row(
            "SELECT id,name FROM games WHERE root_path=?1",
            [root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
    {
        candidate.registered_game_id = Some(id);
        candidate.name = name;
    }
    Ok(())
}

/// Explicit user-selected root registration. Passing an adapter slug also
/// explicitly selects that identity; otherwise only a unique high-confidence
/// match is attached. Low-confidence roots can be registered without an adapter.
/// Existing install counts, cover, creation time, metadata and identity survive
/// repeated root registration unless a different adapter is explicitly selected.
pub fn register_game(db: &Db, directory: &Path, adapter_slug: Option<&str>) -> AppResult<Game> {
    let selection = adapter_slug.map_or(AdapterSelection::Automatic, AdapterSelection::Explicit);
    register_game_inner(db, directory, selection)
}

/// The reviewed scan can include launcher evidence unavailable to a directory-only probe.
/// Attach an adapter only when both probes suggest the same identity.
pub(crate) fn register_discovered_game(
    db: &Db,
    directory: &Path,
    suggested_adapter_slug: Option<&str>,
) -> AppResult<Game> {
    register_game_inner(
        db,
        directory,
        AdapterSelection::Discovered(suggested_adapter_slug),
    )
}

enum AdapterSelection<'a> {
    Automatic,
    Explicit(&'a str),
    Discovered(Option<&'a str>),
    Catalog {
        slug: &'a str,
        discovery_name: &'a str,
    },
}

/// Evidence and the exact identity to persist for a manual selection. The
/// caller checks the registered root and serializes preview/commit under the
/// write lock, so selecting a package never creates a new registration.
pub(crate) struct GameAdapterSelection {
    pub adapter: GameAdapter,
    pub matched: AdapterMatch,
    pub identification: Value,
}

pub(crate) fn inspect_game_adapter_selection(
    db: &Db,
    root: &Path,
    slug: &str,
) -> AppResult<GameAdapterSelection> {
    // Validate the chosen package directly before checking its disk markers.
    let adapter = plugins::adapter_for(db, slug)?;
    let candidate = identify_at(
        root,
        &active_adapters(db)?,
        vec![DiscoverySource::simple(DiscoveryKind::User)],
    )?;
    let matched = explicit_adapter_match(&candidate, slug)?.clone();
    Ok(GameAdapterSelection {
        adapter,
        matched,
        identification: serde_json::to_value(&candidate)?,
    })
}

impl GameAdapterSelection {
    pub(crate) fn platform(&self) -> &'static str {
        platform_name(self.matched.platform)
    }

    fn already_applied(&self, game: &Game) -> bool {
        game.adapter_slug.as_deref() == Some(&self.adapter.slug)
            && game.adapter_version.as_deref() == Some(&self.adapter.version)
            && game.slug == self.adapter.slug
            && game.platform == self.platform()
            && game.confidence == self.matched.confidence
            && game.steam_app_id == self.adapter.steam_app_id
            && game.metadata["registration"]["kind"] == "explicit"
            // Compare with the same JSON number parsing as metadata_json.
            && game.metadata.get("lastIdentification")
                == serde_json::from_str::<Value>(&self.identification.to_string()).ok().as_ref()
    }
}

pub(crate) fn rebind_game(
    tx: &rusqlite::Transaction<'_>,
    game: Game,
    selected: &GameAdapterSelection,
    requested_name: Option<&str>,
) -> AppResult<Game> {
    let changed_name = requested_name
        .or_else(|| game.automatic_adapter_name(&selected.adapter))
        .filter(|&name| name != game.name);
    let clear_automatic_name =
        requested_name.is_some() && game.metadata.get(AUTOMATIC_GAME_NAME).is_some();
    let already_applied = selected.already_applied(&game);
    if already_applied && changed_name.is_none() && !clear_automatic_name {
        return Ok(game);
    }
    let mut metadata = game.metadata;
    if requested_name.is_some() {
        if let Some(fields) = metadata.as_object_mut() {
            fields.remove(AUTOMATIC_GAME_NAME);
        }
    } else if let Some(name) = changed_name {
        metadata[AUTOMATIC_GAME_NAME] = json!(name);
    }
    if already_applied {
        tx.execute(
            "UPDATE games SET name=COALESCE(?1,name),metadata_json=?2,updated_at=?3 WHERE id=?4",
            params![changed_name, metadata.to_string(), util::now(), game.id],
        )?;
        return get_game_from(tx, &game.id);
    }
    let now = chrono::Utc::now().to_rfc3339();
    update_registration_metadata(&mut metadata, selected.identification.clone(), &now)?;
    tx.execute(
        "UPDATE games SET slug=?1,platform=?2,adapter_slug=?1,adapter_version=?3,confidence=?4,
         steam_app_id=?5,last_scanned_at=?6,metadata_json=?7,updated_at=?6,
         name=COALESCE(?9,name) WHERE id=?8",
        params![
            selected.adapter.slug,
            selected.platform(),
            selected.adapter.version,
            selected.matched.confidence,
            selected.adapter.steam_app_id,
            now,
            metadata.to_string(),
            game.id,
            changed_name,
        ],
    )?;
    get_game_from(tx, &game.id)
}

/// Refresh proven automatic names in the transaction that activates the package.
/// Older registrations lack name provenance; older packages lack a declared title.
pub(crate) fn refresh_bound_game_names(
    tx: &rusqlite::Transaction<'_>,
    adapter: &GameAdapter,
) -> AppResult<()> {
    if adapter.game_name.is_none() {
        return Ok(());
    }
    let name = adapter.display_game_name();
    let key = format!("$.{AUTOMATIC_GAME_NAME}");
    tx.execute(
        "UPDATE games SET name=?1,metadata_json=json_set(metadata_json,?2,?1),updated_at=?3
         WHERE adapter_slug=?4 AND name<>?1 AND CASE WHEN json_valid(metadata_json)
             THEN json_type(metadata_json,?2)='text' AND json_extract(metadata_json,?2)=name
             ELSE 0 END",
        params![name, key, util::now(), adapter.slug],
    )?;
    Ok(())
}

fn explicit_adapter_match<'a>(
    candidate: &'a GameCandidate,
    slug: &str,
) -> AppResult<&'a AdapterMatch> {
    candidate
        .matches
        .iter()
        .find(|m| m.slug == slug && !m.matched_markers.is_empty())
        .ok_or_else(|| invalid("所选适配包没有任何实际匹配的磁盘标记"))
}

fn update_registration_metadata(
    metadata: &mut Value,
    identification: Value,
    now: &str,
) -> AppResult<()> {
    if !metadata.is_object() {
        return Err(invalid("现有游戏 metadata 必须是 JSON 对象"));
    }
    metadata["registration"] = json!({"kind":"explicit", "at":now});
    metadata["lastIdentification"] = identification;
    Ok(())
}

fn register_game_inner(
    db: &Db,
    directory: &Path,
    selection: AdapterSelection<'_>,
) -> AppResult<Game> {
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
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let game = persist_registration(&tx, &root, &adapters, &candidate, selection)?;
    tx.commit()?;
    Ok(game)
}

/// Stored in the release install intent so registration and package activation
/// share its terminal SQLite commit (including rollback on a failed check).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CatalogGameRegistration {
    pub root: PathBuf,
    pub name: String,
}

impl CatalogGameRegistration {
    pub(crate) fn new(
        paths: &crate::storage::AppPaths,
        directory: &Path,
        name: Option<&str>,
    ) -> AppResult<Self> {
        let root = canonical_directory(directory)?;
        if root.parent().is_none() || paths.overlaps_storage(&root) {
            return Err(AppError::Denied(
                "不能登记文件系统根或与管理器数据重叠的游戏目录".into(),
            ));
        }
        let name = name
            .unwrap_or(
                root.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
            )
            .trim();
        if name.is_empty() || name.chars().any(char::is_control) {
            return Err(invalid("游戏名称不能为空或包含控制字符"));
        }
        Ok(Self {
            name: name.to_owned(),
            root,
        })
    }

    pub(crate) fn register(
        &self,
        tx: &rusqlite::Transaction<'_>,
        paths: &crate::storage::AppPaths,
        slug: &str,
    ) -> AppResult<Game> {
        // Re-open the exact canonical directory at commit, not an alias remembered by the UI.
        if canonical_directory(&self.root)? != self.root
            || paths.overlaps_storage(&self.root)
            || self.root.parent().is_none()
        {
            return Err(AppError::Conflict("游戏目录已变更，请重新扫描".into()));
        }
        let adapters = active_adapters_from(tx)?;
        let candidate = identify_at(
            &self.root,
            &adapters,
            vec![DiscoverySource::simple(DiscoveryKind::User)],
        )?;
        persist_registration(
            tx,
            &self.root,
            &adapters,
            &candidate,
            AdapterSelection::Catalog {
                slug,
                discovery_name: &self.name,
            },
        )
    }
}

fn persist_registration(
    tx: &rusqlite::Transaction<'_>,
    root: &Path,
    adapters: &[GameAdapter],
    candidate: &GameCandidate,
    selection: AdapterSelection<'_>,
) -> AppResult<Game> {
    let prepared = plan_registration(tx, root, adapters, candidate, selection, None)?;
    if prepared.already_registered {
        return Ok(prepared.game);
    }
    write_registration(tx, &prepared.game, true)
}

struct RegistrationWrite {
    game: Game,
    already_registered: bool,
}

/// The Agent supplies the reviewed ID and may only create a new registration.
/// It uses the same automatic selection and metadata construction as desktop.
pub(crate) fn prepare_new_registration(
    conn: &rusqlite::Connection,
    root: &Path,
    new_id: &str,
) -> AppResult<(Game, Option<GameAdapter>)> {
    let adapters = active_adapters_from(conn)?;
    let candidate = identify_at(
        root,
        &adapters,
        vec![DiscoverySource::simple(DiscoveryKind::User)],
    )?;
    let game = plan_registration(
        conn,
        root,
        &adapters,
        &candidate,
        AdapterSelection::Automatic,
        Some(new_id),
    )?
    .game;
    let adapter = adapters
        .into_iter()
        .find(|adapter| Some(adapter.slug.as_str()) == game.adapter_slug.as_deref());
    Ok((game, adapter))
}

pub(crate) fn apply_new_registration(
    tx: &rusqlite::Transaction<'_>,
    game: &Game,
) -> AppResult<Game> {
    write_registration(tx, game, false)
}

fn plan_registration(
    conn: &rusqlite::Connection,
    root: &Path,
    adapters: &[GameAdapter],
    candidate: &GameCandidate,
    selection: AdapterSelection<'_>,
    new_id: Option<&str>,
) -> AppResult<RegistrationWrite> {
    let suggested = candidate.suggested_adapter_slug.as_deref();
    let selected_slug = match selection {
        AdapterSelection::Automatic => suggested,
        AdapterSelection::Explicit(slug) => Some(slug),
        AdapterSelection::Catalog { slug, .. } => {
            if suggested != Some(slug) {
                return Err(AppError::Conflict(
                    "游戏目录的磁盘标记未能唯一确认此云端适配包，未自动安装或绑定".into(),
                ));
            }
            Some(slug)
        }
        AdapterSelection::Discovered(reviewed) => suggested.filter(|slug| Some(*slug) == reviewed),
    };
    let selected = if let Some(slug) = selected_slug {
        plugins::validate_slug(slug)?;
        adapters
            .iter()
            .find(|a| a.slug == slug)
            .ok_or_else(|| AppError::NotFound(format!("未启用的适配包: {slug}")))?;
        Some(explicit_adapter_match(candidate, slug)?)
    } else {
        None
    };
    let root_text = path_text(root)?;
    let now = chrono::Utc::now().to_rfc3339();
    let existing = conn
        .query_row(
            &format!("SELECT {GAME_COLUMNS} FROM games WHERE root_path=?1"),
            [&root_text],
            game_from_row,
        )
        .optional()?;
    if new_id.is_some() && existing.is_some() {
        return Err(AppError::Conflict(
            "该目录已登记，请使用现有游戏记录".into(),
        ));
    }
    if let (AdapterSelection::Catalog { slug, .. }, Some(game)) = (&selection, &existing) {
        if game.adapter_slug.as_deref() == Some(*slug)
            && game.adapter_version.as_deref() == selected.map(|m| m.version.as_str())
        {
            return Ok(RegistrationWrite {
                game: game.clone(),
                already_registered: true,
            });
        }
        return Err(AppError::Conflict(
            "该目录已在游戏库中，不会更改其适配包".into(),
        ));
    }
    let retain_identity = !matches!(selection, AdapterSelection::Explicit(_))
        && existing.as_ref().is_some_and(|g| g.adapter_slug.is_some());
    let old = existing.as_ref();
    let adapter = selected.and_then(|m| adapters.iter().find(|a| a.slug == m.slug));
    let slug = if retain_identity {
        old.unwrap().slug.clone()
    } else {
        adapter.map_or_else(|| "unknown".into(), |a| a.slug.clone())
    };
    let automatic_name = match old {
        Some(game) if !retain_identity => adapter.and_then(|a| game.automatic_adapter_name(a)),
        Some(_) => None,
        None => Some(match (selection, adapter) {
            (AdapterSelection::Catalog { discovery_name, .. }, Some(a))
                if a.game_name.is_none() =>
            {
                discovery_name
            }
            (_, Some(a)) => a.display_game_name(),
            (_, None) => root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&root_text),
        }),
    };
    let name = automatic_name
        .or_else(|| old.map(|g| g.name.as_str()))
        .unwrap();
    let id = old
        .map(|g| g.id.clone())
        .or_else(|| new_id.map(str::to_owned))
        .unwrap_or_else(|| util::new_id("game"));
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
    update_registration_metadata(&mut metadata, serde_json::to_value(candidate)?, &now)?;
    if let Some(name) = automatic_name {
        metadata[AUTOMATIC_GAME_NAME] = json!(name);
    }
    Ok(RegistrationWrite {
        game: Game {
            id,
            name: name.to_owned(),
            slug,
            root_path: root_text,
            platform,
            adapter_slug,
            adapter_version: version,
            confidence,
            cover_path: old.and_then(|game| game.cover_path.clone()),
            steam_app_id: app_id,
            last_scanned_at: Some(now.clone()),
            installed_mod_count: old.map_or(0, |game| game.installed_mod_count),
            enabled_mod_count: old.map_or(0, |game| game.enabled_mod_count),
            metadata,
            created_at: old.map_or_else(|| now.clone(), |game| game.created_at.clone()),
            updated_at: now,
        },
        already_registered: false,
    })
}

fn write_registration(
    tx: &rusqlite::Transaction<'_>,
    game: &Game,
    allow_refresh: bool,
) -> AppResult<Game> {
    const INSERT: &str = "INSERT INTO games (id,name,slug,root_path,platform,adapter_slug,adapter_version,confidence,steam_app_id,last_scanned_at,metadata_json,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)";
    const REFRESH: &str = " ON CONFLICT(root_path) DO UPDATE SET name=excluded.name,slug=excluded.slug,platform=excluded.platform,adapter_slug=excluded.adapter_slug,adapter_version=excluded.adapter_version,confidence=excluded.confidence,steam_app_id=excluded.steam_app_id,last_scanned_at=excluded.last_scanned_at,metadata_json=excluded.metadata_json,updated_at=excluded.updated_at";
    let sql = if allow_refresh {
        std::borrow::Cow::Owned(format!("{INSERT}{REFRESH}"))
    } else {
        std::borrow::Cow::Borrowed(INSERT)
    };
    tx.execute(
        &sql,
        params![
            game.id,
            game.name,
            game.slug,
            game.root_path,
            game.platform,
            game.adapter_slug,
            game.adapter_version,
            game.confidence,
            game.steam_app_id,
            game.last_scanned_at,
            game.metadata.to_string(),
            game.created_at,
            game.updated_at
        ],
    )?;
    Ok(tx.query_row(
        &format!("SELECT {GAME_COLUMNS} FROM games WHERE root_path=?1"),
        [&game.root_path],
        game_from_row,
    )?)
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
    let fields = metadata
        .as_object_mut()
        .ok_or_else(|| invalid("Existing game metadata must be an object"))?;
    let now = chrono::Utc::now().to_rfc3339();
    fields.remove(AUTOMATIC_GAME_NAME);
    fields.insert("onlineIdentity".into(), evidence);
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

fn read_steam_appmanifest(file: &Path, expected_id: u32) -> AppResult<SteamManifest> {
    let manifest = parse_appmanifest(&read_metadata(file)?)?;
    if manifest.app_id != expected_id {
        return Err(invalid("manifest 文件名与 AppState App ID 不一致"));
    }
    Ok(manifest)
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
    /// Root itself is depth 0; omitted depth scans explicitly selected trees.
    pub max_depth: usize,
    pub max_directories: usize,
    pub max_entries: usize,
    pub page: pages::PageRequest,
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
            max_depth: usize::MAX,
            max_directories: usize::MAX,
            max_entries: usize::MAX,
            page: pages::PageRequest::default(),
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
    #[serde(flatten)]
    pub presentation: NativeMessage,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub candidates: Vec<GameCandidate>,
    pub warnings: Vec<ScanWarning>,
    pub directories_checked: usize,
    pub entries_checked: usize,
    pub truncated: bool,
    pub next_cursor: Option<uuid::Uuid>,
}

fn read_metadata(path: &Path) -> AppResult<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_METADATA_BYTES as u64 {
        return Err(invalid("启动器元数据须为大小受限的普通文件"));
    }
    let file = crate::safe_fs::open_regular(path)?;
    let opened = file.metadata()?;
    if opened.len() > MAX_METADATA_BYTES as u64 {
        return Err(invalid("启动器元数据大小超限"));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_METADATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    crate::safe_fs::verify_regular_at(&file, path)?;
    let current = file.metadata()?;
    if opened.len() != current.len() || opened.modified()? != current.modified()? {
        return Err(AppError::Conflict("启动器元数据在读取期间变化".into()));
    }
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(invalid("启动器元数据大小超限"));
    }
    String::from_utf8(bytes).map_err(|_| invalid("启动器元数据须为 UTF-8"))
}

struct Scanner<'a> {
    adapters: &'a [GameAdapter],
    options: &'a ScanOptions,
    report: ScanReport,
    spool: rusqlite::Connection,
    cancel: &'a tokio_util::sync::CancellationToken,
}

// Filename evidence for an unadapted installation, not an adapter match or a launch target.
fn install_hint(root: &Path, name: &std::ffi::OsStr, directory: bool) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    if directory {
        return name == ".egstore"
            || (name == "contents"
                && root
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("app")));
    }
    matches!(
        name.as_str(),
        "unityplayer.dll" | "unityplayer.so" | "project.godot" | "steam_appid.txt"
    ) || (name.starts_with("goggame-") && name.ends_with(".info"))
        || name.ends_with(".x86_64")
        || (name.ends_with(".exe")
            && ![
                "setup",
                "install",
                "unins",
                "vc_redist",
                "dxsetup",
                "crashpad",
                "crashreport",
                "unitycrash",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix)))
}
impl Scanner<'_> {
    fn warn(&mut self, path: &Path, presentation: NativeMessage) {
        // Warnings are also bounded if a directory contains many broken files.
        if self.report.warnings.len() < 256 {
            self.report.warnings.push(ScanWarning {
                path: display_path(path),
                presentation,
            });
        }
    }
    fn warn_error(&mut self, path: &Path, error: impl Into<AppError>) {
        self.warn(path, error.into().presentation());
    }

    fn limit(&mut self, path: &Path) {
        self.report.truncated = true;
        self.warn(path, NativeMessage::new("scan.limit", []));
    }
    fn checkpoint(&self) -> AppResult<()> {
        if self.cancel.is_cancelled() {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn probe(&mut self, path: &Path, source: DiscoverySource) -> AppResult<()> {
        self.checkpoint()?;
        match canonical_directory(path) {
            Ok(root) => {
                self.record_probe(&root, source)?;
            }
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                if source.metadata_path.is_some() {
                    self.warn(
                        path,
                        NativeMessage::new("scan.launcher_directory_missing", []),
                    );
                }
            }
            Err(e) => self.warn_error(path, e),
        }
        Ok(())
    }

    fn record_probe(&mut self, root: &Path, source: DiscoverySource) -> AppResult<bool> {
        let root_text = root
            .to_str()
            .ok_or_else(|| invalid("扫描目录不是 Unicode 路径"))?;
        if self.report.directories_checked >= self.options.max_directories {
            let found: bool = self.spool.query_row(
                "SELECT EXISTS(SELECT 1 FROM probes WHERE root = ?1)",
                [root_text],
                |row| row.get(0),
            )?;
            if !found {
                self.limit(root);
                return Ok(false);
            }
        }
        self.report.directories_checked += self.spool.execute(
            "INSERT OR IGNORE INTO probes(root) VALUES (?1)",
            [root_text],
        )?;
        self.spool.execute(
            "INSERT OR IGNORE INTO sources(root, source) VALUES (?1, ?2)",
            params![root_text, serde_json::to_string(&source)?],
        )?;
        Ok(true)
    }

    fn count_entry(&mut self, path: &Path) -> AppResult<bool> {
        self.checkpoint()?;
        if self.report.entries_checked >= self.options.max_entries {
            self.limit(path);
            return Ok(false);
        }
        self.report.entries_checked += 1;
        Ok(true)
    }

    fn entries(&mut self, path: &Path) -> Option<fs::ReadDir> {
        match fs::read_dir(path) {
            Ok(entries) => Some(entries),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                self.warn_error(path, e);
                None
            }
        }
    }

    fn is_steam_non_game_directory(&self, root: &Path) -> AppResult<bool> {
        let named = |path: &Path, name: &str| {
            path.file_name()
                .is_some_and(|file_name| file_name.eq_ignore_ascii_case(name))
        };
        let Some(common) = root.parent().filter(|p| named(p, "common")) else {
            return Ok(false);
        };
        let Some(apps) = common.parent().filter(|p| named(p, "steamapps")) else {
            return Ok(false);
        };

        // An explicit tree scan may run without Steam discovery. Use the sibling
        // manifest to avoid guessing from an installation directory's name.
        let cached: Option<String> = self
            .spool
            .query_row(
                "SELECT roots FROM steam_non_game_dirs WHERE common = ?1",
                [common.to_str()],
                |row| row.get(0),
            )
            .optional()?;
        let excluded: Vec<String> = match cached {
            Some(roots) => serde_json::from_str(&roots)?,
            None => {
                let roots: Vec<String> = STEAM_NON_GAME_APP_IDS
                    .into_iter()
                    .filter_map(|id| {
                        let file = apps.join(format!("appmanifest_{id}.acf"));
                        let manifest = read_steam_appmanifest(&file, id).ok()?;
                        let path = common.join(&manifest.install_dir);
                        // A link or junction must not mark a different app's directory.
                        crate::safe_fs::identity(&path).ok().flatten()?;
                        let path = canonical_directory(&path).ok()?;
                        (path.parent() == Some(common)).then_some(())?;
                        path.to_str().map(str::to_owned)
                    })
                    .collect();
                self.spool.execute(
                    "INSERT INTO steam_non_game_dirs(common, roots) VALUES (?1, ?2)",
                    params![common.to_str(), serde_json::to_string(&roots)?],
                )?;
                roots
            }
        };
        Ok(excluded.iter().any(|path| Path::new(path) == root))
    }

    fn trees<'a>(
        &mut self,
        roots: impl Iterator<Item = (&'a PathBuf, usize)>,
        kind: DiscoveryKind,
    ) -> AppResult<()> {
        self.spool.execute("DELETE FROM queue", [])?;
        // Start with all selected roots; each receives its own requested depth.
        for (path, depth) in roots {
            self.checkpoint()?;
            match canonical_directory(path) {
                Ok(path) => {
                    let depth = i64::try_from(depth).unwrap_or(i64::MAX);
                    self.spool.execute("INSERT INTO queue(path,level,remaining) VALUES (?1,0,?2)
                        ON CONFLICT(path) DO UPDATE SET remaining=MAX(remaining,excluded.remaining)",
                        params![path.to_str(),depth])?;
                }
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    if kind == DiscoveryKind::User {
                        self.warn(
                            path,
                            NativeMessage::new("scan.selected_directory_missing", []),
                        );
                    }
                }
                Err(e) => self.warn_error(path, e),
            }
        }
        while let Some((id, path, level, remaining)) = self
            .spool
            .query_row(
                "SELECT id,path,level,remaining FROM queue WHERE pending=1 ORDER BY id LIMIT 1",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?
        {
            self.checkpoint()?;
            self.spool
                .execute("UPDATE queue SET pending=0 WHERE id=?1", [id])?;
            let root = Path::new(&path);
            match canonical_directory(root) {
                Ok(current) if current == root => {}
                Ok(_) => {
                    self.warn(root, NativeMessage::new("scan.directory_replaced", []));
                    continue;
                }
                Err(e) => {
                    self.warn_error(root, e);
                    continue;
                }
            }
            if self.is_steam_non_game_directory(root)?
                || !self.record_probe(root, DiscoverySource::simple(kind.clone()))?
            {
                continue;
            }
            let check_hints = kind == DiscoveryKind::User && level <= 1;
            if remaining == 0 && !check_hints {
                continue;
            }
            let cached: Option<bool> = self
                .spool
                .query_row("SELECT hint FROM listings WHERE root=?1", [&path], |r| {
                    r.get(0)
                })
                .optional()?;
            let hint = match cached {
                Some(hint) => hint,
                None => {
                    let mut hint = false;
                    let entries = crate::safe_fs::identity(root).and_then(|id| {
                        let id = id.ok_or_else(|| invalid("扫描目录不存在"))?;
                        crate::safe_fs::directory_entries(root, &id)
                    });
                    match entries {
                        Ok(entries) => {
                            for entry in entries {
                                if !self.count_entry(root)? {
                                    break;
                                }
                                let (name, directory) = match entry {
                                    Ok(entry) => entry,
                                    Err(e) => {
                                        self.warn_error(root, e);
                                        continue;
                                    }
                                };
                                if install_hint(root, &name, directory)
                                    && (directory
                                        || fs::symlink_metadata(root.join(&name))
                                            .is_ok_and(|metadata| metadata.file_type().is_file()))
                                {
                                    hint = true;
                                }
                                if directory
                                    && !matches!(
                                        name.to_str(),
                                        Some(
                                            ".git"
                                                | ".egstore"
                                                | "node_modules"
                                                | "compatdata"
                                                | "workshop"
                                                | "downloading"
                                        )
                                    )
                                {
                                    if let Some(child) = root.join(name).to_str() {
                                        self.spool.execute("INSERT OR IGNORE INTO children(root,path) VALUES (?1,?2)", params![path,child])?;
                                    }
                                }
                            }
                        }
                        Err(e) => self.warn_error(root, e),
                    }
                    self.spool.execute(
                        "INSERT INTO listings(root,hint) VALUES (?1,?2)",
                        params![path, hint],
                    )?;
                    hint
                }
            };
            if check_hints && hint {
                self.spool
                    .execute("UPDATE probes SET hint=1 WHERE root=?1", [&path])?;
            }
            if remaining > 0 {
                self.spool.execute("INSERT INTO queue(path,level,remaining) SELECT path,?2+1,?3-1 FROM children WHERE root=?1
                    ON CONFLICT(path) DO UPDATE SET remaining=excluded.remaining,pending=1
                    WHERE queue.remaining<excluded.remaining", params![path,level,remaining])?;
            }
        }
        Ok(())
    }
    fn enqueue_steam(&mut self, path: &Path) -> AppResult<()> {
        if let Some(path) = path.to_str() {
            self.spool.execute(
                "INSERT OR IGNORE INTO steam_queue(path) VALUES (?1)",
                [path],
            )?;
        } else {
            self.warn(path, NativeMessage::new("scan.steam_non_unicode", []));
        }
        Ok(())
    }
    fn steam(&mut self, initial: &[PathBuf]) -> AppResult<()> {
        for path in initial {
            self.enqueue_steam(path)?;
        }
        while let Some((id, path)) = self
            .spool
            .query_row(
                "SELECT id,path FROM steam_queue WHERE pending ORDER BY id LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        PathBuf::from(row.get::<_, String>(1)?),
                    ))
                },
            )
            .optional()?
        {
            self.checkpoint()?;
            self.spool
                .execute("UPDATE steam_queue SET pending=0 WHERE id=?1", [id])?;
            let mut library = match canonical_directory(&path) {
                Ok(path) => path,
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn_error(&path, e);
                    continue;
                }
            };
            if library.file_name().is_some_and(|n| n == "steamapps") {
                library.pop();
            }
            if self.spool.execute(
                "INSERT OR IGNORE INTO steam_seen(path) VALUES (?1)",
                [library.to_str()],
            )? == 0
            {
                continue;
            }
            for suffix in ["steamapps/libraryfolders.vdf", "config/libraryfolders.vdf"] {
                let file = library.join(suffix);
                match read_metadata(&file).and_then(|s| parse_library_folders(&s)) {
                    Ok(paths) => {
                        for path in paths {
                            let path = PathBuf::from(path);
                            if path.is_absolute() {
                                self.enqueue_steam(&path)?;
                            } else {
                                self.warn(
                                    &file,
                                    NativeMessage::new("scan.steam_library_not_absolute", []),
                                );
                            }
                        }
                    }
                    Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => self.warn_error(&file, e),
                }
            }
            let apps = library.join("steamapps");
            let common = apps.join("common");
            let common = match canonical_directory(&common) {
                Ok(common) if common.starts_with(&library) => common,
                Ok(_) => {
                    self.warn(&common, NativeMessage::new("scan.steam_common_escape", []));
                    continue;
                }
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn_error(&common, e);
                    continue;
                }
            };
            for entry in self.entries(&apps).into_iter().flatten() {
                if !self.count_entry(&apps)? {
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.warn_error(&apps, e);
                        continue;
                    }
                };
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
                let manifest = match app_id(id).and_then(|id| read_steam_appmanifest(&file, id)) {
                    Ok(m) if STEAM_NON_GAME_APP_IDS.contains(&m.app_id) => continue,
                    Ok(m) => m,
                    Err(e) => {
                        self.warn_error(&file, e);
                        continue;
                    }
                };
                let game = common.join(&manifest.install_dir);
                match game.canonicalize() {
                    Ok(path) if path.starts_with(&common) && path != common => {
                        let source = DiscoverySource {
                            kind: DiscoveryKind::Steam,
                            metadata_path: Some(display_path(&file)),
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
                            self.probe(&runtime, source)?;
                        } else {
                            self.probe(&path, source)?;
                        }
                    }
                    Ok(_) => self.warn(&game, NativeMessage::new("scan.steam_install_escape", [])),
                    Err(e) => self.warn_error(&game, e),
                }
            }
        }
        Ok(())
    }
    fn epic_manifests(&mut self, directories: &[PathBuf]) -> AppResult<()> {
        let mut seen = BTreeSet::new();
        for directory in directories {
            self.checkpoint()?;
            let directory = match canonical_directory(directory) {
                Ok(path) => path,
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn_error(directory, e);
                    continue;
                }
            };
            if !seen.insert(directory.clone()) {
                continue;
            }
            for entry in self.entries(&directory).into_iter().flatten() {
                if !self.count_entry(&directory)? {
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.warn_error(&directory, e);
                        continue;
                    }
                };
                if entry.path().extension().is_none_or(|s| s != "item") {
                    continue;
                }
                let file = entry.path();
                let parsed = read_metadata(&file)
                    .and_then(|s| serde_json::from_str::<EpicManifest>(&s).map_err(AppError::from));
                match parsed {
                    Ok(manifest) => {
                        if let Err(e) = display_text(&manifest.display_name) {
                            self.warn_error(&file, e);
                            continue;
                        }
                        if !manifest.install_location.is_absolute() {
                            self.warn(
                                &file,
                                NativeMessage::new("scan.epic_install_not_absolute", []),
                            );
                            continue;
                        }
                        self.probe(
                            &manifest.install_location,
                            DiscoverySource {
                                kind: DiscoveryKind::Epic,
                                metadata_path: Some(display_path(&file)),
                                display_name: Some(manifest.display_name),
                                steam_app_id: None,
                                complete_install: Some(
                                    !manifest.incomplete && !manifest.needs_validation,
                                ),
                            },
                        )?;
                    }
                    Err(e) => self.warn_error(&file, e),
                }
            }
        }
        Ok(())
    }
    fn legendary(&mut self, files: &[PathBuf]) -> AppResult<()> {
        let mut seen = BTreeSet::new();
        for file in files {
            self.checkpoint()?;
            let canonical = match file.canonicalize() {
                Ok(path) => path,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn_error(file, e);
                    continue;
                }
            };
            if seen.contains(&canonical) {
                continue;
            }
            // Validate the supplied file itself so a link cannot stand in for a manifest.
            let entries = match read_metadata(file).and_then(|s| {
                serde_json::from_str::<BTreeMap<String, Value>>(&s).map_err(AppError::from)
            }) {
                Ok(entries) => entries,
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    self.warn_error(file, e);
                    continue;
                }
            };
            seen.insert(canonical.clone());
            for entry in entries.into_values() {
                if !self.count_entry(file)? {
                    break;
                }
                let entry: LegendaryInstall = match serde_json::from_value(entry) {
                    Ok(entry) => entry,
                    Err(e) => {
                        self.warn_error(file, e);
                        continue;
                    }
                };
                if !entry.install_path.is_absolute() {
                    self.warn(
                        file,
                        NativeMessage::new("scan.legendary_install_not_absolute", []),
                    );
                    continue;
                }
                if let Err(e) = display_text(&entry.title) {
                    self.warn_error(file, e);
                    continue;
                }
                self.probe(
                    &entry.install_path,
                    DiscoverySource {
                        kind: DiscoveryKind::Epic,
                        metadata_path: Some(display_path(&canonical)),
                        display_name: Some(entry.title),
                        steam_app_id: None,
                        complete_install: entry.needs_verification.then_some(false),
                    },
                )?;
            }
        }
        Ok(())
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

static SCAN_PAGES: pages::Sessions<ScanReport> = pages::Sessions::new();

/// Discovery doesn't authorize writes or register any candidate. Each page is bounded.
pub fn scan_games_page(
    db: &Db,
    options: ScanOptions,
    scope: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> AppResult<ScanReport> {
    let page = options.page.clone();
    let db_worker = db.clone();
    let (mut report, cursor) =
        SCAN_PAGES.request(db, scope.into(), &page, cancel, move |producer| {
            scan_with(&db_worker, &options, &producer.cancel, |report| {
                producer.emit(report)
            })
        })?;
    report.next_cursor = cursor;
    Ok(report)
}

fn scan_with(
    db: &Db,
    options: &ScanOptions,
    cancel: &tokio_util::sync::CancellationToken,
    mut emit: impl FnMut(ScanReport) -> AppResult<()>,
) -> AppResult<ScanReport> {
    if options.max_directories == 0 || options.max_entries == 0 {
        return Err(invalid("扫描目录和条目范围必须为正数"));
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
    let common = if options.include_common_roots {
        common_roots()
    } else {
        CommonRoots::default()
    };
    // A private, auto-deleted SQLite file bounds RAM for broad traversals and
    // merges overlapping roots/launcher evidence without retaining the tree.
    let directory = tempfile::tempdir()?;
    let spool = rusqlite::Connection::open(directory.path().join("scan.sqlite"))?;
    spool.execute_batch("PRAGMA cache_size=-2048; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;
        CREATE TABLE probes(root TEXT PRIMARY KEY, hint INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE sources(root TEXT NOT NULL, source TEXT NOT NULL, UNIQUE(root,source));
        CREATE TABLE listings(root TEXT PRIMARY KEY, hint INTEGER NOT NULL);
        CREATE TABLE children(root TEXT NOT NULL, path TEXT NOT NULL, PRIMARY KEY(root,path));
        CREATE TABLE queue(id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, level INTEGER NOT NULL,
                           remaining INTEGER NOT NULL, pending INTEGER NOT NULL DEFAULT 1);
        CREATE INDEX queue_pending ON queue(pending,id);
        CREATE TABLE steam_queue(id INTEGER PRIMARY KEY,path TEXT NOT NULL UNIQUE,pending INTEGER NOT NULL DEFAULT 1);
        CREATE INDEX steam_pending ON steam_queue(pending,id);
        CREATE TABLE steam_seen(path TEXT NOT NULL PRIMARY KEY);
        CREATE TABLE steam_non_game_dirs(common TEXT PRIMARY KEY, roots TEXT NOT NULL);
        BEGIN;")?;
    let mut scanner = Scanner {
        adapters: &adapters,
        options,
        report: ScanReport::default(),
        spool,
        cancel,
    };
    let roots = |explicit: &[PathBuf], defaults: &[PathBuf]| {
        explicit.iter().chain(defaults).cloned().collect::<Vec<_>>()
    };
    scanner.steam(&roots(&options.steam_roots, &common.steam))?;
    scanner.epic_manifests(&roots(&options.epic_manifest_dirs, &common.epic_manifests))?;
    scanner.legendary(&roots(
        &options.legendary_installed_files,
        &common.legendary_installed_files,
    ))?;
    scanner.trees(
        options.user_roots.iter().map(|p| (p, options.max_depth)),
        DiscoveryKind::User,
    )?;
    scanner.trees(
        options
            .epic_roots
            .iter()
            .map(|p| (p, options.max_depth))
            .chain(common.epic.iter().map(|p| {
                // Automatic roots remain targeted; a selected root is an explicit deep-scan request.
                (
                    p,
                    if options.max_depth == usize::MAX {
                        2
                    } else {
                        options.max_depth
                    },
                )
            })),
        DiscoveryKind::Epic,
    )?;
    for root in &common.game_instances {
        scanner.probe(root, DiscoverySource::simple(DiscoveryKind::Common))?;
    }
    let mut roots = scanner
        .spool
        .prepare("SELECT root,hint FROM probes ORDER BY root")?;
    let mut sources = scanner
        .spool
        .prepare("SELECT source FROM sources WHERE root=?1 ORDER BY rowid LIMIT 257")?;
    let mut result_budget = 0;
    for row in roots.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))? {
        scanner.checkpoint()?;
        let (root, hint) = row?;
        let path = Path::new(&root);
        if !canonical_directory(path).is_ok_and(|canonical| canonical == path) {
            continue;
        }
        let mut provenance = sources
            .query_map([&root], |r| r.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str::<DiscoverySource>(&row?)?))
            .collect::<AppResult<Vec<_>>>()?;
        let overflow = provenance.len() > 256;
        provenance.truncate(256);
        let metadata = provenance.iter().any(|s| s.metadata_path.is_some());
        let user = provenance.iter().any(|s| s.kind == DiscoveryKind::User);
        let mut candidate = identify_at(path, scanner.adapters, provenance)?;
        apply_registered_game_name(&db.conn(), &root, &mut candidate)?;
        if candidate.matches.is_empty() && !metadata {
            if hint {
                candidate.add_warning(NativeMessage::new("scan.no_adapter_match", []));
            } else if candidate.registered_game_id.is_none() || !user {
                continue;
            }
        }
        if overflow {
            candidate.add_warning(NativeMessage::new("scan.source_limit", []));
        }
        let bytes = serde_json::to_vec(&candidate)?.len();
        if !scanner.report.candidates.is_empty()
            && (scanner.report.candidates.len() >= 64 || result_budget + bytes > 512 * 1024)
        {
            let mut page = ScanReport {
                candidates: std::mem::take(&mut scanner.report.candidates),
                warnings: std::mem::take(&mut scanner.report.warnings),
                ..scanner.report
            };
            sort_candidates(&mut page.candidates);
            emit(page)?;
            result_budget = 0;
        }
        result_budget += bytes;
        scanner.report.candidates.push(candidate);
    }
    sort_candidates(&mut scanner.report.candidates);
    Ok(scanner.report)
}

fn sort_candidates(candidates: &mut [GameCandidate]) {
    candidates.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.root_path.cmp(&b.root_path))
    });
}

#[cfg(test)]
pub fn scan_games(db: &Db, options: &ScanOptions) -> AppResult<ScanReport> {
    let mut candidates = Vec::new();
    let mut warnings = Vec::new();
    let mut report = scan_with(
        db,
        options,
        &tokio_util::sync::CancellationToken::new(),
        |mut page| {
            candidates.append(&mut page.candidates);
            warnings.append(&mut page.warnings);
            Ok(())
        },
    )?;
    candidates.append(&mut report.candidates);
    warnings.append(&mut report.warnings);
    sort_candidates(&mut candidates);
    report.candidates = candidates;
    report.warnings = warnings;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{plugins::test_fixtures, storage::AppPaths};

    fn db(temp: &tempfile::TempDir) -> Db {
        Db::open(&temp.path().join("manager/db.sqlite")).unwrap()
    }

    fn db_with_adapters(temp: &tempfile::TempDir) -> Db {
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let db = Db::open(&paths.db_file()).unwrap();
        test_fixtures::install(&db, &paths).unwrap();
        db
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

    fn match_with_confidence(confidence: f64) -> AdapterMatch {
        AdapterMatch {
            slug: "fixture-game".into(),
            name: "Fixture Game".into(),
            version: "1.0.0".into(),
            platform: host_platform(),
            confidence,
            matched_markers: vec!["game.exe".into()],
            missing_required_markers: vec![],
            distinctive_file_matched: true,
            reasons: vec![],
        }
    }

    fn bg3(root: &Path) {
        make_file(root, "bin/bg3.exe");
        make_file(root, "bin/bg3_dx11.exe");
        fs::create_dir_all(root.join("Data")).unwrap();
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
        let db = db_with_adapters(&temp);
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
    fn auto_match_requires_at_least_80_percent_and_a_unique_best_match() {
        assert_eq!(HIGH_CONFIDENCE, 0.80);

        let below_threshold = match_with_confidence(0.799_999);
        assert!(auto_match(Some(&below_threshold), false).is_none());

        let at_threshold = match_with_confidence(0.80);
        assert_eq!(
            auto_match(Some(&at_threshold), false).map(|matched| matched.slug.as_str()),
            Some("fixture-game")
        );
        assert!(auto_match(Some(&at_threshold), true).is_none());
        assert!(auto_match(None, false).is_none());
    }

    #[test]
    fn generic_bg3_data_marker_keeps_8_percent_evidence_without_guessing_identity() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let game = temp.path().join("unrelated-folder");
        fs::create_dir_all(game.join("Data")).unwrap();

        let identified = identify_game(&db, &game).unwrap();
        assert_eq!(identified.status, IdentificationStatus::Candidate);
        assert_eq!(identified.name, "unrelated-folder");
        assert_eq!(identified.platform, "unknown");
        assert_eq!(identified.suggested_adapter_slug, None);
        assert!(identified.requires_confirmation);
        assert!((identified.confidence - (0.7 / 9.0)).abs() < 1e-12);
        let weak_match = identified
            .matches
            .iter()
            .find(|matched| matched.slug == "baldurs-gate-3")
            .expect("the raw weak adapter evidence remains available");
        assert!((weak_match.confidence - identified.confidence).abs() < 1e-12);
        assert_eq!(weak_match.matched_markers, ["Data"]);
        assert!(!weak_match.distinctive_file_matched);

        let report = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![game.clone()],
                max_depth: 0,
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        let scanned = &report.candidates[0];
        assert_eq!(scanned.status, IdentificationStatus::Candidate);
        assert_eq!(scanned.name, "unrelated-folder");
        assert_eq!(scanned.platform, "unknown");
        assert_eq!(scanned.suggested_adapter_slug, None);
        assert!((scanned.confidence - identified.confidence).abs() < 1e-12);
        assert_eq!(scanned.matches[0].slug, "baldurs-gate-3");
        assert!(scanned.registered_game_id.is_none());
        assert!(scanned.requires_confirmation);
        assert!(list_games(&db).unwrap().is_empty());
    }

    #[test]
    fn discovered_registration_respects_a_launcher_downgrade() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let steam = temp.path().join("Steam");
        let game = steam.join("steamapps/common/game-root");
        bg3(&game);
        // The files match BG3, but the manifest belongs to a different app.
        fs::write(
            steam.join("steamapps/appmanifest_123.acf"),
            acf("123", "game-root"),
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
        let scanned = &report.candidates[0];
        assert_eq!(scanned.name, "A game");
        assert_eq!(scanned.suggested_adapter_slug, None);
        assert!(scanned.confidence < HIGH_CONFIDENCE);
        assert_eq!(identify_game(&db, &game).unwrap().confidence, 1.0);
        let registered =
            register_discovered_game(&db, &game, scanned.suggested_adapter_slug.as_deref())
                .unwrap();
        assert_eq!(registered.adapter_slug, None);
        assert_eq!(registered.adapter_version, None);
        assert_eq!(registered.confidence, 0.0);
        assert_eq!(registered.slug, "unknown");
        assert_eq!(list_games(&db).unwrap().len(), 1);
    }

    #[test]
    fn discovered_registration_rechecks_a_stale_strong_suggestion_before_binding() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let game = temp.path().join("game-root");
        bg3(&game);

        let report = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![game.clone()],
                max_depth: 0,
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(
            report.candidates[0].suggested_adapter_slug.as_deref(),
            Some("baldurs-gate-3")
        );
        assert_eq!(
            report.candidates[0].status,
            IdentificationStatus::HighConfidence
        );

        fs::remove_file(game.join("bin/bg3_dx11.exe")).unwrap();
        let registered = register_discovered_game(
            &db,
            &game,
            report.candidates[0].suggested_adapter_slug.as_deref(),
        )
        .unwrap();
        assert_eq!(registered.adapter_slug, None);
        assert_eq!(registered.adapter_version, None);
        assert_eq!(registered.confidence, 0.0);
        assert_eq!(registered.slug, "unknown");
    }

    #[test]
    fn all_six_adapter_layouts_are_identified_without_claiming_native_runtime_support() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        for package in test_fixtures::packages().unwrap() {
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
            assert!(result.requires_confirmation);
            if adapter.slug == "minecraft-java" {
                assert_eq!(result.status, IdentificationStatus::Candidate);
                assert_eq!(result.suggested_adapter_slug, None);
                assert_eq!(result.platform, "unknown");
                assert!(result.confidence < HIGH_CONFIDENCE);
                assert!(result
                    .matches
                    .iter()
                    .any(|matched| matched.slug == adapter.slug));
            } else {
                assert_eq!(result.status, IdentificationStatus::HighConfidence);
                assert_eq!(
                    result.suggested_adapter_slug.as_deref(),
                    Some(adapter.slug.as_str())
                );
            }
        }
    }

    #[test]
    fn tied_distinctive_adapters_remain_ambiguous() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("game");
        skyrim(&root);
        let root = root.canonicalize().unwrap();
        let first = test_fixtures::packages().unwrap().remove(1).adapter;
        let mut second = first.clone();
        second.slug = "other-layout".into();
        let result = identify_at(&root, &[first, second], vec![]).unwrap();
        assert_eq!(result.status, IdentificationStatus::Candidate);
        assert_eq!(result.suggested_adapter_slug, None);
        assert_eq!(result.name, "game");
        assert_eq!(result.matches.len(), 2);
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn minecraft_generic_markers_stay_candidates_until_explicit_identity_selection() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
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
        assert_eq!(selected.name, registered.name);
        assert_eq!(selected.adapter_slug.as_deref(), Some("minecraft-java"));
        assert!(selected.confidence < HIGH_CONFIDENCE);
    }

    #[test]
    fn declared_game_name_is_used_for_scan_and_registrations() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let mut adapter = plugins::adapter_for(&db, "skyrim-se").unwrap();
        let package_name = adapter.name.clone();
        adapter.game_name = Some("上古卷轴 V：天际".into());
        db.conn()
            .execute(
                "UPDATE plugins SET adapter_json=?1 WHERE slug=?2",
                params![serde_json::to_string(&adapter).unwrap(), adapter.slug],
            )
            .unwrap();
        let name = adapter.display_game_name();
        let direct = temp.path().join("different-directory-name");
        let discovered = temp.path().join("another-directory-name");
        skyrim(&direct);
        skyrim(&discovered);

        let identified = identify_game(&db, &direct).unwrap();
        assert_eq!(identified.name, name);
        assert_eq!(identified.matches[0].name, package_name);
        let report = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![discovered.clone()],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates[0].name, name);

        let manual = register_game(&db, &direct, None).unwrap();
        let scanned = register_discovered_game(
            &db,
            &discovered,
            report.candidates[0].suggested_adapter_slug.as_deref(),
        )
        .unwrap();
        for game in [manual, scanned] {
            assert_eq!(game.name, name);
            assert_eq!(game.metadata[AUTOMATIC_GAME_NAME], name);
        }
    }

    #[test]
    fn refreshing_bound_names_requires_matching_automatic_name_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let mut adapter = plugins::adapter_for(&db, "skyrim-se").unwrap();
        assert!(adapter.game_name.is_none());
        let [automatic, legacy, edited, unbound] =
            ["auto", "legacy", "edited", "unbound"].map(|folder| {
                let root = temp.path().join(folder);
                skyrim(&root);
                register_game(&db, &root, None).unwrap()
            });
        assert_eq!(automatic.name, adapter.name);
        assert_eq!(legacy.name, automatic.name);
        assert_eq!(legacy.adapter_slug.as_deref(), Some(adapter.slug.as_str()));
        assert_eq!(
            legacy.metadata["lastIdentification"]["suggestedAdapterSlug"],
            adapter.slug
        );
        assert_eq!(
            legacy.metadata["lastIdentification"]["matches"][0]["name"],
            legacy.name
        );
        db.conn().execute(
            "UPDATE games SET metadata_json=json_remove(metadata_json,'$.automaticName') WHERE id=?1",
            [&legacy.id],
        ).unwrap();
        db.conn()
            .execute("UPDATE games SET name='我的天际' WHERE id=?1", [&edited.id])
            .unwrap();
        db.conn()
            .execute(
                "UPDATE games SET adapter_slug=NULL WHERE id=?1",
                [&unbound.id],
            )
            .unwrap();

        let mut conn = db.conn();
        let tx = conn.transaction().unwrap();
        adapter.name = "新版包名称".into();
        refresh_bound_game_names(&tx, &adapter).unwrap();
        assert_eq!(
            get_game_from(&tx, &automatic.id).unwrap().name,
            automatic.name
        );
        adapter.game_name = Some("天际特别版".into());
        refresh_bound_game_names(&tx, &adapter).unwrap();
        tx.commit().unwrap();
        drop(conn);

        let after = get_game(&db, &automatic.id).unwrap();
        assert_eq!(after.name, "天际特别版");
        assert_eq!(after.metadata[AUTOMATIC_GAME_NAME], after.name);
        assert_eq!(
            after.metadata["lastIdentification"],
            automatic.metadata["lastIdentification"]
        );
        assert_eq!(after.adapter_version, automatic.adapter_version);
        assert_eq!(get_game(&db, &legacy.id).unwrap().name, legacy.name);
        assert_eq!(get_game(&db, &edited.id).unwrap().name, "我的天际");
        assert_eq!(get_game(&db, &unbound.id).unwrap().name, unbound.name);
        let identified = identify_game(&db, Path::new(&edited.root_path)).unwrap();
        assert_eq!(identified.name, "我的天际");
        assert_eq!(
            identified.registered_game_id.as_deref(),
            Some(edited.id.as_str())
        );
    }

    #[test]
    fn explicit_registration_is_idempotent_and_preserves_install_state() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
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
        let db = db_with_adapters(&temp);
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
    fn steam_skips_only_the_redistributables_app_id() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let apps = steam.join("steamapps");
        let common = apps.join("common");
        make_file(&common.join("Steamworks Shared"), "runtime.exe");
        make_file(&common.join("Localized Runtime"), "runtime.exe");
        make_file(&common.join("Steamworks Adventure"), "game.exe");
        for (directory, name) in [
            ("Steamworks Shared", "Steamworks Common Redistributables"),
            ("Localized Runtime", "共享运行库"),
        ] {
            fs::write(
                apps.join("appmanifest_228980.acf"),
                acf("228980", directory).replace("A game", name),
            )
            .unwrap();
            fs::write(
                apps.join("appmanifest_123.acf"),
                acf("123", "Steamworks Adventure").replace("A game", "Steamworks Adventure"),
            )
            .unwrap();
            let report = scan_games(
                &db,
                &ScanOptions {
                    steam_roots: vec![steam.clone()],
                    ..scan_options()
                },
            )
            .unwrap();
            assert!(report.warnings.is_empty());
            assert_eq!(report.candidates.len(), 1);
            let source = &report.candidates[0].sources[0];
            assert_eq!(source.steam_app_id.as_deref(), Some("123"));
            assert_eq!(source.display_name.as_deref(), Some("Steamworks Adventure"));
        }
    }

    #[test]
    fn steam_redistributables_are_not_rediscovered_under_a_selected_common_root() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let steam = temp.path().join("Steam");
        let apps = steam.join("steamapps");
        let common = apps.join("common");
        let redist = common.join("Steamworks Shared");
        let other_shared = temp.path().join("Local Games/Steamworks Shared");
        let game = common.join("Steamworks Adventure");
        make_file(&redist, "runtime.exe");
        skyrim(&redist.join("nested"));
        make_file(&other_shared, "game.exe");
        make_file(&game, "game.exe");
        fs::write(
            apps.join("appmanifest_228980.acf"),
            acf("228980", "Steamworks Shared"),
        )
        .unwrap();
        fs::write(
            apps.join("appmanifest_123.acf"),
            acf("123", "Steamworks Adventure"),
        )
        .unwrap();
        let expected =
            [&game, &other_shared].map(|path| display_path(&path.canonicalize().unwrap()));
        for steam_roots in [vec![], vec![steam.clone()]] {
            let report = scan_games(
                &db,
                &ScanOptions {
                    steam_roots,
                    user_roots: vec![common.clone(), other_shared.clone()],
                    ..scan_options()
                },
            )
            .unwrap();
            let candidates: BTreeSet<_> =
                report.candidates.into_iter().map(|c| c.root_path).collect();
            assert_eq!(candidates, BTreeSet::from(expected.clone()));
        }
        let report = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![redist],
                ..scan_options()
            },
        )
        .unwrap();
        assert!(report.candidates.is_empty());
        assert!(list_games(&db).unwrap().is_empty());
    }

    #[test]
    fn steam_redistributables_folder_name_without_matching_metadata_is_not_excluded() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let apps = temp.path().join("Steam/steamapps");
        let common = apps.join("common");
        let game = common.join("Steamworks Shared");
        make_file(&game, "game.exe");
        let options = ScanOptions {
            user_roots: vec![common],
            ..scan_options()
        };
        let assert_game_visible = || {
            let report = scan_games(&db, &options).unwrap();
            assert_eq!(report.candidates.len(), 1);
            assert_eq!(
                report.candidates[0].root_path,
                display_path(&game.canonicalize().unwrap())
            );
        };
        assert_game_visible();
        fs::write(
            apps.join("appmanifest_228980.acf"),
            acf("123", "Steamworks Shared"),
        )
        .unwrap();
        assert_game_visible();
        fs::write(
            apps.join("appmanifest_228980.acf"),
            acf("228980", "Another Directory"),
        )
        .unwrap();
        assert_game_visible();
    }

    #[test]
    fn steam_excludes_ubisoft_connect_services_but_not_similarly_named_games() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let apps = steam.join("steamapps");
        let common = apps.join("common");
        let services = "Ubisoft Connect Services";
        let game = "Ubisoft Connect Services Adventure";
        let game_directory = common.join(game);
        let unrelated = temp.path().join("Local Games").join(services);
        make_file(&common.join(services), "service.exe");
        make_file(&common.join("Steamworks Shared"), "runtime.exe");
        make_file(&game_directory, "game.exe");
        make_file(&unrelated, "game.exe");
        for (id, directory) in [
            ("4099820", services),
            ("228980", "Steamworks Shared"),
            ("123", game),
        ] {
            fs::write(
                apps.join(format!("appmanifest_{id}.acf")),
                acf(id, directory).replace("A game", directory),
            )
            .unwrap();
        }
        let expected =
            [&game_directory, &unrelated].map(|path| display_path(&path.canonicalize().unwrap()));
        for steam_roots in [vec![], vec![steam.clone()]] {
            let report = scan_games(
                &db,
                &ScanOptions {
                    steam_roots,
                    user_roots: vec![common.clone(), unrelated.clone()],
                    ..scan_options()
                },
            )
            .unwrap();
            assert_eq!(report.candidates.len(), 2);
            let actual: BTreeSet<_> = report.candidates.into_iter().map(|c| c.root_path).collect();
            assert_eq!(actual, BTreeSet::from(expected.clone()));
        }
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
            report.candidates[0].sources[0].steam_app_id.as_deref(),
            Some("123")
        );
        assert_eq!(
            report.candidates[0].sources[0].display_name.as_deref(),
            Some(game)
        );
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
        let db = db_with_adapters(&temp);
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
            user_path(&game.join("Contents/MacOS").canonicalize().unwrap())
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
        let db = db_with_adapters(&temp);
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
        let db = db_with_adapters(&temp);
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
    fn chosen_library_lists_adapted_and_unadapted_installs_without_registering() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let root = temp.path().join("Games");
        skyrim(&root.join("Skyrim"));
        make_file(&root, "Portable/StartGame.EXE");
        make_file(&root, "Portable/tools/Helper.exe");
        make_file(&root, "Downloads/setup.exe");
        make_file(&root, "Notes/readme.txt");
        let report = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![root.clone(), root.join("missing")],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 2);
        assert!(report.candidates[0].suggested_adapter_slug.is_some());
        let portable = &report.candidates[1];
        assert_eq!(portable.name, "Portable");
        assert_eq!(portable.status, IdentificationStatus::Unidentified);
        assert!(portable.suggested_adapter_slug.is_none());
        let warning = report
            .warnings
            .iter()
            .find(|w| w.presentation.key == "scan.selected_directory_missing")
            .unwrap();
        let wire = serde_json::to_value(warning).unwrap();
        assert_eq!(wire["path"], warning.path);
        assert_eq!(wire["message"], "所选扫描目录不存在");
        assert_eq!(wire["args"], json!({}));
        assert_eq!(wire["key"], "scan.selected_directory_missing");
        assert_eq!(portable.warning_details[0].key, "scan.no_adapter_match");
        assert_eq!(portable.warning_details[0].message, portable.warnings[0]);
        assert!(list_games(&db).unwrap().is_empty());
    }

    #[test]
    fn user_scan_is_shallow_bounded_deduplicated_and_reports_existing_registrations() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
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
        .unwrap()
        .candidates
        .iter()
        .any(|candidate| candidate.root_path.ends_with("hidden")));
    }

    #[test]
    fn explicit_tree_pages_keep_deep_candidates_without_registering_new_games() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let root = temp.path().join("selected");
        let deep = root.join("one/two/three/four/five/game");
        fs::create_dir_all(&deep).unwrap();
        let existing = register_game(&db, &deep, None).unwrap();
        for n in 0..66 {
            make_file(&root.join(format!("portable-{n:02}")), "game.exe");
        }
        let mut options = ScanOptions {
            user_roots: vec![root],
            ..scan_options()
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let first = scan_games_page(&db, options.clone(), "test", &cancel).unwrap();
        assert_eq!(first.candidates.len(), 64);
        assert!(first
            .candidates
            .iter()
            .any(|row| row.registered_game_id.as_deref() == Some(existing.id.as_str())));
        options.page.cursor = first.next_cursor;
        assert!(options.page.cursor.is_some());
        let second = scan_games_page(&db, options, "test", &cancel).unwrap();
        assert_eq!(second.candidates.len(), 3);
        assert!(second.next_cursor.is_none());
        assert_eq!(list_games(&db).unwrap().len(), 1);
    }

    #[test]
    #[ignore = "large filesystem stress case: creates more than 100,000 entries"]
    fn selected_scan_paginates_more_than_old_depth_and_directory_entry_limits() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let root = temp.path().join("selected");
        fs::create_dir_all(&root).unwrap();
        // At least 100k total entries without creating 100k separate files.
        let broad = root.join("broad");
        fs::create_dir(&broad).unwrap();
        for directory in 0..10_001 {
            let path = broad.join(format!("d{directory:05}"));
            fs::create_dir(&path).unwrap();
            for file in 0..9 {
                fs::write(path.join(format!("x{file}")), []).unwrap();
            }
        }
        let deep = root.join("one/two/three/four/five/six");
        // Unidentified explicit root hints are intentionally limited to level 0/1;
        // an existing path picked as a second root can still be registered/reconciled.
        for n in 0..66 {
            make_file(&deep.join(format!("game-{n:02}")), "game.exe");
        }
        let mut options = ScanOptions {
            user_roots: vec![root, deep],
            ..scan_options()
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let first = scan_games_page(&db, options.clone(), "test", &cancel).unwrap();
        assert!(first.entries_checked > 100_000);
        assert!(first.directories_checked > 10_000);
        assert!(!first.truncated);
        assert_eq!(first.candidates.len(), 64);
        options.page.cursor = first.next_cursor;
        assert!(options.page.cursor.is_some());
        let second = scan_games_page(&db, options, "test", &cancel).unwrap();
        assert_eq!(second.candidates.len(), 2);
        assert!(second.next_cursor.is_none());
        assert!(list_games(&db).unwrap().is_empty());
    }

    #[test]
    fn overlapping_tree_scans_keep_each_roots_depth_and_count_entries_once() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let library = temp.path().join("library");
        let nested = library.join("nested");
        make_file(&nested, "Portable/game.exe");
        let result = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![library.clone(), nested.clone(), library.join(".")],
                epic_roots: vec![nested.clone(), library],
                max_depth: 1,
                max_directories: 3,
                max_entries: 3,
                ..scan_options()
            },
        )
        .unwrap();
        assert!(!result.truncated);
        assert_eq!((result.directories_checked, result.entries_checked), (3, 3));
        assert_eq!(result.candidates.len(), 1);
        let portable = &result.candidates[0];
        assert_eq!(portable.name, "Portable");
        assert_eq!(portable.sources.len(), 2);
        assert!(portable
            .sources
            .iter()
            .any(|s| s.kind == DiscoveryKind::User));
        assert!(portable
            .sources
            .iter()
            .any(|s| s.kind == DiscoveryKind::Epic));
    }

    #[test]
    fn duplicate_launcher_inputs_count_once_and_return_display_metadata_paths() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let manifest_dir = temp.path().join("Manifests");
        let game = temp.path().join("Arcade");
        fs::create_dir_all(&manifest_dir).unwrap();
        fs::create_dir(&game).unwrap();
        let item = manifest_dir.join("Arcade.item");
        fs::write(
            &item,
            json!({"DisplayName":"Arcade", "InstallLocation":game}).to_string(),
        )
        .unwrap();
        let installed = temp.path().join("installed.json");
        fs::write(
            &installed,
            json!({"id":{"title":"Arcade", "install_path":game}}).to_string(),
        )
        .unwrap();
        let result = scan_games(
            &db,
            &ScanOptions {
                epic_manifest_dirs: vec![
                    manifest_dir.clone(),
                    manifest_dir.canonicalize().unwrap(),
                ],
                legendary_installed_files: vec![
                    installed.clone(),
                    installed.canonicalize().unwrap(),
                ],
                max_entries: 2,
                ..scan_options()
            },
        )
        .unwrap();
        assert!(!result.truncated);
        assert_eq!(result.entries_checked, 2);
        assert_eq!(result.candidates.len(), 1);
        let metadata: BTreeSet<_> = result.candidates[0]
            .sources
            .iter()
            .map(|s| s.metadata_path.as_deref().unwrap().to_owned())
            .collect();
        assert_eq!(
            metadata,
            [item, installed]
                .map(|p| display_path(&p.canonicalize().unwrap()))
                .into()
        );
    }

    #[cfg(windows)]
    #[test]
    fn canonical_registration_matches_display_paths_with_native_case_and_separators() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let game = temp.path().join("Arcade");
        fs::create_dir(&game).unwrap();
        let canonical = game.canonicalize().unwrap();
        assert!(canonical.to_str().unwrap().starts_with(r"\\?\"));
        let raw = canonical.to_str().unwrap();
        let display = path_text(&user_path(&canonical)).unwrap();
        assert_ne!(raw, display);
        let metadata = json!({"lastIdentification":{"rootPath":raw, "sources":[{"metadataPath":raw}]}, "custom":raw});
        db.conn().execute("INSERT INTO games (id,name,slug,root_path,metadata_json,created_at,updated_at) VALUES ('legacy','Arcade','unknown',?1,?2,'old','old')", params![raw,metadata.to_string()]).unwrap();
        let variant = game.with_file_name("aRCaDE");
        let input = if variant.exists() { variant } else { game };
        let input = PathBuf::from(user_path(&input).to_str().unwrap().replace('\\', "/"));
        let identified = identify_game(&db, &input).unwrap();
        assert_eq!(identified.root_path, display);
        assert_eq!(identified.registered_game_id.as_deref(), Some("legacy"));
        let report = scan_games(
            &db,
            &ScanOptions {
                user_roots: vec![input, canonical.clone()],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(report.candidates.len(), 1);
        let candidate = &report.candidates[0];
        assert_eq!(candidate.root_path, display);
        assert_eq!(candidate.registered_game_id.as_deref(), Some("legacy"));
        let stored = get_game(&db, "legacy").unwrap();
        assert_eq!(stored.root_path, raw);
        assert_eq!(stored.metadata, metadata);
        let dto = serde_json::to_value(stored).unwrap();
        assert_eq!(dto["rootPath"], display);
        assert_eq!(dto["metadata"]["lastIdentification"]["rootPath"], display);
        assert_eq!(
            dto["metadata"]["lastIdentification"]["sources"][0]["metadataPath"],
            display
        );
        assert_eq!(dto["metadata"]["custom"], raw);
        let registered =
            register_discovered_game(&db, Path::new(&candidate.root_path), None).unwrap();
        assert_eq!(registered.id, "legacy");
        assert_eq!(registered.root_path, raw);
        assert_eq!(list_games(&db).unwrap().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn native_backslashes_and_case_distinct_installations_keep_separate_ids() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let library = temp.path().join("library");
        let first = library.join(r"game\Name");
        let mut roots = vec![first.clone(), library.join("game/Name")];
        fs::create_dir_all(&first).unwrap();
        let case_variant = library.join(r"Game\Name");
        if !case_variant.exists() {
            roots.push(case_variant);
        }
        let mut ids = BTreeSet::new();
        for root in &roots {
            fs::create_dir_all(root).unwrap();
            assert!(ids.insert(register_game(&db, root, None).unwrap().id));
        }
        let result = scan_games(
            &db,
            &ScanOptions {
                user_roots: roots,
                max_depth: 0,
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(result.candidates.len(), ids.len());
        for candidate in result.candidates {
            assert!(ids.remove(&candidate.registered_game_id.unwrap()));
            assert_eq!(
                identify_game(&db, Path::new(&candidate.root_path))
                    .unwrap()
                    .root_path,
                candidate.root_path
            );
        }
    }

    #[test]
    fn fresh_database_discovers_and_registers_games_without_an_adapter() {
        let temp = tempfile::tempdir().unwrap();
        let db = db(&temp);
        let steam = temp.path().join("Steam");
        let game = steam.join("steamapps/common/Skyrim");
        skyrim(&game);
        fs::write(
            steam.join("steamapps/appmanifest_489830.acf"),
            acf("489830", "Skyrim"),
        )
        .unwrap();

        let identified = identify_game(&db, &game).unwrap();
        assert_eq!(identified.status, IdentificationStatus::Unidentified);
        assert!(identified.matches.is_empty());
        assert!(identified.suggested_adapter_slug.is_none());

        let scanned = scan_games(
            &db,
            &ScanOptions {
                steam_roots: vec![steam],
                ..scan_options()
            },
        )
        .unwrap();
        assert_eq!(scanned.candidates.len(), 1);
        let candidate = &scanned.candidates[0];
        assert_eq!(candidate.name, "A game");
        assert_eq!(candidate.status, IdentificationStatus::Unidentified);
        assert!(candidate.matches.is_empty());
        assert!(candidate.suggested_adapter_slug.is_none());
        assert!(candidate.requires_confirmation);
        assert_eq!(candidate.sources[0].steam_app_id.as_deref(), Some("489830"));
        assert!(list_games(&db).unwrap().is_empty());
        assert!(matches!(
            register_game(&db, &game, Some("skyrim-se")),
            Err(AppError::NotFound(_))
        ));
        assert!(list_games(&db).unwrap().is_empty());

        let registered = register_discovered_game(
            &db,
            Path::new(&candidate.root_path),
            candidate.suggested_adapter_slug.as_deref(),
        )
        .unwrap();
        assert_eq!(registered.slug, "unknown");
        assert!(registered.adapter_slug.is_none());
        assert!(registered.adapter_version.is_none());
        assert!(registered.steam_app_id.is_none());
        assert!(plugins::list_plugins(&db).unwrap().is_empty());
    }

    #[test]
    fn disabled_installed_adapters_do_not_match_or_bind() {
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
        let game = temp.path().join("game");
        skyrim(&game);
        let reviewed = identify_game(&db, &game).unwrap();
        assert_eq!(reviewed.status, IdentificationStatus::HighConfidence);
        assert_eq!(
            reviewed.suggested_adapter_slug.as_deref(),
            Some("skyrim-se")
        );
        db.conn()
            .execute("UPDATE plugins SET enabled=0", [])
            .unwrap();
        assert_eq!(
            identify_game(&db, &game).unwrap().status,
            IdentificationStatus::Unidentified
        );
        assert!(register_game(&db, &game, Some("skyrim-se")).is_err());
        let registered =
            register_discovered_game(&db, &game, reviewed.suggested_adapter_slug.as_deref())
                .unwrap();
        assert!(registered.adapter_slug.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn marker_and_steam_directory_symlinks_cannot_escape_scan_roots() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let db = db_with_adapters(&temp);
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
