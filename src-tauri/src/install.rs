//! Adapter-driven deployment. A preview may materialize inert staging files,
//! but only the approved FileEngine plan can change game files.
use crate::{
    archive,
    artifacts::{self, DownloadOrigin},
    db::Db,
    error::{AppError, AppResult},
    fomod, game_roots, games, paths,
    plugins::{self, ArchiveLayout, ArchiveRule},
    transaction::{fingerprint, FileEngine, FileOp, FilePlan, Fingerprint},
    trash, util,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[path = "install_replacement.rs"]
mod replacement;
pub(crate) use replacement::{
    path_key, validate_apply, validate_rollback, validate_trash_restore_ownership,
    verify_unowned_paths,
};

#[path = "gta_layout.rs"]
mod gta_layout;
pub(crate) mod gta_rpf;

#[path = "install_library.rs"]
mod library;
use crate::mod_organization::{self, OrganizationSuggestion};
pub use library::{
    match_library_install, match_library_install_cancellable, plan_library_install,
    plan_library_install_cancellable, plan_library_install_with_organization, LibraryInstallMatch,
    LibraryInstallMode, LibraryInstallRequest, LibraryMatchRequest, LibrarySourceKind,
};
pub(crate) use library::{validate_library_apply, LIBRARY_INSTALL_KIND};

use tokio_util::sync::CancellationToken;
type Payload = BTreeMap<String, PathBuf>;
type MappedPayload = (Payload, Vec<String>);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualMapping {
    pub source: String,
    pub destination: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FomodSelection {
    Automatic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstallRequest {
    pub game_id: String,
    pub artifact_path: PathBuf,
    /// The source filename, without the download cache's unique ID prefix.
    pub filename: String,
    pub name: String,
    pub mod_type: String,
    pub version: Option<String>,
    pub expected_sha256: Option<String>,
    pub folder_name: Option<String>,
    #[serde(default)]
    pub mappings: Vec<ManualMapping>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fomod_selection: Option<FomodSelection>,
    #[serde(default = "local_source")]
    pub source: String,
    pub source_mod_id: Option<String>,
    pub replace_install_id: Option<String>,
}
fn local_source() -> String {
    "local".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub before: Option<Fingerprint>,
}

impl InstalledFile {
    fn fingerprint(&self) -> Fingerprint {
        Fingerprint {
            kind: "file".into(),
            size: self.size,
            sha256: self.sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledMod {
    #[serde(default)]
    pub tag_ids: Vec<String>,
    pub id: String,
    pub game_id: String,
    pub name: String,
    pub version: Option<String>,
    pub source: String,
    /// Exact coordinates recorded at deployment, not proof of authorship.
    pub download_origin: Option<DownloadOrigin>,
    pub status: String,
    pub install_root: PathBuf,
    pub files: Vec<InstalledFile>,
    pub metadata: serde_json::Value,
    pub installed_at: String,
}

impl InstalledMod {
    pub(crate) fn into_user_paths(mut self) -> Self {
        self.install_root = paths::user_path(&self.install_root).into_owned();
        for file in &mut self.files {
            file.path = paths::user_path(&file.path).into_owned();
        }
        if let Some(serde_json::Value::String(path)) = self.metadata.get_mut("artifactPath") {
            paths::set_user_path_text(path);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallInspection {
    pub install_id: String,
    pub game_id: String,
    pub status: String,
    pub can_uninstall: bool,
    pub files: Vec<InstallFileInspection>,
    pub reconciliation: InstallReconciliation,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallFileInspection {
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub path: PathBuf,
    pub expected_fingerprint: Fingerprint,
    pub current_fingerprint: Option<Fingerprint>,
    pub state: String,
    pub replaced: bool,
    pub other_install_ids: Vec<String>,
    pub reasons: Vec<String>,
}

/// Read-only coordination advice for an install that overlaps another
/// deployment or participates in a replacement chain. The suggested action
/// never authorizes a write; each operation must recheck its own preconditions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReconciliation {
    pub recommended_action: String,
    pub blocking_install_ids: Vec<String>,
    pub rollback_transaction_id: Option<String>,
    pub successor_install_id: Option<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    pub install_id: String,
    pub game_id: String,
    pub mod_type: String,
    #[serde(serialize_with = "crate::paths::serialize_user_path")]
    pub install_root: PathBuf,
    pub artifact_sha256: String,
    pub download_origin: Option<DownloadOrigin>,
    #[serde(serialize_with = "crate::transaction::serialize_user_plan")]
    pub plan: FilePlan,
    pub warnings: Vec<String>,
    pub ignored_files: Vec<String>,
    /// Source hashes prove integrity, not authenticity or code safety.
    pub requires_review: bool,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::Invalid(message.into())
}

fn basename(value: &str) -> AppResult<()> {
    plugins::validate_relative_path(value, false)?;
    if value.contains('/') || value.starts_with('.') || value.len() > 180 {
        return Err(invalid("必须使用单个普通文件名"));
    }
    Ok(())
}

fn document(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !name.contains('/')
        && [
            "readme",
            "license",
            "licence",
            "changelog",
            "changes",
            "credits",
        ]
        .iter()
        .any(|prefix| name == *prefix || name.starts_with(&format!("{prefix}.")))
}

fn required(files: &BTreeMap<String, PathBuf>, prefix: &str, rule: &ArchiveRule) -> bool {
    rule.required_files
        .iter()
        .all(|name| files.contains_key(&format!("{prefix}{name}")))
}

fn logical_files(
    listing: &archive::ArchiveListing,
    staging: &Path,
) -> AppResult<BTreeMap<String, PathBuf>> {
    let mut files = BTreeMap::new();
    for item in listing.entries.iter().filter(|e| !e.is_dir) {
        let relative = paths::safe_join(Path::new(""), &item.path)?;
        let name = relative
            .to_str()
            .ok_or_else(|| invalid("归档路径必须为 UTF-8"))?
            .replace('\\', "/");
        files.insert(name, staging.join(relative));
    }
    if files.is_empty() {
        return Err(invalid("归档中没有可安装的文件"));
    }
    Ok(files)
}

fn strip_wrapper(
    files: &BTreeMap<String, PathBuf>,
    wrapper: Option<&str>,
) -> BTreeMap<String, PathBuf> {
    let Some(wrapper) = wrapper else {
        return files.clone();
    };
    let prefix = format!("{wrapper}/");
    if files.keys().all(|name| name.starts_with(&prefix)) {
        files
            .iter()
            .map(|(name, path)| (name[prefix.len()..].into(), path.clone()))
            .collect()
    } else {
        files.clone()
    }
}

fn map_rule(
    rule: &ArchiveRule,
    files: &BTreeMap<String, PathBuf>,
    folder_name: Option<&str>,
) -> AppResult<Option<MappedPayload>> {
    match rule.layout {
        ArchiveLayout::StripPrefix => {
            let prefix = format!("{}/", rule.source_prefix.as_ref().unwrap());
            if !files.keys().any(|p| p.starts_with(&prefix)) || !required(files, &prefix, rule) {
                return Ok(None);
            }
            let mut payload = BTreeMap::new();
            let mut ignored = Vec::new();
            for (name, path) in files {
                if let Some(relative) = name.strip_prefix(&prefix) {
                    payload.insert(relative.into(), path.clone());
                } else if document(name) {
                    ignored.push(name.clone());
                } else {
                    return Ok(None);
                }
            }
            Ok(Some((payload, ignored)))
        }
        ArchiveLayout::ModDirectory => {
            if required(files, "", rule) {
                let folder = folder_name.ok_or_else(|| {
                    invalid("归档根部是模组内容，请指定 folderName 保留独立模组目录")
                })?;
                basename(folder)?;
                return Ok(Some((
                    files
                        .iter()
                        .map(|(n, p)| (format!("{folder}/{n}"), p.clone()))
                        .collect(),
                    Vec::new(),
                )));
            }
            let mut payload = BTreeMap::new();
            let mut ignored = Vec::new();
            for (name, path) in files {
                if let Some((top, _)) = name.split_once('/') {
                    if !required(files, &format!("{top}/"), rule) {
                        return Ok(None);
                    }
                    payload.insert(name.clone(), path.clone());
                } else if document(name) {
                    ignored.push(name.clone());
                } else {
                    return Ok(None);
                }
            }
            if payload.is_empty() {
                return Ok(None);
            }
            Ok(Some((payload, ignored)))
        }
        _ => Ok(None),
    }
}

fn artifact(engine: &FileEngine, request: &InstallRequest) -> AppResult<PathBuf> {
    engine.paths.stable_directory(&engine.paths.root)?;
    engine.paths.stable_directory(&engine.paths.downloads())?;
    engine.paths.stable_directory(&engine.paths.staging())?;
    engine.paths.guard(&request.artifact_path, &[])?;
    // A retained transport copy has the same game binding as its library entry.
    if let Some(verified) =
        crate::mod_library::for_install_path(engine, &request.game_id, &request.artifact_path)?
    {
        return Err(invalid(format!(
            "此归档已归入游戏 Mod 库，请使用 install_library_mod 和 artifactId {}",
            verified.entry.id
        )));
    }
    let path = paths::ensure_within(
        &request.artifact_path,
        &[engine.paths.downloads(), engine.paths.staging()],
    )?;
    if !std::fs::symlink_metadata(&request.artifact_path)?
        .file_type()
        .is_file()
    {
        return Err(AppError::Denied(
            "载荷必须是普通文件，不能是符号链接".into(),
        ));
    }
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| invalid("载荷文件名无效"))?;
    if [".part", ".rejected", ".json"]
        .iter()
        .any(|suffix| filename.to_ascii_lowercase().ends_with(suffix))
    {
        return Err(AppError::Denied(
            "不能安装下载片段、隔离文件或缓存元数据".into(),
        ));
    }
    Ok(path)
}

fn check_mapping_metadata(mappings: &[ManualMapping], cancel: &CancellationToken) -> AppResult<()> {
    // Bound mapping memory independent of how many entries the archive has.
    mappings.iter().try_fold(0usize, |bytes, mapping| {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        bytes
            .checked_add(128)
            .and_then(|bytes| bytes.checked_add(mapping.source.len()))
            .and_then(|bytes| bytes.checked_add(mapping.destination.len()))
            .filter(|bytes| *bytes <= 64 * 1024 * 1024)
            .ok_or_else(|| invalid("安装文件映射超过元数据预算"))
    })?;
    Ok(())
}

/// Produces the exact deployment plan and saves its install manifest before
/// execution, so a restart between filesystem commit and UI refresh is harmless.
pub fn plan_install(engine: &FileEngine, request: &InstallRequest) -> AppResult<InstallPreview> {
    plan_install_cancellable(engine, request, &CancellationToken::new())
}

pub(crate) fn plan_install_cancellable(
    engine: &FileEngine,
    request: &InstallRequest,
    cancel: &CancellationToken,
) -> AppResult<InstallPreview> {
    plan_install_with_organization(engine, request, None, cancel)
}

pub fn plan_install_with_organization(
    engine: &FileEngine,
    request: &InstallRequest,
    organization: Option<OrganizationSuggestion>,
    cancel: &CancellationToken,
) -> AppResult<InstallPreview> {
    let check_cancel = || {
        if cancel.is_cancelled() {
            Err(AppError::Cancelled)
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    check_mapping_metadata(&request.mappings, cancel)?;
    basename(&request.filename)?;
    if request.name.trim().is_empty()
        || request.name.len() > 512
        || request.source.len() > 128
        || request.version.as_ref().is_some_and(|v| v.len() > 256)
    {
        return Err(invalid("模组元数据为空或超过限制"));
    }
    if request.fomod_selection.is_some() && !request.mappings.is_empty() {
        return Err(invalid("FOMOD 自动选择与手动文件映射不能同时使用"));
    }
    let game = games::get_game(&engine.db, &request.game_id)?;
    let adapter = game_roots::adapter(&engine.db, &game)?;
    let platform = game_roots::platform(&game)?;
    let ty = adapter
        .mod_types
        .iter()
        .find(|ty| ty.id == request.mod_type)
        .ok_or_else(|| invalid("适配包未声明该模组类型"))?;
    let install_root = adapter.resolve_root(
        &ty.root,
        platform,
        Path::new(&game.root_path),
        &game_roots::registered_user_roots(&engine.db, &game.id)?,
        &engine.roots()?,
    )?;
    gta_layout::check_edition(&game, &adapter.slug)?;
    gta_layout::check_copy_type(&adapter.slug, Some(&ty.id))?;
    let artifact = artifact(engine, request)?;
    let artifact_sha256 = archive::hash_file_cancellable(&artifact, cancel)?;
    let catalog = artifacts::for_path(&engine.db, &artifact)?;
    if let Some(record) = &catalog {
        if record.artifact.path != artifact
            || record.artifact.sha256 != artifact_sha256
            || record.artifact.filename != request.filename
        {
            return Err(AppError::Conflict(
                "下载完成后载荷或来源文件名已变化，保留现场并重新下载".into(),
            ));
        }
    }
    let download_origin = catalog.as_ref().and_then(artifacts::ArtifactRecord::origin);
    let (source, project_id, version) = match (&catalog, &download_origin) {
        (Some(record), _) if record.source_id == artifacts::WEB_SOURCE => {
            if !matches!(request.source.as_str(), "local" | artifacts::WEB_SOURCE)
                || request
                    .source_mod_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty())
                || request.version.as_ref().is_some_and(|v| !v.is_empty())
            {
                return Err(AppError::Conflict(
                    "网页下载不接受其他来源、项目或版本标签".into(),
                ));
            }
            (artifacts::WEB_SOURCE, None, None)
        }
        (Some(record), Some(origin)) => {
            if (request.source != "local" && request.source != origin.source_id)
                || request
                    .source_mod_id
                    .as_ref()
                    .is_some_and(|id| id != &origin.project_id)
                || request
                    .version
                    .as_ref()
                    .is_some_and(|version| version != &record.version)
            {
                return Err(AppError::Conflict(
                    "安装参数的来源、项目或版本与下载记录不符".into(),
                ));
            }
            let version = &record.version;
            (
                origin.source_id.as_str(),
                Some(origin.project_id.as_str()),
                (!version.trim().is_empty()
                    && version.len() <= 256
                    && !version.chars().any(char::is_control))
                .then_some(version.as_str()),
            )
        }
        _ if request.source == artifacts::WEB_SOURCE => {
            return Err(AppError::Conflict("载荷没有对应的网页下载记录".into()));
        }
        _ => (
            request.source.as_str(),
            request.source_mod_id.as_deref(),
            request.version.as_deref(),
        ),
    };
    if let Some(hash) = &request.expected_sha256 {
        if hash.len() != 64
            || !hash.bytes().all(|b| b.is_ascii_hexdigit())
            || !hash.eq_ignore_ascii_case(&artifact_sha256)
        {
            return Err(AppError::Conflict(
                "载荷 SHA-256 与预期不符，拒绝安装".into(),
            ));
        }
    }
    let rules: Vec<_> = adapter
        .archive_rules
        .iter()
        .filter(|r| r.mod_type == request.mod_type)
        .collect();
    let single_file_rule = rules
        .iter()
        .find(|rule| rule.layout == ArchiveLayout::SingleFile);
    if request.fomod_selection.is_some()
        && (ty.id != "fomod"
            || single_file_rule.is_some()
            || !rules
                .iter()
                .any(|rule| rule.layout == ArchiveLayout::Manual))
    {
        return Err(invalid(
            "FOMOD 自动选择需要适配包声明的 fomod 模组类型和手动归档规则",
        ));
    }
    let mut warnings = vec![
        ty.notes.clone(),
        adapter.load_order.notes.clone(),
        "安装文件不等于加载器已启用；代码、游戏版本和依赖仍需审查".into(),
    ];
    if request.expected_sha256.is_none() {
        warnings.push("没有来源 SHA-256；计算出的本地 hash 不证明来源真实性".into());
    }
    let mut ignored_files = Vec::new();
    let payload: BTreeMap<String, PathBuf>;
    let staged_hashes;
    let mut staging_cleanup = None;
    if let Some(rule) = single_file_rule.filter(|_| request.mappings.is_empty()) {
        let ext = Path::new(&request.filename)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !ty.extensions.contains(&ext) {
            return Err(invalid("载荷扩展名与模组类型不符"));
        }
        if ext == "jar" || ext == "zip" || !rule.required_files.is_empty() {
            let listing = archive::list_verified_archive(&artifact, &artifact_sha256, cancel)?;
            let names: BTreeSet<_> = listing
                .entries
                .iter()
                .filter(|e| !e.is_dir)
                .map(|e| e.path.replace('\\', "/"))
                .collect();
            if !rule.required_files.iter().all(|name| names.contains(name)) {
                return Err(invalid("单文件包缺少根部必需文件"));
            }
        }
        payload = BTreeMap::from([(request.filename.clone(), artifact.clone())]);
        staged_hashes = None;
    } else {
        let staging = engine.paths.staging().join(util::new_id("install"));
        engine.stable_path(&staging, true)?;
        let extraction = archive::extract_verified_archive_cancellable(
            &artifact,
            &staging,
            &artifact_sha256,
            cancel,
        )?;

        let automatic_mappings;
        let mappings = match request.fomod_selection {
            Some(FomodSelection::Automatic) => {
                let selection = fomod::preview_extracted(
                    &extraction,
                    &staging,
                    artifact.clone(),
                    artifact_sha256.clone(),
                )?
                .automatic_selection;
                if selection.status != "safe" {
                    return Err(AppError::Unsupported(
                        "FOMOD 需要用户选择。请先预览配置，确认选项与文件映射后再安装".into(),
                    ));
                }
                automatic_mappings = selection
                    .mappings
                    .into_iter()
                    .map(|mapping| ManualMapping {
                        source: mapping.source,
                        destination: mapping.destination,
                    })
                    .collect::<Vec<_>>();
                warnings.push("已从校验后的 FOMOD 归档选定确定性映射，请核对完整计划".into());
                automatic_mappings.as_slice()
            }
            None => request.mappings.as_slice(),
        };
        staging_cleanup = Some(extraction.cleanup);
        let listing = extraction.listing;
        staged_hashes = Some(extraction.files);
        let files = logical_files(&listing, &staging)?;
        if !mappings.is_empty() {
            let mut mapped = BTreeMap::new();
            let mut selected = BTreeSet::new();
            for mapping in mappings {
                check_cancel()?;
                plugins::validate_relative_path(&mapping.source, false)?;
                plugins::validate_relative_path(&mapping.destination, false)?;
                let src = files
                    .get(&mapping.source)
                    .ok_or_else(|| invalid(format!("归档无此文件: {}", mapping.source)))?;
                if mapped
                    .insert(mapping.destination.clone(), src.clone())
                    .is_some()
                    || !selected.insert(mapping.source.clone())
                {
                    return Err(invalid("文件映射含重复源或目标"));
                }
            }
            ignored_files = files
                .keys()
                .filter(|n| !selected.contains(*n))
                .cloned()
                .collect();
            if request.fomod_selection.is_none() {
                warnings.push("手动文件映射：必须逐项核对安装选项、加载顺序和依赖".into());
            }
            payload = mapped;
        } else {
            if files
                .keys()
                .any(|n| n.to_ascii_lowercase().split('/').any(|p| p == "fomod"))
            {
                return Err(AppError::Unsupported(
                    "检测到 FOMOD 配置；请选择适配包的 fomod 类型并指定 fomodSelection=automatic，或在人工确认后提交 mappings"
                        .into(),
                ));
            }
            let mut mapped = None;
            // Preserve real mod directories. Only attempt a single wrapper strip
            // if the unstripped payload does not satisfy any declaration.
            for candidate in [
                files.clone(),
                strip_wrapper(&files, listing.stripped_root.as_deref()),
            ] {
                for rule in &rules {
                    if let Some(result) =
                        map_rule(rule, &candidate, request.folder_name.as_deref())?
                    {
                        mapped = Some(result);
                        break;
                    }
                }
                if mapped.is_some() {
                    break;
                }
            }
            let (files, ignored) = mapped.ok_or_else(|| {
                AppError::Unsupported(
                    "归档布局无法唯一安全匹配；请提供审查后的 mappings 文件映射".into(),
                )
            })?;
            payload = files;
            ignored_files = ignored;
        }
    }
    if payload.is_empty() {
        return Err(invalid("安装计划没有文件"));
    }
    let mut aliases = BTreeSet::new();
    let mut ops = Vec::new();
    for (name, src) in payload {
        plugins::validate_relative_path(&name, false)?;
        if name
            .split('/')
            .any(|p| p.eq_ignore_ascii_case(".amm-retained"))
            || !aliases.insert(name.to_lowercase())
        {
            return Err(invalid("载荷含保留目录或大小写冲突"));
        }
        let dst = paths::ensure_within(
            &install_root.join(&name),
            std::slice::from_ref(&install_root),
        )?;
        gta_layout::check_copy_destination(&adapter.slug, Path::new(&game.root_path), &dst)?;
        ops.push(FileOp::Copy { src, dst });
    }
    check_cancel()?;
    let guard = crate::transaction::write_lock();
    check_cancel()?;
    let replacement = request
        .replace_install_id
        .as_deref()
        .map(|id| {
            replacement::Replacement::load(
                engine,
                id,
                &game.id,
                &install_root,
                &request.mod_type,
                download_origin.as_ref(),
            )
        })
        .transpose()?;
    let drafts = replacement
        .as_ref()
        .map(|r| r.drafts(engine, &ops))
        .transpose()?
        .unwrap_or_default();
    let summary = match (&replacement, &download_origin) {
        (Some(replacement), Some(next)) => {
            let previous = replacement.previous.download_origin.as_ref().unwrap();
            format!(
                "替换 {}（{}，项目 ID：{}，版本 ID：{} → {}） · {}",
                request.name,
                next.source_id,
                next.project_id,
                previous.version_id,
                next.version_id,
                game.name
            )
        }
        (_, Some(origin)) => format!(
            "安装 {}（{}，项目 ID：{}，版本 ID：{}）到 {}",
            request.name, origin.source_id, origin.project_id, origin.version_id, game.name
        ),
        _ => format!("安装 {} 到 {}", request.name, game.name),
    };
    let internal = replacement.is_some();
    let ops = drafts
        .iter()
        .map(|draft| draft.operation.clone())
        .chain(ops)
        .collect();
    check_cancel()?;
    let mut plan = engine.build_locked_plan(&guard, ops, summary, internal)?;
    plan.requires_review = true;
    validate_payload_sources(&plan, &artifact, &artifact_sha256, staged_hashes.as_ref())?;
    require_install_space(&plan, &install_root, cancel)?;
    if let Some(replacement) = &replacement {
        replacement.inspect_plan(engine, &plan, None)?;
        warnings.push("这是明确选择的版本替换，不判断版本先后、加载器兼容性或依赖是否满足".into());
    } else {
        verify_unowned_paths(
            engine,
            plan.entries.iter().map(|entry| entry.operation.dst()),
        )?;
    }
    if archive::hash_file_cancellable(&artifact, cancel)? != artifact_sha256 {
        return Err(AppError::Conflict(
            "生成预览时载荷已变化，请重新生成计划".into(),
        ));
    }
    let files: Vec<_> = plan
        .entries
        .iter()
        .filter(|entry| matches!(entry.operation, FileOp::Copy { .. }))
        .map(|entry| {
            let source = entry.source.as_ref().unwrap();
            InstalledFile {
                path: entry.operation.dst().into(),
                sha256: source.sha256.clone(),
                size: source.size,
                // The replaced version exclusively owned these bytes. Its
                // backup belongs to the journal, not to the new ownership baseline.
                before: if internal { None } else { entry.before.clone() },
            }
        })
        .collect();
    let id = util::new_id("install");
    let mut metadata = serde_json::json!({"transactionId":plan.id,"artifactPath":artifact,"artifactSha256":artifact_sha256,
        "adapterSlug":adapter.slug,"adapterVersion":adapter.version,"modType":request.mod_type,"rootId":ty.root,
        "sourceModId":project_id,"downloadOrigin":download_origin,"replacesInstallId":request.replace_install_id,"warnings":warnings,"ignoredFiles":ignored_files});
    mod_organization::stage(&engine.db, &mut metadata, organization);
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    FileEngine::persist_plan(&tx, &plan, internal)?;
    tx.execute("INSERT INTO installs(id,game_id,name,version,source,status,install_root,files_json,metadata_json,installed_at,updated_at) VALUES (?1,?2,?3,?4,?5,'planned',?6,?7,?8,?9,?9)",
        params![id,game.id,request.name,version,source,install_root.to_string_lossy(),serde_json::to_string(&files)?,metadata.to_string(),util::now()])?;
    tx.execute("INSERT INTO install_transactions(install_id,transaction_id,action,created_at) VALUES (?1,?2,'install',?3)",params![id,plan.id,util::now()])?;
    if let Some(replacement) = replacement {
        tx.execute("INSERT INTO install_replacements(transaction_id,replaced_install_id,replacement_install_id) VALUES (?1,?2,?3)",
            params![plan.id,replacement.previous.id,id])?;
        trash::record_prepared(engine, &tx, &plan.id, drafts)?;
    }
    tx.execute(
        "UPDATE file_transactions SET game_id=?1 WHERE id=?2",
        params![game.id, plan.id],
    )?;
    check_cancel()?;
    tx.commit()?;
    if let Some(cleanup) = &mut staging_cleanup {
        cleanup.keep();
    }
    Ok(InstallPreview {
        install_id: id,
        game_id: game.id,
        mod_type: request.mod_type.clone(),
        install_root,
        artifact_sha256,
        download_origin,
        plan,
        warnings,
        ignored_files,
        requires_review: true,
    })
}

fn require_install_space(
    plan: &FilePlan,
    install_root: &Path,
    cancel: &CancellationToken,
) -> AppResult<()> {
    // Advisory admission. The FileEngine still owns writes/backups and handles
    // actual I/O errors and rollback if the available space changes after review.
    let required_space = plan.entries.iter().try_fold(0u64, |total, entry| {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let bytes = match (&entry.operation, &entry.source) {
            (FileOp::Copy { .. }, Some(source)) => source.size.checked_add(4096),
            _ => Some(0),
        };
        bytes
            .and_then(|bytes| total.checked_add(bytes))
            .ok_or_else(|| invalid("安装空间预算溢出"))
    })?;
    let mut volume_path = install_root;
    while !volume_path.try_exists()? {
        volume_path = volume_path
            .parent()
            .ok_or_else(|| invalid("无法检查安装卷可用空间"))?;
    }
    crate::safe_fs::DiskBudget::new(volume_path)?.require(required_space)
}

fn validate_payload_sources(
    plan: &FilePlan,
    artifact: &Path,
    artifact_sha256: &str,
    staged_hashes: Option<&BTreeMap<PathBuf, String>>,
) -> AppResult<()> {
    for entry in &plan.entries {
        let FileOp::Copy { src, .. } = &entry.operation else {
            continue;
        };
        let expected = match staged_hashes {
            Some(hashes) => hashes.get(src).map(String::as_str),
            None if src == artifact => Some(artifact_sha256),
            _ => None,
        };
        if expected.is_none() || entry.source.as_ref().map(|fp| fp.sha256.as_str()) != expected {
            return Err(AppError::Conflict(
                "安装预览的源路径或内容与已验证载荷不符".into(),
            ));
        }
    }
    Ok(())
}

// Keep read-only dependency reports and the persisted UI status in sync with
// the same journal rules, including the interval before reconciliation runs.
pub(crate) const INSTALL_STATE: &str = "CASE
      WHEN EXISTS(SELECT 1 FROM install_transactions a JOIN file_transactions f ON f.id=a.transaction_id WHERE a.install_id=installs.id AND a.action='install' AND f.status='rolled_back') THEN 'rolled_back'
      WHEN NOT EXISTS(SELECT 1 FROM install_transactions a JOIN file_transactions f ON f.id=a.transaction_id WHERE a.install_id=installs.id AND a.action='install' AND f.status='committed') THEN 'planned'
      WHEN EXISTS(SELECT 1 FROM install_replacements r JOIN file_transactions f ON f.id=r.transaction_id WHERE r.replaced_install_id=installs.id AND f.status='committed') THEN 'superseded'
      WHEN EXISTS(SELECT 1 FROM install_transactions a JOIN file_transactions f ON f.id=a.transaction_id WHERE a.install_id=installs.id AND a.action='uninstall' AND f.status='committed'
        AND EXISTS(SELECT 1 FROM trash_items t WHERE t.transaction_id=f.id AND NOT EXISTS(SELECT 1 FROM trash_restore_links r JOIN file_transactions rf ON rf.id=r.transaction_id WHERE r.item_id=t.id AND rf.status='committed'))
        AND EXISTS(SELECT 1 FROM trash_items t WHERE t.transaction_id=f.id AND EXISTS(SELECT 1 FROM trash_restore_links r JOIN file_transactions rf ON rf.id=r.transaction_id WHERE r.item_id=t.id AND rf.status='committed'))) THEN 'partially_restored'
      WHEN EXISTS(SELECT 1 FROM install_transactions a JOIN file_transactions f ON f.id=a.transaction_id WHERE a.install_id=installs.id AND a.action='uninstall' AND f.status='committed'
        AND (f.kind='gta_rpf' OR EXISTS(SELECT 1 FROM trash_items t WHERE t.transaction_id=f.id AND NOT EXISTS(SELECT 1 FROM trash_restore_links r JOIN file_transactions rf ON rf.id=r.transaction_id WHERE r.item_id=t.id AND rf.status='committed')))) THEN 'uninstalled'
      WHEN EXISTS(SELECT 1 FROM install_missing_removals m WHERE m.install_id=installs.id) THEN 'removed_missing'
      ELSE 'enabled' END";

pub(crate) const FORGET_MISSING_FILES: &str = "forget_missing_files";

pub(crate) fn effective_install_state() -> String {
    format!("CASE WHEN EXISTS(SELECT 1 FROM install_transactions a WHERE a.install_id=installs.id) THEN ({INSTALL_STATE}) ELSE status END")
}

/// Derived from durable transaction states, not a success flag set by the UI.
/// Safe after both normal execution and startup journal recovery.
pub fn reconcile(db: &Db) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    reconcile_from(&tx)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn reconcile_from(conn: &rusqlite::Connection) -> AppResult<()> {
    conn.execute_batch(&format!("UPDATE installs SET status={INSTALL_STATE}
      WHERE EXISTS(SELECT 1 FROM install_transactions a WHERE a.install_id=installs.id);
      UPDATE games SET installed_mod_count=(SELECT count(*) FROM installs WHERE game_id=games.id AND status IN ('enabled','partially_restored')),
        enabled_mod_count=(SELECT count(*) FROM installs WHERE game_id=games.id AND status='enabled');"))?;
    crate::mod_management::restore_membership(conn)
}

fn read_installs(
    engine: &FileEngine,
    game_id: &str,
    include_other_owners: bool,
) -> AppResult<Vec<InstalledMod>> {
    let state = effective_install_state();
    let other_owners = if include_other_owners {
        format!(" OR ({state}) IN ('enabled','installed','partially_restored')")
    } else {
        String::new()
    };
    let conn = engine.db.conn();
    let mut stmt = conn.prepare(&format!(
        "SELECT id,game_id,coalesce((SELECT display_name FROM install_organizations WHERE install_id=installs.id),name),version,source,{state} AS status,install_root,files_json,metadata_json,installed_at,
        (SELECT json_group_array(tag_id ORDER BY tag_id) FROM install_mod_tags WHERE install_id=installs.id)
        FROM installs WHERE game_id=?1{other_owners} ORDER BY installed_at DESC,rowid DESC"
    ))?;
    let rows = stmt.query_map([game_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
            r.get::<_, String>(8)?,
            r.get::<_, String>(9)?,
            r.get::<_, String>(10)?,
        ))
    })?;
    rows.map(|row| {
        let (id, game_id, name, version, source, status, root, files, meta, installed_at, tags) =
            row?;
        let metadata: serde_json::Value = serde_json::from_str(&meta)?;
        let download_origin = metadata
            .get("downloadOrigin")
            .and_then(|origin| DownloadOrigin::deserialize(origin).ok());
        Ok(InstalledMod {
            tag_ids: serde_json::from_str(&tags)?,
            id,
            game_id,
            name,
            version,
            source,
            download_origin,
            status,
            install_root: root.into(),
            files: serde_json::from_str(&files)?,
            metadata,
            installed_at,
        })
    })
    .collect()
}

pub fn list_installs(engine: &FileEngine, game_id: &str) -> AppResult<Vec<InstalledMod>> {
    games::get_game(&engine.db, game_id)?;
    reconcile(&engine.db)?;
    read_installs(engine, game_id, false)
}

pub fn get_install(engine: &FileEngine, id: &str) -> AppResult<InstalledMod> {
    let game: Option<String> = engine
        .db
        .conn()
        .query_row("SELECT game_id FROM installs WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    list_installs(engine, &game.ok_or_else(|| AppError::NotFound(id.into()))?)?
        .into_iter()
        .find(|i| i.id == id)
        .ok_or_else(|| AppError::NotFound(id.into()))
}

fn inspect_file_state(
    engine: &FileEngine,
    path: &Path,
    expected: &Fingerprint,
    roots: &[PathBuf],
) -> (Option<Fingerprint>, String, Vec<String>) {
    let guarded = match engine.paths.guard(path, roots) {
        Ok(guarded) if guarded == path => guarded,
        Ok(_) => {
            return (
                None,
                "unreadable".into(),
                vec!["当前文件路径经过链接解析后发生变化，不能安全读取".into()],
            );
        }
        Err(error) => {
            return (
                None,
                "unreadable".into(),
                vec![format!("当前文件不可读取: {error}")],
            );
        }
    };
    match fingerprint(&guarded) {
        Ok(current) if current.is_none() => (None, "missing".into(), vec!["当前文件不存在".into()]),
        Ok(current) => {
            let state = if current.as_ref() == Some(expected) {
                "owned"
            } else {
                "modified"
            };
            let reasons = if state == "modified" {
                vec!["当前文件指纹与安装记录不一致".into()]
            } else {
                Vec::new()
            };
            (current, state.into(), reasons)
        }
        Err(error) => (
            None,
            "unreadable".into(),
            vec![format!("当前文件不可读取: {error}")],
        ),
    }
}

fn committed_install_transaction(
    engine: &FileEngine,
    install_id: &str,
) -> AppResult<Option<String>> {
    Ok(engine
        .db
        .conn()
        .query_row(
            "SELECT it.transaction_id
             FROM install_transactions it
             JOIN file_transactions ft ON ft.id=it.transaction_id
             WHERE it.install_id=?1 AND it.action='install' AND ft.status='committed'
             ORDER BY ft.created_at DESC, ft.rowid DESC LIMIT 1",
            [install_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn committed_replacement_predecessor(
    engine: &FileEngine,
    install_id: &str,
) -> AppResult<Option<(String, String)>> {
    Ok(engine
        .db
        .conn()
        .query_row(
            "SELECT r.transaction_id,r.replaced_install_id
             FROM install_replacements r
             JOIN file_transactions ft ON ft.id=r.transaction_id
             WHERE r.replacement_install_id=?1 AND ft.status='committed'
             ORDER BY ft.created_at DESC, ft.rowid DESC LIMIT 1",
            [install_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn committed_replacement_successor(
    engine: &FileEngine,
    install_id: &str,
) -> AppResult<Option<(String, String)>> {
    Ok(engine
        .db
        .conn()
        .query_row(
            "SELECT r.transaction_id,r.replacement_install_id
             FROM install_replacements r
             JOIN file_transactions ft ON ft.id=r.transaction_id
             WHERE r.replaced_install_id=?1 AND ft.status='committed'
             ORDER BY ft.created_at DESC, ft.rowid DESC LIMIT 1",
            [install_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

fn reconciliation(
    engine: &FileEngine,
    installed: &InstalledMod,
    files: &[InstallFileInspection],
    candidates: &[&InstalledMod],
) -> AppResult<InstallReconciliation> {
    let mut blocking_install_ids: Vec<_> = files
        .iter()
        .flat_map(|file| file.other_install_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut reasons = Vec::new();
    let mut recommended_action = "review_file_state".to_owned();
    let mut rollback_transaction_id = None;
    let mut successor_install_id = None;

    let predecessor = committed_replacement_predecessor(engine, &installed.id)?;
    let successor = committed_replacement_successor(engine, &installed.id)?;
    let enabled_successor = successor.as_ref().and_then(|(transaction, id)| {
        candidates
            .iter()
            .find(|candidate| candidate.id == *id && candidate.status == "enabled")
            .map(|_| (transaction.clone(), id.clone()))
    });

    if installed.status != "enabled" {
        recommended_action = "review_install_state".into();
        reasons.push(format!("安装状态为 {}，不能直接协调", installed.status));
    } else if let Some((transaction, predecessor_id)) = predecessor {
        recommended_action = "rollback_replacement".into();
        rollback_transaction_id = Some(transaction);
        reasons.push(format!(
            "这是替换事务中的新版本，优先回滚以恢复旧安装 {}",
            predecessor_id
        ));
    } else if let Some((transaction, successor_id)) = enabled_successor {
        recommended_action = "rollback_successor".into();
        rollback_transaction_id = Some(transaction);
        successor_install_id = Some(successor_id.clone());
        reasons.push(format!(
            "当前路径被较新的替换安装 {} 占用，优先回滚该替换事务",
            successor_id
        ));
    } else if !blocking_install_ids.is_empty() {
        let owners: Vec<_> = candidates
            .iter()
            .copied()
            .filter(|candidate| blocking_install_ids.binary_search(&candidate.id).is_ok())
            .collect();
        match engine.shared_install_rollback(installed, files, &owners) {
            Ok(transaction) => {
                blocking_install_ids.clear();
                recommended_action = "rollback_install".into();
                rollback_transaction_id = Some(transaction);
                reasons.push(
                    "事务日志已确认本次安装是共享文件的最后写入者，回滚会还原先前安装的文件".into(),
                );
            }
            Err(error) => {
                recommended_action = "review_shared_paths".into();
                reasons.push(format!(
                    "共享路径无法直接卸载：{error}。请核对关联安装 {}，从可回滚的安装开始处理",
                    blocking_install_ids.join(", ")
                ));
            }
        }
    } else if files.iter().any(|file| file.replaced) {
        let transaction = committed_install_transaction(engine, &installed.id)?;
        if transaction.is_some() && files.iter().all(|file| file.state == "owned") {
            recommended_action = "rollback_install".into();
            rollback_transaction_id = transaction;
            reasons.push("安装时覆盖了原文件，只能通过回滚安装事务恢复原内容".into());
        } else {
            reasons.push("覆盖过的原文件已被修改、缺失或无法读取，需人工审查".into());
        }
    } else if !files.is_empty()
        && installed.status == "enabled"
        && files.iter().all(|file| file.state == "owned")
    {
        recommended_action = "safe_uninstall".into();
        reasons.push("所有文件仍与安装清单一致，且没有共享路径".into());
    } else if !files.is_empty() && files.iter().all(|file| file.state == "missing") {
        match verify_missing_manifest(engine, installed) {
            Ok(()) => {
                recommended_action = FORGET_MISSING_FILES.into();
                reasons.push("安装清单中的文件均已不存在，没有需要恢复的原文件。若用户明确要求删除，可仅移除管理记录，无需卸载或回滚".into());
            }
            Err(err) => reasons.push(format!("文件均已缺失，但不能安全移除记录：{err}")),
        }
    } else if files.iter().any(|file| file.state == "modified") {
        reasons.push("至少一个文件在安装后发生修改".into());
    } else if files.iter().any(|file| file.state == "missing") {
        reasons.push("至少一个安装文件已经缺失".into());
    } else {
        reasons.push("至少一个安装文件无法安全读取".into());
    }

    Ok(InstallReconciliation {
        recommended_action,
        blocking_install_ids,
        rollback_transaction_id,
        successor_install_id,
        reasons,
    })
}

/// Reads one install manifest and the current managed files without changing
/// transaction state, install rows, plans, or files on disk.
pub fn inspect_install(engine: &FileEngine, id: &str) -> AppResult<InstallInspection> {
    let game_id: Option<String> = engine
        .db
        .conn()
        .query_row("SELECT game_id FROM installs WHERE id=?1", [id], |row| {
            row.get(0)
        })
        .optional()?;
    let game_id = game_id.ok_or_else(|| AppError::NotFound(id.into()))?;
    let installs = read_installs(engine, &game_id, true)?;
    let installed = installs
        .iter()
        .find(|install| install.id == id)
        .ok_or_else(|| AppError::NotFound(id.into()))?;
    if installed.metadata["gtaRpf"] == true {
        return gta_rpf::inspect(engine, installed);
    }
    let roots = engine.roots()?;
    let candidates: Vec<_> = installs
        .iter()
        .filter(|other| {
            other.id != installed.id
                && matches!(
                    other.status.as_str(),
                    "enabled" | "installed" | "partially_restored"
                )
        })
        .collect();

    let owner_paths: Vec<_> = candidates
        .iter()
        .map(|other| {
            let keys: BTreeSet<_> = other
                .files
                .iter()
                .map(|file| replacement::path_key(&file.path))
                .collect();
            (&other.id, keys)
        })
        .collect();
    let files: Vec<InstallFileInspection> = installed
        .files
        .iter()
        .map(|file| {
            let expected = file.fingerprint();
            let (current_fingerprint, state, mut reasons) =
                inspect_file_state(engine, &file.path, &expected, &roots);
            let key = replacement::path_key(&file.path);
            let other_install_ids: Vec<_> = owner_paths
                .iter()
                .filter(|(_, paths)| replacement::overlaps(paths, &key))
                .map(|(id, _)| (*id).clone())
                .collect();
            let replaced = file.before.is_some();
            if replaced {
                reasons.push("安装时覆盖了既有文件，不能直接卸载；需要回滚安装事务".into());
            }
            if !other_install_ids.is_empty() {
                reasons.push(format!(
                    "与其他安装共享路径: {}",
                    other_install_ids.join(", ")
                ));
            }
            if installed.status != "enabled" {
                reasons.push(format!("安装状态为 {}，不能直接卸载", installed.status));
            }
            InstallFileInspection {
                path: file.path.clone(),
                expected_fingerprint: expected,
                current_fingerprint,
                state,
                replaced,
                other_install_ids,
                reasons,
            }
        })
        .collect();
    let can_uninstall = !files.is_empty()
        && installed.status == "enabled"
        && files.iter().all(|file| {
            file.state == "owned" && !file.replaced && file.other_install_ids.is_empty()
        });
    let reconciliation = reconciliation(engine, installed, &files, &candidates)?;
    Ok(InstallInspection {
        install_id: installed.id.clone(),
        game_id: installed.game_id.clone(),
        status: installed.status.clone(),
        can_uninstall,
        files,
        reconciliation,
    })
}

fn uninstall_sources(
    engine: &FileEngine,
    installed: &InstalledMod,
) -> AppResult<BTreeMap<PathBuf, Fingerprint>> {
    let conflict = |message: &str| AppError::Conflict(message.into());
    if installed.status != "enabled" || installed.files.is_empty() {
        return Err(conflict("安装状态已变化或清单为空，不能直接卸载"));
    }
    if installed.files.iter().any(|file| file.before.is_some()) {
        return Err(conflict(
            "此安装覆盖了既有文件，请使用安装事务回滚以恢复原文件",
        ));
    }
    let manifest = checked_manifest(engine, installed)?;
    let keys = manifest
        .keys()
        .map(|path| replacement::path_key(path))
        .collect();
    replacement::verify_ownership(engine, &keys, (Some(&installed.id), None), None)?;
    Ok(manifest
        .into_iter()
        .map(|(path, file)| (path.to_owned(), file.fingerprint()))
        .collect())
}

pub(crate) fn checked_manifest<'a>(
    engine: &FileEngine,
    installed: &'a InstalledMod,
) -> AppResult<BTreeMap<&'a Path, &'a InstalledFile>> {
    let conflict = |message: &str| AppError::Conflict(message.into());
    let game = games::get_game(&engine.db, &installed.game_id)?;
    let mut scopes: Vec<_> = game_roots::registered_user_roots(&engine.db, &game.id)?
        .into_values()
        .collect();
    scopes.push(PathBuf::from(game.root_path));
    let root = &installed.install_root;
    if engine.paths.overlaps_storage(root)
        || engine.paths.guard(root, &scopes)? != *root
        || !scopes.iter().any(|scope| root.starts_with(scope))
    {
        return Err(conflict("安装清单的根目录不在该游戏的登记范围内"));
    }
    let mut files = BTreeMap::new();
    let mut owned = BTreeSet::new();
    for file in &installed.files {
        if file.path == *root
            || !file.path.starts_with(root)
            || engine.paths.guard(&file.path, &scopes)? != file.path
            || !owned.insert(replacement::path_key(&file.path))
        {
            return Err(conflict("安装清单含链接、重复路径或超出安装根的文件"));
        }
        files.insert(file.path.as_path(), file);
    }
    Ok(files)
}

// The absence of every path is not sufficient: confirm that this game actually
// deployed this complete, exclusively owned manifest without replacing anything.
fn verify_missing_manifest(engine: &FileEngine, installed: &InstalledMod) -> AppResult<()> {
    let mut sources = uninstall_sources(engine, installed)?;
    let transaction_id: String = engine
        .db
        .conn()
        .query_row(
            "SELECT a.transaction_id FROM install_transactions a JOIN file_transactions f ON f.id=a.transaction_id
             WHERE a.install_id=?1 AND a.action='install'
             GROUP BY a.install_id HAVING count(*)=1 AND f.status='committed' AND f.game_id=?2",
            params![installed.id, installed.game_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::Conflict("无法核对此游戏唯一的已提交安装事务".into()))?;
    let unsettled: bool = engine.db.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM file_transactions WHERE status IN ('applying','rolling_back','recovery_failed'))
         OR EXISTS(SELECT 1 FROM install_transactions a JOIN file_transactions f ON a.transaction_id=f.id
                   WHERE a.install_id=?1 AND a.action='uninstall' AND f.status='committed')",
        [&installed.id], |r| r.get(0),
    )?;
    if unsettled {
        return Err(AppError::Conflict(
            "请先核对文件事务及回收站恢复记录".into(),
        ));
    }
    let mismatch = || AppError::Conflict("原安装事务与独占新增文件清单不符".into());
    let plan = engine.get_plan(&transaction_id)?;
    if plan.id != transaction_id {
        return Err(mismatch());
    }
    for entry in plan.entries {
        let FileOp::Copy { dst, .. } = entry.operation else {
            return Err(mismatch());
        };
        let expected = sources.remove(&dst).ok_or_else(mismatch)?;
        if entry.before.is_some() || entry.source.as_ref() != Some(&expected) {
            return Err(mismatch());
        }
    }
    if !sources.is_empty() {
        return Err(mismatch());
    }
    Ok(())
}

fn validate_uninstall_moves(
    plan: &FilePlan,
    mut sources: BTreeMap<PathBuf, Fingerprint>,
    mut receipts: BTreeMap<PathBuf, PathBuf>,
) -> AppResult<()> {
    let conflict =
        || AppError::Conflict("卸载操作不再与独占安装清单、原始文件指纹和回收站记录匹配".into());
    for entry in &plan.entries {
        let FileOp::Move { src, dst } = &entry.operation else {
            return Err(conflict());
        };
        let expected = sources.remove(src).ok_or_else(conflict)?;
        if entry.source.as_ref() != Some(&expected)
            || entry.before.is_some()
            || receipts.remove(src).as_ref() != Some(dst)
        {
            return Err(conflict());
        }
    }
    if !sources.is_empty() || !receipts.is_empty() {
        return Err(conflict());
    }
    Ok(())
}

/// FileEngine calls this under WRITE_LOCK before applying any tracked uninstall.
/// It then checks the live fingerprints again before journaling and moving them.
fn validate_uninstall_apply(engine: &FileEngine, plan: &FilePlan, id: &str) -> AppResult<()> {
    let installed = get_install(engine, id)?;
    let sources = uninstall_sources(engine, &installed)?;
    let conn = engine.db.conn();
    let mut stmt = conn.prepare("SELECT id,original_path,trash_path,game_id,is_dir FROM trash_items WHERE transaction_id=?1")?;
    let mut rows = stmt.query([&plan.id])?;
    let mut receipts = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let trash_id: String = row.get(0)?;
        let src = PathBuf::from(row.get::<_, String>(1)?);
        let dst = PathBuf::from(row.get::<_, String>(2)?);
        let game: Option<String> = row.get(3)?;
        let is_dir: bool = row.get(4)?;
        if game.as_deref() != Some(installed.game_id.as_str())
            || is_dir
            || dst.parent().and_then(Path::parent) != Some(engine.paths.trash().as_path())
            || dst != engine.paths.trash_entry(&trash_id).join("payload")
            || receipts.insert(src, dst).is_some()
        {
            return Err(AppError::Conflict(
                "卸载的回收站记录与登记游戏或文件范围不符".into(),
            ));
        }
    }
    validate_uninstall_moves(plan, sources, receipts)
}

/// Only remove the exact bytes this install owns. Modified/shared files need
/// explicit conflict resolution; unknown configs or user files are never moved.
pub fn plan_uninstall(engine: &FileEngine, id: &str) -> AppResult<FilePlan> {
    let guard = crate::transaction::write_lock();
    let installed = get_install(engine, id)?;
    if installed.metadata["gtaRpf"] == true {
        return Err(AppError::Denied(
            "GTA RPF 卸载必须先按剩余贡献重建，使用 prepare_gta_rpf_job。".into(),
        ));
    }
    let sources = uninstall_sources(engine, &installed)?;
    let summary = format!("卸载 {}", installed.name);
    let drafts = trash::prepare_trash(
        engine,
        &trash::TrashRequest {
            paths: sources
                .keys()
                .map(|path| path.to_string_lossy().into())
                .collect(),
            game_id: Some(installed.game_id.clone()),
            reason: summary.clone(),
        },
    )?;
    let ops: Vec<_> = drafts.iter().map(|draft| draft.operation.clone()).collect();
    let receipts = ops
        .iter()
        .filter_map(|op| match op {
            FileOp::Move { src, dst } => Some((src.clone(), dst.clone())),
            _ => None,
        })
        .collect();
    let mut plan = engine.build_locked_plan(&guard, ops, summary, true)?;
    plan.requires_review = true;
    validate_uninstall_moves(&plan, sources, receipts)?;
    let mut conn = engine.db.conn();
    let tx = conn.transaction()?;
    FileEngine::persist_plan(&tx, &plan, true)?;
    trash::record_prepared(engine, &tx, &plan.id, drafts)?;
    tx.execute("INSERT INTO install_transactions(install_id,transaction_id,action,created_at) VALUES (?1,?2,'uninstall',?3)", params![id,plan.id,util::now()])?;
    tx.execute(
        "UPDATE file_transactions SET game_id=?1 WHERE id=?2",
        params![installed.game_id, plan.id],
    )?;
    tx.commit()?;
    Ok(plan)
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;

#[cfg(test)]
mod quota_tests {
    use super::*;

    #[test]
    fn installation_metadata_accepts_more_than_one_hundred_thousand_short_mappings() {
        let mappings: Vec<_> = (0..100_001)
            .map(|i| ManualMapping {
                source: format!("source/{i}"),
                destination: format!("mod/{i}"),
            })
            .collect();
        assert!(check_mapping_metadata(&mappings, &CancellationToken::new()).is_ok());
    }

    #[test]
    fn invalid_install_mapping_discards_temporary_stage_without_game_writes() {
        let (_temp, engine, game) = tests::fixture("stardew-valley");
        let stages = || {
            std::fs::read_dir(engine.paths.staging())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<BTreeSet<_>>()
        };
        let before = stages();
        let payload = tests::zip(&engine, &[("Example/data.txt", b"valid")]);
        let mut request = tests::request(&game, payload, "smapi-mod", "Example.zip");
        request.mappings.push(ManualMapping {
            source: "missing.txt".into(),
            destination: "data.txt".into(),
        });
        let error = plan_install(&engine, &request).unwrap_err();
        assert!(error.to_string().contains("归档无此文件"), "{error}");
        assert_eq!(stages(), before);
        assert!(!Path::new(&game.root_path).join("Mods/data.txt").exists());
    }
}
