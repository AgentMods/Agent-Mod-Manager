//! Downloads are inert artifacts. This module never installs, extracts, executes,
//! removes files, or accepts an external destination path.
use crate::{
    error::{AppError, AppResult},
    sources::{
        pinned_download_client, validate_download_url,
        web_download::{self, ArchiveKind, WebDownload},
        FileHashes, ResolvedMod,
    },
    storage::AppPaths,
};
use futures_util::StreamExt;
use md5::Md5;
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub max_attempts: u32,
    pub retry_delay: Duration,
    /// Maximum time without a successful HTTP read, not a transfer deadline.
    pub read_idle_timeout: Duration,
    pub max_bytes: u64,
}
impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            retry_delay: Duration::from_millis(500),
            read_idle_timeout: Duration::from_secs(30 * 60),
            max_bytes: u64::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPhase {
    Connecting,
    Downloading,
    Retrying,
    Verifying,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub id: String,
    pub phase: DownloadPhase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub attempt: u32,
}
pub type ProgressCallback = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadArtifact {
    /// Identity of this completed file, separate from the resume/progress key.
    pub id: String,
    pub path: PathBuf,
    pub filename: String,
    pub size: u64,
    pub sha256: String,
    pub sha512: String,
    /// True only when all supplied hashes match and a strong source hash exists.
    pub integrity_verified: bool,
    pub requires_review: bool,
    pub auto_install_allowed: bool,
    pub warnings: Vec<String>,
}

impl DownloadArtifact {
    pub(crate) fn into_user_paths(mut self) -> Self {
        self.path = crate::paths::user_path(&self.path).into_owned();
        self
    }
}

#[derive(Clone)]
pub struct DownloadManager {
    root: PathBuf,
    options: DownloadOptions,
    // Clones share this lock; independent instances within this process also
    // take GLOBAL_DOWNLOAD_LOCK. The desktop app enforces a single instance.
    lock: Arc<Mutex<()>>,
}

struct Transfer<'a> {
    url: &'a str,
    filename: &'a str,
    size: Option<u64>,
    hashes: &'a FileHashes,
    warnings: &'a [String],
    kind: TransferKind,
    max_bytes: u64,
}

#[derive(Clone, Copy)]
enum TransferKind {
    Catalog,
    Web(ArchiveKind),
    Mcp(ArchiveKind),
}

impl TransferKind {
    fn archive(self) -> Option<ArchiveKind> {
        match self {
            Self::Catalog => None,
            Self::Web(archive) | Self::Mcp(archive) => Some(archive),
        }
    }

