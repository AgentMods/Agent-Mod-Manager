//! Short-lived, in-memory catalog resolutions. The wire DTO has no download URL.
use super::{
    validate_download_url, FileHashes, ModDependency, ResolveResult, ResolvedMod, SourceStatus,
};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use uuid::Uuid;

const TTL: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING: usize = 256;

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ResolutionScope {
    Desktop,
    Agent(String),
    Mcp(String),
}

#[derive(Debug, Serialize)]
pub struct ResolutionResult {
    pub status: SourceStatus,
    pub download: Option<DownloadPreview>,
    pub homepage_url: Option<String>,
    pub dependencies: Option<Vec<ModDependency>>,
}

#[derive(Debug, Serialize)]
pub struct DownloadPreview {
    pub resolution_id: Uuid,
    pub source_id: String,
    pub project_id: String,
    pub version_id: String,
    pub name: String,
    pub version: String,
    pub filename: String,
    pub size: Option<u64>,
    pub hashes: FileHashes,
    pub homepage_url: Option<String>,
    pub warnings: Vec<String>,
}

impl DownloadPreview {
    fn new(id: Uuid, file: &ResolvedMod) -> Self {
        Self {
            resolution_id: id,
            source_id: file.source_id.clone(),
            project_id: file.project_id.clone(),
            version_id: file.version_id.clone(),
            name: file.name.clone(),
            version: file.version.clone(),
            filename: file.filename.clone(),
            size: file.size,
            hashes: file.hashes.clone(),
            homepage_url: homepage(file.homepage_url.as_deref()),
            warnings: file.warnings.clone(),
        }
    }
}

fn homepage(raw: Option<&str>) -> Option<String> {
    let mut url = validate_download_url(raw?).ok()?;
    url.set_query(None);
    Some(url.to_string())
}

fn unavailable() -> AppError {
    AppError::Denied("下载解析 ID 不可用，请从来源重新解析版本".into())
}

pub(crate) fn resolution_id(raw: &str) -> AppResult<Uuid> {
    Uuid::parse_str(raw)
        .ok()
        .filter(|id| id.hyphenated().to_string() == raw)
        .ok_or_else(|| AppError::Invalid("下载只接受 resolve_mod 返回的 resolution_id".into()))
}

