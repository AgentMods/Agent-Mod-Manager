//! Deploy immutable Mod Library entries using the existing approval/journal engine.
use super::{
    basename, invalid, logical_files, map_rule, strip_wrapper, validate_payload_sources,
    FomodSelection, InstallPreview, InstalledFile, ManualMapping, MappedPayload, Payload,
};
use crate::{
    archive,
    artifacts::DownloadOrigin,
    error::{AppError, AppResult},
    fomod, game_roots, games, mod_library, paths,
    plugins::{self, ArchiveLayout, ArchiveRule, GameAdapter, ModType, RootKind},
    safe_fs,
    transaction::{self, FileEngine, FileOp, FilePlan},
    util,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

pub(crate) const LIBRARY_INSTALL_KIND: &str = "library_install";
const MATCH_MEMBER_LIMIT: usize = 200;
const MATCH_PATH_BYTES: usize = 32 * 1024;
const README_LIMIT: usize = 32 * 1024;
const MANUAL_TYPE: &str = "manual";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryMatchRequest {
    pub game_id: String,
    #[serde(rename = "artifactId", alias = "libraryEntryId")]
    pub library_entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInstallRequest {
    pub game_id: String,
    #[serde(rename = "artifactId", alias = "libraryEntryId")]
    pub library_entry_id: String,
    #[serde(flatten)]
    pub mode: LibraryInstallMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum LibraryInstallMode {
    GtaRpf {
        job_id: String,
    },
    Adapter {
        mod_type: String,
        folder_name: Option<String>,
        fomod_selection: Option<FomodSelection>,
    },
    ApprovedMappings {
        mod_type: Option<String>,
        source_kind: LibrarySourceKind,
        mappings: Vec<ManualMapping>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibrarySourceKind {
    SingleFile,
    Archive,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInstallMatch {
    pub game_id: String,
    #[serde(rename = "artifactId")]
    pub library_entry_id: String,
    pub filename: String,
    pub artifact_sha256: String,
    pub active_install_id: Option<String>,
    pub adapter_slug: Option<String>,
    pub candidates: Vec<LibraryAdapterMatch>,
    pub archive_format: Option<String>,
    pub archive_entries: Vec<String>,
    pub archive_file_count: usize,
    pub archive_entries_truncated: bool,
    pub readme: Option<LibraryReadme>,
    pub fomod_status: Option<&'static str>,
    pub fomod: Option<fomod::FomodPreview>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAdapterMatch {
    pub mod_type: String,
    pub root_id: String,
    pub layout: ArchiveLayout,
    /// `matched`, `requiresFolderName`, `requiresGtaRpf`, `unsupportedEdition`, `requiresUser`, `ambiguous`, or `rootUnavailable`.
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryReadme {
    pub path: String,
    pub text: String,
    pub truncated: bool,
}

#[derive(PartialEq, Eq)]
struct Entry {
    id: String,
    payload: PathBuf,
    archive: PathBuf,
    filename: String,
    sha256: String,
    name: String,
    version: Option<String>,
    source: String,
    origin: Option<DownloadOrigin>,
}

impl Entry {
    fn payload_root(&self) -> PathBuf {
        self.payload.join(&self.sha256)
    }
}

fn load_entry(engine: &FileEngine, game_id: &str, id: &str) -> AppResult<Entry> {
    let verified = mod_library::verify_for_install(engine, game_id, id)?;
    let entry = verified.entry;
    // Only DB-owned archive names and library directory locations flow into
    // the planner. The digest also names the immutable extraction generation.
    basename(&entry.filename)?;
    if entry.sha256.len() != 64
        || !entry
            .sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(invalid("模组库 SHA-256 无效"));
    }
    let version = (!entry.version.trim().is_empty()
        && entry.version.len() <= 256
        && !entry.version.chars().any(char::is_control))
    .then_some(entry.version);
    Ok(Entry {
        id: entry.id,
        payload: verified.payload_path,
        archive: verified.archive_path,
        filename: entry.filename,
        sha256: entry.sha256,
        name: entry.name,
        version,
        source: entry.source_id,
        origin: verified.origin,
    })
}

fn hash_regular(path: &Path, size: u64, cancel: &CancellationToken) -> AppResult<String> {
    let mut file = safe_fs::open_regular(path)?;
    safe_fs::verify_regular_at(&file, path)?;
    if file.metadata()?.len() != size {
        return Err(AppError::Conflict("模组库缓存文件大小已变化".into()));
    }
    let mut reader = (&mut file).take(size.saturating_add(1));
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hash.update(&buffer[..n]);
    }
    safe_fs::verify_regular_at(&file, path)?;
    if total != size || file.metadata()?.len() != size {
        return Err(AppError::Conflict("模组库缓存文件大小已变化".into()));
    }
    Ok(hex::encode(hash.finalize()))
}

fn check_cancel(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

struct ArchiveScan {
    listing: archive::ArchiveListing,
    names: Payload,
    hashes: BTreeMap<PathBuf, String>,
    readme: Option<LibraryReadme>,
    fomod: Option<AppResult<fomod::FomodPreview>>,
}

fn archive_intent(entry: &Entry) -> AppResult<bool> {
    // Game-native .pak files need not be ZIPs. Only recognize supported magic
    // for .pak instead of treating every Unreal package as a broken ZIP.
    if entry.filename.to_ascii_lowercase().ends_with(".pak") {
        let mut magic = [0; 4];
        let mut file = safe_fs::open_regular(&entry.archive)?;
        return Ok(
            file.read_exact(&mut magic).is_ok() && matches!(&magic, b"PK\x03\x04" | b"PK\x05\x06")
        );
    }
    match archive::detect_format(&entry.archive) {
        Ok(_) => Ok(true),
        Err(AppError::Unsupported(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

fn contains_fomod(names: &Payload) -> bool {
    names.keys().any(|name| {
        name.rsplit('/')
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("ModuleConfig.xml"))
            || name
                .split('/')
                .any(|component| component.eq_ignore_ascii_case("fomod"))
    })
}

fn readme_name(names: &Payload) -> Option<&Path> {
    names
        .values()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    matches!(
                        name.to_ascii_lowercase().as_str(),
                        "readme" | "readme.txt" | "readme.md" | "readme.markdown"
                    )
                })
        })
        .min_by_key(|path| (path.components().count(), *path))
        .map(PathBuf::as_path)
}

fn text_readme(path: &Path, bytes: &[u8], truncated: bool) -> Option<LibraryReadme> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(err) if truncated && err.error_len().is_none() => {
            std::str::from_utf8(&bytes[..err.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    if text.contains('\0') || text.trim().is_empty() {
        return None;
    }
    Some(LibraryReadme {
        path: path.to_str()?.replace('\\', "/"),
        text: text
            .chars()
            .filter(|c| {
                (!c.is_control() || matches!(c, '\n' | '\r' | '\t'))
                    && !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            })
            .collect(),
        truncated,
    })
}

fn scan_archive(
    entry: &Entry,
    include_readme: bool,
    parse_fomod: bool,
    cancel: &CancellationToken,
) -> AppResult<ArchiveScan> {
    let preview = if include_readme || parse_fomod {
        let listing = archive::list_verified_archive(&entry.archive, &entry.sha256, cancel)?;
        let names = logical_files(&listing, Path::new(""))?;
        Some((listing, names))
    } else {
        None
    };
    let readme_path = preview
        .as_ref()
        .filter(|_| include_readme)
        .and_then(|(_, names)| readme_name(names).map(Path::to_path_buf));
    let fomod_path = preview
        .as_ref()
        .filter(|(_, names)| parse_fomod && contains_fomod(names))
        .map(|(_, names)| fomod::find_config(names.values().map(PathBuf::as_path)));
    let mut selected = BTreeMap::new();
    if let Some(path) = &readme_path {
        selected.insert(path.clone(), README_LIMIT);
    }
    if let Some(Ok(path)) = &fomod_path {
        selected.insert(path.clone(), fomod::MAX_CONFIG_BYTES + 1);
    }
    let proof = archive::verify_archive_members(&entry.archive, &entry.sha256, &selected, cancel)?;
    let names = match preview {
        Some((listing, names)) if listing.entries == proof.listing.entries => names,
        Some(_) => return Err(AppError::Conflict("模组库归档在预览期间变化".into())),
        None => logical_files(&proof.listing, Path::new(""))?,
    };
    let readme = readme_path.and_then(|path| {
        let bytes = proof.captured.get(&path)?;
        text_readme(&path, &bytes.prefix, bytes.truncated)
    });
    let fomod = fomod_path.map(|result| {
        let path = result?;
        let bytes = proof
            .captured
            .get(&path)
            .ok_or_else(|| invalid("无法读取 FOMOD 配置"))?;
        if bytes.truncated {
            return Err(invalid("FOMOD ModuleConfig.xml 超过 256 KiB"));
        }
        fomod::preview_verified_member(
            &bytes.prefix,
            path.to_string_lossy().replace('\\', "/"),
            entry.archive.clone(),
            entry.sha256.clone(),
            names.keys().cloned(),
        )
    });
    Ok(ArchiveScan {
        listing: proof.listing,
        names,
        hashes: proof.hashes,
        readme,
        fomod,
    })
}

fn adapter_for(engine: &FileEngine, game: &games::Game) -> AppResult<Option<GameAdapter>> {
    game.adapter_slug
        .as_ref()
        .map(|_| game_roots::adapter(&engine.db, game))
        .transpose()
}

fn resolve_root(
    engine: &FileEngine,
    game: &games::Game,
    adapter: Option<&GameAdapter>,
    selected: Option<&ModType>,
) -> AppResult<PathBuf> {
    let registered = Path::new(&game.root_path);
    // Even a registered game can never grant a deployment into app storage.
    engine.paths.stable_directory(registered)?;
    if let Some(adapter) = adapter {
        super::gta_layout::check_edition(game, &adapter.slug)?;
    }
    let root = if let (Some(adapter), Some(ty)) = (adapter, selected) {
        let user_roots = game_roots::registered_user_roots(&engine.db, &game.id)?;
        let declared = adapter
            .install_roots
            .iter()
            .find(|r| r.id == ty.root)
            .ok_or_else(|| invalid("适配包的模组类型未声明安装根"))?;
        let base = match declared.kind {
            RootKind::Game => registered,
            RootKind::RegisteredUser => user_roots
                .get(&ty.root)
                .map(PathBuf::as_path)
                .ok_or_else(|| AppError::Denied("适配包的用户管理根尚未登记".into()))?,
        };
        let expected = paths::lexical_normalize(&base.join(&declared.relative_path));
        let resolved = adapter.resolve_root(
            &ty.root,
            game_roots::platform(game)?,
            registered,
            &user_roots,
            &engine.roots()?,
        )?;
        if resolved != expected {
            return Err(AppError::Denied("适配包安装根不能经过符号链接".into()));
        }
        resolved
    } else {
        registered.to_path_buf()
    };
    if engine.paths.overlaps_storage(&root) {
        return Err(AppError::Denied("安装根不能与管理器存储重叠".into()));
    }
    engine.paths.guard(&root, &engine.roots()?)?;
    Ok(root)
}

fn valid_single_file(
    entry: &Entry,
    scan: Option<&ArchiveScan>,
    ty: &ModType,
    rule: &ArchiveRule,
) -> bool {
    let ext = Path::new(&entry.filename)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    ty.extensions.contains(&ext)
        && (!(ext == "jar" || ext == "zip" || !rule.required_files.is_empty())
            || scan.is_some_and(|scan| {
                rule.required_files
                    .iter()
                    .all(|required| scan.names.contains_key(required))
            }))
}

#[derive(Debug, PartialEq, Eq)]
struct AdapterMapping {
    layout: ArchiveLayout,
    source_kind: LibrarySourceKind,
    payload: MappedPayload,
    needs_folder: bool,
}

fn adapter_mappings(
    entry: &Entry,
    scan: Option<&ArchiveScan>,
    ty: &ModType,
    rules: &[&ArchiveRule],
    folder_name: Option<&str>,
) -> AppResult<Vec<AdapterMapping>> {
    let mut matches = Vec::new();
    for rule in rules
        .iter()
        .copied()
        .filter(|rule| rule.layout == ArchiveLayout::SingleFile)
    {
        if valid_single_file(entry, scan, ty, rule) {
            let next = AdapterMapping {
                layout: rule.layout,
                source_kind: LibrarySourceKind::SingleFile,
                payload: (
                    BTreeMap::from([(entry.filename.clone(), entry.archive.clone())]),
                    Vec::new(),
                ),
                needs_folder: false,
            };
            if !matches.contains(&next) {
                matches.push(next);
            }
        }
    }
    if let Some(scan) = scan.filter(|scan| !contains_fomod(&scan.names)) {
        let stripped;
        let mut candidates = vec![&scan.names];
        if let Some(wrapper) = scan.listing.stripped_root.as_deref() {
            stripped = strip_wrapper(&scan.names, Some(wrapper));
            if stripped != scan.names {
                candidates.push(&stripped);
            }
        }
        for files in candidates {
            let mut found = false;
            for rule in rules.iter().copied().filter(|rule| {
                matches!(
                    rule.layout,
                    ArchiveLayout::StripPrefix | ArchiveLayout::ModDirectory
                )
            }) {
                let needs_folder = rule.layout == ArchiveLayout::ModDirectory
                    && folder_name.is_none()
                    && rule
                        .required_files
                        .iter()
                        .all(|name| files.contains_key(name));
                if let Some(payload) =
                    map_rule(rule, files, folder_name.or(Some("library-placeholder")))?
                {
                    found = true;
                    let next = AdapterMapping {
                        layout: rule.layout,
                        source_kind: LibrarySourceKind::Archive,
                        payload,
                        needs_folder,
                    };
                    if !matches.contains(&next) {
                        matches.push(next);
                    }
                }
            }
            // Keep an existing mod directory if an unstripped rule matches.
            if found {
                break;
            }
        }
    }
    Ok(matches)
}

fn allows_fomod(ty: &ModType, rules: &[&ArchiveRule]) -> bool {
    ty.id == "fomod"
        && rules
            .iter()
            .any(|rule| rule.layout == ArchiveLayout::Manual)
        && !rules
            .iter()
            .any(|rule| rule.layout == ArchiveLayout::SingleFile)
}

pub fn match_library_install(
    engine: &FileEngine,
    request: &LibraryMatchRequest,
) -> AppResult<LibraryInstallMatch> {
    match_library_install_cancellable(engine, request, &CancellationToken::new())
}

pub fn match_library_install_cancellable(
    engine: &FileEngine,
    request: &LibraryMatchRequest,
    cancel: &CancellationToken,
) -> AppResult<LibraryInstallMatch> {
    check_cancel(cancel)?;
    let entry = load_entry(engine, &request.game_id, &request.library_entry_id)?;
    let game = games::get_game(&engine.db, &request.game_id)?;
    let adapter = adapter_for(engine, &game)?;
    let mut warnings =
        vec!["匹配结果与 README 都不证明作者身份或代码安全；部署必须审查完整文件计划".into()];
    if adapter.is_none() {
        warnings.push(
            "没有本地适配包；必须按作者说明提供显式、相对于已登记游戏根目录的文件映射".into(),
        );
    }
    let scan = if archive_intent(&entry)? {
        Some(scan_archive(&entry, true, true, cancel)?)
    } else {
        None
    };
    let fomod_status = scan
        .as_ref()
        .and_then(|scan| scan.fomod.as_ref())
        .map(|preview| match preview {
            Ok(preview) if preview.automatic_selection.status == "safe" => "automatic",
            Ok(_) => "requiresUser",
            Err(_) => "unavailable",
        });
    if let Some(Err(error)) = scan.as_ref().and_then(|scan| scan.fomod.as_ref()) {
        warnings.push(format!("无法安全预览 FOMOD：{error}"));
    }
    let mut candidates = Vec::new();
    if let Some(adapter) = &adapter {
        let gta = super::gta_layout::is_gta(&adapter.slug).then(|| {
            let names = scan
                .as_ref()
                .map(|s| s.names.keys().map(String::as_str).collect())
                .unwrap_or_else(|| vec![entry.filename.as_str()]);
            super::gta_layout::classify(names)
        });
        if let Some(hints) = &gta {
            warnings.extend(hints.warnings.iter().map(|s| (*s).to_string()));
            if adapter.slug == "gta5-enhanced"
                && hints.types.iter().any(|ty| ty.starts_with("rpf-"))
            {
                warnings.push(
                    "增强版 RPF 尚无独立兼容性验证；不要启动或借用 Legacy 工具处理。".to_owned(),
                );
            }
        }
        for ty in &adapter.mod_types {
            if let Some(hints) = &gta {
                if !hints.types.contains(&ty.id.as_str()) {
                    continue;
                }
                if ty.id.starts_with("rpf-") || hints.review {
                    candidates.push(LibraryAdapterMatch {
                        mod_type: ty.id.clone(),
                        root_id: ty.root.clone(),
                        layout: ArchiveLayout::Manual,
                        status: if super::gta_layout::check_edition(&game, &adapter.slug).is_err() {
                            "rootUnavailable"
                        } else if adapter.slug == "gta5-enhanced" && ty.id.starts_with("rpf-") {
                            "unsupportedEdition"
                        } else if hints.review {
                            "requiresUser"
                        } else {
                            "requiresGtaRpf"
                        },
                    });
                    continue;
                }
            }
            let rules: Vec<_> = adapter
                .archive_rules
                .iter()
                .filter(|rule| rule.mod_type == ty.id)
                .collect();
            let matches = adapter_mappings(&entry, scan.as_ref(), ty, &rules, None)?;
            let root_valid = resolve_root(engine, &game, Some(adapter), Some(ty)).is_ok();
            for matched in &matches {
                candidates.push(LibraryAdapterMatch {
                    mod_type: ty.id.clone(),
                    root_id: ty.root.clone(),
                    layout: matched.layout,
                    status: if !root_valid {
                        "rootUnavailable"
                    } else if matches.len() != 1 {
                        "ambiguous"
                    } else if matched.needs_folder {
                        "requiresFolderName"
                    } else {
                        "matched"
                    },
                });
            }
            if let Some(status) = fomod_status.filter(|_| allows_fomod(ty, &rules)) {
                candidates.push(LibraryAdapterMatch {
                    mod_type: ty.id.clone(),
                    root_id: ty.root.clone(),
                    layout: ArchiveLayout::Manual,
                    status: if !root_valid {
                        "rootUnavailable"
                    } else if status == "automatic" {
                        "matched"
                    } else {
                        "requiresUser"
                    },
                });
            } else if matches.is_empty()
                && rules
                    .iter()
                    .any(|rule| rule.layout == ArchiveLayout::Manual)
            {
                candidates.push(LibraryAdapterMatch {
                    mod_type: ty.id.clone(),
                    root_id: ty.root.clone(),
                    layout: ArchiveLayout::Manual,
                    status: if root_valid {
                        "requiresUser"
                    } else {
                        "rootUnavailable"
                    },
                });
            }
        }
    }
    let fomod = scan
        .as_ref()
        .and_then(|scan| scan.fomod.as_ref())
        .and_then(|result| result.as_ref().ok())
        .filter(|preview| serde_json::to_vec(*preview).is_ok_and(|json| json.len() <= 64 * 1024))
        .cloned();
    if matches!(fomod_status, Some("automatic" | "requiresUser")) && fomod.is_none() {
        warnings.push("FOMOD 配置超过匹配报告的展示限制；不能依靠不完整的选项推断映射".into());
    }
    let archive_file_count = scan.as_ref().map_or(0, |scan| scan.names.len());
    let mut budget = MATCH_PATH_BYTES;
    let archive_entries: Vec<_> = scan
        .iter()
        .flat_map(|scan| scan.names.keys())
        .take(MATCH_MEMBER_LIMIT)
        .take_while(|name| {
            budget.checked_sub(name.len()).is_some_and(|left| {
                budget = left;
                true
            })
        })
        .cloned()
        .collect();
    let archive_entries_truncated = archive_entries.len() < archive_file_count;
    Ok(LibraryInstallMatch {
        active_install_id: active_install_id(engine, &game.id, &entry.id)?,
        game_id: game.id,
        library_entry_id: entry.id,
        filename: entry.filename,
        artifact_sha256: entry.sha256,
        adapter_slug: adapter.map(|adapter| adapter.slug),
        candidates,
        archive_format: scan.as_ref().map(|scan| scan.listing.format.clone()),
        archive_entries,
        archive_file_count,
        archive_entries_truncated,
        readme: scan.and_then(|scan| scan.readme),
        fomod_status,
        fomod,
        warnings,
    })
}

// A complete archive can be a native game payload (for example a Minecraft JAR),
// but copying its container bytes cannot produce a native plugin or executable.
fn validate_single_file_target(entry: &Entry, destination: &Path) -> AppResult<()> {
    let native_binary = destination
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ["asi", "dll", "exe", "so", "dylib"]
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        });
    if native_binary && archive_intent(entry)? {
        return Err(invalid(format!(
            "不能把整个归档 {} 当作原生程序文件 {} 安装。请用 match_mod_install 查看成员，再用 sourceKind=archive 指定包内源文件。",
            entry.filename, paths::display_path(destination)
        )));
    }
    Ok(())
}

fn approved_mappings(
    entry: &Entry,
    mode: LibrarySourceKind,
    scan: Option<&ArchiveScan>,
    mappings: &[ManualMapping],
    cancel: &CancellationToken,
) -> AppResult<MappedPayload> {
    if mappings.is_empty() {
        return Err(invalid("必须提供至少一条审核过的文件映射"));
    }
    if mode == LibrarySourceKind::SingleFile && mappings.len() != 1 {
        return Err(invalid("单文件安装必须只映射原始文件名一次"));
    }
    super::check_mapping_metadata(mappings, cancel)?;
    let mut selected = BTreeSet::new();
    let mut payload = BTreeMap::new();
    for mapping in mappings {
        check_cancel(cancel)?;
        plugins::validate_relative_path(&mapping.source, false)?;
        plugins::validate_relative_path(&mapping.destination, false)?;
        let source = match mode {
            LibrarySourceKind::SingleFile if mapping.source == entry.filename => {
                validate_single_file_target(entry, Path::new(&mapping.destination))?;
                entry.archive.clone()
            }
            LibrarySourceKind::SingleFile => {
                return Err(invalid("单文件映射来源必须为模组库原始文件名"))
            }
            LibrarySourceKind::Archive => scan
                .and_then(|scan| scan.names.get(&mapping.source))
                .cloned()
                .ok_or_else(|| invalid(format!("归档无此文件: {}", mapping.source)))?,
        };
        if !selected.insert(mapping.source.as_str())
            || payload
                .insert(mapping.destination.clone(), source)
                .is_some()
        {
            return Err(invalid("文件映射含重复源或目标"));
        }
    }
    let ignored = if mode == LibrarySourceKind::Archive {
        scan.into_iter()
            .flat_map(|scan| scan.names.keys())
            .filter(|name| !selected.contains(name.as_str()))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    Ok((payload, ignored))
}

fn destinations(engine: &FileEngine, root: &Path, payload: &Payload) -> AppResult<Vec<PathBuf>> {
    let mut all = BTreeMap::new();
    let mut leaves = BTreeSet::new();
    let mut destinations = Vec::with_capacity(payload.len());
    if payload.is_empty() {
        return Err(invalid("安装计划没有文件"));
    }
    let roots = engine.roots()?;
    for name in payload.keys() {
        plugins::validate_relative_path(name, false)?;
        for parent in Path::new(name)
            .ancestors()
            .filter(|path| !path.as_os_str().is_empty())
        {
            let parent = parent
                .to_str()
                .expect("relative names are UTF-8")
                .replace('\\', "/");
            let key = parent.to_lowercase();
            if all
                .insert(key, parent.as_str().to_owned())
                .is_some_and(|old| old != parent)
            {
                return Err(invalid("安装目标含大小写路径冲突"));
            }
        }
        if name
            .split('/')
            .any(|part| part.eq_ignore_ascii_case(".amm-retained"))
        {
            return Err(invalid("安装目标不能使用事务保留目录"));
        }
        leaves.insert(name.as_str());
        let dst = paths::safe_join(root, name)?;
        if engine.paths.guard(&dst, &roots)? != dst {
            return Err(AppError::Denied("安装目标不能经过符号链接".into()));
        }
        destinations.push(dst);
    }
    for name in payload.keys() {
        if name
            .match_indices('/')
            .any(|(i, _)| leaves.contains(&name[..i]))
        {
            return Err(invalid("文件映射的目标相互包含"));
        }
    }
    Ok(destinations)
}

fn select_type<'a>(
    adapter: Option<&'a GameAdapter>,
    mod_type: Option<&str>,
) -> AppResult<Option<&'a ModType>> {
    match (adapter, mod_type) {
        (Some(adapter), Some(mod_type)) => adapter
            .mod_types
            .iter()
            .find(|ty| ty.id == mod_type)
            .map(Some)
            .ok_or_else(|| invalid("适配包未声明该模组类型")),
        (None, Some(_)) => Err(invalid("此游戏没有适配包；不得指定适配包模组类型")),
        (_, None) => Ok(None),
    }
}

fn validate_cache(
    root: &Path,
    scan: &ArchiveScan,
    cancel: &CancellationToken,
) -> AppResult<BTreeMap<PathBuf, String>> {
    let mut directories = BTreeSet::new();
    let mut sizes = BTreeMap::new();
    for item in &scan.listing.entries {
        let relative = if item.is_dir
            && item
                .path
                .split(['/', '\\'])
                .all(|part| part.is_empty() || part == ".")
        {
            continue;
        } else {
            paths::safe_join(Path::new(""), &item.path)?
        };
        let mut parents = relative.ancestors();
        if !item.is_dir {
            parents.next();
            sizes.insert(relative.clone(), item.size);
        }
        directories.extend(
            parents
                .filter(|path| !path.as_os_str().is_empty())
                .map(Path::to_path_buf),
        );
    }
    let mut todo = vec![PathBuf::new()];
    let mut verified = BTreeMap::new();
    while let Some(relative) = todo.pop() {
        // Windows no-follow helpers reject empty path components; joining an
        // empty relative path appends a trailing separator to the root.
        let directory = if relative.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(&relative)
        };
        let identity = safe_fs::identity(&directory)?
            .ok_or_else(|| AppError::Conflict("模组库载荷含链接或目录已变化".into()))?;
        for item in safe_fs::directory_entries(&directory, &identity)? {
            check_cancel(cancel)?;
            let (name, is_dir) = item?;
            let relative = relative.join(name);
            if is_dir && directories.remove(&relative) {
                todo.push(relative);
                continue;
            }
            let path = root.join(&relative);
            if !is_dir {
                if let Some((expected, size)) = scan.hashes.get(&relative).zip(sizes.get(&relative))
                {
                    if hash_regular(&path, *size, cancel)? == *expected {
                        verified.insert(path, expected.clone());
                        continue;
                    }
                }
            }
            return Err(AppError::Conflict(
                "模组库缓存载荷含未验证的文件、目录或链接".into(),
            ));
        }
    }
    if !directories.is_empty() || verified.len() != scan.hashes.len() {
        return Err(AppError::Conflict(
            "模组库缓存载荷缺少已验证的文件或目录".into(),
        ));
    }
    Ok(verified)
}

/// The optional GTA builder only receives snapshots of members verified against the library archive.
/// Never accept a caller-supplied extraction path. Preserve wrapper directories for explicit selections.
pub(super) fn gta_payload(
    engine: &FileEngine,
    game_id: &str,
    artifact_id: &str,
    cancel: &CancellationToken,
) -> AppResult<(mod_library::LibraryEntry, Payload)> {
    check_cancel(cancel)?;
    let entry = load_entry(engine, game_id, artifact_id)?;
    let members = if archive_intent(&entry)? {
        let scan = scan_archive(&entry, false, false, cancel)?;
        materialize(engine, &entry, &scan, cancel)?;
        scan.names
            .into_iter()
            .map(|(name, rel)| (name, entry.payload_root().join(rel)))
            .collect()
    } else {
        BTreeMap::from([(entry.filename.clone(), entry.archive.clone())])
    };
    let verified = mod_library::verify_for_install(engine, game_id, artifact_id)?;
    if verified.entry.sha256 != entry.sha256 || verified.archive_path != entry.archive {
        return Err(AppError::Conflict("GTA 模组原包在暂存期间发生变化".into()));
    }
    Ok((verified.entry, members))
}

/// An invalid existing cache is never overwritten or silently repaired. A new
/// extraction becomes visible only by publishing the complete directory.
fn materialize(
    engine: &FileEngine,
    entry: &Entry,
    scan: &ArchiveScan,
    cancel: &CancellationToken,
) -> AppResult<BTreeMap<PathBuf, String>> {
    let root = entry.payload_root();
    let parent = root.parent().unwrap();
    safe_fs::create_dir_all(parent)?;
    engine.paths.stable_directory(parent)?;
    match std::fs::symlink_metadata(&root) {
        Ok(_) => {
            engine.paths.stable_directory(&root)?;
            return validate_cache(&root, scan, cancel);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    // The extractor owns the unique scratch directory and cleans it only if
    // its identity still matches. Crash leftovers never become a generation.
    let staged = parent.join(util::new_id(".build"));
    let extracted = archive::extract_verified_archive_cancellable(
        &entry.archive,
        &staged,
        &entry.sha256,
        cancel,
    )?;
    if extracted.listing.entries != scan.listing.entries
        || extracted.files.len() != scan.hashes.len()
        || scan
            .hashes
            .iter()
            .any(|(name, hash)| extracted.files.get(&staged.join(name)) != Some(hash))
    {
        return Err(AppError::Conflict("解压结果与已验证归档内容不符".into()));
    }
    let identity = safe_fs::identity(&staged)?
        .ok_or_else(|| AppError::Conflict("暂存载荷目录已变化".into()))?;
    validate_cache(&staged, scan, cancel)?;
    check_cancel(cancel)?;
    safe_fs::rename_checked(&staged, &root, &identity)?;
    validate_cache(&root, scan, cancel)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallBinding {
    library_entry_id: String,
    artifact_sha256: String,
    source_kind: LibrarySourceKind,
    mod_type: String,
    adapter_slug: Option<String>,
    adapter_version: Option<String>,
    root_id: Option<String>,
}

fn active_install_id(
    engine: &FileEngine,
    game_id: &str,
    artifact_id: &str,
) -> AppResult<Option<String>> {
    Ok(engine
        .db
        .conn()
        .query_row(
            &format!(
                "SELECT id FROM installs WHERE game_id=?1 AND
          (json_extract(metadata_json, '$.libraryEntryId')=?2
           OR json_extract(metadata_json, '$.downloadOrigin.artifactId')=?2)
          AND ({}) IN ('enabled','partially_restored') LIMIT 1",
                super::INSTALL_STATE
            ),
            params![game_id, artifact_id],
            |row| row.get(0),
        )
        .optional()?)
}

/// Multiple pending reviews are allowed. Recheck under WRITE_LOCK at plan and apply.
fn require_artifact_not_active(
    engine: &FileEngine,
    game_id: &str,
    artifact_id: &str,
) -> AppResult<()> {
    if let Some(id) = active_install_id(engine, game_id, artifact_id)? {
        return Err(AppError::Conflict(format!(
            "该模组库归档已有活动安装记录（{id}）。请用 inspect_install 检查现有文件和归属，再按检查结果处理。"
        )));
    }
    Ok(())
}

pub fn plan_library_install(
    engine: &FileEngine,
    request: &LibraryInstallRequest,
) -> AppResult<InstallPreview> {
    plan_library_install_cancellable(engine, request, &CancellationToken::new())
}

pub fn plan_library_install_cancellable(
    engine: &FileEngine,
    request: &LibraryInstallRequest,
    cancel: &CancellationToken,
) -> AppResult<InstallPreview> {
    plan_library_install_with_organization(engine, request, None, cancel)
}

pub fn plan_library_install_with_organization(
    engine: &FileEngine,
    request: &LibraryInstallRequest,
    organization: Option<crate::mod_organization::OrganizationSuggestion>,
    cancel: &CancellationToken,
) -> AppResult<InstallPreview> {
    check_cancel(cancel)?;
    if let LibraryInstallMode::GtaRpf { job_id } = &request.mode {
        return super::gta_rpf::plan_install(
            engine,
            &request.game_id,
            &request.library_entry_id,
            job_id,
            organization,
            cancel,
        );
    }
    let entry = load_entry(engine, &request.game_id, &request.library_entry_id)?;
    let game = games::get_game(&engine.db, &request.game_id)?;
    let adapter = adapter_for(engine, &game)?;
    let requested_type = match &request.mode {
        LibraryInstallMode::GtaRpf { .. } => unreachable!("handled above"),
        LibraryInstallMode::Adapter { mod_type, .. } => Some(mod_type.as_str()),
        LibraryInstallMode::ApprovedMappings { mod_type, .. } => mod_type.as_deref(),
    };
    if let Some(adapter) = &adapter {
        super::gta_layout::check_copy_type(&adapter.slug, requested_type)?;
    }
    let ty = select_type(adapter.as_ref(), requested_type)?;
    let root = resolve_root(engine, &game, adapter.as_ref(), ty)?;
    let mod_type = ty.map_or(MANUAL_TYPE, |ty| ty.id.as_str());
    let mut warnings =
        vec!["模组库 SHA-256 只证明载荷完整性；安装不证明代码安全、加载器或依赖已就绪".into()];
    if let (Some(adapter), Some(ty)) = (&adapter, ty) {
        warnings.extend([ty.notes.clone(), adapter.load_order.notes.clone()]);
    }
    let scan;
    let (source_kind, (payload, ignored_files)) = match &request.mode {
        LibraryInstallMode::GtaRpf { .. } => unreachable!("handled above"),
        LibraryInstallMode::ApprovedMappings {
            source_kind,
            mappings,
            ..
        } => {
            scan = (*source_kind == LibrarySourceKind::Archive)
                .then(|| scan_archive(&entry, false, false, cancel))
                .transpose()?;
            warnings.push(if ty.is_some() {
                "安装使用显式文件映射；必须核对作者说明、选项、加载顺序及依赖".into()
            } else {
                "未选择适配包安装规则；只按审核过的作者文件映射相对于已登记游戏根目录安装".into()
            });
            (
                *source_kind,
                approved_mappings(&entry, *source_kind, scan.as_ref(), mappings, cancel)?,
            )
        }
        LibraryInstallMode::Adapter {
            folder_name,
            fomod_selection,
            ..
        } => {
            let adapter = adapter
                .as_ref()
                .ok_or_else(|| invalid("没有可用于自动匹配的本地适配包"))?;
            let ty = ty.ok_or_else(|| invalid("请选择适配包模组类型"))?;
            if let Some(folder) = folder_name {
                basename(folder)?;
            }
            let rules: Vec<_> = adapter
                .archive_rules
                .iter()
                .filter(|rule| rule.mod_type == ty.id)
                .collect();
            let use_fomod = fomod_selection.is_some();
            if use_fomod && (!allows_fomod(ty, &rules) || folder_name.is_some()) {
                return Err(invalid(
                    "FOMOD 自动选择必须使用适配包声明的 FOMOD 类型，不接受目录名",
                ));
            }
            let needs_scan = archive_intent(&entry)?;
            scan = (needs_scan || use_fomod)
                .then(|| scan_archive(&entry, false, use_fomod, cancel))
                .transpose()?;
            if use_fomod {
                let preview = scan
                    .as_ref()
                    .and_then(|scan| scan.fomod.as_ref())
                    .ok_or_else(|| invalid("归档没有有效的 FOMOD 配置"))?;
                let preview = preview
                    .as_ref()
                    .map_err(|error| invalid(error.to_string()))?;
                let selection = &preview.automatic_selection;
                if selection.status != "safe" {
                    return Err(AppError::Unsupported(
                        "FOMOD 需要用户选择；请先审核配置与文件映射".into(),
                    ));
                }
                let mappings: Vec<_> = selection
                    .mappings
                    .iter()
                    .map(|mapping| ManualMapping {
                        source: mapping.source.clone(),
                        destination: mapping.destination.clone(),
                    })
                    .collect();
                warnings.push("已从验证过的 FOMOD 配置生成确定性文件映射，仍需审核".into());
                (
                    LibrarySourceKind::Archive,
                    approved_mappings(
                        &entry,
                        LibrarySourceKind::Archive,
                        scan.as_ref(),
                        &mappings,
                        cancel,
                    )?,
                )
            } else {
                let mut matched =
                    adapter_mappings(&entry, scan.as_ref(), ty, &rules, folder_name.as_deref())?;
                if matched.len() != 1 {
                    return Err(AppError::Unsupported(
                        "此 Mod 未匹配到唯一的适配包自动安装规则。请按作者说明使用 mode=approvedMappings 提交逐文件映射，或明确选择 FOMOD 安装选项".into(),
                    ));
                }
                let matched = matched.pop().unwrap();
                if matched.needs_folder {
                    return Err(invalid("归档根部是模组内容，请指定 folderName"));
                }
                (matched.source_kind, matched.payload)
            }
        }
    };
    check_cancel(cancel)?;
    let guard = transaction::write_lock();
    check_cancel(cancel)?;
    require_artifact_not_active(engine, &game.id, &entry.id)?;
    let current_entry = load_entry(engine, &game.id, &entry.id)?;
    let current_game = games::get_game(&engine.db, &game.id)?;
    let current_adapter = adapter_for(engine, &current_game)?;
    let current_type = select_type(current_adapter.as_ref(), requested_type)?;
    if current_entry != entry
        || current_game.root_path != game.root_path
        || serde_json::to_value(&current_adapter)? != serde_json::to_value(&adapter)?
        || resolve_root(
            engine,
            &current_game,
            current_adapter.as_ref(),
            current_type,
        )? != root
    {
        return Err(AppError::Conflict(
            "模组库或游戏安装配置已变化，请重新生成计划".into(),
        ));
    }
    let destinations = destinations(engine, &root, &payload)?;
    if let Some(adapter) = &current_adapter {
        for dst in &destinations {
            super::gta_layout::check_copy_destination(
                &adapter.slug,
                Path::new(&game.root_path),
                dst,
            )?;
        }
    }
    super::verify_unowned_paths(engine, destinations.iter().map(PathBuf::as_path))?;
    let is_archive = source_kind == LibrarySourceKind::Archive;
    let hashes = if is_archive {
        materialize(
            engine,
            &entry,
            scan.as_ref()
                .ok_or_else(|| invalid("缺少验证归档的文件清单"))?,
            cancel,
        )?
    } else {
        BTreeMap::from([(entry.archive.clone(), entry.sha256.clone())])
    };
    let payload_root = entry.payload_root();
    let ops = payload
        .into_values()
        .zip(destinations)
        .map(|(src, dst)| FileOp::Copy {
            src: if is_archive {
                payload_root.join(src)
            } else {
                src
            },
            dst,
        })
        .collect();
    let mut plan = engine.build_locked_library_plan(
        &guard,
        ops,
        format!("从模组库安装 {} 到 {}", entry.name, game.name),
        &hashes,
    )?;
    plan.requires_review = true;
    validate_payload_sources(
        &plan,
        &entry.archive,
        &entry.sha256,
        is_archive.then_some(&hashes),
    )?;
    super::require_install_space(&plan, &root, cancel)?;
    if load_entry(engine, &game.id, &entry.id)? != entry {
        return Err(AppError::Conflict(
            "模组库条目已变化，请重新生成计划".into(),
        ));
    }
    let files: Vec<_> = plan
        .entries
        .iter()
        .map(|entry| {
            let source = entry
                .source
                .as_ref()
                .expect("library plan contains only verified copies");
            InstalledFile {
                path: entry.operation.dst().into(),
                size: source.size,
                sha256: source.sha256.clone(),
                before: entry.before.clone(),
            }
        })
        .collect();
    let binding = InstallBinding {
        library_entry_id: entry.id,
        artifact_sha256: entry.sha256.clone(),
        source_kind,
        mod_type: mod_type.to_owned(),
        adapter_slug: adapter.as_ref().map(|a| a.slug.clone()),
        adapter_version: adapter.as_ref().map(|a| a.version.clone()),
        root_id: ty.map(|ty| ty.root.clone()),
    };
    let mut metadata = serde_json::to_value(&binding)?;
    metadata["transactionId"] = serde_json::json!(plan.id);
    metadata["artifactPath"] = serde_json::json!(entry.archive);
    metadata["sourceModId"] =
        serde_json::json!(entry.origin.as_ref().map(|origin| &origin.project_id));
    metadata["downloadOrigin"] = serde_json::json!(entry.origin);
    metadata["warnings"] = serde_json::json!(warnings);
    metadata["ignoredFiles"] = serde_json::json!(ignored_files);
    crate::mod_organization::stage(&engine.db, &mut metadata, organization);
    check_cancel(cancel)?;
    let id = util::new_id("install");
    let timestamp = util::now();
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    FileEngine::persist_plan_kind(&tx, &plan, LIBRARY_INSTALL_KIND)?;
    tx.execute("INSERT INTO installs(id,game_id,name,version,source,status,install_root,files_json,metadata_json,installed_at,updated_at) VALUES (?1,?2,?3,?4,?5,'planned',?6,?7,?8,?9,?9)",
        params![id, game.id, entry.name, entry.version, entry.source, root.to_string_lossy(), serde_json::to_string(&files)?, metadata.to_string(), timestamp])?;
    tx.execute("INSERT INTO install_transactions(install_id,transaction_id,action,created_at) VALUES (?1,?2,'install',?3)", params![id, plan.id, timestamp])?;
    tx.execute(
        "UPDATE file_transactions SET game_id=?1 WHERE id=?2",
        params![game.id, plan.id],
    )?;
    tx.commit()?;
    Ok(InstallPreview {
        install_id: id,
        game_id: game.id,
        mod_type: binding.mod_type,
        install_root: root,
        artifact_sha256: entry.sha256,
        download_origin: entry.origin,
        plan,
        warnings,
        ignored_files,
        requires_review: true,
    })
}

/// Called for the dedicated journal kind only. Check durable provenance and
/// current game/adapter binding before FileEngine permits any source exception.
pub(crate) fn validate_library_apply(engine: &FileEngine, plan: &FilePlan) -> AppResult<()> {
    let (game_id, root, metadata): (String, String, String) = engine.db.conn().query_row(
        "SELECT i.game_id,i.install_root,i.metadata_json FROM installs i JOIN install_transactions t ON t.install_id=i.id WHERE t.transaction_id=?1 AND t.action='install'",
        [&plan.id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional()?.ok_or_else(|| AppError::Denied("模组库安装事务缺少对应安装清单".into()))?;
    let binding: InstallBinding = serde_json::from_str(&metadata)?;
    require_artifact_not_active(engine, &game_id, &binding.library_entry_id)?;
    super::verify_unowned_paths(
        engine,
        plan.entries.iter().map(|entry| entry.operation.dst()),
    )?;
    let entry = load_entry(engine, &game_id, &binding.library_entry_id)?;
    let game = games::get_game(&engine.db, &game_id)?;
    if entry.sha256 != binding.artifact_sha256 || game.adapter_slug != binding.adapter_slug {
        return Err(AppError::Conflict(
            "模组库来源或游戏适配包已变化，请重新生成计划".into(),
        ));
    }
    let adapter = adapter_for(engine, &game)?;
    let ty = select_type(
        adapter.as_ref(),
        binding.root_id.as_ref().map(|_| binding.mod_type.as_str()),
    )?;
    if ty.map(|ty| &ty.root) != binding.root_id.as_ref()
        || adapter.as_ref().map(|a| &a.version) != binding.adapter_version.as_ref()
        || resolve_root(engine, &game, adapter.as_ref(), ty)? != Path::new(&root)
    {
        return Err(AppError::Conflict(
            "安装根或适配包版本已变化，请重新生成计划".into(),
        ));
    }
    let root = Path::new(&root);
    let payload = entry.payload_root();
    if binding.source_kind == LibrarySourceKind::Archive {
        engine.paths.stable_directory(&payload)?;
    }
    for planned in &plan.entries {
        let FileOp::Copy { src, dst } = &planned.operation else {
            return Err(AppError::Denied("模组库安装只能复制已验证文件".into()));
        };
        let is_library_source = match binding.source_kind {
            LibrarySourceKind::SingleFile => {
                validate_single_file_target(&entry, dst)?;
                src == &entry.archive
                    && planned
                        .source
                        .as_ref()
                        .is_some_and(|source| source.sha256 == entry.sha256)
            }
            LibrarySourceKind::Archive => src != &payload && src.starts_with(&payload),
        };
        if let Some(adapter) = &adapter {
            super::gta_layout::check_copy_destination(
                &adapter.slug,
                Path::new(&game.root_path),
                dst,
            )?;
        }
        if !is_library_source || dst == root || !dst.starts_with(root) {
            return Err(AppError::Denied(
                "模组库安装的来源或目标超出已审核范围".into(),
            ));
        }
        safe_fs::verify_regular_at(&safe_fs::open_regular(src)?, src)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "install_library_tests.rs"]
mod tests;
