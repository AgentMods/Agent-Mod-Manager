//! Desktop Agent receipt of completed downloads from two fixed local inboxes.
//! This is intentionally separate from the native desktop archive picker.
#![cfg(not(any(target_os = "android", target_os = "ios")))]

use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    download::safe_filename,
    error::{AppError, AppResult},
    games,
    mod_library::{self, ApprovedArtifact, ReceivedChecks},
    paths,
    safe_fs::{self, FileIdentity},
    sources::{
        resolutions::ResolutionScope,
        web_download::{
            validated_endpoint, ArchiveKind, MAX_ARCHIVE_SIGNATURE_BYTES, MAX_WEB_URL_BYTES,
        },
    },
    transaction::FileEngine,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

const NAME: &str = "file_received_mod";
const FOLDERS_NAME: &str = "list_mod_receipt_folders";
const MAX_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 512;
const EFFECT: Effect = Effect::UncatalogedDownload;

pub(super) fn register(registry: &ToolRegistry) -> AppResult<()> {
    registry.register_batch(vec![
        Arc::new(ReceiptFoldersTool),
        Arc::new(ReceivedModTool),
    ])
}

struct ReceiptFoldersTool;
struct ReceivedModTool;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Args {
    #[serde(skip_serializing_if = "Option::is_none")]
    game_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    game_name: Option<String>,
    filename: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mod_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_page: Option<String>,
}

impl Args {
    fn parse(value: Value) -> AppResult<Self> {
        let args: Self = serde_json::from_value(value)
            .map_err(|_| AppError::Invalid("接收下载参数无效，不允许指定源路径".into()))?;
        args.normalized()
    }

    fn normalized(mut self) -> AppResult<Self> {
        match (&mut self.game_id, &mut self.game_name) {
            (Some(id), None) => normalize_label(id, Some(MAX_NAME_BYTES))?,
            (None, Some(name)) => normalize_label(name, None)?,
            _ => {
                return Err(AppError::Invalid(
                    "必须且只能指定 gameId 或 gameName".into(),
                ));
            }
        }
        archive_kind(&self.filename)?;
        if let Some(name) = &mut self.mod_name {
            normalize_label(name, Some(MAX_NAME_BYTES))?;
        }
        if let Some(page) = &mut self.project_page {
            *page = project_page(page)?;
        }
        Ok(self)
    }

    fn target(&self, engine: &FileEngine, selected: Option<&[String]>) -> AppResult<Target> {
        if let Some(id) = &self.game_id {
            super::workspace::resolve(selected, Some(id))?;
            let game = games::get_game(&engine.db, id)?;
            return Ok(Target {
                id: Some(game.id),
                name: game.name,
            });
        }
        if selected.is_some_and(|ids| ids.len() != 1) {
            return Err(AppError::Invalid("请提供当前工作区中的目标游戏 ID".into()));
        }
        let requested = self.game_name.as_deref().unwrap_or_default();
        let key = target_key(requested);
        let mut matches = games::list_games(&engine.db)?
            .into_iter()
            .filter(|game| selected.is_none_or(|ids| ids.contains(&game.id)))
            .filter(|game| target_key(&game.name) == key);
        let game = matches.next();
        if matches.next().is_some() {
            return Err(AppError::Conflict("存在重名的游戏，请指定 gameId".into()));
        }
        if selected.is_some() && game.is_none() {
            return Err(AppError::Invalid("游戏名称与当前工作区选择不符".into()));
        }
        Ok(game.map_or_else(
            || Target {
                id: None,
                name: requested.into(),
            },
            |game| Target {
                id: Some(game.id),
                name: game.name,
            },
        ))
    }

    fn name(&self) -> &str {
        self.mod_name.as_deref().unwrap_or(&self.filename)
    }
}

struct Target {
    id: Option<String>,
    name: String,
}

