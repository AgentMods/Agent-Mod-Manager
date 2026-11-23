use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    download::DownloadManager,
    download_tasks::{agent_download_schema, AgentDownloadArgs, CompletedSource, DownloadTarget},
    error::{AppError, AppResult},
    sources::{
        resolutions::ResolutionScope,
        web_download::{WebDownload, MAX_WEB_URL_BYTES},
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) const NAME: &str = "download_web_mod";

pub(super) fn register(registry: &ToolRegistry) -> AppResult<()> {
    registry.register(Arc::new(WebDownloadTool))
}

struct WebDownloadTool;

#[derive(Deserialize, Serialize)]
struct Transfer {
    url: String,
}

pub(super) fn history_arguments(raw: &str) -> String {
    // Keep only the canonical public URL. In particular, do not store a
    // rejected signed URL in the assistant tool call or tool-start event.
    AgentDownloadArgs::<Transfer>::from_history(raw)
        .and_then(|mut args| {
            args.transfer.url = WebDownload::parse(&args.transfer.url)
                .ok()?
                .url()
                .to_owned();
            serde_json::to_string(&args).ok()
        })
        .unwrap_or_else(|| "{}".into())
}

fn request(context: &ToolContext, args: &Value) -> AppResult<(WebDownload, DownloadTarget)> {
    if context.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if !matches!(context.resolution_scope, ResolutionScope::Agent(_)) {
        return Err(AppError::Denied(
            "网页归档下载只在桌面 Agent 对话内接受审批".into(),
        ));
    }
    let args = AgentDownloadArgs::<Transfer>::parse(args)?;
    let file = WebDownload::parse(&args.transfer.url)?;
    let mut classification = args.classification.for_agent(context)?;
    classification.omit_transport_page(file.url());
    Ok((file, classification))
}

fn preview(request: &WebDownload, classification: &DownloadTarget) -> Value {
    json!({
        "url": request.url(),
        "filename": request.filename(),
        "sizePolicy": "按实际可用磁盘空间执行，未声明大小的响应使用较小预算",
        "destination": "Mods/",
        "classification": classification,
        "redirects": "允许经逐跳公网校验的 HTTPS 重定向，最终下载主机可能不同",
        "warnings": ["未绑定来源、项目或版本；下载后须检查归档内容", "本次只下载，不解压、不安装"],
    })
}

#[async_trait::async_trait]
impl AgentTool for WebDownloadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: NAME.into(),
            description: "仅用于用户明确点名通过管理器的直链下载工具传输公开 HTTPS ZIP/JAR/7z/RAR/tar.gz 归档。常规 Mod 下载请先用来源 MCP，无法取得文件时用 Playwright MCP 的浏览器下载。不要猜测 URL，不要提交网页、私有链接、登录链接或带参数/签名的地址。聊天未关联游戏时提供登记游戏 ID 或明确的游戏名称，作为 Mods 库游戏分类。结果不是目录版本、不会自动安装；安装需要另外核对文件和创建安装计划。".into(),
            parameters: agent_download_schema(json!({"url":{"type":"string","minLength":1,"maxLength":MAX_WEB_URL_BYTES}}), vec!["url"]),
            effect: Effect::UncatalogedDownload,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let (request, classification) = request(context, args)?;
        Ok(PreparedTool {
            preview: preview(&request, &classification),
            effect: Effect::UncatalogedDownload,
            plan: None,
        })
    }

    async fn execute(
        &self,
        context: &ToolContext,
        args: Value,
        prepared: PreparedTool,
    ) -> AppResult<Value> {
        let (request, classification) = request(context, &args)?;
        if prepared.effect != Effect::UncatalogedDownload
            || prepared.plan.is_some()
            || prepared.preview != preview(&request, &classification)
        {
            return Err(AppError::Denied(
                "网页下载与审批的地址或文件名不一致".into(),
            ));
        }
        let progress = context.progress.as_ref().map(|notify| {
            let notify = notify.clone();
            Arc::new(move |progress| notify(json!({"download":progress})))
                as crate::download::ProgressCallback
        });
        let artifact = DownloadManager::new(&context.engine.paths)?
            .download_web(&request, context.cancel.clone(), progress)
            .await?;
        let artifact = classification
            .finish(
                &context.engine,
                artifact,
                CompletedSource::Web(request.url()),
            )
            .await?;
        Ok(json!(artifact.into_user_paths()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::tests::setup;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn invalid_web_arguments_do_not_record_secrets_or_extra_claims() {
        let url = "https://example.com/file.zip";
        let normalized = json!({"url":url}).to_string();
        assert_eq!(history_arguments(&normalized), normalized);
        for raw in [
            json!({"url":"https://example.com/file.zip?token=private"}).to_string(),
            json!({"url":url,"source_id":"modrinth"}).to_string(),
            json!({"url":url,"projectPage":"https://example.com/mod?token=private"}).to_string(),
            json!({"url":"https://example.com/mod-page"}).to_string(),
            format!(r#"{{"url":"{url}","url":"{url}"}}"#),
        ] {
            assert_eq!(history_arguments(&raw), "{}");
        }
    }

    #[tokio::test]
    async fn prepared_url_cannot_be_substituted_and_tool_stays_agent_only() {
        let (_dir, engine, _) = setup();
        let conversation = crate::history::History::new(engine.db.clone())
            .create_conversation("Fixture", None)
            .unwrap();
        let mut context = ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: ResolutionScope::Agent(conversation.id),
        };
        let args = json!({"url":"https://example.com/file.zip","gameName":"Example Game"});
        let prepared = WebDownloadTool.prepare(&context, &args).await.unwrap();
        assert_eq!(prepared.preview["url"], args["url"]);
        assert_eq!(prepared.preview["filename"], "file.zip");
        assert!(matches!(
            WebDownloadTool
                .execute(
                    &context,
                    json!({"url":"https://example.net/file.zip","gameName":"Example Game"}),
                    prepared
                )
                .await,
            Err(AppError::Denied(_))
        ));
        context.resolution_scope = ResolutionScope::Mcp("fixture".into());
        assert!(matches!(
            WebDownloadTool.prepare(&context, &args).await,
            Err(AppError::Denied(_))
        ));
        crate::storage::assert_downloads_empty(&context.engine.paths);
        assert_eq!(
            std::fs::read_dir(context.engine.paths.mod_library())
                .unwrap()
                .count(),
            0
        );
    }
}
