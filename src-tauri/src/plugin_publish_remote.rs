//! Fixed-origin Mod客栈 transport. No public generic remote call/URL interface.
//! Wire contract observed via the real endpoint's tools/list on 2026-11-18.
//! Tests use local transports; production never uploads without reviewed consent.
use crate::{
    error::{AppError, AppResult},
    plugin_publish::{
        self as publication, PublicPackage, PublicationPhase, PublicationRecord, PublicationState,
        PublicationStep,
    },
    settings::Settings,
    transaction::FileEngine,
};
use rmcp::{
    model::CallToolRequestParams,
    service::RunningService,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    RoleClient, ServiceExt,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::Url;

const ENDPOINT: &str = "https://www.glossmod.com/api/v1/mcp";
const SECRET_REF: &str = "mcp.glossmod_http.api_key";
const MAX_RESPONSE: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);
const OWN_WORK_LIMIT: usize = 20;

fn failed() -> AppError {
    AppError::Other("Mod 客栈请求失败或结果不确定，请核对发布记录后继续；不会自动重发".into())
}

fn remote_error(error: AppError) -> AppError {
    match error {
        AppError::Cancelled => AppError::Cancelled,
        _ => failed(),
    }
}

fn upload_client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(TIMEOUT)
        .build()
        .map_err(|_| failed())
}

async fn put_bytes(
    http: &reqwest::Client,
    key: &str,
    url: Url,
    payload: &[u8],
    cancel: &CancellationToken,
) -> AppResult<()> {
    let mut token =
        reqwest::header::HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| failed())?;
    token.set_sensitive(true);
    let request = http
        .put(url)
        .header(reqwest::header::AUTHORIZATION, token)
        .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
        .header(reqwest::header::CONTENT_LENGTH, payload.len())
        .body(payload.to_vec())
        .send();
    let response = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(AppError::Cancelled),
        response = request => response.map_err(|_| failed())?,
    };
    if !response.status().is_success() {
        return Err(failed());
    }
    // Completion is confirmed only by the subsequent MCP status/hash checks.
    Ok(())
}

struct Transport {
    service: RunningService<RoleClient, ()>,
    http: reqwest::Client,
    key: String,
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.service.cancellation_token().cancel();
    }
}

impl Transport {
    async fn connect(settings: &Settings, cancel: &CancellationToken) -> AppResult<Self> {
        let key = settings
            .read_secret(SECRET_REF)
            .map_err(|_| AppError::Other("无法读取 Mod 客栈凭据".into()))?
            .filter(|key| {
                !key.is_empty() && key.len() <= 8192 && !key.chars().any(char::is_control)
            })
            .ok_or_else(|| AppError::Denied("请先在设置中配置 Mod 客栈 API Key".into()))?;
        let config = StreamableHttpClientTransportConfig::with_uri(ENDPOINT)
            .auth_header(key.clone())
            .max_concurrent_requests(1)
            .max_sse_event_size(MAX_RESPONSE)
            .reinit_on_expired_session(false)
            .control_request_timeout(Duration::from_secs(2));
        let client = reqwest_mcp::Client::builder()
            .redirect(reqwest_mcp::redirect::Policy::none())
            .retry(reqwest_mcp::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| failed())?;
        let io = StreamableHttpClientTransport::with_client(client, config);
        let service = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            result = tokio::time::timeout(TIMEOUT, ().serve(io)) => result.map_err(|_| failed())?.map_err(|_| failed())?,
        };
        let http = upload_client()?;
        Ok(Self { service, http, key })
    }

    async fn call<T: DeserializeOwned>(
        &self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> AppResult<T> {
        if !matches!(
            name,
            "create_upload"
                | "start_upload"
                | "get_upload_status"
                | "complete_upload"
                | "publish_work"
                | "list_works"
        ) {
            return Err(AppError::Denied("发布流程不能调用其它远程工具".into()));
        }
        let arguments = args.as_object().cloned().ok_or_else(failed)?;
        if serde_json::to_vec(&args)?.len() > 256 * 1024 {
            return Err(failed());
        }
        let request = CallToolRequestParams::new(name.to_owned()).with_arguments(arguments);
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            response = tokio::time::timeout(TIMEOUT, self.service.peer().call_tool(request)) => response.map_err(|_| failed())?.map_err(|_| failed())?,
        };
        let raw = serde_json::to_value(response).map_err(|_| failed())?;
        parse_result(raw, &self.key)
    }

    async fn put(&self, path: &str, payload: &[u8], cancel: &CancellationToken) -> AppResult<()> {
        put_bytes(&self.http, &self.key, upload_url(path)?, payload, cancel).await
    }
}