fn target_key(name: &str) -> String {
    name.split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

fn disallowed_display_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

fn normalize_label(raw: &mut String, max_bytes: Option<usize>) -> AppResult<()> {
    if max_bytes.is_some_and(|max| raw.len() > max)
        || raw.trim().is_empty()
        || raw.chars().any(disallowed_display_char)
    {
        return Err(AppError::Invalid("游戏或模组名称不合法".into()));
    }
    *raw = raw.trim().into();
    Ok(())
}

fn archive_kind(filename: &str) -> AppResult<ArchiveKind> {
    if filename != filename.trim()
        || filename.chars().any(disallowed_display_char)
        || safe_filename(filename).is_err()
    {
        return Err(AppError::Invalid(
            "文件名必须是安全的单个普通归档文件名".into(),
        ));
    }
    let lower = filename.to_ascii_lowercase();
    if [".zip", ".jar", ".mrpack"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        Ok(ArchiveKind::Zip)
    } else if lower.ends_with(".7z") {
        Ok(ArchiveKind::SevenZip)
    } else if lower.ends_with(".rar") {
        Ok(ArchiveKind::Rar)
    } else if lower.ends_with(".gz") || lower.ends_with(".tgz") {
        // Playwright sanitizes all dots before the final extension, so
        // release.tar.gz may actually be saved as release-tar-1.gz.
        // The library still parses the gzip stream as TAR before publishing it.
        Ok(ArchiveKind::TarGzip)
    } else {
        Err(AppError::Unsupported(
            "只接收 ZIP、JAR、MRPACK、7z、RAR 和 gzip 压缩的 TAR 归档".into(),
        ))
    }
}

fn project_page(raw: &str) -> AppResult<String> {
    let invalid = || AppError::Invalid("项目页必须是不含凭据或参数的公开 HTTPS 网页".into());
    if raw.len() > MAX_WEB_URL_BYTES || raw.chars().any(char::is_whitespace) {
        return Err(invalid());
    }
    let url = validated_endpoint(raw).map_err(|_| invalid())?;
    if url.query().is_some()
        || url.scheme() != "https"
        || !matches!(url.host(), Some(url::Host::Domain(_)))
        || url.as_str().len() > MAX_WEB_URL_BYTES
    {
        return Err(invalid());
    }
    Ok(url.into())
}

/// Don't replay rejected arguments that could contain an external path or a
/// signed URL supplied by an outbound MCP tool.
fn history_arguments(raw: &str) -> String {
    serde_json::from_str::<Args>(raw)
        .ok()
        .and_then(|args| args.normalized().ok())
        .and_then(|args| serde_json::to_string(&args).ok())
        .unwrap_or_else(|| "{}".into())
}

fn require_desktop_agent(context: &ToolContext) -> AppResult<()> {
    if context.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if !matches!(context.resolution_scope, ResolutionScope::Agent(_)) {
        return Err(AppError::Denied("下载收件只在桌面 Agent 对话中可用".into()));
    }
    Ok(())
}

#[derive(Debug)]
struct Folders {
    incoming: PathBuf,
    playwright: Option<PathBuf>,
}

fn same_path(a: &Path, b: &Path) -> bool {
    paths::contains_path(a, b) && paths::contains_path(b, a)
}

impl Folders {
    fn configured(engine: &FileEngine) -> AppResult<Self> {
        let incoming = crate::storage::incoming_downloads(&engine.paths.root);
        engine.paths.stable_directory(&incoming)?;
        engine.paths.guard_file_path(&incoming, &[], false)?;
        let playwright = mod_library::playwright_receipt_dir(engine)?;
        Ok(Self {
            incoming,
            playwright,
        })
    }

    fn preview(&self) -> Value {
        json!({
            "incoming": paths::display_path(&self.incoming),
            "playwright": self.playwright.as_deref().map(paths::display_path),
            "outsideFolders": "使用桌面 Mods 页的文件选择器；不要把绝对路径传给 file_received_mod",
        })
    }

    fn find(&self, filename: &str) -> AppResult<Receipt> {
        safe_filename(filename)?;
        // Never enumerate, recurse, trust the caller's filesystem paths, or
        // silently choose between two files with the same basename.
        let incoming = Receipt::open(self.incoming.join(filename), Kind::Incoming)?;
        let playwright = self
            .playwright
            .as_ref()
            .map(|root| Receipt::open(root.join(filename), Kind::Playwright))
            .transpose()?
            .flatten();
        match (incoming, playwright) {
            (Some(_), Some(_)) => Err(AppError::Conflict(
                "两个接收目录存在同名文件，无法判定来源；请通过桌面文件选择器明确选择".into(),
            )),
            (Some(file), None) | (None, Some(file)) => Ok(file),
            (None, None) => Err(AppError::NotFound(
                "接收目录没有找到归档；请先确认下载已完成，其他目录中的文件请用桌面选择器".into(),
            )),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
enum Kind {
    Incoming,
    Playwright,
}

#[derive(Debug)]
struct Receipt {
    path: PathBuf,
    file: File,
    kind: Kind,
    identity: FileIdentity,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptFingerprint {
    identity: FileIdentity,
    size: u64,
    sha256: String,
}

fn changed() -> AppError {
    AppError::Conflict("接收文件已移动、替换或修改，请重新确认下载".into())
}

impl Receipt {
    fn open(path: PathBuf, kind: Kind) -> AppResult<Option<Self>> {
        let file = match safe_fs::open_regular(&path) {
            Ok(file) => file,
            Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        safe_fs::verify_regular_at(&file, &path)?;
        let canonical = path.canonicalize()?;
        if !same_path(&paths::user_path(&path), &paths::user_path(&canonical))
            || path.file_name() != canonical.file_name()
        {
            return Err(AppError::Denied(
                "接收归档必须使用真实的非链接文件名".into(),
            ));
        }
        // Never authorize by a canonical path alone: the non-link open and
        // checks against the original spelling precede this canonicalization.
        let identity = safe_fs::identity(&canonical)?.ok_or_else(changed)?;
        safe_fs::verify_regular_at(&file, &canonical)?;
        safe_fs::verify_regular_at(&file, &path)?;
        Ok(Some(Self {
            path: canonical,
            file,
            kind,
            identity,
        }))
    }

    fn verify_at_path(&self) -> AppResult<()> {
        safe_fs::verify_regular_at(&self.file, &self.path)?;
        if safe_fs::identity(&self.path)?.as_ref() != Some(&self.identity) {
            return Err(changed());
        }
        Ok(())
    }

    fn fingerprint(
        &mut self,
        archive: ArchiveKind,
        cancel: &CancellationToken,
    ) -> AppResult<ReceiptFingerprint> {
        self.verify_at_path()?;
        let before = self.file.metadata()?;
        if before.len() == 0 || before.len() > MAX_ARCHIVE_BYTES {
            return Err(AppError::Invalid("接收归档必须非空且不超过 4 GiB".into()));
        }
        self.file.rewind()?;
        let mut input = (&mut self.file).take(before.len() + 1);
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        let mut prefix = [0; MAX_ARCHIVE_SIGNATURE_BYTES];
        let mut size = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if size < prefix.len() as u64 {
                let start = size as usize;
                let copied = (prefix.len() - start).min(count);
                prefix[start..start + copied].copy_from_slice(&buffer[..copied]);
            }
            size += count as u64;
            if size > before.len() {
                return Err(changed());
            }
            digest.update(&buffer[..count]);
        }
        let after = self.file.metadata()?;
        if size != before.len() || after.len() != size || before.modified()? != after.modified()? {
            return Err(changed());
        }
        self.verify_at_path()?;
        if !archive.matches(&prefix[..size.min(prefix.len() as u64) as usize]) {
            return Err(AppError::UnsafeArchive("接收文件不是声明格式的归档".into()));
        }
        self.file.rewind()?;
        Ok(ReceiptFingerprint {
            identity: self.identity.clone(),
            size,
            sha256: hex::encode(digest.finalize()),
        })
    }
}

fn receipt_preview(
    args: &Args,
    game: &Target,
    receipt: &Receipt,
    fingerprint: &ReceiptFingerprint,
) -> Value {
    json!({
        "gameId": game.id,
        "gameName": game.name,
        "filename": args.filename,
        "modName": args.name(),
        "projectPage": args.project_page,
        "receiptFolder": receipt.kind,
        "sourcePath": paths::display_path(&receipt.path),
        "sourceFingerprint": fingerprint,
        "warnings": ["本地哈希不证明下载来源或真实性", "只归档到管理器 Mods，不安装，不移除原文件"],
    })
}

fn inspect(
    engine: &FileEngine,
    args: &Args,
    cancel: &CancellationToken,
    selected: Option<&[String]>,
) -> AppResult<(Target, Receipt, ReceiptFingerprint)> {
    let game = args.target(engine, selected)?;
    let mut receipt = Folders::configured(engine)?.find(&args.filename)?;
    let fingerprint = receipt.fingerprint(archive_kind(&args.filename)?, cancel)?;
    Ok((game, receipt, fingerprint))
}

fn commit(
    engine: &FileEngine,
    args: &Args,
    game: &Target,
    mut receipt: Receipt,
    fingerprint: ReceiptFingerprint,
    cancel: &CancellationToken,
) -> AppResult<Value> {
    // Repeating the same receipt recovers the same publication (even after a
    // crash); different bytes, classification, or approved metadata get a new ID.
    let id_material = if let Some(id) = game.id.as_deref() {
        ("gameId", id)
    } else {
        ("gameName", game.name.as_str())
    };
    let id = format!(
        "received-{}",
        hex::encode(Sha256::digest(serde_json::to_vec(&(
            id_material,
            &receipt.kind,
            &args.filename,
            args.name(),
            &args.project_page,
            &fingerprint.sha256,
        ))?))
    );
    receipt.verify_at_path()?;
    let entry = mod_library::ingest_received_checked(
        engine,
        &ApprovedArtifact {
            artifact_id: &id,
            path: &receipt.path,
            filename: &args.filename,
            size: fingerprint.size,
            sha256: &fingerprint.sha256,
        },
        &game.name,
        game.id.as_deref(),
        ReceivedChecks {
            source: &mut receipt.file,
            expected_identity: &fingerprint.identity,
            mod_name: args.name(),
            project_page: args.project_page.as_deref(),
            cancel,
        },
    )?;
    Ok(json!(entry))
}

fn require_empty(args: &Value) -> AppResult<()> {
    if args.as_object().is_none_or(|object| !object.is_empty()) {
        return Err(AppError::Invalid("接收目录查看不接受参数".into()));
    }
    Ok(())
}

#[async_trait::async_trait]
impl AgentTool for ReceiptFoldersTool {
    fn history_arguments(&self, raw: &str) -> Option<String> {
        let valid =
            serde_json::from_str::<Value>(raw).is_ok_and(|args| require_empty(&args).is_ok());
        // Keep an invalid call invalid while avoiding any persistence of the
        // extraneous arguments (such as a provider's external path).
        Some(if valid { "{}" } else { "null" }.into())
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: FOLDERS_NAME.into(),
            description: "读取管理器下载收件目录 incoming 与原厂 Playwright 的输出目录 playwright（不可用时为 null）。可把 incoming 作为第三方 MCP 的明确下载保存目录；只接受直接存放在目录里的已完成归档。其他目录中的文件请使用桌面 Mods 页的选择器。".into(),
            parameters: json!({"type":"object","properties":{},"additionalProperties":false}),
            effect: Effect::Read,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        require_desktop_agent(context)?;
        require_empty(args)?;
        Ok(PreparedTool {
            preview: json!({}),
            effect: Effect::Read,
            plan: None,
        })
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        _prepared: PreparedTool,
    ) -> AppResult<Value> {
        require_desktop_agent(context)?;
        require_empty(&args)?;
        let engine = context.engine.clone();
        blocking(move || Ok(Folders::configured(&engine)?.preview())).await
    }
}

#[async_trait::async_trait]
impl AgentTool for ReceivedModTool {
    fn history_arguments(&self, raw: &str) -> Option<String> {
        Some(history_arguments(raw))
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: NAME.into(),
            description: "将已下载完成的归档收入管理器 Mods，不会安装。RAR 仅支持非加密单卷归档。只传已登记游戏的 gameId 或准确的游戏名称 gameName（也可用于未登记游戏）。仅使用 list_mod_receipt_folders 中的 incoming 或原厂 Playwright 输出目录；提供下载完成事件所报告的实际保存文件名（不是建议文件名，不含任何目录或 URL），仅从目录直属文件验证收件。其他保存位置须用桌面文件选择器，不能通过此工具导入。projectPage 仅为可选的公开项目网页（无凭据、查询参数或片段），不是下载 URL 或来源/版本证据。".into(),
            parameters: json!({
                "type":"object",
                "properties":{
                    "gameId":{"type":"string","minLength":1,"maxLength":MAX_NAME_BYTES},
                    "gameName":{"type":"string","minLength":1},
                    "filename":{"type":"string","minLength":1,"maxLength":180},
                    "modName":{"type":"string","minLength":1,"maxLength":MAX_NAME_BYTES},
                    "projectPage":{"type":"string","minLength":1,"maxLength":MAX_WEB_URL_BYTES}
                },
                "required":["filename"],
                "oneOf":[{"required":["gameId"]},{"required":["gameName"]}],
                "additionalProperties":false
            }),
            effect: EFFECT,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        require_desktop_agent(context)?;
        let args = Args::parse(args.clone())?;
        let engine = context.engine.clone();
        let scope = context.resolution_scope.clone();
        let cancel = context.cancel.clone();
        blocking(move || {
            let ResolutionScope::Agent(conversation) = &scope else {
                unreachable!()
            };
            let selected = super::workspace::selection(&engine.db, conversation)?;
            let (game, receipt, fingerprint) =
                inspect(&engine, &args, &cancel, selected.as_deref())?;
            Ok(PreparedTool {
                preview: receipt_preview(&args, &game, &receipt, &fingerprint),
                effect: EFFECT,
                plan: None,
            })
        })
        .await
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        require_desktop_agent(context)?;
        let args = Args::parse(args)?;
        if prepared.effect != EFFECT || prepared.plan.is_some() {
            return Err(changed());
        }
        let engine = context.engine.clone();
        let scope = context.resolution_scope.clone();
        let cancel = context.cancel.clone();
        blocking(move || {
            let ResolutionScope::Agent(conversation) = &scope else {
                unreachable!()
            };
            let selected = super::workspace::selection(&engine.db, conversation)?;
            let (game, receipt, fingerprint) =
                inspect(&engine, &args, &cancel, selected.as_deref())?;
            if prepared.preview != receipt_preview(&args, &game, &receipt, &fingerprint) {
                return Err(changed());
            }
            commit(&engine, &args, &game, receipt, fingerprint, &cancel)
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| AppError::Other("下载收件任务未完成".into()))?
}

#[cfg(test)]
#[path = "received_download_tests.rs"]
mod tests;
