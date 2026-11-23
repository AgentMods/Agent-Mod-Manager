//! Declarative game packages. Loading never expands environment variables, runs
//! a script, installs a mod, or grants filesystem access. All paths in an
//! adapter are relative to an explicitly registered root.
mod mcp;
#[cfg(test)]
pub(crate) mod test_fixtures;
pub use mcp::PluginMcpServer;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::storage::AppPaths;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RootKind {
    Game,
    RegisteredUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallRoot {
    pub id: String,
    pub kind: RootKind,
    pub relative_path: String,
    pub platforms: Vec<Platform>,
    /// Display only: these strings must never be interpolated into write paths.
    pub discovery_hints: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Marker {
    pub path: String,
    pub kind: MarkerKind,
    pub required: bool,
    pub platforms: Vec<Platform>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModType {
    pub id: String,
    pub name: String,
    pub root: String,
    /// Payload hints, not an executable allowlist or a substitute for inspection.
    pub extensions: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveLayout {
    /// Copy the payload intact; e.g. a Minecraft JAR is itself a ZIP container.
    SingleFile,
    /// Preserve the mod directory and its manifest, never flatten it.
    ModDirectory,
    /// Remove only this rule's sourcePrefix after inspecting archive entries.
    StripPrefix,
    /// Needs a reviewed file plan, e.g. FOMOD choices or a script extender.
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveRule {
    pub mod_type: String,
    pub layout: ArchiveLayout,
    pub source_prefix: Option<String>,
    pub required_files: Vec<String>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdapterPath {
    pub root: String,
    pub path: String,
    pub platforms: Vec<Platform>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadOrderKind {
    None,
    LoaderDependencies,
    BethesdaPlugins,
    Redmod,
    Bg3ModSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoadOrder {
    pub kind: LoadOrderKind,
    pub paths: Vec<AdapterPath>,
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceReference {
    pub title: String,
    pub url: String,
    pub checked_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GameAdapter {
    pub schema_version: u32,
    pub slug: String,
    pub name: String,
    /// Adapter package version, not a game or mod-loader version.
    pub version: String,
    pub platforms: Vec<Platform>,
    /// Required markers must all match; at least one marker must match.
    pub markers: Vec<Marker>,
    pub steam_app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<PluginMcpServer>,
    pub install_roots: Vec<InstallRoot>,
    pub mod_types: Vec<ModType>,
    pub archive_rules: Vec<ArchiveRule>,
    pub load_order: LoadOrder,
    pub save_paths: Vec<AdapterPath>,
    pub log_paths: Vec<AdapterPath>,
    pub uninstall_notes: Vec<String>,
    pub sources: Vec<SourceReference>,
    /// Default library name. Older manifests use the package name for both purposes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillMetadata {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GamePackage {
    pub adapter: GameAdapter,
    pub skill: SkillMetadata,
    pub skill_markdown: String,
}

pub(crate) fn valid_cover_url(value: &str) -> bool {
    if value.len() > 2048
        || !value.starts_with("https://")
        || value
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return false;
    }
    url::Url::parse(value).is_ok_and(|url| {
        matches!(url.host(), Some(url::Host::Domain(domain)) if {
            let domain = domain.trim_end_matches('.');
            domain.contains('.')
                && ![".localhost", ".local", ".internal"].iter().any(|suffix| domain.ends_with(suffix))
        }) && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.fragment().is_none()
    })
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

pub fn validate_slug(slug: &str) -> AppResult<()> {
    let valid = !slug.is_empty()
        && slug.len() <= 64
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && !slug.contains("--");
    let reserved = ["con", "prn", "aux", "nul"].contains(&slug)
        || (slug.len() == 4
            && (slug.starts_with("com") || slug.starts_with("lpt"))
            && matches!(slug.as_bytes()[3], b'1'..=b'9'));
    if !valid || reserved {
        return Err(invalid(format!("无效或保留的适配包 slug: {slug}")));
    }
    Ok(())
}

/// SemVer 2.0 syntax, without imposing a bounded integer on numeric components.
pub fn validate_version(version: &str) -> AppResult<()> {
    fn identifiers(value: &str, prerelease: bool) -> bool {
        value.split('.').all(|id| {
            !id.is_empty()
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !(prerelease
                    && id.len() > 1
                    && id.starts_with('0')
                    && id.bytes().all(|b| b.is_ascii_digit()))
        })
    }
    let mut parts = version.split('+');
    let base = parts.next().unwrap_or_default();
    let build = parts.next();
    let (core, pre) = base
        .split_once('-')
        .map_or((base, None), |(a, b)| (a, Some(b)));
    let numbers: Vec<_> = core.split('.').collect();
    if version.len() > 128
        || parts.next().is_some()
        || numbers.len() != 3
        || numbers.iter().any(|n| {
            n.is_empty()
                || !n.bytes().all(|b| b.is_ascii_digit())
                || (n.len() > 1 && n.starts_with('0'))
        })
        || pre.is_some_and(|p| !identifiers(p, true))
        || build.is_some_and(|b| !identifiers(b, false))
    {
        return Err(invalid(format!("适配包版本必须符合 SemVer: {version}")));
    }
    Ok(())
}

/// Portable fixed relative paths only: no interpolation, globbing, ADS, device
/// paths or Windows aliases, including when validation runs on a Unix host.
pub fn validate_relative_path(path: &str, allow_root: bool) -> AppResult<()> {
    if allow_root && path == "." {
        return Ok(());
    }
    if path.is_empty()
        || path.len() > 1024
        || path.contains(['\\', ':', '%', '$', '*', '?', '"', '<', '>', '|'])
        || path.chars().any(char::is_control)
    {
        return Err(AppError::PathEscape(format!("不是固定相对路径: {path}")));
    }
    for part in path.split('/') {
        let stem = part
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if part.is_empty()
            || part == "."
            || part == ".."
            || (part.contains('~') && !crate::paths::is_unreal_mods_dir(part))
            || part.ends_with(['.', ' '])
            || ["con", "prn", "aux", "nul"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("com") || stem.starts_with("lpt"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(AppError::PathEscape(format!("路径含不安全组件: {path}")));
        }
    }
    Ok(())
}

fn validate_platforms(platforms: &[Platform], supported: &[Platform]) -> AppResult<()> {
    let unique: BTreeSet<_> = platforms.iter().collect();
    if platforms.is_empty()
        || unique.len() != platforms.len()
        || platforms.iter().any(|p| !supported.contains(p))
    {
        return Err(invalid("platforms 不能为空、重复或超出适配包支持范围"));
    }
    Ok(())
}

impl GameAdapter {
    pub fn display_game_name(&self) -> &str {
        self.game_name.as_deref().unwrap_or(&self.name)
    }

    pub fn validate(&self) -> AppResult<()> {
        validate_slug(&self.slug)?;
        validate_version(&self.version)?;
        self.mcp_configs()?;
        if self.schema_version != 1 || self.name.trim().is_empty() || self.name.len() > 256 {
            return Err(invalid("适配包名称为空或 schemaVersion 不受支持"));
        }
        if self.game_name.as_ref().is_some_and(|name| {
            name.is_empty()
                || name.trim() != name
                || name.len() > 256
                || name.chars().any(char::is_control)
        }) {
            return Err(invalid(
                "gameName 须为 1 至 256 字节的游戏名称，且不能含首尾空白或控制字符",
            ));
        }
        validate_platforms(
            &self.platforms,
            &[Platform::Windows, Platform::Macos, Platform::Linux],
        )?;
        if self.steam_app_id.as_ref().is_some_and(|id| {
            id.parse::<u32>()
                .map_or(true, |n| n == 0 || n.to_string() != *id)
        }) {
            return Err(invalid("steamAppId 必须为正整数的十进制字符串"));
        }
        if self
            .cover_url
            .as_deref()
            .is_some_and(|url| !valid_cover_url(url))
        {
            return Err(invalid("coverUrl 须为不含凭据或片段的公网 HTTPS 图片地址"));
        }
        if self.markers.is_empty()
            || self.install_roots.is_empty()
            || self.mod_types.is_empty()
            || self.archive_rules.is_empty()
        {
            return Err(invalid("适配包缺少标记、安装根、模组类型或归档规则"));
        }
        for marker in &self.markers {
            validate_relative_path(&marker.path, false)?;
            validate_platforms(&marker.platforms, &self.platforms)?;
        }
        let mut roots = BTreeMap::new();
        for root in &self.install_roots {
            validate_slug(&root.id)?;
            validate_relative_path(&root.relative_path, true)?;
            validate_platforms(&root.platforms, &self.platforms)?;
            if roots.insert(root.id.as_str(), root).is_some() {
                return Err(invalid("安装根 id 重复"));
            }
            if root.kind == RootKind::RegisteredUser && root.discovery_hints.is_empty() {
                return Err(invalid("用户管理根必须说明如何由用户定位并登记"));
            }
        }
        let mut types = BTreeSet::new();
        for ty in &self.mod_types {
            validate_slug(&ty.id)?;
            if !types.insert(ty.id.as_str())
                || !roots.contains_key(ty.root.as_str())
                || ty.name.trim().is_empty()
            {
                return Err(invalid("模组类型重复或引用未知安装根"));
            }
            if ty.extensions.iter().any(|s| {
                s.is_empty()
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            }) {
                return Err(invalid("模组扩展名必须为小写字母或数字，不含点号"));
            }
        }
        let mut ruled = BTreeSet::new();
        for rule in &self.archive_rules {
            if !types.contains(rule.mod_type.as_str()) {
                return Err(invalid("归档规则引用未知模组类型"));
            }
            ruled.insert(rule.mod_type.as_str());
            if (rule.layout == ArchiveLayout::StripPrefix) != rule.source_prefix.is_some() {
                return Err(invalid("只有 strip_prefix 规则必须包含 sourcePrefix"));
            }
            if let Some(prefix) = &rule.source_prefix {
                validate_relative_path(prefix, false)?;
            }
            for required in &rule.required_files {
                validate_relative_path(required, false)?;
            }
        }
        if ruled != types {
            return Err(invalid("每种模组类型都必须声明归档规则"));
        }
        for path in self
            .load_order
            .paths
            .iter()
            .chain(&self.save_paths)
            .chain(&self.log_paths)
        {
            validate_relative_path(&path.path, true)?;
            let root = roots
                .get(path.root.as_str())
                .ok_or_else(|| invalid("数据路径引用未知安装根"))?;
            validate_platforms(&path.platforms, &root.platforms)?;
        }
        if self.uninstall_notes.is_empty()
            || self.uninstall_notes.iter().any(|n| n.trim().is_empty())
            || self.sources.is_empty()
        {
            return Err(invalid("适配包必须提供卸载说明和一手资料来源"));
        }
        for source in &self.sources {
            let url = url::Url::parse(&source.url).map_err(|_| invalid("资料来源 URL 无效"))?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || source.title.trim().is_empty()
                || chrono::NaiveDate::parse_from_str(&source.checked_at, "%Y-%m-%d").is_err()
            {
                return Err(invalid("资料来源须为 HTTPS URL，并附标题和核对日期"));
            }
        }
        Ok(())
    }

    /// Resolve a root using trusted host registrations, never discoveryHints.
    /// `registered_user_roots` maps adapter root IDs to explicitly registered
    /// directories; do not populate it from untrusted agent/tool arguments.
    /// `managed_roots` is the file engine's authoritative allowlist.
    pub fn resolve_root(
        &self,
        id: &str,
        platform: Platform,
        game_root: &Path,
        registered_user_roots: &BTreeMap<String, PathBuf>,
        managed_roots: &[PathBuf],
    ) -> AppResult<PathBuf> {
        self.validate()?;
        let root = self
            .install_roots
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| AppError::NotFound(format!("安装根: {id}")))?;
        if !root.platforms.contains(&platform) {
            return Err(AppError::Unsupported(format!(
                "安装根 {id} 不支持 {platform:?}"
            )));
        }
        let base = match root.kind {
            RootKind::Game => game_root,
            RootKind::RegisteredUser => registered_user_roots
                .get(id)
                .map(PathBuf::as_path)
                .ok_or_else(|| {
                    AppError::Denied(format!(
                        "先登记用户管理根 {id}；discoveryHints 不授予写入权限"
                    ))
                })?,
        };
        if !base.is_absolute() {
            return Err(AppError::PathEscape("管理根必须是绝对路径".into()));
        }
        if !fs::symlink_metadata(base)?.file_type().is_dir() || base.canonicalize()? != base {
            return Err(AppError::Denied(
                "此游戏/用户管理根缺失或已被链接替换，请重新核对原登记目录".into(),
            ));
        }
        let base = crate::paths::ensure_within(base, managed_roots)?;
        let target = crate::paths::ensure_within(
            &base.join(&root.relative_path),
            std::slice::from_ref(&base),
        )?;
        crate::paths::ensure_within(&target, managed_roots)
    }

    pub fn resolve_path(
        &self,
        path: &AdapterPath,
        platform: Platform,
        game_root: &Path,
        registered_user_roots: &BTreeMap<String, PathBuf>,
        managed_roots: &[PathBuf],
    ) -> AppResult<PathBuf> {
        validate_relative_path(&path.path, true)?;
        if !path.platforms.contains(&platform) {
            return Err(AppError::Unsupported("数据路径不支持当前平台".into()));
        }
        let root = self.resolve_root(
            &path.root,
            platform,
            game_root,
            registered_user_roots,
            managed_roots,
        )?;
        crate::paths::ensure_within(&root.join(&path.path), &[root])
    }
}

/// Package frontmatter is deliberately a small, auditable YAML subset: exactly
/// name and description, each a single-line letter-led plain, single-quoted or
/// JSON string. Ambiguous YAML booleans/numbers must be quoted.
/// YAML tags, aliases, multiline scalars and executable extensions are rejected.
pub fn parse_skill(markdown: &str, expected_slug: &str) -> AppResult<SkillMetadata> {
    validate_slug(expected_slug)?;
    if markdown.contains('\0') {
        return Err(invalid("SKILL.md 包含空字节"));
    }
    let mut lines = markdown.lines();
    if lines.next() != Some("---") {
        return Err(invalid("SKILL.md 缺少 YAML frontmatter"));
    }
    let mut fields = BTreeMap::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line == "---" {
            closed = true;
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let (key, raw) = line
            .split_once(':')
            .ok_or_else(|| invalid("frontmatter 必须使用单行键值"))?;
        if !["name", "description"].contains(&key) || fields.contains_key(key) {
            return Err(invalid("frontmatter 含未知或重复键"));
        }
        let raw = raw.trim();
        let value = if raw.starts_with('"') {
            serde_json::from_str::<String>(raw)
                .map_err(|_| invalid("frontmatter 双引号字符串无效"))?
        } else if raw.starts_with('\'') && raw.ends_with('\'') && raw.len() >= 2 {
            let inner = &raw[1..raw.len() - 1];
            if inner.replace("''", "").contains('\'') {
                return Err(invalid("frontmatter 单引号转义无效"));
            }
            inner.replace("''", "'")
        } else {
            if !raw.chars().next().is_some_and(char::is_alphabetic)
                || ["true", "false", "yes", "no", "on", "off", "null"]
                    .contains(&raw.to_ascii_lowercase().as_str())
                || raw.contains(": ")
                || raw.contains(" #")
            {
                return Err(invalid("frontmatter 不支持此 YAML 标量语法"));
            }
            raw.to_string()
        };
        if value.chars().any(char::is_control) {
            return Err(invalid("frontmatter 不能包含控制字符"));
        }
        fields.insert(key, value);
    }
    if !closed || !lines.any(|line| !line.trim().is_empty()) {
        return Err(invalid("SKILL.md frontmatter 未闭合或正文为空"));
    }
    let name = fields
        .remove("name")
        .ok_or_else(|| invalid("SKILL.md 缺少 name"))?;
    let description = fields
        .remove("description")
        .ok_or_else(|| invalid("SKILL.md 缺少 description"))?;
    if name != expected_slug
        || description.trim().is_empty()
        || description.chars().count() > 1024
        || description.contains(['<', '>'])
    {
        return Err(invalid(
            "SKILL.md 名称须与目录相同，描述须为 1..=1024 字符且不含尖括号",
        ));
    }
    Ok(SkillMetadata { name, description })
}

pub fn parse_package(
    directory_name: &str,
    adapter_json: &str,
    skill_markdown: &str,
) -> AppResult<GamePackage> {
    validate_slug(directory_name)?;
    let adapter: GameAdapter = serde_json::from_str(adapter_json)?;
    adapter.validate()?;
    if adapter.slug != directory_name {
        return Err(invalid("adapter slug 与目录不一致"));
    }
    let skill = parse_skill(skill_markdown, directory_name)?;
    Ok(GamePackage {
        adapter,
        skill,
        skill_markdown: skill_markdown.to_string(),
    })
}

/// Read exactly two regular files, rejecting package and file symlinks. No
/// recursive discovery and no package-provided code is loaded.
pub fn load_package(directory: &Path) -> AppResult<GamePackage> {
    if !fs::symlink_metadata(directory)?.file_type().is_dir() {
        return Err(invalid("适配包目录必须是普通目录"));
    }
    let slug = directory
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("适配包目录名无效"))?;
    validate_slug(slug)?;
    parse_package(
        slug,
        &read_local_text(&directory.join("adapter.json"), None)?,
        &read_local_text(&directory.join("SKILL.md"), None)?,
    )
}

/// Read a regular-file snapshot, not an unbounded stream. A changed length
/// (including a file that grows during the read) invalidates the snapshot.
/// Callers additionally validate the managed directory and recorded hashes.
pub(crate) fn read_local_text(path: &Path, limit: Option<u64>) -> AppResult<String> {
    use std::io::Read;
    let mut file = crate::safe_fs::open_regular(path)?;
    crate::safe_fs::verify_regular_at(&file, path)?;
    let before = file.metadata()?;
    let len = before.len();
    if limit.is_some_and(|limit| len > limit) {
        return Err(invalid("文本文件超过读取上限"));
    }
    let mut text = String::new();
    (&mut file)
        .take(len.saturating_add(1))
        .read_to_string(&mut text)?;
    crate::safe_fs::verify_regular_at(&file, path)?;
    let after = file.metadata()?;
    if text.len() as u64 != len || after.len() != len || after.modified()? != before.modified()? {
        return Err(AppError::Conflict("文本文件在读取期间改变".into()));
    }
    Ok(text)
}

/// Park directories registered by earlier desktop releases, including local edits.
/// If the process stops before the database commit, a missing source is safe to retry.
pub(crate) fn retire_builtin_adapters(db: &Db, paths: &AppPaths) -> AppResult<()> {
    let _files = crate::transaction::write_lock();
    let mut conn = db.conn();
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let legacy: Vec<(String, PathBuf)> = {
        let mut stmt =
            tx.prepare("SELECT slug,install_path FROM plugins WHERE source='builtin'")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?)))
        })?;
        rows.collect::<Result<_, _>>()?
    };
    for (slug, registered) in legacy {
        validate_slug(&slug)?;
        let package = paths.plugins().join("games").join(&slug);
        if registered == package {
            if let Some(id) = crate::safe_fs::identity(&package)? {
                let backup = paths.backups().join("retired-adapters");
                crate::safe_fs::create_dir_all(&backup)?;
                let destination = backup.join(format!("{slug}-{}", uuid::Uuid::new_v4()));
                crate::safe_fs::rename_checked(&package, &destination, &id)?;
            }
        }
        tx.execute("UPDATE games SET adapter_slug=NULL,adapter_version=NULL,updated_at=?2 WHERE adapter_slug=?1", params![slug, crate::util::now()])?;
        tx.execute(
            "DELETE FROM skills WHERE name=?1 AND source='builtin'",
            [&slug],
        )?;
        tx.execute(
            "DELETE FROM plugins WHERE slug=?1 AND source='builtin'",
            [&slug],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub slug: String,
    pub name: String,
    pub game_name: String,
    pub version: String,
    pub description: String,
    pub source: String,
    /// Known cloud presence by slug, not proof about these bytes or the current owner.
    pub has_cloud_evidence: bool,
    pub enabled: bool,
    pub validated: bool,
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub install_path: String,
    pub cover_url: Option<String>,
    /// Commit time for the verified installed content, if it has deployment provenance.
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mcp_server_names: Vec<String>,
}

pub fn list_plugins(db: &Db) -> AppResult<Vec<PluginInfo>> {
    let conn = db.conn();
    // The publication writer commits only confirmed receipts. This lookup is
    // keyed by the release slug, deliberately not the installed version.
    let mut statement = conn.prepare(
        "SELECT p.slug,p.name,p.game_name,p.version,p.description,p.source,p.enabled,
               p.validated,p.install_path,p.adapter_json,
               EXISTS (
                   SELECT 1 FROM manager_publications AS m
                   WHERE m.slug=p.slug AND m.state='completed' AND m.approved_digest=m.digest
                     AND CASE WHEN json_valid(m.remote_json) THEN
                         json_extract(m.remote_json, '$.slug')=m.slug
                         AND json_extract(m.remote_json, '$.version')=m.version
                         AND json_type(m.remote_json, '$.id')='integer'
                         AND json_extract(m.remote_json, '$.id')>0
                         AND (m.target_plugin_id IS NULL OR json_extract(m.remote_json, '$.id')=m.target_plugin_id)
                         AND json_type(m.remote_json, '$.releaseId')='integer'
                         AND json_extract(m.remote_json, '$.releaseId')>0
                     ELSE 0 END

               ),
               CASE WHEN p.validated THEN p.package_updated_at END
        FROM plugins AS p ORDER BY p.name,p.slug",
    )?;
    let rows = statement.query_map([], |r| {
        let slug: String = r.get(0)?;
        let version: String = r.get(3)?;
        let validated: bool = r.get(7)?;
        let adapter = if validated {
            r.get::<_, Option<String>>(9)?
                .and_then(|json| serde_json::from_str::<GameAdapter>(&json).ok())
                .filter(|adapter| adapter.slug == slug && adapter.version == version)
        } else {
            None
        };
        let (cover_url, mcp_server_names) = adapter.map_or_else(Default::default, |adapter| {
            (
                adapter.cover_url.filter(|url| valid_cover_url(url)),
                adapter
                    .mcp_servers
                    .into_iter()
                    .map(|server| server.name)
                    .collect(),
            )
        });
        Ok(PluginInfo {
            slug,
            name: r.get(1)?,
            game_name: r.get(2)?,
            version,
            description: r.get(4)?,
            source: r.get(5)?,
            has_cloud_evidence: r.get(10)?,
            enabled: r.get(6)?,
            validated,
            install_path: r.get(8)?,
            cover_url,
            updated_at: r.get(11)?,
            mcp_server_names,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn matches_registered_adapter(json: &str, adapter: &GameAdapter) -> AppResult<bool> {
    if json.is_empty() {
        return Ok(false);
    }
    let registered: GameAdapter = serde_json::from_str(json)?;
    Ok(serde_json::to_value(registered)? == serde_json::to_value(adapter)?)
}

/// Revalidate stored JSON at use time; a database boolean alone is not trust.
pub fn adapter_for(db: &Db, slug: &str) -> AppResult<GameAdapter> {
    adapter_for_from(&db.conn(), slug)
}

pub(crate) fn adapter_for_from(conn: &rusqlite::Connection, slug: &str) -> AppResult<GameAdapter> {
    validate_slug(slug)?;
    let row: Option<(String, bool, bool, String)> = conn
        .query_row(
            "SELECT adapter_json,enabled,validated,version FROM plugins WHERE slug=?1",
            [slug],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let (json, enabled, validated, version) =
        row.ok_or_else(|| AppError::NotFound(format!("适配包: {slug}")))?;
    if !enabled || !validated {
        return Err(AppError::Denied(format!("适配包未启用或未验证: {slug}")));
    }
    let adapter: GameAdapter = serde_json::from_str(&json)?;
    adapter.validate()?;
    if adapter.slug != slug || adapter.version != version {
        return Err(invalid("数据库适配包身份与声明不一致"));
    }
    Ok(adapter)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, AppPaths, Db) {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("manager")).unwrap();
        let db = Db::open(&paths.db_file()).unwrap();
        (temp, paths, db)
    }

    fn seed_installed_cloud_test_plugin(db: &Db) {
        db.conn()
            .execute(
                "INSERT INTO plugins(slug,name,game_name,version,source,install_path,adapter_json,created_at,updated_at)
                 VALUES ('local-game','Game','Game','1.0.0','external','local-game','{}','now','now')",
                [],
            )
            .unwrap();
    }

    fn assert_installed_cloud_evidence(db: &Db, expected: bool) {
        let installed = list_plugins(db).unwrap();
        let plugin = &installed[0];
        assert_eq!(plugin.source, "external");
        assert_eq!(plugin.version, "1.0.0");
        assert_eq!(plugin.has_cloud_evidence, expected);
    }

    #[test]
    fn installed_cloud_evidence_requires_a_confirmed_publication_for_the_exact_slug() {
        let (_temp, paths, db) = fixture();
        seed_installed_cloud_test_plugin(&db);
        assert_installed_cloud_evidence(&db, false);
        let remote = serde_json::json!({
            "id": 42, "releaseId": 71, "slug": "local-game", "version": "2.0.0",
            "tags": [], "hidden": true
        })
        .to_string();
        db.conn().execute(
            "INSERT INTO manager_publications(id,preview_json,digest,slug,version,approved_digest,state,remote_json,created_at,updated_at)
             VALUES ('publication','{}','digest','local-game-other','2.0.0','digest','completed',?1,'now','now')",
            [&remote],
        ).unwrap();
        assert_installed_cloud_evidence(&db, false);
        for state in ["intended", "pending", "unknown", "error"] {
            db.conn()
                .execute(
                    "UPDATE manager_publications SET slug='local-game',state=?1,remote_json=NULL",
                    [state],
                )
                .unwrap();
            assert_installed_cloud_evidence(&db, false);
        }
        for invalid_receipt in [None, Some("not json"), Some(r#"{"id":42}"#)] {
            db.conn()
                .execute(
                    "UPDATE manager_publications SET state='completed',remote_json=?1",
                    [invalid_receipt],
                )
                .unwrap();
            assert_installed_cloud_evidence(&db, false);
        }
        db.conn()
            .execute("UPDATE manager_publications SET remote_json=?1", [remote])
            .unwrap();
        assert_installed_cloud_evidence(&db, true);
        drop(db);
        // A hidden but confirmed release for a different version is still durable slug evidence.
        assert_installed_cloud_evidence(&Db::open(&paths.db_file()).unwrap(), true);
    }

    #[test]
    fn external_package_fixtures_validate_and_roundtrip() {
        let packages = test_fixtures::packages().unwrap();
        let mut names = BTreeSet::new();
        for p in packages {
            assert!(names.insert(p.adapter.slug.clone()));
            let json = serde_json::to_string(&p.adapter).unwrap();
            parse_package(&p.adapter.slug, &json, &p.skill_markdown).unwrap();
        }
    }

    #[test]
    fn game_name_is_optional_and_independent_of_package_title() {
        let (slug, json, skill) = test_fixtures::PACKAGES[0];
        let mut manifest: serde_json::Value = serde_json::from_str(json).unwrap();
        manifest["name"] = "幻兽帕鲁 Windows 模组适配包".into();
        let parse =
            |manifest: &serde_json::Value| parse_package(slug, &manifest.to_string(), skill);
        let legacy = parse(&manifest).unwrap().adapter;
        assert_eq!(legacy.display_game_name(), legacy.name);
        assert!(serde_json::to_value(legacy)
            .unwrap()
            .get("gameName")
            .is_none());

        manifest["gameName"] = "幻兽帕鲁".into();
        let adapter = parse(&manifest).unwrap().adapter;
        assert_eq!(adapter.display_game_name(), "幻兽帕鲁");
        assert_eq!(adapter.name, "幻兽帕鲁 Windows 模组适配包");
        assert_eq!(
            serde_json::to_value(adapter).unwrap()["gameName"],
            "幻兽帕鲁"
        );
        for name in ["", " ", " 游戏", "游戏\n名", &"x".repeat(257)] {
            manifest["gameName"] = name.into();
            assert!(parse(&manifest).is_err());
        }
    }

    #[test]
    fn covers_are_optional_and_only_public_https_addresses_are_accepted() {
        for mut package in test_fixtures::packages().unwrap() {
            assert!(package
                .adapter
                .cover_url
                .as_deref()
                .is_some_and(valid_cover_url));
            package.adapter.cover_url = None;
            assert!(package.adapter.validate().is_ok());
            assert!(serde_json::to_value(&package.adapter)
                .unwrap()
                .get("coverUrl")
                .is_none());
            for url in [
                "file:///tmp/image.jpg",
                "data:image/png;base64,AAAA",
                "http://example.com/image.png",
                "https://user:pass@example.com/image.png",
                "https://127.0.0.1/image.jpg",
                "https://sub.localhost/image.jpg",
                "https://example.com:444/image.png",
                "https://example.com/image.png#fragment",
            ] {
                package.adapter.cover_url = Some(url.into());
                assert!(package.adapter.validate().is_err(), "{url}");
            }
        }
    }

    #[test]
    fn rejects_portable_path_attacks() {
        for value in [
            "",
            "../Data",
            "Data/../Mods",
            "/Mods",
            "C:/Mods",
            "C:\\Mods",
            "\\\\server\\Mods",
            "Data//Mods",
            "Data/./Mods",
            "Data/name:stream",
            "%APPDATA%/Mods",
            "$HOME/Mods",
            "~/Mods",
            "~user/Mods",
            "Data/PROGRA~1/file.pak",
            "~mods/../outside.pak",
            "Data/*",
            "Data/NUL.txt",
            "Data/COM1",
            "Data/file.",
            "Data/file ",
            "Data/\0",
        ] {
            assert!(
                validate_relative_path(value, false).is_err(),
                "accepted {value:?}"
            );
        }
        assert!(validate_relative_path(".", false).is_err());
        validate_relative_path(".", true).unwrap();
        validate_relative_path("PlayerProfiles/Public/modsettings.lsx", false).unwrap();
        validate_relative_path("Content/Paks/~Mods", true).unwrap();
    }

    #[test]
    fn validates_slug_and_semver() {
        for slug in [
            "", "../foo", "Foo", "a_b", "a--b", "-a", "a-", "con", "lpt1",
        ] {
            assert!(validate_slug(slug).is_err());
        }
        for version in [
            "",
            "1",
            "1.0",
            "v1.0.0",
            "01.0.0",
            "1.0.0-01",
            "1.0.0-",
            "1.0.0+",
            "1.0.0+x+y",
            "1.0.0-a..b",
        ] {
            assert!(validate_version(version).is_err(), "{version}");
        }
        for version in ["0.1.0", "1.2.3-rc.1+build.004", "1.0.0-alpha-1"] {
            validate_version(version).unwrap();
        }
    }

    #[test]
    fn validates_frontmatter_identity_boundaries_and_duplicates() {
        let make = |description: &str| {
            format!("---\nname: minecraft-java\ndescription: {description}\n---\nGuidance.\n")
        };
        parse_skill(&make(&"a".repeat(1024)), "minecraft-java").unwrap();
        assert!(parse_skill(&make(&"a".repeat(1025)), "minecraft-java").is_err());
        parse_skill(&make(&"界".repeat(1024)), "minecraft-java").unwrap();
        for text in [
            make(""),
            make("|"),
            make("[a]"),
            make("true"),
            make("null"),
            make("123"),
            make("# comment only"),
            make("\"\\u0000\""),
            make("'a'b'"),
            make("a").replace("name: minecraft-java", "name: other"),
            make("a").replace("description:", "name: duplicate\ndescription:"),
            make("a").replace("---\nGuidance.", "Guidance."),
        ] {
            assert!(parse_skill(&text, "minecraft-java").is_err(), "{text}");
        }
        parse_skill(
            &make("\"Guide: install safely\"").replace('\n', "\r\n"),
            "minecraft-java",
        )
        .unwrap();
    }

    #[test]
    fn rejects_unknown_code_fields_identity_and_root_references() {
        let (slug, json, skill) = test_fixtures::PACKAGES[0];
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["command"] = "run me".into();
        assert!(parse_package(slug, &value.to_string(), skill).is_err());
        value.as_object_mut().unwrap().remove("command");
        value["modTypes"][0]["root"] = "missing".into();
        assert!(parse_package(slug, &value.to_string(), skill).is_err());
        assert!(parse_package("different", json, skill).is_err());
    }

    #[test]
    fn validates_paths_in_every_declarative_location() {
        let (slug, json, skill) = test_fixtures::PACKAGES[1];
        for pointer in [
            "/markers/0/path",
            "/installRoots/0/relativePath",
            "/archiveRules/0/sourcePrefix",
            "/loadOrder/paths/0/path",
            "/savePaths/0/path",
            "/logPaths/0/path",
        ] {
            let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
            *value.pointer_mut(pointer).unwrap() = "../outside".into();
            assert!(
                parse_package(slug, &value.to_string(), skill).is_err(),
                "{pointer}"
            );
        }
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value["archiveRules"][0]["requiredFiles"] = serde_json::json!(["C:/outside"]);
        assert!(parse_package(slug, &value.to_string(), skill).is_err());
        value = serde_json::from_str(json).unwrap();
        value["installRoots"][0]["platforms"] = serde_json::json!(["macos"]);
        assert!(parse_package(slug, &value.to_string(), skill).is_err());
    }

    #[test]
    fn accepts_large_text_but_rejects_invalid_package_files() {
        let (slug, json, skill) = test_fixtures::PACKAGES[0];
        let large_adapter = format!("{json}{}", " ".repeat(256 * 1024));
        let large_skill = format!("{skill}\n{}", "指南\n".repeat(32 * 1024));
        let parsed = parse_package(slug, &large_adapter, &large_skill).unwrap();
        assert_eq!(parsed.skill_markdown, large_skill);
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join(slug);
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("adapter.json"), json).unwrap();
        assert!(load_package(&package).is_err());
        fs::write(package.join("SKILL.md"), [0xff, 0xfe]).unwrap();
        assert!(load_package(&package).is_err());
    }

    #[test]
    fn database_tampering_is_revalidated() {
        let (_temp, paths, db) = fixture();
        test_fixtures::install(&db, &paths).unwrap();
        db.conn().execute("UPDATE plugins SET adapter_json=replace(adapter_json,'archive/pc/mod','../escape') WHERE slug='cyberpunk-2077'", []).unwrap();
        assert!(adapter_for(&db, "cyberpunk-2077").is_err());
    }

    #[test]
    fn bg3_requires_explicit_registered_and_managed_user_root() {
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        let user = temp.path().join("user-data");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&user).unwrap();
        let game = game.canonicalize().unwrap();
        let user = user.canonicalize().unwrap();
        let adapter = test_fixtures::packages().unwrap().pop().unwrap().adapter;
        let mut registered = BTreeMap::new();
        assert!(adapter
            .resolve_root(
                "user-mods",
                Platform::Windows,
                &game,
                &registered,
                std::slice::from_ref(&game)
            )
            .is_err());
        registered.insert("user-mods".into(), user.clone());
        assert!(adapter
            .resolve_root(
                "user-mods",
                Platform::Windows,
                &game,
                &registered,
                std::slice::from_ref(&game)
            )
            .is_err());
        let resolved = adapter
            .resolve_root(
                "user-mods",
                Platform::Windows,
                &game,
                &registered,
                &[game.clone(), user.clone()],
            )
            .unwrap();
        assert_eq!(resolved, user.canonicalize().unwrap().join("Mods"));
        assert!(!resolved.starts_with(game.canonicalize().unwrap()));
        assert!(!resolved.exists()); // resolving never creates game/user paths
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_package_files() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let outside = temp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let package = temp.path().join("minecraft-java");
        fs::create_dir_all(&package).unwrap();
        fs::write(outside.join("adapter.json"), test_fixtures::PACKAGES[0].1).unwrap();
        symlink(outside.join("adapter.json"), package.join("adapter.json")).unwrap();
        fs::write(package.join("SKILL.md"), test_fixtures::PACKAGES[0].2).unwrap();
        assert!(load_package(&package).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn resolved_paths_cannot_follow_symlinks_outside_the_registered_root() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let game = temp.path().join("game");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&game).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, game.join("mods")).unwrap();
        let adapter = test_fixtures::packages().unwrap().remove(0).adapter;
        assert!(adapter
            .resolve_root(
                "mods",
                Platform::Windows,
                &game,
                &BTreeMap::new(),
                std::slice::from_ref(&game)
            )
            .is_err());
        symlink(&outside, game.join("saves")).unwrap();
        assert!(adapter
            .resolve_path(
                &adapter.save_paths[0],
                Platform::Windows,
                &game,
                &BTreeMap::new(),
                std::slice::from_ref(&game)
            )
            .is_err());
        assert!(adapter
            .resolve_root(
                "mods",
                Platform::Windows,
                Path::new("relative"),
                &BTreeMap::new(),
                &[game]
            )
            .is_err());
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
}