fn upload_url(path: &str) -> AppResult<Url> {
    // The official site's uploads module and API docs use precisely this PUT
    // route. Same origin alone must not authorize writes to unrelated APIs.
    let parts: Vec<_> = path.split('/').collect();
    let positive_id = |value: &str| {
        value
            .parse::<i64>()
            .is_ok_and(|id| id > 0 && id.to_string() == value)
    };
    if parts.len() != 7
        || parts[..4] != ["", "api", "v1", "uploads"]
        || parts[5] != "multipart"
        || !positive_id(parts[4])
        || !positive_id(parts[6])
    {
        return Err(AppError::Denied(
            "拒绝非同源、安全 API 路径的上传地址".into(),
        ));
    }
    let origin = Url::parse(ENDPOINT).map_err(|_| failed())?;
    let url = origin.join(path).map_err(|_| failed())?;
    if url.origin() != origin.origin() || url.query().is_some() || url.fragment().is_some() {
        return Err(AppError::Denied("拒绝跨源上传地址".into()));
    }
    Ok(url)
}

fn parse_result<T: DeserializeOwned>(raw: Value, key: &str) -> AppResult<T> {
    let encoded = serde_json::to_string(&raw).map_err(|_| failed())?;
    fn echoes(value: &Value, key: &str) -> bool {
        match value {
            Value::String(text) => text.contains(key),
            Value::Array(items) => items.iter().any(|item| echoes(item, key)),
            Value::Object(items) => items
                .iter()
                .any(|(name, value)| name.contains(key) || echoes(value, key)),
            _ => false,
        }
    }
    if encoded.len() > MAX_RESPONSE
        || (!key.is_empty() && echoes(&raw, key))
        || raw["isError"] == true
    {
        return Err(failed());
    }
    let value = if let Some(value) = raw.get("structuredContent") {
        value.clone()
    } else {
        let content = raw["content"]
            .as_array()
            .filter(|items| items.len() == 1)
            .ok_or_else(failed)?;
        if content[0]["type"] != "text" {
            return Err(failed());
        }
        serde_json::from_str(content[0]["text"].as_str().ok_or_else(failed)?)
            .map_err(|_| failed())?
    };
    if !key.is_empty() && echoes(&value, key) {
        return Err(failed());
    }
    serde_json::from_value(value).map_err(|_| failed())
}

#[derive(Deserialize)]
struct UploadedFile {
    id: i64,
    original_name: String,
    content_type: String,
    size_bytes: u64,
    sha256: String,
    status: String,
}
#[derive(Deserialize)]
struct Safety {
    blocked: bool,
    manually_reviewed: bool,
    scan_state: String,
    verdict: String,
    // Required by the official tools/list safety schema, not guessed enums.
    vt_state: String,
    vt_verdict: String,
}
#[derive(Deserialize)]
struct UploadStatus {
    file: UploadedFile,
    part_size: u64,
    uploaded_bytes: u64,
    next_part_number: u64,
    next_part_bytes: u64,
    #[serde(default)]
    next_part_path: Option<String>,
    upload_method: String,
    #[serde(default)]
    safety: Option<Safety>,
}
#[derive(Deserialize)]
struct PublishedWork {
    work: PublishedWorkInfo,
}
#[derive(Deserialize)]
struct PublishedWorkInfo {
    id: i64,
    status: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    mod_version: String,
    #[serde(default)]
    files: Vec<PublishedFile>,
}
#[derive(Deserialize)]
struct PublishedFile {
    file_id: i64,
}

#[derive(Deserialize)]
struct OwnWorks {
    items: Option<Vec<OwnWork>>,
}

#[derive(Deserialize)]
struct OwnWork {
    id: i64,
    status: String,
    title: String,
    summary: String,
    mod_version: String,
    files: Option<Vec<OwnAttachment>>,
}