pub(crate) fn request_id(args: &Value) -> AppResult<Uuid> {
    let raw = args
        .as_object()
        .filter(|args| args.len() == 1)
        .and_then(|args| args.get("resolution_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Invalid("下载只接受 resolve_mod 返回的 resolution_id".into()))?;
    resolution_id(raw)
}

// Do not preserve unexpected text (including legacy URL-bearing arguments) in
// tool-call history. An invalid payload still fails ordinary tool validation.
pub(crate) fn history_arguments(raw: &str) -> String {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Args {
        resolution_id: String,
    }

    if let Some(id) = serde_json::from_str::<Args>(raw)
        .ok()
        .and_then(|args| resolution_id(&args.resolution_id).ok())
    {
        return serde_json::json!({"resolution_id":id}).to_string();
    }
    "{}".into()
}

/// Older structured tool records remain on disk but must not send signed URLs
/// back to the model. Do not rewrite user/assistant prose or unrelated tools.
pub(crate) fn scrub_model_history(messages: &mut [Value]) {
    let mut resolves = HashSet::new();
    for message in messages {
        match message["role"].as_str() {
            Some("assistant") => {
                resolves.clear();
                for call in message
                    .get_mut("tool_calls")
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    match call["function"]["name"].as_str() {
                        Some("download_mod") => {
                            let args = call["function"]["arguments"].as_str().unwrap_or_default();
                            call["function"]["arguments"] = Value::String(history_arguments(args));
                        }
                        Some("resolve_mod") => {
                            if let Some(id) = call["id"].as_str() {
                                resolves.insert(id.to_owned());
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("tool")
                if message["tool_call_id"]
                    .as_str()
                    .is_some_and(|id| resolves.remove(id)) =>
            {
                let Some(content) = message["content"].as_str() else {
                    continue;
                };
                let Ok(Value::Object(mut result)) = serde_json::from_str::<Value>(content) else {
                    continue;
                };
                let Some(download) = result.get_mut("download").and_then(Value::as_object_mut)
                else {
                    continue;
                };
                if download.remove("download_url").is_some() {
                    // Legacy homepage fields were not guaranteed to be free of query secrets.
                    download.remove("homepage_url");
                    result.remove("homepage_url");
                    message["content"] = Value::String(Value::Object(result).to_string());
                }
            }
            _ => {}
        }
    }
}

struct Entry {
    scope: ResolutionScope,
    file: ResolvedMod,
    expires: Instant,
}

#[derive(Clone, Default)]
pub(crate) struct Resolutions(Arc<Mutex<HashMap<Uuid, Entry>>>);

impl Resolutions {
    pub(crate) fn publish(
        &self,
        scope: ResolutionScope,
        result: ResolveResult,
    ) -> AppResult<ResolutionResult> {
        let download = result
            .download
            .map(|file| self.insert(scope, file))
            .transpose()?;
        Ok(ResolutionResult {
            status: result.status,
            download,
            homepage_url: homepage(result.homepage_url.as_deref()),
            dependencies: result.dependencies,
        })
    }

    fn insert(&self, scope: ResolutionScope, file: ResolvedMod) -> AppResult<DownloadPreview> {
        crate::download::safe_filename(&file.filename)?;
        if file.download_url.len() > 8192
            || file
                .homepage_url
                .as_ref()
                .is_some_and(|url| url.len() > 2048)
            || [
                &file.source_id,
                &file.project_id,
                &file.version_id,
                &file.name,
                &file.version,
            ]
            .into_iter()
            .any(|field| field.len() > 512)
        {
            return Err(AppError::Invalid("下载解析信息超过大小限制".into()));
        }
        let id = Uuid::new_v4();
        let preview = DownloadPreview::new(id, &file);
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        entries.retain(|_, entry| entry.expires > now);
        let oldest = (entries.len() >= MAX_PENDING)
            .then(|| {
                entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires)
                    .map(|(id, _)| *id)
            })
            .flatten();
        if let Some(oldest) = oldest {
            entries.remove(&oldest);
        }
        entries.insert(
            id,
            Entry {
                scope,
                file,
                expires: now + TTL,
            },
        );
        Ok(preview)
    }

    pub(crate) fn preview(&self, scope: &ResolutionScope, id: Uuid) -> AppResult<DownloadPreview> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let entry = Self::check(&mut entries, scope, id)?;
        Ok(DownloadPreview::new(id, &entry.file))
    }

    pub(crate) fn take(&self, scope: &ResolutionScope, id: Uuid) -> AppResult<ResolvedMod> {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::check(&mut entries, scope, id)?;
        entries
            .remove(&id)
            .map(|entry| entry.file)
            .ok_or_else(unavailable)
    }

    fn check<'a>(
        entries: &'a mut HashMap<Uuid, Entry>,
        scope: &ResolutionScope,
        id: Uuid,
    ) -> AppResult<&'a Entry> {
        let now = Instant::now();
        entries.retain(|_, entry| entry.expires > now);
        entries
            .get(&id)
            .filter(|entry| &entry.scope == scope)
            .ok_or_else(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SourceState;
    use serde_json::json;

    const SIGNED: &str = "https://cdn.example.com/file.jar?token=private-signed-value";

    fn fixture() -> ResolveResult {
        let homepage_url =
            Some("https://provider.example.com/file?token=private-homepage-value".into());
        ResolveResult {
            status: SourceStatus::new("modrinth", SourceState::Available, "OK"),
            homepage_url: homepage_url.clone(),
            dependencies: Some(Vec::new()),
            download: Some(ResolvedMod {
                source_id: "modrinth".into(),
                project_id: "Project1".into(),
                version_id: "Version1".into(),
                name: "Fixture".into(),
                version: "v1.0".into(),
                download_url: SIGNED.into(),
                filename: "file.jar".into(),
                size: Some(42),
                hashes: FileHashes {
                    sha512: Some("a".repeat(128)),
                    ..Default::default()
                },
                homepage_url,
                warnings: vec!["review".into()],
            }),
        }
    }

    #[test]
    fn resolution_public_metadata_is_redacted_and_single_use() {
        let store = Resolutions::default();
        let scope = ResolutionScope::Desktop;
        let result = store.publish(scope.clone(), fixture()).unwrap();
        let wire = serde_json::to_value(&result).unwrap();
        let text = wire.to_string();
        for private in [
            "download_url",
            "cdn.example.com",
            "private-signed-value",
            "private-homepage-value",
        ] {
            assert!(!text.contains(private));
        }
        assert_eq!(wire["homepage_url"], "https://provider.example.com/file");
        assert_eq!(wire["dependencies"], json!([]));
        let preview = result.download.unwrap();
        let id = preview.resolution_id;
        assert_eq!(
            wire["download"],
            serde_json::to_value(store.preview(&scope, id).unwrap()).unwrap()
        );
        let taken = store.take(&scope, id).unwrap();
        assert_eq!(taken.download_url, SIGNED);
        assert_eq!(taken.hashes, preview.hashes);
        assert_eq!(taken.project_id, preview.project_id);
        assert_eq!(taken.version_id, preview.version_id);
        assert_eq!(taken.size, preview.size);
        assert!(store.take(&scope, id).is_err());
        assert!(store.preview(&scope, id).is_err());
    }

    #[test]
    fn wrong_scope_expiration_and_restart_do_not_authorize_downloads() {
        let store = Resolutions::default();
        let scope = ResolutionScope::Agent("same-id".into());
        let id = store
            .publish(scope.clone(), fixture())
            .unwrap()
            .download
            .unwrap()
            .resolution_id;
        for other in [
            ResolutionScope::Desktop,
            ResolutionScope::Agent("other".into()),
            ResolutionScope::Mcp("same-id".into()),
        ] {
            assert!(store.preview(&other, id).is_err());
            assert!(store.take(&other, id).is_err());
        }
        assert!(Resolutions::default().take(&scope, id).is_err());
        assert!(store.preview(&scope, id).is_ok());
        store.0.lock().unwrap().get_mut(&id).unwrap().expires = Instant::now();
        assert!(store.take(&scope, id).is_err());
        assert!(store.0.lock().unwrap().is_empty());
    }

    #[test]
    fn cache_is_bounded_and_manual_results_do_not_issue_ids() {
        let store = Resolutions::default();
        let scope = ResolutionScope::Desktop;
        let mut manual = fixture();
        manual.download = None;
        assert!(store
            .publish(scope.clone(), manual)
            .unwrap()
            .download
            .is_none());
        assert!(store.0.lock().unwrap().is_empty());
        let first = store
            .publish(scope.clone(), fixture())
            .unwrap()
            .download
            .unwrap()
            .resolution_id;
        store.0.lock().unwrap().get_mut(&first).unwrap().expires =
            Instant::now() + Duration::from_secs(1);
        for _ in 0..MAX_PENDING {
            store.publish(scope.clone(), fixture()).unwrap();
        }
        assert_eq!(store.0.lock().unwrap().len(), MAX_PENDING);
        assert!(store.preview(&scope, first).is_err());
    }

    #[test]
    fn download_arguments_cannot_inject_or_preserve_raw_urls() {
        let id = Uuid::new_v4();
        let valid = json!({"resolution_id":id});
        assert_eq!(history_arguments(&valid.to_string()), valid.to_string());
        assert_eq!(request_id(&valid).unwrap(), id);
        for invalid in [
            json!({"resolved":{"download_url":SIGNED}}),
            json!({"resolution_id":id,"filename":"other.jar"}),
            json!({"resolution_id":SIGNED}),
            json!({"resolutionId":id}),
        ] {
            assert!(request_id(&invalid).is_err());
            assert_eq!(history_arguments(&invalid.to_string()), "{}");
        }
        assert_eq!(
            history_arguments(&format!(
                r#"{{"resolution_id":"{id}","resolution_id":"{id}"}}"#
            )),
            "{}"
        );
    }

    #[test]
    fn legacy_resolution_urls_are_omitted_when_replaying_context_to_model() {
        let mut messages = vec![
            json!({"role":"assistant","tool_calls":[
                {"id":"resolve","function":{"name":"resolve_mod","arguments":"{}"}},
                {"id":"download","function":{"name":"download_mod","arguments":json!({"resolved":{"download_url":SIGNED}}).to_string()}}
            ]}),
            json!({"role":"tool","tool_call_id":"resolve","content":json!({
                "homepage_url":SIGNED,"download":{"download_url":SIGNED,"homepage_url":SIGNED,"version_id":"Version1"}
            }).to_string()}),
            json!({"role":"tool","tool_call_id":"download","content":"{}"}),
            json!({"role":"user","content":"Unchanged user text"}),
        ];
        scrub_model_history(&mut messages);
        let text = serde_json::to_string(&messages).unwrap();
        assert!(!text.contains(SIGNED));
        assert!(text.contains("Version1"));
        assert_eq!(messages[0]["tool_calls"][1]["function"]["arguments"], "{}");
        assert_eq!(messages[3]["content"], "Unchanged user text");
    }
}
