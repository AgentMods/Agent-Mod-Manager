use super::{
    policy::Effect,
    tools::{AgentTool, PreparedTool, ToolContext, ToolDefinition, ToolRegistry},
};
use crate::{
    download::DownloadManager,
    error::{AppError, AppResult},
    sources::{
        resolutions::ResolutionScope,
        web_download::{WebDownload, MAX_WEB_BYTES, MAX_WEB_URL_BYTES},
    },
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) const NAME: &str = "download_web_mod";

pub(super) fn register(registry: &ToolRegistry) -> AppResult<()> {
    registry.register(Arc::new(WebDownloadTool))
}

struct WebDownloadTool;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    url: String,
}

pub(super) fn history_arguments(raw: &str) -> String {
    // Keep only the canonical public URL. In particular, do not store a
    // rejected signed URL in the assistant tool call or tool-start event.
    serde_json::from_str::<Args>(raw)
        .ok()
        .and_then(|args| WebDownload::parse(&args.url).ok())
        .map_or_else(|| "{}".into(), |file| json!({"url":file.url()}).to_string())
}

fn request(context: &ToolContext, args: Value) -> AppResult<WebDownload> {
    if context.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if !matches!(context.resolution_scope, ResolutionScope::Agent(_)) {
        return Err(AppError::Denied(
            "网页归档下载只在桌面 Agent 对话内接受审批".into(),
        ));
    }
    let args: Args = serde_json::from_value(args).map_err(|_| {
        AppError::Invalid("只接受 url：不含签名或查询参数的公开 HTTPS 归档直链".into())
    })?;
    WebDownload::parse(&args.url)
}

fn preview(request: &WebDownload) -> Value {
    json!({
        "url": request.url(),
        "filename": request.filename(),
        "maxBytes": MAX_WEB_BYTES,
        "destination": "downloads/",
        "redirects": "允许经逐跳公网校验的 HTTPS 重定向，最终下载主机可能不同",
        "warnings": ["未绑定来源、项目或版本；下载后须检查归档内容", "本次只下载，不解压、不安装"],
    })
}

#[async_trait::async_trait]
impl AgentTool for WebDownloadTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: NAME.into(),
            description: "仅在目录来源不能下载、且用户要求下载时，提交用户明确给出或证据中直接出现的公开 HTTPS ZIP/JAR/7z/tar.gz 直链，经用户审批后下载至管理器 downloads。不要猜测 URL，不要提交网页、私有链接、登录链接或带参数/签名的地址。结果不是目录版本、不会自动安装；安装需要另外核对文件和创建安装计划。".into(),
            parameters: json!({"type":"object","properties":{"url":{"type":"string","minLength":1,"maxLength":MAX_WEB_URL_BYTES}},"required":["url"],"additionalProperties":false}),
            effect: Effect::UncatalogedDownload,
        }
    }

    async fn prepare(&self, context: &ToolContext, args: &Value) -> AppResult<PreparedTool> {
        let request = request(context, args.clone())?;
        Ok(PreparedTool {
            preview: preview(&request),
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
        let request = request(context, args)?;
        if prepared.effect != Effect::UncatalogedDownload
            || prepared.plan.is_some()
            || prepared.preview != preview(&request)
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
        crate::artifacts::record_web(&context.engine, &artifact)?;
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
            json!({"url":"https://example.com/mod-page"}).to_string(),
            format!(r#"{{"url":"{url}","url":"{url}"}}"#),
        ] {
            assert_eq!(history_arguments(&raw), "{}");
        }
    }

    #[tokio::test]
    async fn prepared_url_cannot_be_substituted_and_tool_stays_agent_only() {
        let (_dir, engine, _) = setup();
        let mut context = ToolContext {
            engine,
            cancel: CancellationToken::new(),
            progress: None,
            resolution_scope: ResolutionScope::Agent("fixture".into()),
        };
        let args = json!({"url":"https://example.com/file.zip"});
        let prepared = WebDownloadTool.prepare(&context, &args).await.unwrap();
        assert_eq!(prepared.preview["url"], args["url"]);
        assert_eq!(prepared.preview["filename"], "file.zip");
        assert!(matches!(
            WebDownloadTool
                .execute(
                    &context,
                    json!({"url":"https://example.net/file.zip"}),
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
        assert!(std::fs::read_dir(context.engine.paths.downloads())
            .unwrap()
            .next()
            .is_none());
    }
}
