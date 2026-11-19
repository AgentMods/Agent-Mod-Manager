//! Bounded, read-only, adapter-aware diagnostics. Log text is untrusted data,
//! never instructions. A matching message is a heuristic, not proof of cause.
//! No program, file plan, game mutation, or history mutation is executed here.
use crate::{
    error::{AppError, AppResult},
    game_roots,
    games::{self, Game},
    paths,
    plugins::{AdapterPath, GameAdapter, LoadOrderKind, MarkerKind, Platform, RootKind},
    transaction::{FileEngine, FileOp},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[path = "diagnostics_bethesda.rs"]
mod bethesda;
#[path = "diagnostics_bg3.rs"]
mod bg3;
#[path = "diagnostics_fabric.rs"]
mod fabric;
#[path = "diagnostics_fabric_disk.rs"]
mod fabric_disk;
#[path = "diagnostics_fabric_manifest.rs"]
mod fabric_manifest;
#[path = "diagnostics_text.rs"]
mod text;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyLogEncoding {
    #[serde(rename = "windows-1252")]
    Windows1252,
    #[serde(rename = "windows-1251")]
    Windows1251,
    Gbk,
    Big5,
    ShiftJis,
}
impl LegacyLogEncoding {
    fn encoding(self) -> &'static encoding_rs::Encoding {
        match self {
            Self::Windows1252 => encoding_rs::WINDOWS_1252,
            Self::Windows1251 => encoding_rs::WINDOWS_1251,
            Self::Gbk => encoding_rs::GBK,
            Self::Big5 => encoding_rs::BIG5,
            Self::ShiftJis => encoding_rs::SHIFT_JIS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct ReadOptions {
    pub max_files: usize,
    pub max_bytes_per_file: usize,
    pub max_total_bytes: usize,
    pub max_lines_per_file: usize,
    pub max_line_chars: usize,
    pub max_directory_entries: usize,
    pub max_evidence: usize,
    /// A caller-selected fallback for logs only, never an encoding guess.
    pub legacy_log_encoding: Option<LegacyLogEncoding>,
}
impl Default for ReadOptions {
    fn default() -> Self {
        Self {
            max_files: 16,
            max_bytes_per_file: 256 * 1024,
            max_total_bytes: 2 * 1024 * 1024,
            max_lines_per_file: 2000,
            max_line_chars: 2048,
            max_directory_entries: 256,
            max_evidence: 64,
            legacy_log_encoding: None,
        }
    }
}
impl ReadOptions {
    fn validate(&self) -> AppResult<()> {
        if self.max_files == 0
            || self.max_files > 32
            || self.max_bytes_per_file == 0
            || self.max_bytes_per_file > 1024 * 1024
            || self.max_total_bytes == 0
            || self.max_total_bytes > 4 * 1024 * 1024
            || self.max_lines_per_file == 0
            || self.max_lines_per_file > 10_000
            || self.max_line_chars == 0
            || self.max_line_chars > 8192
            || self.max_directory_entries == 0
            || self.max_directory_entries > 2048
            || self.max_evidence == 0
            || self.max_evidence > 128
        {
            return Err(AppError::Invalid("诊断读取上限超出允许范围".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Log,
    LoadOrder,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadStatus {
    Read,
    Missing,
    Denied,
    Binary,
    UnknownEncoding,
    IoError,
    LimitReached,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub number: usize,
    pub text: String,
    pub truncated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticFile {
    pub file: String,
    pub kind: FileKind,
    pub status: ReadStatus,
    pub size: Option<u64>,
    pub modified_at: Option<String>,
    pub bytes_read: usize,
    pub truncated: bool,
    /// Decoder used for the bounded prefix; not proof of the game's encoding.
    pub encoding: Option<String>,
    pub lines: Vec<LogLine>,
    pub notes: Vec<String>,
    #[serde(skip)]
    bg3: Option<Result<Vec<bg3::Finding>, &'static str>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadIssue {
    pub root: String,
    pub path: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogReport {
    pub game_id: String,
    pub adapter_slug: String,
    pub adapter_version: String,
    pub files: Vec<DiagnosticFile>,
    pub issues: Vec<ReadIssue>,
    pub bytes_read: usize,
    pub truncated: bool,
    pub limitations: Vec<String>,
}

impl LogReport {
    fn issue(&mut self, declaration: &AdapterPath, message: String) {
        if self.issues.len() >= 64 {
            self.truncated = true;
            return;
        }
        self.issues.push(ReadIssue {
            root: declaration.root.clone(),
            path: declaration.path.clone(),
            message: bounded_text(&message, 2048).0,
        });
    }
}

struct Context {
    game: Game,
    adapter: GameAdapter,
    platform: Platform,
    root: PathBuf,
    users: BTreeMap<String, PathBuf>,
    managed: Vec<PathBuf>,
    allowed: Vec<PathBuf>,
}

fn context(engine: &FileEngine, game_id: &str) -> AppResult<Context> {
    let game = games::get_game(&engine.db, game_id)?;
    let root = PathBuf::from(&game.root_path);
    stable_directory(&root)?;
    let adapter = game_roots::adapter(&engine.db, &game)?;
    let platform = game_roots::platform(&game)?;
    if !adapter.platforms.contains(&platform) {
        return Err(AppError::Unsupported("适配包不支持登记的游戏平台".into()));
    }
    let managed = engine.roots()?;
    // Require exact host registrations, not merely a broad parent root.
    if !managed.contains(&root) {
        return Err(AppError::Denied("游戏根尚未在文件引擎登记".into()));
    }
    let users = game_roots::registered_user_roots(&engine.db, game_id)?;
    let mut allowed = vec![root.clone()];
    for (id, path) in &users {
        if !adapter
            .install_roots
            .iter()
            .any(|r| r.id == *id && r.kind == crate::plugins::RootKind::RegisteredUser)
        {
            return Err(AppError::Denied(format!("未知的用户管理根登记: {id}")));
        }
        stable_directory(path)?;
        if !managed.contains(path) {
            return Err(AppError::Denied(format!("用户根未纳入文件引擎: {id}")));
        }
        allowed.push(path.clone());
    }
    Ok(Context {
        game,
        adapter,
        platform,
        root,
        users,
        managed,
        allowed,
    })
}

fn stable_directory(path: &Path) -> AppResult<()> {
    if !path.is_absolute()
        || !fs::symlink_metadata(path)?.file_type().is_dir()
        || path.canonicalize()? != path
    {
        return Err(AppError::Denied(format!(
            "管理根缺失、不是 canonical 目录或已被替换: {}",
            path.display()
        )));
    }
    Ok(())
}

impl Context {
    fn resolve(&self, path: &AdapterPath) -> AppResult<PathBuf> {
        let root = self.root(&path.root)?;
        self.guard(&paths::lexical_normalize(&root.join(&path.path)))?;
        let resolved = self.adapter.resolve_path(
            path,
            self.platform,
            &self.root,
            &self.users,
            &self.managed,
        )?;
        self.guard(&resolved)?;
        Ok(resolved)
    }
    fn root(&self, id: &str) -> AppResult<PathBuf> {
        let declaration = self
            .adapter
            .install_roots
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| AppError::NotFound(format!("适配管理根: {id}")))?;
        let base = match declaration.kind {
            RootKind::Game => &self.root,
            RootKind::RegisteredUser => self
                .users
                .get(id)
                .ok_or_else(|| AppError::Denied(format!("请先登记用户管理根: {id}")))?,
        };
        // Inspect the declared spelling before the adapter canonicalizes it;
        // otherwise an in-root symlink could disappear from the audit trail.
        self.guard(&paths::lexical_normalize(
            &base.join(&declaration.relative_path),
        ))?;
        let root =
            self.adapter
                .resolve_root(id, self.platform, &self.root, &self.users, &self.managed)?;
        self.guard(&root)?;
        Ok(root)
    }
    fn guard(&self, path: &Path) -> AppResult<()> {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        {
            return Err(AppError::PathEscape(path.display().to_string()));
        }
        let base = self
            .allowed
            .iter()
            .filter(|r| path.starts_with(r))
            .max_by_key(|p| p.components().count())
            .ok_or_else(|| AppError::PathEscape(path.display().to_string()))?;
        stable_directory(base)?;
        let mut current = base.clone();
        for part in path
            .strip_prefix(base)
            .map_err(|e| AppError::Invalid(e.to_string()))?
            .components()
        {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(m) if m.file_type().is_symlink() => {
                    return Err(AppError::Denied(format!(
                        "诊断不跟随符号链接: {}",
                        current.display()
                    )))
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(e.into()),
            }
        }
        if paths::ensure_within(path, std::slice::from_ref(base))? != path {
            return Err(AppError::Denied("诊断路径已被重定向".into()));
        }
        Ok(())
    }
}

fn blank_file(path: &Path, kind: FileKind, status: ReadStatus, note: String) -> DiagnosticFile {
    DiagnosticFile {
        file: path.display().to_string(),
        kind,
        status,
        size: None,
        modified_at: None,
        bytes_read: 0,
        truncated: false,
        encoding: None,
        lines: vec![],
        notes: vec![note],
        bg3: None,
    }
}
fn failed_file(path: &Path, kind: FileKind, error: AppError) -> DiagnosticFile {
    let status = match &error {
        AppError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => ReadStatus::Missing,
        AppError::Io(e) if e.kind() == std::io::ErrorKind::PermissionDenied => ReadStatus::Denied,
        AppError::PathEscape(_) | AppError::Denied(_) => ReadStatus::Denied,
        _ => ReadStatus::IoError,
    };
    blank_file(path, kind, status, error.to_string())
}

fn clean_text(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if (c.is_control() && c != '\t')
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                c.escape_unicode().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

fn bounded_text(text: &str, limit: usize) -> (String, bool) {
    let clean = clean_text(text);
    let truncated = clean.chars().count() > limit;
    (clean.chars().take(limit).collect(), truncated)
}

fn read_file(
    ctx: &Context,
    path: &Path,
    kind: FileKind,
    options: &ReadOptions,
    budget: usize,
    inspect_structure: bool,
) -> DiagnosticFile {
    let result = (|| -> AppResult<DiagnosticFile> {
        ctx.guard(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() {
            return Err(AppError::Denied(
                "只读取普通文件，不打开目录、管道、设备或链接".into(),
            ));
        }
        let limit = budget.min(options.max_bytes_per_file);
        if limit == 0 {
            return Ok(blank_file(
                path,
                kind,
                ReadStatus::LimitReached,
                "达到总读取字节上限".into(),
            ));
        }
        let file = crate::safe_fs::open_regular(path)?;
        ctx.guard(path)?;
        crate::safe_fs::verify_regular_at(&file, path)?;
        let opened = file.metadata()?;
        let mut bytes = Vec::new();
        let read_error = (&file).take(limit as u64).read_to_end(&mut bytes).err();
        let prefix_truncated = opened.len() > bytes.len() as u64;
        let mut report = DiagnosticFile {
            file: path.display().to_string(),
            kind,
            status: ReadStatus::Read,
            size: Some(opened.len()),
            modified_at: opened
                .modified()
                .ok()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
            bytes_read: bytes.len(),
            truncated: prefix_truncated,
            encoding: None,
            lines: vec![],
            notes: vec![],
            bg3: None,
        };
        if let Some(error) = read_error {
            report.status = ReadStatus::IoError;
            report.notes.push(error.to_string());
            return Ok(report);
        }
        let unchanged = crate::safe_fs::verify_regular_at(&file, path).is_ok()
            && file.metadata().is_ok_and(|after| {
                opened.len() == after.len() && opened.modified().ok() == after.modified().ok()
            });
        if !unchanged {
            report.status = ReadStatus::IoError;
            report
                .notes
                .push("读取期间文件有变化，未据此解析证据；请重试".into());
            return Ok(report);
        }
        let fallback = match kind {
            FileKind::Log => options.legacy_log_encoding.map(LegacyLogEncoding::encoding),
            // The asterisk-based Bethesda format has a documented legacy
            // code page; do not apply that fallback to arbitrary XML/logs.
            FileKind::LoadOrder
                if matches!(ctx.adapter.load_order.kind, LoadOrderKind::BethesdaPlugins) =>
            {
                Some(encoding_rs::WINDOWS_1252)
            }
            FileKind::LoadOrder => None,
        };
        let decoded = match text::decode(&bytes, prefix_truncated, fallback) {
            Ok(decoded) => decoded,
            Err((status, note)) => {
                report.status = status;
                report.notes.push(note.into());
                return Ok(report);
            }
        };
        if fallback.is_some_and(|fallback| fallback.name() == decoded.encoding) {
            report
                .notes
                .push("使用旧编码回退解码；编码选择可能影响证据文本".into());
        }
        report.encoding = Some(decoded.encoding.to_ascii_lowercase());
        for (index, (raw, terminated)) in text::physical_lines(&decoded.text).enumerate() {
            if index >= options.max_lines_per_file {
                report.truncated = true;
                break;
            }
            // A clipped final physical line must not become fabricated evidence.
            if prefix_truncated && !terminated {
                break;
            }
            let (text, truncated) = bounded_text(raw, options.max_line_chars);
            report.lines.push(LogLine {
                number: index + 1,
                text,
                truncated,
            });
            report.truncated |= truncated;
        }
        if report.truncated {
            report
                .notes
                .push("仅检查有界文件前缀；末尾或过长行可能遗漏，不代表当前运行状态".into());
        }
        if inspect_structure
            && kind == FileKind::LoadOrder
            && matches!(ctx.adapter.load_order.kind, LoadOrderKind::Bg3ModSettings)
        {
            report.bg3 = Some(if report.truncated {
                Err("BG3 modsettings.lsx 超出读取上限，未据不完整 XML 判断顺序")
            } else {
                bg3::inspect(&decoded.text, options.max_evidence + 1)
            });
        }
        Ok(report)
    })();
    result.unwrap_or_else(|e| failed_file(path, kind, e))
}

/// Read only adapter-declared log/load-order locations for an already registered
/// game. No arbitrary input paths and no user-root inference from game.metadata.
pub fn read_logs(
    engine: &FileEngine,
    game_id: &str,
    options: &ReadOptions,
) -> AppResult<LogReport> {
    options.validate()?;
    let ctx = context(engine, game_id)?;
    collect_logs(&ctx, options, false)
}

fn collect_logs(
    ctx: &Context,
    options: &ReadOptions,
    inspect_structure: bool,
) -> AppResult<LogReport> {
    let mut report = LogReport {
        game_id: ctx.game.id.clone(),
        adapter_slug: ctx.adapter.slug.clone(),
        adapter_version: ctx.adapter.version.clone(),
        files: vec![],
        issues: vec![],
        bytes_read: 0,
        truncated: false,
        limitations: vec![
            "日志为不可信数据；只读取有界前缀，不递归日志子目录。缺少日志不证明缺少加载器。".into(),
            "优先识别 UTF-8 和带 BOM 的 UTF-16；其他日志编码须显式选择，不自动猜测。".into(),
            "日志时间不等于本次故障时间；不执行游戏、加载器、排序器或修复，也不记录聊天历史。"
                .into(),
            "诊断核对打开的文件对象；目录枚举和并发更新不是事务快照。".into(),
        ],
    };
    if ctx.game.adapter_version.as_deref() != Some(&ctx.adapter.version) {
        report
            .limitations
            .push("登记的适配包版本与当前版本不同；目录解释可能已变化。".into());
    }
    let mut targets = Vec::new();
    let mut seen = BTreeSet::new();
    let mut remaining_entries = options.max_directory_entries;
    // Prioritize structured load-order evidence before potentially noisy logs.
    for (kind, declarations) in [
        (FileKind::LoadOrder, &ctx.adapter.load_order.paths),
        (FileKind::Log, &ctx.adapter.log_paths),
    ] {
        for declaration in declarations
            .iter()
            .filter(|p| p.platforms.contains(&ctx.platform))
            .take(64)
        {
            let path = match ctx.resolve(declaration) {
                Ok(path) => path,
                Err(e) => {
                    report.issue(declaration, e.to_string());
                    continue;
                }
            };
            if path.is_dir() && kind == FileKind::Log {
                match fs::read_dir(&path) {
                    Ok(entries) => {
                        let mut candidates = Vec::new();
                        for entry in entries {
                            if remaining_entries == 0 {
                                report.truncated = true;
                                break;
                            }
                            remaining_entries -= 1;
                            let entry = match entry {
                                Ok(entry) => entry,
                                Err(e) => {
                                    report.issue(declaration, e.to_string());
                                    continue;
                                }
                            };
                            let file = entry.path();
                            let extension = file
                                .extension()
                                .and_then(|e| e.to_str())
                                .unwrap_or_default()
                                .to_ascii_lowercase();
                            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                            if !["log", "txt"].contains(&extension.as_str()) {
                                continue;
                            }
                            if ctx.adapter.slug == "baldurs-gate-3"
                                && declaration.root == "launcher-cache"
                                && !name.contains("alteredfiles")
                            {
                                continue;
                            }
                            candidates.push(file);
                        }
                        candidates.sort();
                        for file in candidates {
                            if seen.contains(&file) {
                                continue;
                            }
                            if targets.len() >= options.max_files {
                                report.truncated = true;
                                break;
                            }
                            seen.insert(file.clone());
                            targets.push((file, kind));
                        }
                    }
                    Err(e) => report.issue(declaration, e.to_string()),
                }
            } else if seen.contains(&path) {
                continue;
            } else if targets.len() < options.max_files {
                seen.insert(path.clone());
                targets.push((path, kind));
            } else {
                report.truncated = true;
            }
        }
    }
    if ctx.adapter.log_paths.len() > 64 || ctx.adapter.load_order.paths.len() > 64 {
        report.truncated = true;
    }
    for (file, kind) in targets {
        let result = read_file(
            ctx,
            &file,
            kind,
            options,
            options.max_total_bytes.saturating_sub(report.bytes_read),
            inspect_structure,
        );
        report.bytes_read += result.bytes_read;
        report.truncated |= result.truncated || result.status == ReadStatus::LimitReached;
        report.files.push(result);
    }
    Ok(report)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Observed,
    Heuristic,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CauseKind {
    Loader,
    Version,
    Dependency,
    MissingFile,
    LoadOrder,
    Transaction,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub id: String,
    pub file: String,
    /// Physical one-based file line. None for a stat or DB observation.
    pub line: Option<usize>,
    pub text: String,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cause {
    pub kind: CauseKind,
    pub basis: Basis,
    pub summary: String,
    pub evidence_ids: Vec<String>,
}
/// Input only for FileEngine::plan_file_ops. This is not a FilePlan and has no
/// transaction ID/approval. This module never calls plan_file_ops or applies it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOpsInput {
    pub ops: Vec<FileOp>,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairSuggestion {
    pub kind: CauseKind,
    pub description: String,
    pub requires_confirmation: bool,
    pub file_ops_input: Option<FileOpsInput>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    pub game_id: String,
    pub status: String,
    pub causes: Vec<Cause>,
    pub evidence: Vec<Evidence>,
    pub repair_suggestions: Vec<RepairSuggestion>,
    pub logs: LogReport,
    pub unknowns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fabric_startup: Option<fabric::FabricEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fabric_disk: Option<fabric_disk::FabricDiskEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bethesda_plugins: Option<bethesda::BethesdaEvidence>,
}

impl Diagnosis {
    fn unknown(&mut self, message: impl AsRef<str>) {
        let (message, truncated) = bounded_text(message.as_ref(), 2048);
        self.logs.truncated |= truncated;
        if self.unknowns.contains(&message) {
            return;
        }
        if self.unknowns.len() < 63 {
            self.unknowns.push(message);
        } else {
            self.logs.truncated = true;
            if self.unknowns.len() == 63 {
                self.unknowns
                    .push("未知项达到 64 条上限；其他不可验证项已省略。".into());
            }
        }
    }

    // Keep all source evidence behind one bounded construction point.
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        options: &ReadOptions,
        kind: CauseKind,
        basis: Basis,
        file: &str,
        line: Option<usize>,
        text: &str,
        source: &str,
        summary: &str,
    ) {
        if self.evidence.len() >= options.max_evidence {
            self.logs.truncated = true;
            return;
        }
        let id = format!("e{}", self.evidence.len() + 1);
        self.evidence.push(Evidence {
            id: id.clone(),
            file: file.into(),
            line,
            text: bounded_text(text, options.max_line_chars).0,
            source: source.into(),
        });
        if let Some(cause) = self
            .causes
            .iter_mut()
            .find(|c| c.kind == kind && c.basis == basis && c.summary == summary)
        {
            cause.evidence_ids.push(id);
        } else {
            self.causes.push(Cause {
                kind,
                basis,
                summary: summary.into(),
                evidence_ids: vec![id],
            });
        }
    }
}

fn pattern(line: &str, adapter: &str) -> Option<(CauseKind, &'static str)> {
    let lower = line.to_lowercase();
    let has = |values: &[&str]| values.iter().any(|v| lower.contains(v));
    // Suppress common explicit success/negation messages. This is still a
    // language-specific heuristic and must never be described as a verifier.
    if has(&[
        "no errors",
        "0 errors",
        "no missing dependencies",
        "all dependencies satisfied",
        "no version mismatch",
    ]) {
        return None;
    }
    if has(&[
        "unsupported runtime",
        "runtime version mismatch",
        "wrong game version",
        "incompatible version",
        "compiled against a different version",
        "newer version of skyrim",
        "newer version of fallout",
        "unsupported version",
        "requires version",
    ]) || (adapter == "minecraft-java"
        && has(&[
            "unsupportedclassversionerror",
            "requires java",
            "class file version",
        ]))
    {
        return Some((
            CauseKind::Version,
            "日志提示运行时/游戏/模组版本可能不兼容；未验证实际版本约束",
        ));
    }
    if has(&[
        "missing dependency",
        "missing dependencies",
        "requires the following mods",
        "could not find required mod",
        "missing master",
        "mandatory dependencies",
        "mod resolution encountered an incompatible mod set",
    ]) || (adapter == "stardew-valley" && lower.contains("requires mods which aren't installed"))
        || (adapter == "minecraft-java" && lower.contains("incompatible mods found"))
    {
        return Some((
            CauseKind::Dependency,
            "日志提示可能缺少依赖或存在不兼容依赖；未解算版本或自动下载",
        ));
    }
    if has(&[
        "filenotfoundexception",
        "no such file or directory",
        "file not found",
        "could not find file",
        "missing file",
    ]) {
        return Some((
            CauseKind::MissingFile,
            "日志报告文件缺失；未将日志中的任意路径作为读取或修复目标",
        ));
    }
    if has(&[
        "failed to load",
        "could not load",
        "loader not found",
        "not loaded",
        "skipped mods",
        "failed to initialize",
    ]) {
        return Some((
            CauseKind::Loader,
            "日志提示加载器或模组加载失败；可能有其他根因，未执行程序验证",
        ));
    }
    None
}

/// Adapter-aware, non-mutating diagnosis. It reports observations/heuristics,
/// never a success verdict. Missing data produces explicit unknowns.
pub fn diagnose_game(
    engine: &FileEngine,
    game_id: &str,
    options: &ReadOptions,
) -> AppResult<Diagnosis> {
    options.validate()?;
    let ctx = context(engine, game_id)?;
    let logs = collect_logs(&ctx, options, true)?;
    let fabric_path = ctx.root.join("logs").join("latest.log");
    let fabric_log = logs.files.iter().find(|file| {
        ctx.adapter.slug == "minecraft-java"
            && file.kind == FileKind::Log
            && Path::new(&file.file) == fabric_path
    });
    let fabric_startup = fabric_log.map(fabric::inspect);
    let fabric_disk = fabric_disk::inspect(engine, &ctx, fabric_startup.as_ref());
    let fabric_failure = fabric_startup
        .as_ref()
        .filter(|startup| startup.status == fabric::FabricStatus::ResolutionFailed)
        .and_then(|startup| startup.failure_line)
        .and_then(|number| {
            let file = fabric_log?;
            let line = file
                .lines
                .iter()
                .find(|line| !line.truncated && line.number == number)?;
            let details = fabric::dependency_details(file, number).unwrap_or_default();
            Some((file.file.clone(), line.clone(), details))
        });
    let mut result = Diagnosis { game_id: game_id.into(), status: "unknown".into(), causes: vec![], evidence: vec![], repair_suggestions: vec![], logs,
        unknowns: vec!["没有运行游戏或校验加载器 ABI；没有完整依赖解算、存档兼容性验证或自动排序。".into(),
            "没有读取聊天/工具历史作为游戏故障证据，也没有将诊断结果写入历史；由调用方按会话权限记录。".into()], fabric_startup, fabric_disk, bethesda_plugins: None };
    if let Some((file, marker, details)) = &fabric_failure {
        if details.is_empty() {
            result.add(
                options,
                CauseKind::Dependency,
                Basis::Observed,
                file,
                Some(marker.number),
                &marker.text,
                "fabric_startup",
                "Fabric 日志记录了依赖解析失败；未确定具体冲突或修复方案",
            );
        }
        for (line, summary) in details {
            result.add(
                options,
                CauseKind::Dependency,
                Basis::Observed,
                file,
                Some(line.number),
                &line.text,
                "fabric_dependency",
                summary,
            );
        }
    }
    for file in result.logs.files.clone() {
        if file.status != ReadStatus::Read {
            continue;
        }
        if file.kind == FileKind::Log {
            for line in file.lines.iter().filter(|l| !l.truncated) {
                if fabric_failure.as_ref().is_some_and(|(source, _, details)| {
                    source == &file.file && details.iter().any(|(row, _)| row.number == line.number)
                }) {
                    continue;
                }
                if let Some((kind, summary)) = pattern(&line.text, &ctx.adapter.slug) {
                    result.add(
                        options,
                        kind,
                        Basis::Heuristic,
                        &file.file,
                        Some(line.number),
                        &line.text,
                        "log",
                        summary,
                    );
                }
            }
        }
        if file.kind == FileKind::LoadOrder
            && matches!(ctx.adapter.load_order.kind, LoadOrderKind::BethesdaPlugins)
        {
            inspect_bethesda_order(&ctx, options, &file, &mut result);
            let (evidence, findings) = bethesda::inspect(&ctx, options, &file);
            result.bethesda_plugins = Some(evidence);
            for finding in findings {
                finding.add_to(&mut result, options);
            }
        }
        match &file.bg3 {
            Some(Ok(findings)) => {
                for finding in findings {
                    let Some(line) = finding
                        .line
                        .checked_sub(1)
                        .and_then(|index| file.lines.get(index))
                    else {
                        continue;
                    };
                    result.add(
                        options,
                        CauseKind::LoadOrder,
                        Basis::Observed,
                        &file.file,
                        Some(line.number),
                        &line.text,
                        "load_order",
                        finding.summary,
                    );
                }
            }
            Some(Err(note)) => result.unknown(*note),
            None => {}
        }
    }
    if matches!(ctx.adapter.load_order.kind, LoadOrderKind::Bg3ModSettings) {
        result.unknown("BG3 modsettings.lsx 只核对配置内部的 UUID/LSX 结构；不映射 PAK、不验证依赖、实际顺序或加载状态。");
    }
    if matches!(ctx.adapter.load_order.kind, LoadOrderKind::BethesdaPlugins) {
        result.unknown("插件诊断只在有界范围内读取 Plugins.txt 声明的 loose 插件 TES4 MAST；不验证 MO2/VFS、非默认配置、归档/BSA/BA2、ESL slot 或完整有效顺序。");
    }
    for marker in ctx
        .adapter
        .markers
        .iter()
        .filter(|m| m.required && m.platforms.contains(&ctx.platform))
        .take(128)
    {
        let path = ctx.root.join(&marker.path);
        match ctx
            .guard(&path)
            .and_then(|_| fs::symlink_metadata(&path).map_err(AppError::from))
        {
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => result.add(
                options,
                CauseKind::MissingFile,
                Basis::Observed,
                &path.display().to_string(),
                None,
                "适配包必需标记不存在",
                "filesystem",
                "缺少适配包必需文件/目录，安装布局可能不完整",
            ),
            Ok(m)
                if !match marker.kind {
                    MarkerKind::File => m.is_file(),
                    MarkerKind::Directory => m.is_dir(),
                } =>
            {
                result.add(
                    options,
                    CauseKind::MissingFile,
                    Basis::Observed,
                    &path.display().to_string(),
                    None,
                    "标记的文件类型与适配包声明不一致",
                    "filesystem",
                    "适配包标记的文件类型不匹配",
                )
            }
            Err(e) => result.unknown(format!("无法校验标记 {}: {e}", marker.path)),
            _ => {}
        }
    }
    if ctx.adapter.markers.len() > 128 {
        result.logs.truncated = true;
        result.unknown("适配包标记超过 128 项，未检查剩余标记。");
    }
    inspect_installs(engine, &ctx, options, &mut result)?;
    inspect_transactions(engine, game_id, options, &mut result)?;
    if !result.logs.issues.is_empty() {
        result.unknown("部分适配日志或顺序根未登记/不可读，不能据此断言加载器缺失。");
    }
    if result
        .logs
        .files
        .iter()
        .any(|f| f.status != ReadStatus::Read)
    {
        result.unknown("部分文件缺失、拒绝读取、为二进制或编码不支持；具体状态见 files，不能将其当作无错误日志。");
    }
    if result.logs.truncated {
        result.unknown("证据受到文件、字节、行数或目录项上限限制；未检查部分内容。");
    }
    if result
        .logs
        .files
        .iter()
        .all(|f| f.status != ReadStatus::Read || f.lines.is_empty())
    {
        result.unknown("没有读取到非空日志/顺序内容；无法确认本次故障。");
    }
    let kinds: BTreeSet<_> = result.causes.iter().map(|c| c.kind).collect();
    for kind in [
        CauseKind::Loader,
        CauseKind::Version,
        CauseKind::Dependency,
        CauseKind::MissingFile,
        CauseKind::LoadOrder,
        CauseKind::Transaction,
    ] {
        if !kinds.contains(&kind) {
            continue;
        }
        let description = match kind {
            CauseKind::Loader => "核对实际启动方式、加载器安装说明和失败上下文；不要仅凭缺失日志安装加载器。",
            CauseKind::Version => "记录游戏、加载器、Java/原生运行时和模组的实际版本，按作者兼容表核对；不猜测应升级还是降级。",
            CauseKind::Dependency => "核对依赖名称、来源、版本约束和启用状态；安装包来源及授权确认后另生成事务计划。",
            CauseKind::MissingFile => "核对安装记录和来源包；有已验证来源时再输出逐文件 FileOps 计划。不要复制未知日志路径。",
            CauseKind::LoadOrder => "备份并核对当前插件列表、依赖与官方固定顺序；本诊断不执行排序或重写列表。",
            CauseKind::Transaction => "先检查待恢复/失败事务及其 journal，使用现有恢复流程；诊断不会自动回滚或声称恢复完成。",
            CauseKind::Unknown => unreachable!(),
        };
        result.repair_suggestions.push(RepairSuggestion {
            kind,
            description: description.into(),
            requires_confirmation: true,
            file_ops_input: None,
        });
    }
    if !result.causes.is_empty() {
        result.status = "findings".into();
    } else {
        result.causes.push(Cause {
            kind: CauseKind::Unknown,
            basis: Basis::Unknown,
            summary: "未在已检查的有限证据中识别到受支持模式；不表示游戏健康或故障已解决".into(),
            evidence_ids: vec![],
        });
    }
    Ok(result)
}

fn inspect_bethesda_order(
    ctx: &Context,
    options: &ReadOptions,
    file: &DiagnosticFile,
    result: &mut Diagnosis,
) {
    let data = match ctx.root("data") {
        Ok(path) => path,
        Err(e) => {
            result.unknown(e.to_string());
            return;
        }
    };
    let mut names = BTreeSet::new();
    for line in file.lines.iter().filter(|l| !l.truncated) {
        let text = line.text.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let enabled = text.starts_with('*');
        let name = text.strip_prefix('*').unwrap_or(text).trim();
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if crate::plugins::validate_relative_path(name, false).is_err()
            || name.contains('/')
            || !["esp", "esm", "esl"].contains(&ext.as_str())
        {
            result.add(
                options,
                CauseKind::LoadOrder,
                Basis::Observed,
                &file.file,
                Some(line.number),
                &line.text,
                "load_order",
                "插件列表包含非插件名/不安全条目；没有把条目当成路径访问",
            );
            continue;
        }
        if !names.insert(name.to_lowercase()) {
            result.add(
                options,
                CauseKind::LoadOrder,
                Basis::Observed,
                &file.file,
                Some(line.number),
                &line.text,
                "load_order",
                "插件列表存在重复名称；未自动重排或删除条目",
            );
        }
        if enabled {
            let path = data.join(name);
            match ctx
                .guard(&path)
                .and_then(|_| fs::symlink_metadata(&path).map_err(AppError::from))
            {
                Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => result.add(
                    options,
                    CauseKind::MissingFile,
                    Basis::Observed,
                    &file.file,
                    Some(line.number),
                    &line.text,
                    "load_order",
                    "启用列表中的插件文件在当前布局中不存在（文件名大小写按当前文件系统）",
                ),
                Ok(m) if !m.is_file() => result.add(
                    options,
                    CauseKind::MissingFile,
                    Basis::Observed,
                    &file.file,
                    Some(line.number),
                    &line.text,
                    "load_order",
                    "启用的插件名对应的目标不是普通文件",
                ),
                Err(e) => result.unknown(format!("无法校验插件 {name}: {e}")),
                _ => {}
            }
        }
    }
}

fn inspect_installs(
    engine: &FileEngine,
    ctx: &Context,
    options: &ReadOptions,
    result: &mut Diagnosis,
) -> AppResult<()> {
    // Match install::InstalledFile.path, also accepting legacy absolute strings.
    // Do not call list_installs: it synchronizes status and would mutate the DB.
    let rows = {
        let conn = engine.db.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT substr(id,1,512), CASE WHEN length(CAST(files_json AS BLOB))<=262144 THEN files_json ELSE '' END,
                    length(CAST(files_json AS BLOB)), trim(dependencies_json)<>'[]'
             FROM installs WHERE game_id=?1 AND
             CASE WHEN EXISTS (SELECT 1 FROM install_transactions WHERE install_id=installs.id)
               THEN {} ELSE status END='enabled' ORDER BY id LIMIT 129",
            crate::install::INSTALL_STATE,
        ))?;
        let rows = stmt.query_map([&ctx.game.id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, usize>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if rows.len() > 128 {
        result.logs.truncated = true;
        result.unknown("仅检查前 128 条启用安装记录。");
    }
    if !rows.is_empty() {
        result.unknown(
            "仅核对已部署安装记录中的文件是否存在；不更新安装状态、不校验内容哈希或来源真实性。",
        );
    }
    let mut checks = 0;
    for (id, files, length, dependencies) in rows.into_iter().take(128) {
        if dependencies {
            result.unknown(format!(
                "安装 {id} 有依赖记录；尚未解算其约束，不能仅凭名称断言已满足。"
            ));
        }
        if length > 262144 {
            result.logs.truncated = true;
            result.unknown(format!("安装 {id} 文件记录过大，未检查。"));
            continue;
        }
        let files: Vec<Value> = match serde_json::from_str(&files) {
            Ok(v) => v,
            Err(_) => {
                result.unknown(format!("安装 {id} 的 files_json 不是可解释数组。"));
                continue;
            }
        };
        for file in files {
            if checks >= 512 {
                result.logs.truncated = true;
                result.unknown("安装文件核对达到 512 项上限。");
                return Ok(());
            }
            checks += 1;
            let Some(path) = file
                .as_str()
                .or_else(|| file.get("path").and_then(Value::as_str))
            else {
                result.unknown(format!("安装 {id} 含未知文件记录格式。"));
                continue;
            };
            if path.len() > 8192 || path.chars().any(char::is_control) {
                result.unknown(format!("安装 {id} 含过长或控制字符路径，未访问。"));
                continue;
            }
            let path = PathBuf::from(path);
            if let Err(e) = ctx.guard(&path) {
                result.unknown(format!("忽略安装 {id} 越权/未知路径: {e}"));
                continue;
            }
            match fs::symlink_metadata(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => result.add(
                    options,
                    CauseKind::MissingFile,
                    Basis::Observed,
                    &path.display().to_string(),
                    None,
                    &format!("enabled 安装 {id} 的记录文件不存在"),
                    "install_record",
                    "启用安装记录中的文件不存在；记录可能陈旧，未自动重装",
                ),
                Ok(m) if !m.is_file() => {
                    result.unknown(format!("安装 {id} 的文件记录实际不是普通文件。"))
                }
                Err(e) => result.unknown(format!("无法核对安装 {id}: {e}")),
                _ => {}
            }
        }
    }
    Ok(())
}

fn inspect_transactions(
    engine: &FileEngine,
    game_id: &str,
    options: &ReadOptions,
    result: &mut Diagnosis,
) -> AppResult<()> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare("SELECT substr(id,1,512),status FROM file_transactions WHERE game_id=?1 AND status IN ('applying','rolling_back','recovery_failed','failed') ORDER BY created_at DESC LIMIT 17")?;
    for (index, row) in stmt
        .query_map([game_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .enumerate()
    {
        if index >= 16 {
            result.logs.truncated = true;
            result.unknown("事务观察达到 16 项上限。");
            break;
        }
        let (id, status) = row?;
        result.add(
            options,
            CauseKind::Transaction,
            Basis::Observed,
            "database:file_transactions",
            None,
            &format!("{id}: {status}"),
            "database",
            "存在失败或尚未恢复的文件事务；不证明它是本次故障根因",
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "diagnostics_tests.rs"]
mod tests;