    fn validate_endpoint(self, url: &str) -> AppResult<url::Url> {
        match self {
            Self::Catalog => validate_download_url(url),
            Self::Web(_) | Self::Mcp(_) => web_download::validated_endpoint(url),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialMeta {
    id: String,
    fingerprint: String,
    etag: String,
    total: Option<u64>,
}
struct Partial {
    meta: PartialMeta,
    file: tokio::fs::File,
    path: PathBuf,
    offset: u64,
}

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}
fn denied(message: &str) -> AppError {
    AppError::Denied(message.into())
}

pub(crate) fn safe_filename(name: &str) -> AppResult<()> {
    if name.len() > 180 || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(invalid("下载文件名必须是单个普通文件名"));
    }
    crate::paths::safe_join(Path::new("downloads"), name)?;
    Ok(())
}

fn check_plain_dir(path: &Path) -> AppResult<()> {
    let m = std::fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() || !m.is_dir() {
        return Err(denied("下载目录不得是符号链接"));
    }
    Ok(())
}

fn check_regular(path: &Path) -> AppResult<std::fs::Metadata> {
    let m = std::fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() || !m.is_file() {
        return Err(denied("下载缓存不得是链接或特殊文件"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.nlink() != 1 {
            return Err(denied("下载缓存不得是硬链接"));
        }
    }
    Ok(m)
}

fn strong_etag(value: Option<&header::HeaderValue>) -> Option<String> {
    value
        .and_then(|v| v.to_str().ok())
        .filter(|v| {
            v.len() >= 2
                && v.starts_with('"')
                && v.ends_with('"')
                && v.as_bytes()[1..v.len() - 1]
                    .iter()
                    .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
        })
        .map(str::to_owned)
}

fn content_range(value: Option<&header::HeaderValue>) -> AppResult<(u64, u64, u64)> {
    let text = value
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| invalid("206 缺少 Content-Range"))?;
    let (range, total) = text
        .strip_prefix("bytes ")
        .and_then(|t| t.split_once('/'))
        .ok_or_else(|| invalid("Content-Range 格式无效"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| invalid("Content-Range 格式无效"))?;
    let parse = |s: &str| {
        s.parse::<u64>()
            .map_err(|_| invalid("Content-Range 数值无效"))
    };
    let (start, end, total) = (parse(start)?, parse(end)?, parse(total)?);
    if start > end || end >= total {
        return Err(invalid("Content-Range 范围无效"));
    }
    Ok((start, end, total))
}

impl DownloadManager {
    pub fn new(paths: &AppPaths) -> AppResult<Self> {
        Self::with_options(paths, DownloadOptions::default())
    }

    pub fn with_options(paths: &AppPaths, options: DownloadOptions) -> AppResult<Self> {
        if options.max_attempts == 0 || options.max_attempts > 10 || options.max_bytes == 0 {
            return Err(invalid("下载重试次数或大小限制无效"));
        }
        let root = paths.downloads();
        check_plain_dir(&root)?;
        let root = root.canonicalize()?;
        if root.parent() != Some(paths.root.as_path()) {
            return Err(denied("下载目录超出 AppPaths"));
        }
        Ok(Self {
            root,
            options,
            lock: Arc::new(Mutex::new(())),
        })
    }

    fn guard_dir(&self, directory: &Path) -> AppResult<()> {
        check_plain_dir(&self.root)?;
        if self.root.canonicalize()? != self.root {
            return Err(denied("下载根目录已变化"));
        }
        check_plain_dir(directory)?;
        if directory.canonicalize()?.parent() != Some(self.root.as_path()) {
            return Err(denied("下载缓存越界"));
        }
        Ok(())
    }

    /// Agent-friendly entry point. The callback receives only local progress,
    /// never credentials, URLs or arbitrary remote response bodies.
    pub async fn download_mod(
        &self,
        request: &ResolvedMod,
        cancel: CancellationToken,
        progress: ProgressCallback,
    ) -> AppResult<DownloadArtifact> {
        self.download_with_progress(request, cancel, Some(progress))
            .await
    }

    /// Optional-callback variant for background/headless consumers.
    pub async fn download_with_progress(
        &self,
        request: &ResolvedMod,
        cancel: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> AppResult<DownloadArtifact> {
        self.transfer(
            &Transfer {
                url: &request.download_url,
                filename: &request.filename,
                size: request.size,
                hashes: &request.hashes,
                warnings: &request.warnings,
                kind: TransferKind::Catalog,
                max_bytes: self.options.max_bytes,
            },
            cancel,
            progress,
        )
        .await
    }

    pub(crate) async fn download_web(
        &self,
        request: &WebDownload,
        cancel: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> AppResult<DownloadArtifact> {
        self.transfer(
            &Transfer {
                url: request.url(),
                filename: request.filename(),
                size: None,
                hashes: &FileHashes::default(),
                warnings: &["网页直链下载，未绑定任何目录来源、项目或版本；仅检查归档头，内容和来源仍需审查".into()],
                kind: TransferKind::Web(request.archive()),
                max_bytes: self.options.max_bytes,
            },
            cancel,
            progress,
        )
        .await
    }

    /// The caller must authorize and validate the MCP receipt before handing
    /// the transport its short-lived URL. This is not an Agent or IPC input.
    pub(crate) async fn download_mcp_archive(
        &self,
        url: &str,
        filename: &str,
        expected_size: Option<u64>,
        cancel: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> AppResult<DownloadArtifact> {
        let (url, archive) = web_download::parse_mcp_archive(url, filename)?;
        self.transfer(
            &Transfer {
                url: url.as_str(),
                filename,
                size: expected_size,
                hashes: &FileHashes::default(),
                warnings: &["MCP 回执下载，仅检查归档头和回执提供的大小；归档内容仍需审查".into()],
                kind: TransferKind::Mcp(archive),
                max_bytes: self.options.max_bytes,
            },
            cancel,
            progress,
        )
        .await
    }

    async fn transfer(
        &self,
        request: &Transfer<'_>,
        cancel: CancellationToken,
        progress: Option<ProgressCallback>,
    ) -> AppResult<DownloadArtifact> {
        request.kind.validate_endpoint(request.url)?;
        safe_filename(request.filename)?;
        request.hashes.validate()?;
        if request.size.is_some_and(|size| size > request.max_bytes) {
            return Err(invalid("文件超过下载大小限制"));
        }
        let fingerprint = if matches!(request.kind, TransferKind::Mcp(_)) {
            // Do not derive durable paths, progress IDs, or sidecar keys
            // from a short-lived credential.
            uuid::Uuid::new_v4().simple().to_string()
        } else {
            let representation =
                serde_json::to_vec(&(request.url, request.filename, request.size, request.hashes))?;
            let mut key = Sha256::new();
            if matches!(request.kind, TransferKind::Web(_)) {
                key.update(b"web-download\0");
            }
            key.update(representation);
            hex::encode(key.finalize())
        };
        let _lock = tokio::select! { biased; _ = cancel.cancelled() => return Err(AppError::Cancelled), guard = self.lock.lock() => guard };
        let directory = self.root.join(&fingerprint);
        match crate::safe_fs::create_dir(&directory) {
            Ok(()) => {}
            Err(AppError::Io(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        self.guard_dir(&directory)?;
        // A process-wide lock covers all manager instances, without deleting
        // lockfiles or leaving crash-stale leases. Files still use create_new.
        let _global = tokio::select! { biased; _ = cancel.cancelled() => return Err(AppError::Cancelled), guard = GLOBAL_DOWNLOAD_LOCK.lock() => guard };
        let emit = |phase, downloaded, total, attempt| {
            if let Some(callback) = &progress {
                callback(DownloadProgress {
                    id: fingerprint.clone(),
                    phase,
                    downloaded,
                    total,
                    attempt,
                });
            }
        };
        let mut last_error = None;
        for attempt in 1..=self.options.max_attempts {
            emit(DownloadPhase::Connecting, 0, request.size, attempt);
            let result = self
                .attempt(
                    request,
                    &directory,
                    &fingerprint,
                    &cancel,
                    &|phase, n, total| emit(phase, n, total, attempt),
                )
                .await;
            match result {
                Ok(artifact) => {
                    emit(
                        DownloadPhase::Completed,
                        artifact.size,
                        Some(artifact.size),
                        attempt,
                    );
                    return Ok(artifact);
                }
                Err(AppError::Cancelled) => {
                    emit(DownloadPhase::Cancelled, 0, request.size, attempt);
                    return Err(AppError::Cancelled);
                }
                Err(error) => {
                    let retry = matches!(&error, AppError::Other(_));
                    last_error = Some(error);
                    if !retry || attempt == self.options.max_attempts {
                        break;
                    }
                    emit(DownloadPhase::Retrying, 0, request.size, attempt);
                    tokio::select! { biased; _ = cancel.cancelled() => {
                        emit(DownloadPhase::Cancelled, 0, request.size, attempt);
                        return Err(AppError::Cancelled);
                    }, _ = tokio::time::sleep(self.options.retry_delay.saturating_mul(1 << (attempt - 1))) => {} }
                }
            }
        }
        emit(
            DownloadPhase::Failed,
            0,
            request.size,
            self.options.max_attempts,
        );
        Err(last_error.unwrap_or_else(|| invalid("下载未开始")))
    }

    async fn resume(
        &self,
        directory: &Path,
        fingerprint: &str,
        max_bytes: u64,
        cancel: &CancellationToken,
    ) -> AppResult<Option<Partial>> {
        self.guard_dir(directory)?;
        let mut best: Option<Partial> = None;
        let mut entries = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            entries = tokio::fs::read_dir(directory) => entries?,
        };
        while let Some(entry) = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            entry = entries.next_entry() => entry?,
        } {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .strip_suffix(".json")
                .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            {
                continue;
            }
            let Some(id) = name.strip_suffix(".json") else {
                continue;
            };
            let path = directory.join(format!("{id}.part"));
            if directory.join(format!("{id}.rejected")).exists() || !path.try_exists()? {
                continue;
            }
            let meta_path = entry.path();
            if check_regular(&meta_path)?.len() > 4096 {
                continue;
            }
            let file = crate::safe_fs::open_regular(&meta_path)?;
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(file, 4097), &mut data)?;
            if data.len() > 4096 {
                continue;
            }
            let Ok(meta) = serde_json::from_slice::<PartialMeta>(&data) else {
                continue;
            };
            if meta.fingerprint != fingerprint
                || name.strip_suffix(".json") != Some(meta.id.as_str())
                || strong_etag(header::HeaderValue::from_str(&meta.etag).ok().as_ref()).is_none()
            {
                continue;
            }
            let length = check_regular(&path)?.len();
            if length == 0 || meta.total.is_some_and(|t| length >= t) || length > max_bytes {
                continue;
            }
            if best.as_ref().is_some_and(|b| b.offset >= length) {
                continue;
            }
            let mut file = tokio::fs::File::from_std(crate::safe_fs::open_transfer(&path, false)?);
            if file.seek(std::io::SeekFrom::End(0)).await? != length {
                return Err(denied("下载文件长度在续传扫描期间已变化"));
            }
            best = Some(Partial {
                meta,
                file,
                path,
                offset: length,
            });
        }
        Ok(best)
    }

    async fn new_partial(
        &self,
        directory: &Path,
        fingerprint: &str,
        etag: Option<String>,
        total: Option<u64>,
    ) -> AppResult<Partial> {
        self.guard_dir(directory)?;
        let meta = PartialMeta {
            id: uuid::Uuid::new_v4().to_string(),
            fingerprint: fingerprint.into(),
            etag: etag.unwrap_or_default(),
            total,
        };
        let path = directory.join(format!("{}.part", meta.id));
        let file = tokio::fs::File::from_std(crate::safe_fs::open_transfer(&path, true)?);
        let mut sidecar = tokio::fs::File::from_std(crate::safe_fs::create_new(
            &directory.join(format!("{}.json", meta.id)),
        )?);
        sidecar.write_all(&serde_json::to_vec(&meta)?).await?;
        sidecar.sync_all().await?;
        Ok(Partial {
            meta,
            file,
            path,
            offset: 0,
        })
    }

    async fn response(
        &self,
        request_spec: &Transfer<'_>,
        partial: Option<&Partial>,
        cancel: &CancellationToken,
    ) -> AppResult<reqwest::Response> {
        let mut url = request_spec.kind.validate_endpoint(request_spec.url)?;
        for _ in 0..=5 {
            let client = tokio::select! { biased; _ = cancel.cancelled() => return Err(AppError::Cancelled),
            c = pinned_download_client(&url, self.options.read_idle_timeout) => c? };
            let mut request = client
                .get(url.clone())
                .header(header::ACCEPT_ENCODING, "identity");
            if let Some(partial) = partial {
                request = request
                    .header(header::RANGE, format!("bytes={}-", partial.offset))
                    .header(header::IF_RANGE, &partial.meta.etag);
            }
            let response = tokio::select! { biased; _ = cancel.cancelled() => return Err(AppError::Cancelled),
            r = request.send() => r.map_err(|_| AppError::Other("下载网络请求失败".into()))? };
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| invalid("下载重定向没有有效 Location"))?;
                let next = url
                    .join(location)
                    .map_err(|_| invalid("下载重定向 URL 无效"))?;
                url = request_spec.kind.validate_endpoint(next.as_str())?;
                continue;
            }
            return Ok(response);
        }
        Err(invalid("下载重定向次数过多"))
    }

    async fn attempt(
        &self,
        request: &Transfer<'_>,
        directory: &Path,
        fingerprint: &str,
        cancel: &CancellationToken,
        emit: &(impl Fn(DownloadPhase, u64, Option<u64>) + Send + Sync),
    ) -> AppResult<DownloadArtifact> {
        // A signed MCP URL can be echoed in an arbitrary response header.
        // Keep its ETag out of sidecars; catalog and public-link downloads retain resume support.
        let resumable = !matches!(request.kind, TransferKind::Mcp(_));
        let mut partial = if resumable {
            self.resume(directory, fingerprint, request.max_bytes, cancel)
                .await?
        } else {
            None
        };
        if let Some(size) = request.size {
            let offset = partial.as_ref().map_or(0, |p| p.offset);
            crate::safe_fs::DiskBudget::new(directory)?.require(size.saturating_sub(offset))?;
        }
        let mut response = self.response(request, partial.as_ref(), cancel).await?;
        if let Some(p) = &partial {
            let valid = response.status() == StatusCode::PARTIAL_CONTENT
                && strong_etag(response.headers().get(header::ETAG)).as_deref()
                    == Some(p.meta.etag.as_str())
                && content_range(response.headers().get(header::CONTENT_RANGE)).is_ok_and(
                    |(start, end, total)| {
                        start == p.offset
                            && end == total - 1
                            && p.meta.total.is_none_or(|t| t == total)
                            && request.size.is_none_or(|t| t == total)
                            && response.content_length().is_none_or(|n| n == total - start)
                    },
                );
            if !valid {
                // Keep the old bytes for inspection; do not ever append a 200,
                // changed ETag or wrong range to the old representation.
                self.reject(directory, &p.meta.id, "续传验证失败，片段已保留")
                    .await?;
                partial = None;
                response = self.response(request, None, cancel).await?;
            }
        }
        if partial.is_none() && response.status() != StatusCode::OK {
            let code = response.status().as_u16();
            return Err(if code == 408 || code == 429 || code >= 500 {
                AppError::Other(format!("下载返回 HTTP {code}"))
            } else {
                invalid(&format!("下载返回 HTTP {code}"))
            });
        }
        if response
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_some_and(|v| v != "identity")
        {
            return Err(invalid(
                "下载响应不得使用 Content-Encoding，无法安全验证 Range/大小",
            ));
        }
        let total = if partial.is_some() {
            Some(content_range(response.headers().get(header::CONTENT_RANGE))?.2)
        } else {
            response.content_length()
        };
        if total.is_some_and(|n| n > request.max_bytes) {
            return Err(invalid("响应超过下载大小限制"));
        }
        if let (Some(expected), Some(actual)) = (request.size, total) {
            if expected != actual {
                return Err(invalid("响应大小与来源文件大小不符"));
            }
        }
        let mut disk = crate::safe_fs::DiskBudget::new(directory)?;
        let expected = total.or(request.size);
        if let Some(total) = expected {
            disk.require(total - partial.as_ref().map_or(0, |p| p.offset))?;
        }
        // Transfer-Encoding may omit a trustworthy size. Such a response can
        // never claim more than half the initial usable space, and writes are
        // also rechecked against the live space budget below.
        let unknown_limit = disk.limit() / 2;
        let etag = if resumable {
            strong_etag(response.headers().get(header::ETAG))
        } else {
            None
        };
        let mut partial = match partial {
            Some(p) => p,
            None => {
                self.new_partial(directory, fingerprint, etag, expected)
                    .await?
            }
        };
        let mut downloaded = partial.offset;
        let initial_offset = downloaded;
        let mut stream = response.bytes_stream();
        emit(
            DownloadPhase::Downloading,
            downloaded,
            total.or(request.size),
        );
        loop {
            let chunk = tokio::select! { biased;
                _ = cancel.cancelled() => {
                    if partial.meta.etag.is_empty() {
                        self.discard_partial(directory, partial).await;
                    } else if let Err(error) = partial.file.sync_data().await {
                        tracing::warn!(%error, "暂停下载后未能同步下载片段");
                    }
                    return Err(AppError::Cancelled);
                },
                chunk = stream.next() => chunk,
            };
            let Some(chunk) = chunk else {
                break;
            };
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => {
                    if partial.meta.etag.is_empty() {
                        self.discard_partial(directory, partial).await;
                        return Err(AppError::Other(
                            "下载流中断；无安全续传标识，已尝试清理未完成片段".into(),
                        ));
                    }
                    partial.file.sync_data().await?;
                    return Err(AppError::Other("下载流中断，可安全续传的片段已保留".into()));
                }
            };
            downloaded = downloaded
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| invalid("下载长度溢出"))?;
            if downloaded > request.max_bytes
                || total.or(request.size).is_some_and(|n| downloaded > n)
            {
                self.reject(directory, &partial.meta.id, "下载超过预期大小")
                    .await?;
                return Err(invalid("下载超过预期大小，文件已隔离"));
            }
            self.guard_dir(directory)?;
            same_file(&partial.path, &partial.file.metadata().await?)?;
            let disk_check = if expected.is_none() && downloaded - initial_offset > unknown_limit {
                Err(invalid("无声明大小的响应超过可用空间预算"))
            } else {
                disk.charge(chunk.len() as u64)
            };
            if let Err(error) = disk_check {
                self.discard_partial(directory, partial).await;
                return Err(error);
            }
            if let Err(error) = write_chunk(&mut partial.file, &chunk).await {
                if partial.meta.etag.is_empty() {
                    self.discard_partial(directory, partial).await;
                }
                return Err(error.into());
            }
            emit(
                DownloadPhase::Downloading,
                downloaded,
                total.or(request.size),
            );
        }
        partial.file.sync_all().await?;
        if total.or(request.size).is_some_and(|n| downloaded != n) {
            return Err(AppError::Other("下载未达到预期大小，片段已保留".into()));
        }
        emit(DownloadPhase::Verifying, downloaded, Some(downloaded));
        self.guard_dir(directory)?;
        same_file(&partial.path, &partial.file.metadata().await?)?;
        let hashes =
            match hash_open_file(&mut partial.file, downloaded, request.hashes, cancel).await {
                Ok(hashes) => hashes,
                Err(AppError::Cancelled) => {
                    self.discard_partial(directory, partial).await;
                    return Err(AppError::Cancelled);
                }
                Err(error) => return Err(error),
            };
        if request
            .kind
            .archive()
            .is_some_and(|archive| !archive.matches(&hashes.prefix[..hashes.prefix_len]))
        {
            self.reject(
                directory,
                &partial.meta.id,
                "网页下载的文件头与归档类型不匹配",
            )
            .await?;
            return Err(invalid("网页下载的文件头与归档类型不匹配；片段已隔离"));
        }
        let matches = [
            (&request.hashes.sha256, Some(hashes.sha256.as_str())),
            (&request.hashes.sha512, Some(hashes.sha512.as_str())),
            (&request.hashes.sha1, hashes.sha1.as_deref()),
            (&request.hashes.md5, hashes.md5.as_deref()),
        ]
        .into_iter()
        .all(|(expected, actual)| {
            expected
                .as_ref()
                .is_none_or(|h| actual.is_some_and(|actual| h.eq_ignore_ascii_case(actual)))
        });
        if !matches {
            self.reject(directory, &partial.meta.id, "来源 hash 校验失败")
                .await?;
            return Err(invalid(
                "下载 hash 校验失败；可疑文件已隔离，不允许自动安装",
            ));
        }
        self.guard_dir(directory)?;
        same_file(&partial.path, &partial.file.metadata().await?)?;
        let filename = format!("{}-{}", partial.meta.id, request.filename);
        let destination = directory.join(&filename);
        if cancel.is_cancelled() {
            self.discard_partial(directory, partial).await;
            return Err(AppError::Cancelled);
        }
        let checked_destination = destination.clone();
        let source = partial.path;
        let file = partial.file.into_std().await;
        tokio::task::spawn_blocking(move || {
            let identity = crate::safe_fs::identity(&source)?
                .ok_or_else(|| denied("下载片段已被移动或替换"))?;
            crate::safe_fs::verify_regular_at(&file, &source)?;
            if file.metadata()?.len() != downloaded {
                return Err(denied("验证后的下载文件长度已变化"));
            }
            crate::safe_fs::rename_checked(&source, &checked_destination, &identity)
        })
        .await
        .map_err(|_| AppError::Other("下载文件提交任务未完成".into()))??;
        let verified = request.hashes.sha256.is_some() || request.hashes.sha512.is_some();
        let mut warnings = request.warnings.to_vec();
        for (hash, algorithm) in [
            (&request.hashes.sha1, "SHA-1"),
            (&request.hashes.md5, "MD5"),
        ] {
            if hash.is_some() {
                warnings.push(format!(
                    "来源 {algorithm} 已匹配，但仅为旧 API 的弱完整性兼容校验，不证明来源真实性或文件安全"
                ));
            }
        }
        if !verified {
            warnings.push("没有来源 SHA-256/SHA-512；本地计算的 hash 不证明来源真实性".into());
        }
        if suspicious_name(request.filename) {
            warnings.push("此文件类型可能包含可执行代码，需要人工审查".into());
        }
        warnings.push("下载完成后必须审查归档路径、内容和安装计划".into());
        Ok(DownloadArtifact {
            id: partial.meta.id,
            path: destination,
            filename: request.filename.into(),
            size: downloaded,
            sha256: hashes.sha256,
            sha512: hashes.sha512,
            integrity_verified: verified,
            requires_review: true,
            auto_install_allowed: false,
            warnings,
        })
    }

    async fn discard_partial(&self, directory: &Path, partial: Partial) {
        let Ok(()) = self.guard_dir(directory) else {
            return;
        };
        let file = partial.file.into_std().await;
        let result = tokio::task::spawn_blocking(move || -> AppResult<()> {
            let Some(identity) = crate::safe_fs::identity(&partial.path)? else {
                return Ok(());
            };
            crate::safe_fs::verify_regular_at(&file, &partial.path)?;
            drop(file);
            let tombstone = partial
                .path
                .with_extension(format!("discard-{}", uuid::Uuid::new_v4()));
            crate::safe_fs::rename_checked(&partial.path, &tombstone, &identity)?;
            if crate::safe_fs::identity(&tombstone)?.as_ref() == Some(&identity) {
                std::fs::remove_file(tombstone)?;
            }
            // A sidecar without its data file is inert and is skipped on resume.
            Ok(())
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!("未能清理未完成下载片段");
        }
    }

    async fn reject(&self, directory: &Path, id: &str, reason: &str) -> AppResult<()> {
        self.guard_dir(directory)?;
        let path = directory.join(format!("{id}.rejected"));
        let mut file = tokio::fs::File::from_std(crate::safe_fs::create_new(&path)?);
        file.write_all(reason.as_bytes()).await?;
        file.sync_all().await?;
        Ok(())
    }
}

static GLOBAL_DOWNLOAD_LOCK: Mutex<()> = Mutex::const_new(());

async fn write_chunk(file: &mut tokio::fs::File, chunk: &[u8]) -> std::io::Result<()> {
    file.write_all(chunk).await?;
    // Tokio may still be writing after write_all; do not race it with the
    // next handle/path length comparison or report the buffered bytes yet.
    file.flush().await
}

fn same_file(path: &Path, opened: &std::fs::Metadata) -> AppResult<()> {
    let checked = check_regular(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if checked.ino() != opened.ino() || checked.dev() != opened.dev() || opened.nlink() != 1 {
            return Err(denied("下载文件已被替换"));
        }
    }
    if checked.len() != opened.len() {
        return Err(denied("下载文件已变化"));
    }
    Ok(())
}

fn suspicious_name(name: &str) -> bool {
    !matches!(
        name.rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "zip" | "7z" | "rar" | "gz" | "bz2" | "xz"
    )
}

struct ComputedHashes {
    sha256: String,
    sha512: String,
    sha1: Option<String>,
    md5: Option<String>,
    prefix: [u8; 6],
    prefix_len: usize,
}

#[cfg(test)]
async fn hash_file(
    path: &Path,
    expected: &FileHashes,
    cancel: &CancellationToken,
) -> AppResult<ComputedHashes> {
    check_regular(path)?;
    let mut file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    same_file(path, &meta)?;
    hash_open_file(&mut file, meta.len(), expected, cancel).await
}

async fn hash_open_file(
    file: &mut tokio::fs::File,
    expected_size: u64,
    expected: &FileHashes,
    cancel: &CancellationToken,
) -> AppResult<ComputedHashes> {
    file.rewind().await?;
    let (mut sha256, mut sha512) = (Sha256::new(), Sha512::new());
    // SHA-256 is always retained for installation fingerprints; SHA-512 keeps
    // the artifact contract. Legacy hashes are computed only when supplied.
    let mut sha1 = expected.sha1.as_ref().map(|_| Sha1::new());
    let mut md5 = expected.md5.as_ref().map(|_| Md5::new());
    let mut bytes = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    let mut prefix = [0; 6];
    let mut prefix_len = 0;
    loop {
        let n = tokio::select! { biased; _ = cancel.cancelled() => return Err(AppError::Cancelled), n = file.read(&mut bytes) => n? };
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > expected_size {
            return Err(denied("完整性校验期间下载文件长度已变化"));
        }
        let take = (prefix.len() - prefix_len).min(n);
        prefix[prefix_len..prefix_len + take].copy_from_slice(&bytes[..take]);
        prefix_len += take;
        sha256.update(&bytes[..n]);
        sha512.update(&bytes[..n]);
        if let Some(hash) = &mut sha1 {
            hash.update(&bytes[..n]);
        }
        if let Some(hash) = &mut md5 {
            hash.update(&bytes[..n]);
        }
    }
    if total != expected_size {
        return Err(denied("完整性校验期间下载文件长度已变化"));
    }
    Ok(ComputedHashes {
        sha256: hex::encode(sha256.finalize()),
        sha512: hex::encode(sha512.finalize()),
        sha1: sha1.map(|h| hex::encode(h.finalize())),
        md5: md5.map(|h| hex::encode(h.finalize())),
        prefix,
        prefix_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    struct Reply {
        code: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        stall: bool,
        byte_interval: Option<Duration>,
    }
    impl Reply {
        fn new(code: u16, body: &[u8], length: usize, etag: Option<&str>) -> Self {
            let mut headers = vec![("Content-Length", length.to_string())];
            if let Some(tag) = etag {
                headers.push(("ETag", tag.into()));
            }
            Self {
                code,
                headers,
                body: body.into(),
                stall: false,
                byte_interval: None,
            }
        }
        fn range(mut self, value: &str) -> Self {
            self.headers.push(("Content-Range", value.into()));
            self
        }
    }
    struct Server {
        url: String,
        requests: Arc<StdMutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }
    async fn mock(replies: Vec<Reply>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mod.zip", listener.local_addr().unwrap());
        let requests = Arc::new(StdMutex::new(Vec::new()));
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0; 1];
                    if socket.read(&mut byte).await.unwrap() == 0 {
                        return;
                    }
                    bytes.push(byte[0]);
                    assert!(bytes.len() < 16384);
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8(bytes).unwrap().to_lowercase());
                let mut response = format!("HTTP/1.1 {} Test\r\nConnection: close\r\n", reply.code);
                for (name, value) in reply.headers {
                    response.push_str(&format!("{name}: {value}\r\n"));
                }
                response.push_str("\r\n");
                socket.write_all(response.as_bytes()).await.unwrap();
                if let Some(interval) = reply.byte_interval {
                    for (index, byte) in reply.body.iter().enumerate() {
                        if index > 0 {
                            tokio::time::sleep(interval).await;
                        }
                        if socket.write_all(std::slice::from_ref(byte)).await.is_err() {
                            break;
                        }
                    }
                } else {
                    let _ = socket.write_all(&reply.body).await;
                }
                if reply.stall {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                let _ = socket.shutdown().await;
            }
        });
        Server {
            url,
            requests,
            task,
        }
    }
    fn input(url: &str, data: &[u8]) -> ResolvedMod {
        ResolvedMod {
            source_id: "test".into(),
            project_id: "test-mod".into(),
            version_id: "1".into(),
            name: "Test".into(),
            version: "1".into(),
            download_url: url.into(),
            filename: "test.zip".into(),
            size: Some(data.len() as u64),
            hashes: FileHashes {
                sha256: Some(hex::encode(Sha256::digest(data))),
                sha512: Some(hex::encode(Sha512::digest(data))),
                ..Default::default()
            },
            homepage_url: None,
            warnings: Vec::new(),
        }
    }
    fn options(attempts: u32) -> DownloadOptions {
        DownloadOptions {
            max_attempts: attempts,
            retry_delay: Duration::from_millis(1),
            read_idle_timeout: Duration::from_secs(5),
            max_bytes: 1024,
        }
    }
    fn assert_no_transfer_cache(paths: &AppPaths) {
        let downloads = paths.downloads();
        assert_eq!(std::fs::read_dir(&downloads).unwrap().count(), 1);
        assert_eq!(
            std::fs::read_dir(downloads.join("incoming"))
                .unwrap()
                .count(),
            0
        );
    }

    fn setup(attempts: u32) -> (tempfile::TempDir, AppPaths, DownloadManager) {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().join("app")).unwrap();
        let manager = DownloadManager::with_options(&paths, options(attempts)).unwrap();
        (dir, paths, manager)
    }

