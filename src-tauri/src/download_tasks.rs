use crate::{
    artifacts::{self, ArtifactRecord},
    download::{DownloadArtifact, DownloadManager, ProgressCallback},
    error::{AppError, AppResult},
    mod_library::{self, DownloadMetadata, LibraryEntry, NativeDownloadIntent},
    sources::{
        resolutions::{ResolutionScope, Resolutions},
        ResolvedMod,
    },
    transaction::FileEngine,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

/// Library classification supplied independently of a download's transport or source IDs.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassificationInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_page: Option<String>,
}

/// Parse each tool's existing transport keys separately from library metadata.
/// The catch-all preserves strict rejection of unknown keys with serde flatten.
#[derive(Deserialize, Serialize)]
pub(crate) struct AgentDownloadArgs<T> {
    #[serde(flatten)]
    pub transfer: T,
    #[serde(flatten)]
    pub classification: ClassificationInput,
    #[serde(flatten, skip_serializing_if = "Map::is_empty")]
    extra: Map<String, Value>,
}

impl<T: serde::de::DeserializeOwned> AgentDownloadArgs<T> {
    pub(crate) fn parse(args: &Value) -> AppResult<Self> {
        serde_json::from_value::<Self>(args.clone())
            .ok()
            .and_then(Self::validated)
            .ok_or_else(|| {
                AppError::Invalid("下载参数无效；只接受该工具所列字段及公开项目页".into())
            })
    }

    pub(crate) fn from_history(raw: &str) -> Option<Self> {
        serde_json::from_str::<Self>(raw).ok()?.validated()
    }

    fn validated(self) -> Option<Self> {
        (self.extra.is_empty() && self.classification.validate_metadata().is_ok()).then_some(self)
    }
}

impl ClassificationInput {
    pub(crate) fn for_agent(
        mut self,
        context: &crate::agent::tools::ToolContext,
    ) -> AppResult<DownloadTarget> {
        if let ResolutionScope::Agent(conversation) = &context.resolution_scope {
            let game = context.engine.db.conn().query_row(
                "SELECT g.id,g.name FROM conversations c JOIN games g ON g.id=c.game_id WHERE c.id=?1",
                [conversation],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ).optional()?;
            if let Some((id, name)) = game {
                if self.game_id.as_ref().is_some_and(|value| value != &id)
                    || self
                        .game_name
                        .as_ref()
                        .is_some_and(|value| value.trim() != name)
                {
                    return Err(AppError::Invalid(
                        "下载游戏与对话关联的登记游戏不一致".into(),
                    ));
                }
                self.game_id = Some(id);
                self.game_name = None;
            }
        }
        self.resolve_target(&context.engine)
    }

    pub(crate) fn resolve_target(self, engine: &FileEngine) -> AppResult<DownloadTarget> {
        self.validate_metadata()?;
        let game_name =
            match (&self.game_id, &self.game_name) {
                (Some(id), None) if !id.is_empty() => crate::games::get_game(&engine.db, id)?.name,
                (None, Some(name)) => name.clone(),
                _ => return Err(AppError::Invalid(
                    "下载前请选择 Mods 库的游戏：登记游戏 ID（gameId）或明确的游戏名称（gameName）"
                        .into(),
                )),
            };
        let intent = NativeDownloadIntent::new(
            &game_name,
            self.game_id.as_deref(),
            DownloadMetadata {
                project_page: self.project_page.as_deref(),
                author_hint: self.author.as_deref(),
            },
        )?;
        Ok(DownloadTarget {
            game_id: intent.game_id,
            game_name: intent.game_name,
            author: intent.author_hint,
            project_page: intent.project_page,
        })
    }