#[derive(Deserialize)]
struct OwnAttachment {
    file_id: i64,
    name: String,
    size_bytes: u64,
    visibility: String,
    version_type: String,
}

impl OwnWorks {
    fn matching_work(self, file_id: i64, package: &PublicPackage) -> Option<i64> {
        let items = self.items?;
        // The provider does not return a total. A full page cannot rule out
        // another matching work, and missing rows never authorize a resend.
        if items.len() >= OWN_WORK_LIMIT {
            return None;
        }
        let mut attached = items.into_iter().filter(|work| {
            work.files
                .as_ref()
                .is_some_and(|files| files.iter().any(|file| file.file_id == file_id))
        });
        let work = attached.next()?;
        let [file] = work.files.as_deref()? else {
            return None;
        };
        (attached.next().is_none()
            && work.id > 0
            && work.status == package.work_status
            && work.title == package.title
            && work.summary == package.summary
            && work.mod_version == package.version
            && file.name == package.archive_name
            && file.size_bytes == package.archive_size
            && file.visibility == "public"
            && file.version_type == "latest")
            .then_some(work.id)
    }
}

#[async_trait::async_trait]
trait Remote: Send + Sync {
    async fn call(&self, name: &str, args: Value, cancel: &CancellationToken) -> AppResult<Value>;
    async fn put(&self, path: &str, bytes: &[u8], cancel: &CancellationToken) -> AppResult<()>;
}
#[async_trait::async_trait]
impl Remote for Transport {
    async fn call(&self, name: &str, args: Value, cancel: &CancellationToken) -> AppResult<Value> {
        Transport::call(self, name, args, cancel).await
    }
    async fn put(&self, path: &str, bytes: &[u8], cancel: &CancellationToken) -> AppResult<()> {
        Transport::put(self, path, bytes, cancel).await
    }
}

async fn call<T: DeserializeOwned>(
    remote: &dyn Remote,
    name: &str,
    args: Value,
    cancel: &CancellationToken,
) -> AppResult<T> {
    serde_json::from_value(
        remote
            .call(name, args, cancel)
            .await
            .map_err(remote_error)?,
    )
    .map_err(|_| failed())
}

fn identity(status: &UploadStatus, package: &PublicPackage, file_id: Option<i64>) -> AppResult<()> {
    if status.file.id <= 0
        || file_id.is_some_and(|id| id != status.file.id)
        || status.file.original_name != package.archive_name
        || status.file.content_type != "application/zip"
        || status.file.size_bytes != package.archive_size
        || status.uploaded_bytes > package.archive_size
        || (!status.file.sha256.is_empty() && status.file.sha256 != package.archive_sha256)
        || !matches!(
            status.file.status.as_str(),
            "pending" | "uploading" | "ready"
        )
    {
        return Err(failed());
    }
    if status.file.status == "ready"
        && (status.file.sha256 != package.archive_sha256
            || status.uploaded_bytes != package.archive_size)
    {
        return Err(failed());
    }
    Ok(())
}

