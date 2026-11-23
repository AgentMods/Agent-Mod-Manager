//! The desktop runs the built-in organization skill independently of the install chat.
use crate::{
    agent::client::{ModelClient, ModelConfig},
    error::{AppError, AppResult},
    mod_organization::OrganizationSuggestion,
    mod_organization_jobs::{self as jobs, Job},
    settings::Settings,
    skills,
};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const RETRY_CHECK: Duration = Duration::from_secs(30);
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_REPLY_BYTES: usize = 4096;

pub(crate) struct Organizer(CancellationToken);
impl Drop for Organizer {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) fn start(
    settings: Settings,
    wake: Arc<Notify>,
    on_change: impl Fn(&str, &str) + Send + Sync + 'static,
) -> Organizer {
    let cancel = CancellationToken::new();
    let handle = Organizer(cancel.clone());
    tauri::async_runtime::spawn(async move {
        let mut initialized = false;
        while !cancel.is_cancelled() {
            let result = if initialized {
                run_one(&settings, &cancel, &on_change).await
            } else {
                let db = settings.db.clone();
                blocking(move || jobs::initialize(&db)).await.map(|()| {
                    initialized = true;
                    true
                })
            };
            match result {
                Ok(true) => continue,
                Err(AppError::Cancelled) => break,
                Err(error) => tracing::warn!(%error, "内置 Mod 整理任务暂缓"),
                Ok(false) => (),
            }
            // Commits and settings changes wake immediately. The timer also picks
            // up durable retries when no window or chat is open on the game.
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = wake.notified() => (),
                _ = tokio::time::sleep(RETRY_CHECK) => (),
            }
        }
    });
    handle
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| AppError::Other(error.to_string()))?
}

async fn run_one(
    settings: &Settings,
    cancel: &CancellationToken,
    on_change: &impl Fn(&str, &str),
) -> AppResult<bool> {
    let preferences = settings.clone();
    let ready = blocking(move || {
        let Some(skill) = skills::enabled_mod_organizer(&preferences.db)? else {
            return Ok(None);
        };
        let Some(job) = jobs::next(&preferences.db)? else {
            return Ok(None);
        };
        let Some(config) = ModelConfig::configured(&preferences)? else {
            return Ok(None);
        };
        Ok(Some((job, skill, config)))
    })
    .await?;
    let Some((job, skill, config)) = ready else {
        return Ok(false);
    };
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
        reply = tokio::time::timeout(MODEL_TIMEOUT, generate(&config, skill, &job, cancel)) => {
            reply.unwrap_or_else(|_| Err(AppError::Other("Mod 整理模型响应超时".into())))
        }
    };
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let db = settings.db.clone();
    let updated = blocking(move || {
        let result = match response {
            Ok(Some(suggestion)) => jobs::complete(&db, &job, &suggestion),
            Ok(None) => jobs::insufficient(&db, &job).map(|()| false),
            Err(error) => Err(error),
        };
        match result {
            Ok(true) => Ok(Some((job.game_id, job.install_id))),
            Ok(false) => Ok(None),
            Err(error) => {
                // Do not log model responses, install evidence, or provider error bodies.
                let cause = match error {
                    AppError::Db(_) => "database",
                    AppError::Net(_) => "network",
                    AppError::Json(_) | AppError::Invalid(_) => "invalid result",
                    AppError::Denied(_) => "denied",
                    _ => "request failed",
                };
                tracing::warn!(install_id = %job.install_id, cause, "Mod 整理暂未完成，将重试");
                jobs::retry(&db, &job)?;
                Ok(None)
            }
        }
    })
    .await?;
    if let Some((game, install)) = updated {
        on_change(&game, &install);
    }
    Ok(true)
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
enum Reply {
    Organized {
        #[serde(rename = "displayName")]
        display_name: String,
        tags: Vec<String>,
    },
    Insufficient {
        #[serde(rename = "insufficientEvidence")]
        insufficient_evidence: bool,
    },
}

fn decode_reply(text: &str) -> AppResult<Option<OrganizationSuggestion>> {
    let text = text.trim();
    // Some configured providers wrap JSON in a Markdown code block.
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|body| body.strip_suffix("```"))
        .map_or(text, str::trim);
    match serde_json::from_str(text)? {
        Reply::Organized { display_name, tags } => {
            Ok(Some(OrganizationSuggestion { display_name, tags }))
        }
        Reply::Insufficient {
            insufficient_evidence: true,
        } => Ok(None),
        _ => Err(AppError::Invalid("Mod 整理模型未返回有效结果".into())),
    }
}

async fn generate(
    config: &ModelConfig,
    skill: &str,
    job: &Job,
    cancel: &CancellationToken,
) -> AppResult<Option<OrganizationSuggestion>> {
    let input = json!({"locale":job.locale,"evidence":job.evidence}).to_string();
    let contains_key = |value: &str| {
        config
            .api_key
            .as_deref()
            .is_some_and(|key| !key.is_empty() && value.contains(key))
    };
    if contains_key(&input) {
        return Err(AppError::Denied("整理资料含有模型凭据".into()));
    }
    let messages = [
        json!({"role":"system","content":skill}),
        json!({"role":"user","content":input}),
    ];
    let reply = ModelClient::new()?
        .stream(
            config,
            &messages,
            &[],
            cancel,
            |_, full| {
                if full.content.len() > MAX_REPLY_BYTES || full.reasoning.len() > 256 * 1024 {
                    return Err(AppError::Invalid("Mod 整理回复过长".into()));
                }
                Ok(())
            },
            || Ok(()),
        )
        .await?;
    if reply.finish_reason.as_deref() != Some("stop")
        || reply.content.len() > MAX_REPLY_BYTES
        || contains_key(&reply.content)
    {
        return Err(AppError::Invalid("Mod 整理模型未返回完整的分类结果".into()));
    }
    decode_reply(&reply.content)
}

#[cfg(test)]
#[path = "mod_organizer_tests.rs"]
mod tests;
