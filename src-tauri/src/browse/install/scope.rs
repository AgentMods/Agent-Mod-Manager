mod tool_definitions;

use super::{stock, three_dm, verified_install, GameScope, WorkKey};
use crate::{
    agent::{
        policy::Effect,
        tools::{AgentTool, PreparedTool, ToolContext, ToolRegistry},
    },
    artifacts::DownloadOrigin,
    db::Db,
    error::{AppError, AppResult},
    sources::resolutions::Resolutions,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::Mutex;

const MOD_INN_FILES: &str = "glossmod_work_files";
const MOD_INN_DOWNLOAD: &str = "glossmod_attachment_download";

/// Created solely from the result of native browse capability verification.
/// It is deliberately not a deserializable TurnRequest field.
pub(crate) struct InstallScope {
    id: String,
    key: WorkKey,
    game: GameScope,
    receipt: Mutex<Option<stock::StockReceipt>>,
}

impl InstallScope {
    pub(super) fn new(id: String, key: WorkKey, game: GameScope) -> Self {
        Self {
            id,
            key,
            game,
            receipt: Mutex::new(None),
        }
    }

    pub(crate) fn identity(&self) -> Value {
        json!({
            "gameId": self.key.game_id,
            "providerGameId": self.key.provider_game_id,
            "sourceId": self.key.source_id,
            "catalogSourceId": self.key.catalog_id(),
            "projectId": self.key.origin_project(),
        })
    }

    pub(crate) fn instruction(&self) -> String {
        let identity = self.identity();
        let workflow = if self.key.source_id == "glossmod_http" {
            "用官方 Mod 客栈 list_work_files 查询此 work_id，核对附件、版本与 entitlement；只使用官方 get_work_file_download 下载本作品的附件。"
        } else if self.key.source_id == "glossmod_3dm" {
            "用原厂 3DM MCP get-mod-detail 查询作品，id 取 projectId 的整数值。用返回的 download.resolution_id 调用 download_mod。若不能验证官方直链，提示用户在原作品页选择文件并手动导入。"
        } else if stock::download_tool(&self.key.source_id).is_some() {
            "已有可信下载回执时，用 file_received_mod 归档回执中的精确文件名；省略 gameName、modName 和 projectPage。否则用原厂附件或版本列表确认版本，对同一作品调用 download_file 或 download_version。若无法验证、账号拒绝或只能从网站手动下载，明确停止并提示用户到原作品页选择并手动导入。下载工具直接返回 artifactId 时，使用该已归档原包。"
        } else {
            "用 resolve_mod 解析这个作品，source_id 取 catalogSourceId；需要时用 list_mod_versions 或 ask_user 确认版本。用 download_mod 消费该会话签发的 resolution_id。"
        };
        format!("用户从浏览卡片发起了本机安装。下列 JSON 由原生浏览结果生成，不来自翻译、作品文案或模型推断：{identity}\n只处理这一个来源作品和登记游戏，不要搜索替代品、打开任意 URL 或安装其他依赖。{workflow}\n可先用 list_mod_library 检查已有的本作品归档。用 match_mod_install 检查已归档原包，再用 install_library_mod 审批原生安装计划。兼容性、版本、映射或用户意图不明确时用 ask_user；如果无法完成就停下说明，不根据下载成功或模型文本宣称已安装。明确停止于一个经验证的安装事务。")
    }

    pub(crate) fn check_game(&self, db: &Db) -> AppResult<()> {
        if !self.game.current(db) {
            return Err(AppError::Conflict("安装目标游戏已变化".into()));
        }
        Ok(())
    }

    pub(crate) fn allows(&self, name: &str, tool: &dyn AgentTool) -> bool {
        if let Some((source, remote)) = tool.stock_source_tool() {
            if source == self.key.source_id && stock::permits(source, remote) {
                return true;
            }
        }
        let kind = tool.host_tool_kind();
        match name {
            "ask_user"
            | "list_mod_library"
            | "match_mod_install"
            | "install_library_mod"
            | "inspect_install" => kind.is_none(),
            "file_received_mod" => {
                kind.is_none() && stock::download_tool(&self.key.source_id).is_some()
            }
            "list_mod_versions" | "resolve_mod" | "download_mod" | "retry_mod_library_download" => {
                self.key.catalog_id().is_some() && kind.is_none()
            }
            _ => {
                self.key.source_id == "glossmod_http"
                    && (matches!(kind, Some(MOD_INN_DOWNLOAD))
                        || (kind == Some(MOD_INN_FILES)
                            && tool.stock_source_tool()
                                == Some(("glossmod_http", "list_work_files"))))
            }
        }
    }

    fn matches_origin(&self, origin: &DownloadOrigin) -> bool {
        Some(origin.source_id.as_str()) == self.key.origin_id()
            && origin.project_id == self.key.origin_project()
    }

    fn artifact_matches(&self, db: &Db, id: &str) -> AppResult<bool> {
        let native = crate::artifacts::for_id(db, id)?
            .and_then(|record| record.origin())
            .is_some_and(|origin| self.matches_origin(&origin));
        if native || self.stock_matches(db, id, None)? {
            return Ok(true);
        }
        // A previous attempt may have committed its local archive before the
        // dispatcher could link it to the work. Recover only exact receipt bytes.
        if let Some(receipt) = self.stock_receipt(db)? {
            return self.record_stock_archive(db, id, &receipt);
        }
        Ok(false)
    }

    fn stock_matches(&self, db: &Db, id: &str, hash: Option<&str>) -> AppResult<bool> {
        Ok(db.conn().query_row("SELECT EXISTS (SELECT 1 FROM browse_install_downloads d JOIN browse_install_tasks b ON b.id=d.task_id
              JOIN mod_library_entries l ON l.id=d.artifact_id AND l.game_id=b.game_id AND l.sha256=d.sha256 AND l.state='ready'
              WHERE d.artifact_id=?1 AND b.game_id=?2 AND b.source_id=?3 AND b.project_id=?4 AND b.provider_game_id=?5
                AND (?6 IS NULL OR d.sha256=?6))",
            params![id, self.key.game_id, self.key.source_id, self.key.project_id, self.key.provider_game_id, hash],
            |r| r.get(0),
        )?)
    }

    fn stock_receipt(&self, db: &Db) -> AppResult<Option<stock::StockReceipt>> {
        let Some(remote) = stock::download_tool(&self.key.source_id) else {
            return Ok(None);
        };
        if let Some(receipt) = self
            .receipt
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(Some(receipt));
        }
        // Only the exact stock tool identity can establish a persisted receipt.
        // Read the latest attempt once, so a failed later download cannot fall
        // back to an earlier file or version.
        let namespaced = crate::mcp::client::namespaced_tool_name(&self.key.source_id, remote);
        let connection = db.conn();
        let mut stmt = connection.prepare(
            "SELECT t.arguments_json, t.result_json, t.status
             FROM browse_install_tasks b JOIN tool_calls t ON t.conversation_id=b.conversation_id
             WHERE b.game_id=?1 AND b.source_id=?2 AND b.provider_game_id=?3
               AND CASE WHEN b.source_id='nexusmods' AND instr(b.project_id,'/')=0
                        THEN b.provider_game_id || '/' || b.project_id ELSE b.project_id END=?4
               AND t.name=?5
             ORDER BY t.rowid DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![
            self.key.game_id,
            self.key.source_id,
            self.key.provider_game_id,
            self.key.origin_project().as_ref(),
            namespaced,
        ])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        if row.get::<_, String>(2)? != "done" {
            return Ok(None);
        }
        let raw_args = row.get::<_, String>(0)?;
        let Some(raw_result) = row.get::<_, Option<String>>(1)? else {
            return Ok(None);
        };
        let Ok(args) = serde_json::from_str::<Value>(&raw_args) else {
            return Ok(None);
        };
        let Ok(value) = serde_json::from_str::<Value>(&raw_result) else {
            return Ok(None);
        };
        Ok(stock::StockReceipt::from_saved(&self.key, &args, &value))
    }

    fn pending_receipt(&self, db: &Db) -> AppResult<stock::StockReceipt> {
        self.stock_receipt(db)?.ok_or_else(boundary)
    }

    fn record_stock_archive(
        &self,
        db: &Db,
        id: &str,
        receipt: &stock::StockReceipt,
    ) -> AppResult<bool> {
        if !id.strip_prefix("received-").is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            return Ok(false);
        }
        let changed = db.conn().execute(
            "INSERT INTO browse_install_downloads(task_id,artifact_id,file_id,sha256)
             SELECT b.id,l.id,?3,l.sha256 FROM mod_library_entries l
             JOIN browse_install_tasks b ON b.id=?1 AND b.game_id=l.game_id
             WHERE l.id=?2 AND l.sha256=?4 AND l.filename=?5 AND l.size=?6
               AND l.state='ready' AND l.source_id='local'
               AND b.game_id=?7 AND b.source_id=?8 AND b.project_id=?9 AND b.provider_game_id=?10
             ON CONFLICT(task_id,artifact_id) DO UPDATE SET file_id=excluded.file_id
               WHERE browse_install_downloads.sha256=excluded.sha256
                 AND browse_install_downloads.file_id=excluded.file_id",
            params![
                self.id,
                id,
                receipt.file_id,
                receipt.sha256,
                receipt.filename,
                receipt.size,
                self.key.game_id,
                self.key.source_id,
                self.key.project_id,
                self.key.provider_game_id
            ],
        )?;
        Ok(changed == 1)
    }

    pub(crate) fn arguments(
        &self,
        db: &Db,
        tool: &dyn AgentTool,
        name: &str,
        mut args: Value,
    ) -> AppResult<Value> {
        let kind = tool.host_tool_kind();
        if !self.allows(name, tool) {
            return Err(boundary());
        }
        self.check_game(db)?;
        if let Some((source, remote)) = tool.stock_source_tool() {
            if source == self.key.source_id && stock::permits(source, remote) {
                stock::arguments(&self.key, remote, &mut args)?;
                return Ok(args);
            }
        }
        let game = json!(self.key.game_id);
        match name {
            "resolve_mod" | "list_mod_versions" => {
                let catalog = json!(self.key.catalog_id());
                if args["source_id"] == self.key.source_id {
                    args["source_id"] = catalog;
                } else {
                    bind(&mut args, "source_id", catalog)?;
                }
                bind(&mut args, "project_id", json!(self.key.origin_project()))?;
                bind(&mut args, "game_id", json!(self.key.provider_game_id))?;
            }
            "download_mod" | "retry_mod_library_download" => {
                bind_classification(&mut args, game)?;
                if name == "retry_mod_library_download"
                    && !self
                        .artifact_matches(db, args["artifactId"].as_str().ok_or_else(boundary)?)?
                {
                    return Err(boundary());
                }
            }
            "list_mod_library" | "inspect_install" | "match_mod_install" => {
                bind(&mut args, "game_id", game)?;
                if name == "match_mod_install"
                    && !self
                        .artifact_matches(db, args["artifact_id"].as_str().ok_or_else(boundary)?)?
                {
                    return Err(boundary());
                }
            }
            "install_library_mod" => {
                let request = args.get_mut("request").ok_or_else(boundary)?;
                bind(request, "gameId", game)?;
                if request["mode"] == "gtaRpf"
                    || !self.artifact_matches(
                        db,
                        request["artifactId"].as_str().ok_or_else(boundary)?,
                    )?
                {
                    return Err(boundary());
                }
            }
            "file_received_mod" => {
                let receipt = self.pending_receipt(db)?;
                let fields = args.as_object().ok_or_else(boundary)?;
                if fields.keys().any(|field| {
                    !matches!(
                        field.as_str(),
                        "filename" | "gameId" | "gameName" | "modName" | "projectPage"
                    )
                }) || fields.get("gameName").is_some_and(|name| {
                    !name.is_null()
                        && !name.as_str().is_some_and(|name| {
                            crate::mod_library::game_key(name)
                                == crate::mod_library::game_key(&self.game.name)
                        })
                }) {
                    return Err(boundary());
                }
                bind(&mut args, "filename", json!(receipt.filename))?;
                bind(&mut args, "gameId", game)?;
                // Keep native classification and a stable receipt identity across retries.
                args = receipt.import_args(&self.key);
            }
            "ask_user" => {}
            _ => {
                let work = self
                    .key
                    .project_id
                    .parse::<std::num::NonZeroU64>()
                    .map_err(|_| boundary())?;
                bind(&mut args, "work_id", json!(work))?;
                if kind == Some(MOD_INN_DOWNLOAD) {
                    bind_classification(&mut args, game)?;
                } else if args.as_object().is_none_or(|obj| obj.len() != 1) {
                    // Do not relay model-controlled URL, file paths, headers, or other fields.
                    return Err(boundary());
                }
            }
        }
        Ok(args)
    }

    pub(crate) fn validate_prepared(
        &self,
        db: &Db,
        name: &str,
        tool: &dyn AgentTool,
        args: &Value,
        prepared: &PreparedTool,
    ) -> AppResult<()> {
        let view = &prepared.preview;
        let kind = tool.host_tool_kind();
        if let Some((source, remote)) = tool.stock_source_tool() {
            if source == self.key.source_id && stock::permits(source, remote) {
                if prepared.effect != Effect::ExternalWrite
                    || prepared.plan.is_some()
                    || view["tool"] != remote
                    || view["arguments"] != *args
                {
                    return Err(boundary());
                }
                return Ok(());
            }
        }
        match name {
            "download_mod" => {
                if view["source_id"].as_str() != self.key.catalog_id()
                    || view["project_id"] != self.key.origin_project().as_ref()
                    || view["classification"]["gameId"] != self.key.game_id
                    || prepared.effect != Effect::ManagerWrite
                    || prepared.plan.is_some()
                {
                    return Err(boundary());
                }
            }
            "install_library_mod" => {
                let id = args["request"]["artifactId"]
                    .as_str()
                    .ok_or_else(boundary)?;
                if view["gameId"] != self.key.game_id
                    || view["installId"].as_str().is_none()
                    || prepared.effect != Effect::ManagerWrite
                    || prepared.plan.is_none()
                {
                    return Err(boundary());
                }
                if view["downloadOrigin"].is_null() {
                    let hash = view["artifactSha256"].as_str().ok_or_else(boundary)?;
                    if !self.stock_matches(db, id, Some(hash))? {
                        return Err(boundary());
                    }
                } else {
                    let origin: DownloadOrigin =
                        serde_json::from_value(view["downloadOrigin"].clone())
                            .map_err(|_| boundary())?;
                    if !self.matches_origin(&origin) || origin.artifact_id != id {
                        return Err(boundary());
                    }
                }
            }
            "file_received_mod"
                if prepared.effect != Effect::UncatalogedDownload
                    || prepared.plan.is_some()
                    || !self
                        .pending_receipt(db)?
                        .matches_preview(&self.key.game_id, view) =>
            {
                return Err(boundary());
            }
            _ if kind == Some(MOD_INN_DOWNLOAD)
                && (view["serverId"] != self.key.source_id
                    || view["work_id"] != args["work_id"]
                    || view["file_id"] != args["file_id"]
                    || view["classification"]["gameId"] != self.key.game_id
                    || prepared.effect != Effect::UncatalogedDownload
                    || prepared.plan.is_some()) =>
            {
                return Err(boundary());
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn record_plan(
        &self,
        db: &Db,
        name: &str,
        args: &Value,
        prepared: &PreparedTool,
    ) -> AppResult<()> {
        self.check_game(db)?;
        if name != "install_library_mod" {
            return Ok(());
        }
        let plan = prepared.plan.as_ref().ok_or_else(boundary)?;
        let view = &prepared.preview;
        let conn = db.conn();
        if verified_install(&conn, &self.id)?.is_some() {
            return Err(boundary());
        }
        let count = conn.execute(
            "UPDATE browse_install_tasks SET install_id=?2,transaction_id=?3,artifact_id=?4 WHERE id=?1",
            params![self.id, view["installId"].as_str(), plan.id, args["request"]["artifactId"].as_str()],
        )?;
        if count != 1 {
            return Err(boundary());
        }
        Ok(())
    }

    pub(crate) fn before_tool(&self, stock: Option<(&str, &str)>) {
        if stock.is_some_and(|(_, remote)| stock::downloading(remote)) {
            *self.receipt.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    pub(crate) async fn finish_tool(
        &self,
        registry: &ToolRegistry,
        resolutions: &Resolutions,
        context: &ToolContext,
        invocation: (&str, Option<(&str, &str)>),
        args: &Value,
        value: Value,
    ) -> AppResult<Value> {
        let db = &context.engine.db;
        let (name, stock) = invocation;
        if let Some((source, remote)) = stock {
            if source == self.key.source_id && stock::permits(source, remote) {
                self.check_game(db)?;
                if stock::downloading(remote) {
                    let receipt =
                        stock::transfer_receipt(&self.key, args, &value, context, registry).await?;
                    let payload = receipt.saved(&self.key);
                    *self.receipt.lock().unwrap_or_else(|e| e.into_inner()) = Some(receipt);
                    return Ok(payload);
                }
                if source == "glossmod_3dm" {
                    let resolution = three_dm::resolution(&self.key, &value)?;
                    return Ok(serde_json::to_value(
                        resolutions.publish(context.resolution_scope.clone(), resolution)?,
                    )?);
                }
                return stock::list_result(&value);
            }
            if source == "glossmod_http" && remote == "list_work_files" {
                return stock::list_result(&value);
            }
        }
        if name == "file_received_mod" {
            self.check_game(db)?;
            let receipt = self.pending_receipt(db)?;
            let id = value["artifactId"].as_str().ok_or_else(boundary)?;
            if value["gameId"] != self.key.game_id
                || value["filename"] != receipt.filename
                || value["sha256"] != receipt.sha256
                || value["size"] != receipt.size
            {
                return Err(boundary());
            }
            if !self.record_stock_archive(db, id, &receipt)? {
                return Err(boundary());
            }
            *self.receipt.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        if name == "list_mod_library" {
            let entries = value.as_array().ok_or_else(boundary)?;
            let mut matching = Vec::new();
            for entry in entries {
                if let Some(id) = entry["artifactId"].as_str() {
                    if self.artifact_matches(db, id)? {
                        matching.push(entry.clone());
                    }
                }
            }
            return Ok(Value::Array(matching));
        }
        Ok(value)
    }

    pub(crate) fn verified(&self, db: &Db) -> AppResult<bool> {
        Ok(verified_install(&db.conn(), &self.id)?.is_some())
    }
}

pub(super) fn bind(args: &mut Value, key: &str, expected: Value) -> AppResult<()> {
    let args = args.as_object_mut().ok_or_else(boundary)?;
    if args
        .get(key)
        .is_some_and(|current| !current.is_null() && *current != expected)
    {
        return Err(boundary());
    }
    args.insert(key.into(), expected);
    Ok(())
}

fn bind_classification(args: &mut Value, game: Value) -> AppResult<()> {
    let obj = args.as_object_mut().ok_or_else(boundary)?;
    // Never persist a model-authored page/author as evidence of the identity.
    for key in ["gameName", "author", "projectPage"] {
        obj.remove(key);
    }
    bind(args, "gameId", game)
}

pub(super) fn boundary() -> AppError {
    AppError::Denied("工具、作品、附件、游戏或安装预览超出页内安装范围".into())
}