fn distributable(status: &UploadStatus, package: &PublicPackage) -> AppResult<bool> {
    identity(status, package, Some(status.file.id))?;
    if status.file.status != "ready" {
        return Ok(false);
    }
    let Some(safety) = &status.safety else {
        return Ok(false);
    };
    if safety.blocked {
        return Err(AppError::Denied("Mod 客栈安全扫描阻止此适配包发布".into()));
    }
    // tools/list exposes strings, not scan-state enums. This positive predicate
    // follows the official downloadInfo-DafQFobk.js (read-only, 2026-11-18),
    // rather than assuming scan_state is "completed" or "clean". An explicit
    // manual release can retain the original adverse detection verdict.
    let risk = [&safety.verdict, &safety.vt_verdict].iter().any(|verdict| {
        matches!(
            verdict.as_str(),
            "malicious" | "suspicious" | "blocked" | "unsafe"
        )
    });
    Ok(safety.manually_reviewed
        || (!risk
            && safety.scan_state != "skipped"
            && (safety.verdict == "clean"
                || (safety.vt_state == "complete" && safety.vt_verdict == "clean"))))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconciliationOutcome {
    Verified,
    Unconfirmed,
}

/// A desktop-initiated check of already approved remote effects. The saved
/// preview, not the current local draft, describes what may have been sent.
pub async fn reconcile_publication(
    engine: &FileEngine,
    settings: &Settings,
    id: &str,
    cancel: &CancellationToken,
) -> AppResult<ReconciliationOutcome> {
    let record = publication::get_publication(engine, id)?;
    if record.reconciliation_step().is_none() {
        return Ok(ReconciliationOutcome::Unconfirmed);
    }
    let transport = Transport::connect(settings, cancel).await?;
    reconcile_with(engine, &transport, record, cancel).await
}

async fn reconcile_with(
    engine: &FileEngine,
    remote: &dyn Remote,
    record: PublicationRecord,
    cancel: &CancellationToken,
) -> AppResult<ReconciliationOutcome> {
    let (Some(step), Some(file_id)) = (record.reconciliation_step(), record.file_id) else {
        return Ok(ReconciliationOutcome::Unconfirmed);
    };
    let package = &record.preview.package;
    let status: UploadStatus = call(
        remote,
        "get_upload_status",
        json!({"file_id": file_id}),
        cancel,
    )
    .await?;
    identity(&status, package, Some(file_id))?;
    if !distributable(&status, package)? {
        return Ok(ReconciliationOutcome::Unconfirmed);
    }
    let work_id = if step == PublicationStep::PublishWork {
        // `scope=mine` binds this lookup to the same authenticated account as
        // get_upload_status. Public catalog results are not ownership evidence.
        let works: OwnWorks = call(
            remote,
            "list_works",
            json!({"scope": "mine", "status": "published", "keyword": package.title, "limit": OWN_WORK_LIMIT}),
            cancel,
        )
        .await?;
        let Some(work_id) = works.matching_work(file_id, package) else {
            return Ok(ReconciliationOutcome::Unconfirmed);
        };
        Some(work_id)
    } else {
        None
    };
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    publication::finish_reconciliation(engine, &record, work_id)?;
    Ok(ReconciliationOutcome::Verified)
}

/// Approval is performed by desktop confirmation or the Agent approval broker,
/// never by this transporter. Pending/unknown writes are never blindly replayed.
pub async fn publish_approved(
    engine: &FileEngine,
    settings: &Settings,
    id: &str,
    digest: &str,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    let (record, _) = publication::verified_upload_artifact(engine, id, digest)?;
    if record.state == PublicationState::Completed {
        return Ok(record);
    }
    if record.state != PublicationState::Intended {
        return Err(failed());
    }
    let transport = Transport::connect(settings, cancel).await?;
    publish_with(engine, &transport, id, digest, cancel).await
}

async fn publish_with(
    engine: &FileEngine,
    remote: &dyn Remote,
    id: &str,
    digest: &str,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    let (mut record, bytes) = publication::verified_upload_artifact(engine, id, digest)?;
    if record.state == PublicationState::Completed {
        return Ok(record);
    }
    if record.state != PublicationState::Intended {
        return Err(failed());
    }
    let package = record.preview.package.clone();
    for _ in 0..8 {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let step = match record.phase {
            PublicationPhase::Prepared => PublicationStep::CreateUpload,
            PublicationPhase::UploadCreated => PublicationStep::StartUpload,
            PublicationPhase::UploadStarted => PublicationStep::PutPart,
            PublicationPhase::PartUploaded => PublicationStep::CompleteUpload,
            PublicationPhase::UploadReady => PublicationStep::PublishWork,
            PublicationPhase::AwaitingScan => {
                let status: UploadStatus = call(
                    remote,
                    "get_upload_status",
                    json!({"file_id": record.file_id}),
                    cancel,
                )
                .await?;
                identity(&status, &package, record.file_id)?;
                if !distributable(&status, &package)? {
                    return Ok(record);
                }
                record = publication::finish_step(
                    engine,
                    id,
                    PublicationStep::CompleteUpload,
                    record.file_id,
                    None,
                )?;
                continue;
            }
            PublicationPhase::Completed => return Ok(record),
            _ => return Err(failed()),
        };
        // A persisted UploadReady is not a permanent security clearance. The
        // file may have been revoked while the app was stopped.
        if step == PublicationStep::PublishWork {
            let status: UploadStatus = call(
                remote,
                "get_upload_status",
                json!({"file_id": record.file_id}),
                cancel,
            )
            .await?;
            identity(&status, &package, record.file_id)?;
            if !distributable(&status, &package)? {
                return Ok(record);
            }
        }
        // Establish durable intent before any request that may mutate state.
        // Dropping the future leaves Pending; startup converts it to Unknown.
        record = publication::begin_step(engine, id, digest, step)?;
        let action: AppResult<PublicationRecord> = async {
            match step {
                PublicationStep::CreateUpload => {
                    let status: UploadStatus = call(remote, "create_upload", json!({"request_id": id, "original_name": package.archive_name, "content_type": "application/zip", "size_bytes": package.archive_size}), cancel).await?;
                    identity(&status, &package, None)?;
                    if status.uploaded_bytes != 0 { return Err(failed()); }
                    publication::finish_step(engine, id, step, Some(status.file.id), None)
                }
                PublicationStep::StartUpload => {
                    let status: UploadStatus = call(remote, "start_upload", json!({"file_id": record.file_id}), cancel).await?;
                    identity(&status, &package, record.file_id)?;
                    if status.uploaded_bytes != 0 { return Err(failed()); }
                    publication::finish_step(engine, id, step, record.file_id, None)
                }
                PublicationStep::PutPart => {
                    let status: UploadStatus = call(remote, "get_upload_status", json!({"file_id": record.file_id}), cancel).await?;
                    identity(&status, &package, record.file_id)?;
                    if status.uploaded_bytes != 0 || status.next_part_number != 1
                        || status.next_part_bytes != bytes.len() as u64
                        || status.part_size < bytes.len() as u64 || status.upload_method != "PUT" { return Err(failed()); }
                    let path = status.next_part_path.ok_or_else(failed)?;
                    upload_url(&path)?;
                    if path != format!("/api/v1/uploads/{}/multipart/1", status.file.id) { return Err(failed()); }
                    remote.put(&path, bytes.as_slice(), cancel).await.map_err(remote_error)?;
                    let received: UploadStatus = call(remote, "get_upload_status", json!({"file_id": record.file_id}), cancel).await?;
                    identity(&received, &package, record.file_id)?;
                    if received.uploaded_bytes != bytes.len() as u64 { return Err(failed()); }
                    publication::finish_step(engine, id, step, record.file_id, None)
                }
                PublicationStep::CompleteUpload => {
                    let status: UploadStatus = call(remote, "complete_upload", json!({"file_id": record.file_id}), cancel).await?;
                    identity(&status, &package, record.file_id)?;
                    if status.uploaded_bytes != package.archive_size { return Err(failed()); }
                    if distributable(&status, &package)? {
                        publication::finish_step(engine, id, step, record.file_id, None)
                    } else { publication::await_scan(engine, id) }
                }
                PublicationStep::PublishWork => {
                    let result: PublishedWork = call(remote, "publish_work", json!({"title": package.title, "summary": package.summary, "description": package.description, "tags": package.tags, "game_ids": package.game_ids, "mod_version": package.version, "status": "published", "files": [{"file_id": record.file_id, "name": package.archive_name, "visibility": "public", "version_type": "latest"}]}), cancel).await?;
                    if result.work.id <= 0 || result.work.status != "published"
                        || result.work.title != package.title || result.work.mod_version != package.version
                        || result.work.files.len() != 1
                        || Some(result.work.files[0].file_id) != record.file_id { return Err(failed()); }
                    publication::finish_step(engine, id, step, record.file_id, Some(result.work.id))
                }
            }
        }.await;
        match action {
            Ok(next) => record = next,
            Err(error) => {
                publication::mark_unknown(engine, id, step)?;
                return Err(error);
            }
        }
        if matches!(
            record.phase,
            PublicationPhase::AwaitingScan | PublicationPhase::Completed
        ) {
            return Ok(record);
        }
    }
    Err(failed())
}

#[cfg(test)]
#[path = "plugin_publish_remote_tests.rs"]
mod tests;