    pub(crate) fn validate_metadata(&self) -> AppResult<()> {
        if self.game_id.as_deref().is_some_and(|id| {
            id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        }) || self.game_name.as_deref().is_some_and(|name| {
            name.trim().is_empty() || name.chars().any(char::is_control) || name.contains("://")
        }) {
            return Err(AppError::Invalid("登记游戏 ID 或游戏名称格式无效".into()));
        }
        DownloadMetadata {
            project_page: self.project_page.as_deref(),
            author_hint: self.author.as_deref(),
        }
        .validate()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadTarget {
    game_id: Option<String>,
    game_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_page: Option<String>,
}

/// Keep legacy download fields and add the typed library identity. `path`
/// deliberately names the finalized Mods copy, not the transport cache.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDownloadReceipt {
    #[serde(flatten)]
    pub artifact: DownloadArtifact,
    pub artifact_id: String,
    pub game_id: Option<String>,
    pub game_name: String,
    pub project_page: Option<String>,
    pub author_hint: Option<String>,
}

impl LibraryDownloadReceipt {
    pub(crate) fn into_user_paths(mut self) -> Self {
        self.artifact = self.artifact.into_user_paths();
        self
    }
}

/// Transport-only input. Values do not leave the local coordinator and the
/// signed URL is never an input to the catalog writer.
pub(crate) enum CompletedSource<'a> {
    Catalog(&'a ResolvedMod),
    Web(&'a str),
    ModInn {
        work_id: u64,
        file_id: u64,
        url: &'a str,
    },
}

pub(crate) fn valid_retry_artifact_id(raw: &str) -> bool {
    uuid::Uuid::parse_str(raw).is_ok_and(|id| id.hyphenated().to_string() == raw)
}

fn receipt(
    engine: &FileEngine,
    mut artifact: DownloadArtifact,
    entry: LibraryEntry,
) -> AppResult<LibraryDownloadReceipt> {
    artifact.path = crate::paths::safe_join(&engine.paths.root, &entry.archive_relative_path)?;
    Ok(LibraryDownloadReceipt {
        artifact,
        artifact_id: entry.id,
        game_id: entry.game_id,
        game_name: entry.game_name,
        project_page: entry.project_page,
        author_hint: entry.author_hint,
    })
}

impl DownloadTarget {
    fn metadata(&self) -> DownloadMetadata<'_> {
        DownloadMetadata {
            project_page: self.project_page.as_deref(),
            author_hint: self.author.as_deref(),
        }
    }

    pub(crate) fn omit_transport_page(&mut self, url: &str) {
        if self
            .project_page
            .as_deref()
            .is_some_and(|page| artifacts::same_public_endpoint(page, url))
        {
            self.project_page = None;
        }
    }

    pub(crate) async fn finish(
        self,
        engine: &FileEngine,
        artifact: DownloadArtifact,
        source: CompletedSource<'_>,
    ) -> AppResult<LibraryDownloadReceipt> {
        let intent =
            NativeDownloadIntent::new(&self.game_name, self.game_id.as_deref(), self.metadata())?;
        let artifact_id = artifact.id.clone();
        // Persist the exact receipt and original approved classification before
        // the first attempt; retry still works if publication never reserves a row.
        let recorded = match source {
            CompletedSource::Catalog(source) => {
                artifacts::record_with_library_intent(engine, &artifact, source, &intent)
            }
            CompletedSource::Web(url) => artifacts::record_web_with_library_intent(
                engine,
                &artifact,
                &intent.without_transport_page(url),
            ),
            CompletedSource::ModInn {
                work_id,
                file_id,
                url,
            } => artifacts::record_glossmod_file_with_library_intent(
                engine,
                &artifact,
                work_id,
                file_id,
                &intent.without_transport_page(url),
            ),
        };
        recorded.map_err(|error| {
            AppError::Other(format!(
                "下载文件已保存，但无法记录回执；请勿重复下载。{error}"
            ))
        })?;
        let engine = engine.clone();
        let result = tokio::task::spawn_blocking(move || {
            let saved = artifacts::for_id(&engine.db, &artifact.id)?
                .ok_or_else(|| AppError::NotFound("已保存的下载回执".into()))?;
            let entry = mod_library::ingest_new_download_with_metadata(
                &engine,
                &saved,
                &self.game_name,
                self.game_id.as_deref(),
                self.metadata(),
            )?;
            receipt(&engine, saved.artifact, entry)
        })
        .await
        .unwrap_or_else(|_| Err(AppError::Other("下载入库任务未完成".into())));
        result.map_err(|error| AppError::Other(format!("下载文件已保存，但尚未入库（artifactId={artifact_id}）。请重试该回执（桌面 Agent: retry_mod_library_download），不要重新下载。{error}")))
    }

    pub(crate) fn saved_for_retry(
        &self,
        engine: &FileEngine,
        id: &str,
    ) -> AppResult<ArtifactRecord> {
        if !valid_retry_artifact_id(id) {
            return Err(AppError::Invalid("下载回执 ID 无效".into()));
        }
        let saved = artifacts::for_id(&engine.db, id)?
            .ok_or_else(|| AppError::NotFound("已保存的下载回执".into()))?;
        let intent = saved.native_library_intent().ok_or_else(|| {
            AppError::Denied("该回执不是已分类的原生下载，不能用重试入库。".into())
        })?;
        intent.check_target(&self.game_name, self.game_id.as_deref())?;
        if self.author.is_some() || self.project_page.is_some() {
            return Err(AppError::Invalid("重试不能更改原下载的作者或项目页".into()));
        }
        Ok(saved)
    }

    pub(crate) async fn retry_saved(
        self,
        engine: &FileEngine,
        saved: ArtifactRecord,
        cancel: CancellationToken,
    ) -> AppResult<LibraryDownloadReceipt> {
        let engine = engine.clone();
        tokio::task::spawn_blocking(move || {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            // Storage rechecks this exact artifact record and its original
            // classification under the write lock; never load a substitute.
            let entry = mod_library::retry_native_download(
                &engine,
                &saved,
                &self.game_name,
                self.game_id.as_deref(),
            )?;
            receipt(&engine, saved.artifact, entry)
        })
        .await
        .map_err(|_| AppError::Other("下载入库重试未完成".into()))?
    }
}

fn agent_game_properties() -> Value {
    json!({
        "gameId":{"type":"string","description":"本地登记游戏 ID；已关联游戏的聊天可省略"},
        "gameName":{"type":"string","description":"未关联登记游戏时可填写明确的游戏名称；也可使用未登记的游戏名称"},
    })
}

pub(crate) fn agent_download_schema(mut transfer: Value, required: Vec<&str>) -> Value {
    let properties = transfer
        .as_object_mut()
        .expect("tool properties are an object");
    properties.extend(agent_game_properties().as_object().unwrap().clone());
    properties.extend([
        ("author".into(), json!({"type":"string","description":"公开作者名"})),
        ("projectPage".into(), json!({"type":"string","description":"公开 HTTPS 项目页，不是下载链接；不含签名、参数、凭据或片段"})),
    ]);
    json!({"type":"object","properties":transfer,"required":required,"additionalProperties":false})
}

pub(crate) fn agent_retry_schema() -> Value {
    let mut properties = agent_game_properties();
    properties["artifactId"] = json!({"type":"string","format":"uuid"});
    json!({"type":"object","properties":properties,"required":["artifactId"],"additionalProperties":false})
}

#[derive(Clone, Default)]
pub struct DownloadTasks(Arc<Mutex<BTreeMap<String, CancellationToken>>>);
struct Guard {
    tasks: DownloadTasks,
    id: String,
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.tasks
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}
impl DownloadTasks {
    pub fn cancel(&self, id: &str) -> AppResult<()> {
        let tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        tasks
            .get(id)
            .ok_or_else(|| AppError::NotFound("下载任务".into()))?
            .cancel();
        Ok(())
    }
    pub(crate) async fn run(
        &self,
        engine: &FileEngine,
        resolutions: &Resolutions,
        id: String,
        resolution_id: uuid::Uuid,
        classification: ClassificationInput,
        progress: ProgressCallback,
    ) -> AppResult<LibraryDownloadReceipt> {
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(AppError::Invalid("下载任务ID无效".into()));
        }
        let cancel = CancellationToken::new();
        {
            let mut tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if tasks.contains_key(&id) {
                return Err(AppError::Conflict("下载任务已运行".into()));
            }
            tasks.insert(id.clone(), cancel.clone());
        }
        let _guard = Guard {
            tasks: self.clone(),
            id,
        };
        let target = classification.resolve_target(engine)?;
        let resolved = resolutions.take(&ResolutionScope::Desktop, resolution_id)?;
        let artifact = DownloadManager::new(&engine.paths)?
            .download_mod(&resolved, cancel, progress)
            .await?;
        target
            .finish(engine, artifact, CompletedSource::Catalog(&resolved))
            .await
    }

    /// Retry local classification by immutable receipt ID. No resolution token,
    /// replacement file path, or source credentials are needed.
    pub async fn retry(
        &self,
        engine: &FileEngine,
        artifact_id: &str,
        classification: ClassificationInput,
    ) -> AppResult<LibraryDownloadReceipt> {
        let target = classification.resolve_target(engine)?;
        let saved = target.saved_for_retry(engine, artifact_id)?;
        target
            .retry_saved(engine, saved, CancellationToken::new())
            .await
    }
}

#[cfg(test)]
#[path = "download_tasks_tests.rs"]
mod tests;