    #[tokio::test]
    async fn verifies_hashes_reports_progress_and_never_installs() {
        let data = b"archive contents";
        let server = mock(vec![Reply::new(200, data, data.len(), Some("\"v1\""))]).await;
        let (_dir, paths, manager) = setup(1);
        let events = Arc::new(StdMutex::new(Vec::new()));
        let capture = events.clone();
        let result = manager
            .download_with_progress(
                &input(&server.url, data),
                CancellationToken::new(),
                Some(Arc::new(move |p| capture.lock().unwrap().push(p))),
            )
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&result.path).await.unwrap(), data);
        assert!(result.path.starts_with(paths.downloads()));
        assert!(
            result.integrity_verified && result.requires_review && !result.auto_install_allowed
        );
        let events = events.lock().unwrap();
        assert_eq!(events.last().unwrap().phase, DownloadPhase::Completed);
        assert!(events
            .iter()
            .any(|p| p.phase == DownloadPhase::Downloading && p.downloaded == data.len() as u64));
    }

    #[tokio::test]
    async fn direct_web_download_retains_a_mismatched_payload_as_rejected() {
        let html = b"<!doctype html><html>log in</html>";
        let server = mock(vec![Reply::new(200, html, html.len(), Some("\"html\""))]).await;
        let (_dir, paths, manager) = setup(1);
        let request = WebDownload::parse(&server.url).unwrap();
        let error = manager
            .download_web(&request, CancellationToken::new(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("文件头与归档类型不匹配"));
        let found: Vec<_> = walkdir::WalkDir::new(paths.downloads())
            .min_depth(2)
            .into_iter()
            .map(|entry| entry.unwrap().into_path())
            .collect();
        assert_eq!(found.len(), 3);
        assert!(found.iter().any(
            |path| path.extension().unwrap() == "part" && std::fs::read(path).unwrap() == html
        ));
        assert!(found
            .iter()
            .any(|path| path.extension().unwrap() == "rejected"));
        assert!(!found.iter().any(|path| path.extension().unwrap() == "zip"));
    }

    #[tokio::test]
    async fn direct_web_download_rechecks_redirects_and_does_not_emit_cdn_query() {
        let mut redirect = Reply::new(302, b"", 0, None);
        redirect
            .headers
            .push(("Location", "http://169.254.169.254/mod.zip".into()));
        let server = mock(vec![redirect]).await;
        let (_dir, _, manager) = setup(1);
        let request = WebDownload::parse(&server.url).unwrap();
        assert!(matches!(
            manager
                .download_web(&request, CancellationToken::new(), None)
                .await,
            Err(AppError::Denied(_))
        ));
        assert_eq!(server.requests.lock().unwrap().len(), 1);

        let mut redirect = Reply::new(302, b"", 0, None);
        redirect.headers.push((
            "Location",
            "/opaque-cdn-url?token=private-cdn-redirect".into(),
        ));
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("fixture.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"fixture").unwrap();
        let data = zip.finish().unwrap().into_inner();
        let mut reply = Reply::new(200, &data, data.len(), None);
        reply.headers.push((
            "Content-Disposition",
            "attachment; filename=ignored.ps1".into(),
        ));
        let server = mock(vec![redirect, reply]).await;
        let (_dir, _, manager) = setup(1);
        let request = WebDownload::parse(&server.url).unwrap();
        let artifact = manager
            .download_web(&request, CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(artifact.filename, "mod.zip");
        assert_eq!(std::fs::read(&artifact.path).unwrap(), data);
        assert!(
            !artifact.integrity_verified
                && artifact.requires_review
                && !artifact.auto_install_allowed
        );
        assert!(!serde_json::to_string(&artifact)
            .unwrap()
            .contains("private-cdn-redirect"));
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("get /opaque-cdn-url?token=private-cdn-redirect "));
    }

    #[tokio::test]
    async fn web_download_can_start_above_former_one_gib_limit() {
        const DECLARED: usize = 1024 * 1024 * 1024 + 1;
        let (_dir, paths, _) = setup(1);
        if crate::safe_fs::available_data_space(&paths.downloads()).unwrap() < DECLARED as u64 {
            return;
        }
        let mut reply = Reply::new(200, b"", DECLARED, None);
        reply.stall = true;
        let server = mock(vec![reply]).await;
        let manager = DownloadManager::new(&paths).unwrap();
        let request = WebDownload::parse(&server.url).unwrap();
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let progress = Arc::new(move |event: DownloadProgress| {
            if event.phase == DownloadPhase::Downloading {
                assert_eq!(event.total, Some(DECLARED as u64));
                signal.cancel();
            }
        });
        assert!(matches!(
            manager.download_web(&request, cancel, Some(progress)).await,
            Err(AppError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn web_download_uses_disk_admission_not_a_static_gib_limit() {
        let (_dir, paths, _) = setup(1);
        let budget = crate::safe_fs::available_data_space(&paths.downloads()).unwrap();
        let impossible = budget.saturating_add(1024 * 1024 * 1024);
        let server = mock(vec![Reply::new(200, b"", impossible as usize, None)]).await;
        let manager = DownloadManager::new(&paths).unwrap();
        let request = WebDownload::parse(&server.url).unwrap();
        assert!(manager
            .download_web(&request, CancellationToken::new(), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("磁盘空间不足"));
        assert_eq!(manager.options.max_bytes, u64::MAX);
    }

    fn mock_signed_url(server: &Server) -> String {
        let origin = server.url.strip_suffix("/mod.zip").unwrap();
        format!("{origin}/opaque-mcp-path?signature=secret-mcp-url")
    }

    #[tokio::test]
    async fn mcp_archive_uses_receipt_name_and_does_not_persist_signed_transport_data() {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file("fixture.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, b"fixture").unwrap();
        let data = zip.finish().unwrap().into_inner();

        let mut redirect = Reply::new(302, b"", 0, None);
        redirect.headers.extend([
            ("Location", "/opaque-cdn?signature=secret-cdn-url".into()),
            ("Set-Cookie", "session=private-cookie".into()),
        ]);
        let mut reply = Reply::new(
            200,
            &data,
            data.len(),
            Some("\"secret-mcp-url:secret-cdn-url\""),
        );
        reply.headers.push((
            "Content-Disposition",
            "attachment; filename=ignored.ps1".into(),
        ));
        let server = mock(vec![redirect, reply]).await;
        let (_dir, _, manager) = setup(1);
        let url = mock_signed_url(&server);
        let events = Arc::new(StdMutex::new(Vec::new()));
        let captured = events.clone();
        let artifact = manager
            .download_mcp_archive(
                &url,
                "Release v1.zip",
                Some(data.len() as u64),
                CancellationToken::new(),
                Some(Arc::new(move |event| captured.lock().unwrap().push(event))),
            )
            .await
            .unwrap();
        assert_eq!(artifact.filename, "Release v1.zip");
        assert_eq!(artifact.size, data.len() as u64);
        assert_eq!(std::fs::read(&artifact.path).unwrap(), data);
        assert_eq!(artifact.sha256, hex::encode(Sha256::digest(&data)));
        assert_eq!(artifact.sha512, hex::encode(Sha512::digest(&data)));
        assert!(
            !artifact.integrity_verified
                && artifact.requires_review
                && !artifact.auto_install_allowed
        );
        let events = events.lock().unwrap();
        assert_eq!(events.last().unwrap().phase, DownloadPhase::Completed);
        let sidecar = artifact
            .path
            .parent()
            .unwrap()
            .join(format!("{}.json", artifact.id));
        let meta = std::fs::read_to_string(sidecar).unwrap();
        assert_eq!(serde_json::from_str::<PartialMeta>(&meta).unwrap().etag, "");
        for text in [
            serde_json::to_string(&artifact).unwrap(),
            serde_json::to_string(&*events).unwrap(),
            meta,
        ] {
            for secret in [
                url.as_str(),
                "secret-mcp-url",
                "secret-cdn-url",
                "private-cookie",
            ] {
                assert!(!text.contains(secret));
            }
        }
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("get /opaque-mcp-path?signature=secret-mcp-url "));
        assert!(requests[1].starts_with("get /opaque-cdn?signature=secret-cdn-url "));
        for request in requests.iter() {
            assert!(!request.contains("\r\nauthorization:"));
            assert!(!request.contains("\r\ncookie:"));
            assert!(!request.contains("\r\nreferer:"));
        }
    }

    #[tokio::test]
    async fn mcp_archive_quarantines_html_disguised_as_an_archive() {
        let html = b"<html>not an archive</html>";
        let mut reply = Reply::new(200, html, html.len(), None);
        reply
            .headers
            .push(("Content-Type", "application/zip".into()));
        let server = mock(vec![reply]).await;
        let (_dir, paths, manager) = setup(1);
        let url = mock_signed_url(&server);
        let message = manager
            .download_mcp_archive(&url, "test.zip", None, CancellationToken::new(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(message.contains("文件头与归档类型不匹配"));
        assert!(!message.contains(&url));
        assert!(!message.contains("secret-mcp-url"));
        let files: Vec<_> = walkdir::WalkDir::new(paths.downloads())
            .min_depth(2)
            .into_iter()
            .map(|entry| entry.unwrap().into_path())
            .collect();
        assert!(files.iter().any(
            |path| path.extension().unwrap() == "part" && std::fs::read(path).unwrap() == html
        ));
        assert!(files
            .iter()
            .any(|path| path.extension().unwrap() == "rejected"));
        assert!(!files.iter().any(|path| path.extension().unwrap() == "zip"));
    }

    #[tokio::test]
    async fn signed_archive_checks_space_before_creating_a_partial() {
        let (_dir, paths, _) = setup(1);
        let manager = DownloadManager::new(&paths).unwrap();
        let impossible = crate::safe_fs::available_data_space(&paths.downloads())
            .unwrap()
            .saturating_add(1024 * 1024 * 1024) as usize;
        let server = mock(vec![Reply::new(200, b"", impossible, None)]).await;
        let url = mock_signed_url(&server);
        let error = manager
            .download_mcp_archive(&url, "test.zip", None, CancellationToken::new(), None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("磁盘空间不足"), "{error}");
        assert_eq!(
            walkdir::WalkDir::new(paths.downloads())
                .min_depth(2)
                .into_iter()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn mcp_archive_validates_receipt_size_in_headers_and_while_streaming() {
        let mut chunked = Reply::new(200, b"6\r\nPK\x03\x04xx\r\n0\r\n\r\n", 0, None);
        chunked.headers = vec![("Transfer-Encoding", "chunked".into())];
        let server = mock(vec![Reply::new(200, b"PK\x03\x04xx", 6, None), chunked]).await;
        let (_dir, _paths, manager) = setup(1);
        let url = mock_signed_url(&server);
        for message in ["响应大小与来源文件大小不符", "下载超过预期大小"] {
            let error = manager
                .download_mcp_archive(&url, "test.zip", Some(5), CancellationToken::new(), None)
                .await
                .unwrap_err();
            assert!(matches!(error, AppError::Invalid(reason) if reason.contains(message)));
        }
    }

    #[tokio::test]
    async fn mcp_archive_rejects_private_redirect_without_exposing_either_url() {
        let private = "https://169.254.169.254/opaque?signature=secret-cdn-url";
        let mut redirect = Reply::new(302, b"", 0, None);
        redirect.headers.push(("Location", private.into()));
        let server = mock(vec![redirect]).await;
        let (_dir, _, manager) = setup(1);
        let url = mock_signed_url(&server);
        let error = manager
            .download_mcp_archive(&url, "test.zip", None, CancellationToken::new(), None)
            .await
            .unwrap_err();
        assert!(matches!(error, AppError::Denied(_)));
        let message = error.to_string();
        for secret in [url.as_str(), private, "secret-mcp-url", "secret-cdn-url"] {
            assert!(!message.contains(secret));
        }
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn public_web_errors_contain_neither_the_approved_link_nor_a_signed_redirect() {
        // An invalid wire status exercises reqwest's error path, whose raw
        // error can contain the signed request URL.
        for (code, expected) in [(403, "HTTP 403"), (9999, "下载网络请求失败")] {
            let mut redirect = Reply::new(302, b"", 0, None);
            let cdn_path = "/opaque-cdn?signature=secret-cdn-url";
            redirect.headers.push(("Location", cdn_path.into()));
            let server = mock(vec![
                redirect,
                Reply::new(code, b"secret-response", 15, None),
            ])
            .await;
            let (_dir, _, manager) = setup(1);
            let request = WebDownload::parse(&server.url).unwrap();
            let error = manager
                .download_web(&request, CancellationToken::new(), None)
                .await
                .unwrap_err();
            let message = serde_json::to_string(&error).unwrap();
            assert!(message.contains(expected));
            for secret in [
                server.url.as_str(),
                cdn_path,
                "secret-cdn-url",
                "secret-response",
            ] {
                assert!(!message.contains(secret));
            }
            assert_eq!(server.requests.lock().unwrap().len(), 2);
        }
    }

    #[test]
    fn chunk_write_waits_for_the_buffered_disk_write() {
        let temp = tempfile::tempfile().unwrap();
        let stat = temp.try_clone().unwrap();
        let mut file = tokio::fs::File::from_std(temp);
        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap()
            .block_on(async {
                let (paused, ready) = tokio::sync::oneshot::channel();
                let (release, blocked) = std::sync::mpsc::channel::<()>();
                let blocker = tokio::task::spawn_blocking(move || {
                    paused.send(()).unwrap();
                    let _ = blocked.recv();
                });
                ready.await.unwrap();
                let mut pending = Box::pin(write_chunk(&mut file, b"archive chunk"));
                let state = futures_util::poll!(pending.as_mut());
                drop(release);
                assert!(state.is_pending(), "do not return merely buffered bytes");
                pending.await.unwrap();
                blocker.await.unwrap();
                assert_eq!(stat.metadata().unwrap().len(), 13);
            });
    }

    #[tokio::test]
    async fn identical_downloads_share_resume_key_but_catalog_distinct_files() {
        let data = b"archive contents";
        let server = mock(vec![
            Reply::new(200, data, data.len(), Some("\"v1\"")),
            Reply::new(200, data, data.len(), Some("\"v1\"")),
        ])
        .await;
        let (_dir, paths, manager) = setup(1);
        let engine = crate::transaction::FileEngine::new(
            crate::db::Db::open(&paths.db_file()).unwrap(),
            paths,
        );
        let request = input(&server.url, data);
        let fingerprint = hex::encode(Sha256::digest(
            serde_json::to_vec(&(
                &request.download_url,
                &request.filename,
                request.size,
                &request.hashes,
            ))
            .unwrap(),
        ));
        let cache = engine.paths.downloads().join(&fingerprint);
        let events = Arc::new(StdMutex::new(Vec::new()));
        let mut artifacts = Vec::new();
        for _ in 0..2 {
            let capture = events.clone();
            let artifact = manager
                .download_with_progress(
                    &request,
                    CancellationToken::new(),
                    Some(Arc::new(move |p| capture.lock().unwrap().push(p))),
                )
                .await
                .unwrap();
            assert!(uuid::Uuid::parse_str(&artifact.id).is_ok());
            assert_eq!(
                artifact.path,
                cache.join(format!("{}-{}", artifact.id, request.filename))
            );
            let meta: PartialMeta = serde_json::from_slice(
                &std::fs::read(cache.join(format!("{}.json", artifact.id))).unwrap(),
            )
            .unwrap();
            assert_eq!(meta.id, artifact.id);
            assert_eq!(meta.fingerprint, fingerprint);
            crate::artifacts::record(&engine, &artifact, &request).unwrap();
            artifacts.push(artifact);
        }
        assert_ne!(artifacts[0].id, artifacts[1].id);
        assert_eq!(crate::artifacts::list(&engine.db).unwrap().len(), 2);
        for artifact in artifacts {
            assert_eq!(std::fs::read(&artifact.path).unwrap(), data);
            let entry = crate::artifacts::for_path(&engine.db, &artifact.path)
                .unwrap()
                .unwrap();
            assert_eq!(entry.artifact, artifact);
        }
        let events = events.lock().unwrap();
        assert!(events.iter().all(|p| p.id == fingerprint));
        assert_eq!(
            events
                .iter()
                .filter(|p| p.phase == DownloadPhase::Completed)
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn sha1_and_md5_match_case_insensitively_without_claiming_strong_verification() {
        let data = b"abc";
        for (sha1, md5) in [(false, false), (true, false), (false, true), (true, true)] {
            for uppercase in [false, true] {
                let server = mock(vec![Reply::new(200, data, data.len(), None)]).await;
                let (_dir, _paths, manager) = setup(1);
                let mut request = input(&server.url, data);
                let casing = |hash: &str| {
                    if uppercase {
                        hash.to_ascii_uppercase()
                    } else {
                        hash.into()
                    }
                };
                request.hashes = FileHashes {
                    sha1: sha1.then(|| casing("a9993e364706816aba3e25717850c26c9cd0d89d")),
                    md5: md5.then(|| casing("900150983cd24fb0d6963f7d28e17f72")),
                    ..Default::default()
                };
                let artifact = manager
                    .download_with_progress(&request, CancellationToken::new(), None)
                    .await
                    .unwrap();
                assert_eq!(std::fs::read(&artifact.path).unwrap(), data);
                assert_eq!(
                    artifact.sha256,
                    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                );
                assert_eq!(artifact.sha512, hex::encode(Sha512::digest(data)));
                assert!(!artifact.integrity_verified);
                assert!(artifact.requires_review && !artifact.auto_install_allowed);
                for (supplied, name) in [(sha1, "SHA-1"), (md5, "MD5")] {
                    assert_eq!(
                        artifact.warnings.iter().any(|w| {
                            w.contains(name)
                                && w.contains("弱完整性")
                                && w.contains("不证明来源真实性或文件安全")
                        }),
                        supplied
                    );
                }
                assert!(artifact
                    .warnings
                    .iter()
                    .any(|w| w.contains("没有来源 SHA-256/SHA-512")));
            }
        }
    }

    #[tokio::test]
    async fn matching_uppercase_strong_and_weak_hashes_keep_strong_verification() {
        let data = b"abc";
        let server = mock(vec![Reply::new(200, data, data.len(), None)]).await;
        let (_dir, _paths, manager) = setup(1);
        let mut request = input(&server.url, data);
        request.hashes.sha256 = request.hashes.sha256.map(|h| h.to_ascii_uppercase());
        request.hashes.sha512 = request.hashes.sha512.map(|h| h.to_ascii_uppercase());
        request.hashes.sha1 = Some("A9993E364706816ABA3E25717850C26C9CD0D89D".into());
        request.hashes.md5 = Some("900150983CD24FB0D6963F7D28E17F72".into());
        let artifact = manager
            .download_with_progress(&request, CancellationToken::new(), None)
            .await
            .unwrap();
        assert!(artifact.integrity_verified);
        assert!(artifact.requires_review && !artifact.auto_install_allowed);
        assert!(artifact
            .warnings
            .iter()
            .any(|w| w.contains("SHA-1") && w.contains("弱完整性")));
        assert!(artifact
            .warnings
            .iter()
            .any(|w| w.contains("MD5") && w.contains("弱完整性")));
    }

    #[tokio::test]
    async fn any_supplied_hash_mismatch_is_quarantined_even_when_all_other_hashes_match() {
        let data = b"abc";
        for algorithm in ["sha256", "sha512", "sha1", "md5"] {
            let server = mock(vec![Reply::new(200, data, data.len(), Some("\"v1\""))]).await;
            let (_dir, paths, manager) = setup(3);
            let mut request = input(&server.url, data);
            request.hashes.sha1 = Some("a9993e364706816aba3e25717850c26c9cd0d89d".into());
            request.hashes.md5 = Some("900150983cd24fb0d6963f7d28e17f72".into());
            let (target, length) = match algorithm {
                "sha256" => (&mut request.hashes.sha256, 64),
                "sha512" => (&mut request.hashes.sha512, 128),
                "sha1" => (&mut request.hashes.sha1, 40),
                "md5" => (&mut request.hashes.md5, 32),
                _ => unreachable!(),
            };
            *target = Some("0".repeat(length));
            let events = Arc::new(StdMutex::new(Vec::new()));
            let capture = events.clone();
            let result = manager
                .download_with_progress(
                    &request,
                    CancellationToken::new(),
                    Some(Arc::new(move |p| capture.lock().unwrap().push(p.phase))),
                )
                .await;
            assert!(matches!(result, Err(AppError::Invalid(_))), "{algorithm}");
            let cache = std::fs::read_dir(paths.downloads())
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let files: Vec<_> = std::fs::read_dir(cache)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            assert!(
                files
                    .iter()
                    .any(|p| p.extension().is_some_and(|s| s == "rejected")),
                "{algorithm}"
            );
            let partial = files
                .iter()
                .find(|p| p.extension().is_some_and(|s| s == "part"))
                .unwrap();
            assert_eq!(std::fs::read(partial).unwrap(), data);
            assert!(
                files
                    .iter()
                    .all(|p| p.extension().is_none_or(|s| s != "zip")),
                "{algorithm}"
            );
            let events = events.lock().unwrap();
            assert!(!events.contains(&DownloadPhase::Completed), "{algorithm}");
            assert_eq!(events.last(), Some(&DownloadPhase::Failed));
            assert_eq!(server.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn weak_only_hash_mismatches_are_quarantined() {
        for hashes in [
            FileHashes {
                sha1: Some("0".repeat(40)),
                ..Default::default()
            },
            FileHashes {
                md5: Some("0".repeat(32)),
                ..Default::default()
            },
        ] {
            let server = mock(vec![Reply::new(200, b"abc", 3, None)]).await;
            let (_dir, paths, manager) = setup(3);
            let mut request = input(&server.url, b"abc");
            request.hashes = hashes;
            assert!(matches!(
                manager
                    .download_with_progress(&request, CancellationToken::new(), None)
                    .await,
                Err(AppError::Invalid(_))
            ));
            let cache = std::fs::read_dir(paths.downloads())
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let files: Vec<_> = std::fs::read_dir(cache)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            assert!(files
                .iter()
                .any(|p| p.extension().is_some_and(|s| s == "rejected")));
            assert!(files
                .iter()
                .all(|p| p.extension().is_none_or(|s| s != "zip")));
            assert_eq!(server.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn malformed_source_hashes_are_rejected_before_network_or_cache_writes() {
        let (_dir, paths, manager) = setup(1);
        let server = mock(vec![]).await;
        for algorithm in ["sha256", "sha512", "sha1", "md5"] {
            let length = match algorithm {
                "sha256" => 64,
                "sha512" => 128,
                "sha1" => 40,
                _ => 32,
            };
            for invalid in [
                String::new(),
                "a".repeat(length - 1),
                "a".repeat(length + 1),
                "g".repeat(length),
                format!(" {}", "a".repeat(length - 1)),
                "é".repeat(length / 2),
            ] {
                let mut request = input(&server.url, b"abc");
                let target = match algorithm {
                    "sha256" => &mut request.hashes.sha256,
                    "sha512" => &mut request.hashes.sha512,
                    "sha1" => &mut request.hashes.sha1,
                    _ => &mut request.hashes.md5,
                };
                *target = Some(invalid);
                assert!(
                    matches!(
                        manager
                            .download_with_progress(&request, CancellationToken::new(), None)
                            .await,
                        Err(AppError::Invalid(_))
                    ),
                    "{algorithm}"
                );
            }
        }
        assert_no_transfer_cache(&paths);
        assert!(server.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn one_file_pass_hashes_multiple_chunks_and_skips_unsupplied_weak_algorithms() {
        let (_dir, paths, _manager) = setup(1);
        let path = paths.downloads().join("hash-test.part");
        let data = vec![0x42; 128 * 1024 + 17];
        std::fs::write(&path, &data).unwrap();
        let expected = FileHashes {
            sha1: Some("a".repeat(40)),
            md5: Some("b".repeat(32)),
            ..Default::default()
        };
        let hashes = hash_file(&path, &expected, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(hashes.sha256, hex::encode(Sha256::digest(&data)));
        assert_eq!(hashes.sha512, hex::encode(Sha512::digest(&data)));
        assert_eq!(hashes.sha1, Some(hex::encode(Sha1::digest(&data))));
        assert_eq!(hashes.md5, Some(hex::encode(Md5::digest(&data))));
        let hashes = hash_file(&path, &FileHashes::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(hashes.sha256, hex::encode(Sha256::digest(&data)));
        assert!(hashes.sha1.is_none() && hashes.md5.is_none());
    }

    #[test]
    fn legacy_serialized_file_hashes_default_new_algorithms_to_none() {
        let hashes: FileHashes =
            serde_json::from_value(serde_json::json!({"sha256":"a".repeat(64),"sha512":null}))
                .unwrap();
        hashes.validate().unwrap();
        assert!(hashes.sha1.is_none() && hashes.md5.is_none());
    }

    #[tokio::test]
    async fn resumes_interrupted_body_using_strong_etag_and_exact_content_range() {
        let data = b"0123456789";
        let server = mock(vec![
            Reply::new(200, &data[..4], 10, Some("\"v1\"")),
            Reply::new(206, &data[4..], 6, Some("\"v1\"")).range("bytes 4-9/10"),
        ])
        .await;
        let (_dir, _paths, manager) = setup(2);
        let mut request = input(&server.url, data);
        request.hashes.sha1 = Some(hex::encode(Sha1::digest(data)));
        request.hashes.md5 = Some(hex::encode(Md5::digest(data)));
        let artifact = manager
            .download_with_progress(&request, CancellationToken::new(), None)
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(&artifact.path).await.unwrap(), data);
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("range: bytes=4-\r\n"));
        assert!(requests[1].contains("if-range: \"v1\"\r\n"));
    }

    #[tokio::test]
    async fn rejects_wrong_range_changed_etag_or_ignored_range_and_restarts() {
        let data = b"0123456789";
        for wrong in [
            Reply::new(206, &data[4..], 6, Some("\"v1\"")).range("bytes 3-8/10"),
            Reply::new(206, &data[4..], 6, Some("\"v2\"")).range("bytes 4-9/10"),
            Reply::new(206, &data[4..], 6, Some("\"v1\"")).range("bytes 4-9/11"),
            Reply::new(200, data, 10, Some("\"v1\"")),
        ] {
            let server = mock(vec![
                Reply::new(200, &data[..4], 10, Some("\"v1\"")),
                wrong,
                Reply::new(200, data, 10, Some("\"v2\"")),
            ])
            .await;
            let (_dir, paths, manager) = setup(2);
            let artifact = manager
                .download_with_progress(&input(&server.url, data), CancellationToken::new(), None)
                .await
                .unwrap();
            assert_eq!(tokio::fs::read(&artifact.path).await.unwrap(), data);
            assert_eq!(server.requests.lock().unwrap().len(), 3);
            let cache = std::fs::read_dir(paths.downloads())
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            assert!(std::fs::read_dir(cache).unwrap().any(|e| e
                .unwrap()
                .path()
                .extension()
                .is_some_and(|e| e == "rejected")));
        }
    }

    #[tokio::test]
    async fn weak_or_missing_etag_restarts_without_range() {
        let data = b"0123456789";
        for tag in [None, Some("W/\"v1\"")] {
            let server = mock(vec![
                Reply::new(200, &data[..4], 10, tag),
                Reply::new(200, data, 10, Some("\"v2\"")),
            ])
            .await;
            let (_dir, _paths, manager) = setup(2);
            manager
                .download_with_progress(&input(&server.url, data), CancellationToken::new(), None)
                .await
                .unwrap();
            assert!(!server.requests.lock().unwrap()[1].contains("range:"));
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_stream_and_keeps_resumable_partial() {
        let data = b"0123456789";
        let mut reply = Reply::new(200, &data[..4], 10, Some("\"v1\""));
        reply.stall = true;
        let server = mock(vec![reply]).await;
        let (_dir, paths, manager) = setup(2);
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let callback: ProgressCallback = Arc::new(move |p| {
            if p.phase == DownloadPhase::Downloading && p.downloaded == 4 {
                signal.cancel();
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            manager.download_with_progress(&input(&server.url, data), cancel, Some(callback)),
        )
        .await
        .unwrap();
        assert!(matches!(result, Err(AppError::Cancelled)));
        let cache = std::fs::read_dir(paths.downloads())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let partial = std::fs::read_dir(cache)
            .unwrap()
            .find_map(|e| {
                let p = e.unwrap().path();
                if p.extension().is_some_and(|s| s == "part") {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(std::fs::read(partial).unwrap(), &data[..4]);
    }

    #[tokio::test]
    async fn cancelled_stream_with_no_validator_discards_payload() {
        let data = b"0123456789";
        let mut reply = Reply::new(200, &data[..4], data.len(), None);
        reply.stall = true;
        let server = mock(vec![reply]).await;
        let (_dir, paths, manager) = setup(1);
        let cancel = CancellationToken::new();
        let signal = cancel.clone();
        let progress = Arc::new(move |event: DownloadProgress| {
            if event.phase == DownloadPhase::Downloading && event.downloaded > 0 {
                signal.cancel();
            }
        });
        assert!(matches!(
            manager
                .download_with_progress(&input(&server.url, data), cancel, Some(progress))
                .await,
            Err(AppError::Cancelled)
        ));
        assert!(!walkdir::WalkDir::new(paths.downloads())
            .into_iter()
            .filter_map(Result::ok)
            .any(|e| e
                .path()
                .extension()
                .is_some_and(|ext| ext == "part" || ext == "zip")));
    }

    #[tokio::test]
    async fn read_idle_timeout_allows_continuous_download_beyond_interval() {
        let (_dir, paths, _) = setup(1);
        let idle = Duration::from_millis(900);
        let data = b"12345678";
        let mut reply = Reply::new(200, data, data.len(), None);
        reply.byte_interval = Some(Duration::from_millis(220));
        let server = mock(vec![reply]).await;
        let manager = DownloadManager::with_options(
            &paths,
            DownloadOptions {
                read_idle_timeout: idle,
                ..options(1)
            },
        )
        .unwrap();
        let started = tokio::time::Instant::now();
        let artifact = manager
            .download_with_progress(&input(&server.url, data), CancellationToken::new(), None)
            .await
            .unwrap();
        assert!(started.elapsed() > idle);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert_eq!(std::fs::read(artifact.path).unwrap(), data);
    }

    #[tokio::test]
    async fn read_idle_timeout_releases_a_stalled_download() {
        let (_dir, paths, _) = setup(1);
        let data = b"12345678";
        let mut reply = Reply::new(200, &data[..4], data.len(), Some("\"v1\""));
        reply.stall = true;
        let server = mock(vec![reply]).await;
        let manager = DownloadManager::with_options(
            &paths,
            DownloadOptions {
                read_idle_timeout: Duration::from_millis(400),
                ..options(1)
            },
        )
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            manager.download_with_progress(
                &input(&server.url, data),
                CancellationToken::new(),
                None,
            ),
        )
        .await
        .expect("the stalled response must release the reader before the watchdog");
        assert!(matches!(result, Err(AppError::Other(_))));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn resume_scans_past_two_thousand_entries_and_checks_cancellation() {
        let (_dir, paths, manager) = setup(1);
        let key = "fixture";
        let dir = paths.downloads().join(key);
        std::fs::create_dir(&dir).unwrap();
        for i in 0..2050 {
            std::fs::write(
                dir.join(format!("{}.json", uuid::Uuid::from_u128(i))),
                b"{}",
            )
            .unwrap();
        }
        let last = std::fs::read_dir(&dir)
            .unwrap()
            .last()
            .unwrap()
            .unwrap()
            .path();
        let id = last.file_stem().unwrap().to_str().unwrap().to_owned();
        std::fs::write(
            &last,
            serde_json::to_vec(&PartialMeta {
                id: id.clone(),
                fingerprint: key.into(),
                etag: "\"test\"".into(),
                total: Some(5),
            })
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join(format!("{id}.part")), b"abc").unwrap();
        let cancel = CancellationToken::new();
        assert_eq!(
            manager
                .resume(&dir, key, u64::MAX, &cancel)
                .await
                .unwrap()
                .unwrap()
                .offset,
            3
        );
        cancel.cancel();
        assert!(matches!(
            manager.resume(&dir, key, u64::MAX, &cancel).await,
            Err(AppError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn resumes_across_manager_instances_after_an_interrupted_call() {
        let data = b"0123456789";
        let server = mock(vec![
            Reply::new(200, &data[..4], 10, Some("\"v1\"")),
            Reply::new(206, &data[4..], 6, Some("\"v1\"")).range("bytes 4-9/10"),
        ])
        .await;
        let (_dir, paths, manager) = setup(1);
        assert!(manager
            .download_with_progress(&input(&server.url, data), CancellationToken::new(), None)
            .await
            .is_err());
        drop(manager);
        let reopened = DownloadManager::with_options(&paths, options(1)).unwrap();
        let artifact = reopened
            .download_mod(
                &input(&server.url, data),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(artifact.path).unwrap(), data);
        assert!(server.requests.lock().unwrap()[1].contains("range: bytes=4-"));
    }

    #[tokio::test]
    async fn redirect_targets_are_revalidated_without_following_private_or_credential_urls() {
        for target in [
            "https://10.0.0.1/a",
            "https://user:secret@example.org/a",
            "file:///etc/passwd",
            "http://example.org/a",
        ] {
            let mut reply = Reply::new(302, b"", 0, None);
            reply.headers.push(("Location", target.into()));
            let server = mock(vec![reply]).await;
            let (_dir, _paths, manager) = setup(1);
            let result = manager
                .download_with_progress(
                    &input(&server.url, b"test"),
                    CancellationToken::new(),
                    None,
                )
                .await;
            assert!(matches!(result, Err(AppError::Denied(_))), "{target}");
            assert_eq!(server.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn body_size_cap_and_unsolicited_206_are_rejected() {
        let (_dir, paths, _) = setup(1);
        let manager = DownloadManager::with_options(
            &paths,
            DownloadOptions {
                max_bytes: 3,
                ..options(1)
            },
        )
        .unwrap();
        let server = mock(vec![Reply::new(200, b"long", 4, None)]).await;
        let mut request = input(&server.url, b"long");
        request.size = None;
        assert!(manager
            .download_with_progress(&request, CancellationToken::new(), None)
            .await
            .is_err());
        let server = mock(vec![
            Reply::new(206, b"ok", 2, Some("\"v1\"")).range("bytes 0-1/2")
        ])
        .await;
        assert!(manager
            .download_with_progress(&input(&server.url, b"ok"), CancellationToken::new(), None)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn hash_mismatch_is_quarantined_with_no_completed_artifact() {
        let server = mock(vec![Reply::new(200, b"evil", 4, Some("\"v1\""))]).await;
        let (_dir, paths, manager) = setup(3);
        assert!(matches!(
            manager
                .download_with_progress(
                    &input(&server.url, b"good"),
                    CancellationToken::new(),
                    None
                )
                .await,
            Err(AppError::Invalid(_))
        ));
        let cache = std::fs::read_dir(paths.downloads())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let files: Vec<_> = std::fs::read_dir(cache)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert!(files
            .iter()
            .any(|p| p.extension().is_some_and(|s| s == "rejected")));
        assert!(files
            .iter()
            .all(|p| p.extension().is_none_or(|s| s != "zip")));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn retries_transient_http_but_not_permanent_error() {
        for code in [429, 503] {
            let server = mock(vec![
                Reply::new(code, b"", 0, None),
                Reply::new(200, b"ok", 2, None),
            ])
            .await;
            let (_dir, _paths, manager) = setup(2);
            manager
                .download_with_progress(&input(&server.url, b"ok"), CancellationToken::new(), None)
                .await
                .unwrap();
            assert_eq!(server.requests.lock().unwrap().len(), 2);
        }
        let server = mock(vec![Reply::new(404, b"", 0, None)]).await;
        let (_dir, _paths, manager) = setup(3);
        assert!(manager
            .download_with_progress(&input(&server.url, b"ok"), CancellationToken::new(), None)
            .await
            .is_err());
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn rejects_size_mismatch_and_path_or_url_injection_before_writing() {
        let (_dir, paths, manager) = setup(1);
        for filename in [
            "../escape.zip",
            "/tmp/escape.zip",
            "dir/a.zip",
            "a\\b.zip",
            "CON.zip",
            "x:stream",
            ".hidden",
        ] {
            let mut request = input("https://example.org/mod.zip", b"test");
            request.filename = filename.into();
            assert!(manager
                .download_with_progress(&request, CancellationToken::new(), None)
                .await
                .is_err());
        }
        for url in [
            "https://user:secret@example.org/a",
            "file:///tmp/a",
            "https://10.0.0.1/a",
        ] {
            assert!(manager
                .download_with_progress(&input(url, b"test"), CancellationToken::new(), None)
                .await
                .is_err());
        }
        assert_no_transfer_cache(&paths);
        let server = mock(vec![Reply::new(200, b"too long", 8, None)]).await;
        assert!(manager
            .download_with_progress(&input(&server.url, b"test"), CancellationToken::new(), None)
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_download_root_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().join("app")).unwrap();
        // Preserve the original directory instead of removing it.
        std::fs::rename(paths.downloads(), paths.root.join("original-downloads")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, paths.downloads()).unwrap();
        assert!(DownloadManager::new(&paths).is_err());
        assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    }
}
