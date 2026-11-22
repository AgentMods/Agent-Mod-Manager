//! Uncataloged archive downloads do not establish provider provenance.
use super::validate_download_url;
use crate::error::{AppError, AppResult};
use percent_encoding::percent_decode_str;
use url::Url;

pub(crate) const MAX_WEB_URL_BYTES: usize = 2048;
const MAX_ENDPOINT_BYTES: usize = 8192;

#[derive(Clone, Copy)]
pub(crate) enum ArchiveKind {
    Zip,
    SevenZip,
    TarGzip,
}

impl ArchiveKind {
    pub(crate) fn matches(self, header: &[u8]) -> bool {
        match self {
            Self::Zip => header.starts_with(b"PK\x03\x04") || header.starts_with(b"PK\x05\x06"),
            Self::SevenZip => header.starts_with(b"7z\xbc\xaf\x27\x1c"),
            Self::TarGzip => header.starts_with(b"\x1f\x8b\x08"),
        }
    }
}

pub(crate) struct WebDownload {
    url: Url,
    filename: String,
    archive: ArchiveKind,
}

impl WebDownload {
    pub(crate) fn parse(raw: &str) -> AppResult<Self> {
        if raw.len() > MAX_WEB_URL_BYTES || !valid_url_text(raw) {
            return Err(AppError::Invalid("网页归档直链格式无效或过长".into()));
        }
        let url = validated_endpoint(raw)?;
        if url.query().is_some() || url.as_str().len() > MAX_WEB_URL_BYTES {
            return Err(AppError::Denied(
                "只接受不含参数或签名的公开归档直链".into(),
            ));
        }
        let segment = url
            .path_segments()
            .and_then(|mut parts| parts.next_back())
            .ok_or_else(|| AppError::Invalid("网页直链未指明归档文件名".into()))?;
        let filename = percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| AppError::Invalid("网页归档文件名必须是 UTF-8".into()))?
            .into_owned();
        let archive = archive_for_filename(&filename)?;
        Ok(Self {
            url,
            filename,
            archive,
        })
    }

    pub(crate) fn url(&self) -> &str {
        self.url.as_str()
    }
    pub(crate) fn filename(&self) -> &str {
        &self.filename
    }
    pub(crate) fn archive(&self) -> ArchiveKind {
        self.archive
    }
}

fn archive_for_filename(filename: &str) -> AppResult<ArchiveKind> {
    crate::download::safe_filename(filename)
        .map_err(|_| AppError::Invalid("网页归档文件名不是安全的单个文件名".into()))?;
    if filename
        .chars()
        .any(|c| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    {
        return Err(AppError::Invalid("网页归档文件名包含方向控制字符".into()));
    }
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".zip") || lower.ends_with(".jar") {
        Ok(ArchiveKind::Zip)
    } else if lower.ends_with(".7z") {
        Ok(ArchiveKind::SevenZip)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        Ok(ArchiveKind::TarGzip)
    } else {
        Err(AppError::Unsupported(
            "网页直链必须指向 ZIP、JAR、7z 或 tar.gz 归档".into(),
        ))
    }
}

/// A signed URL is accepted only by the internal receipt consumer. Its archive
/// filename comes from the receipt, not the URL or an HTTP header.
pub(crate) fn parse_mcp_archive(raw: &str, filename: &str) -> AppResult<(Url, ArchiveKind)> {
    Ok((validated_endpoint(raw)?, archive_for_filename(filename)?))
}

fn valid_url_text(raw: &str) -> bool {
    raw == raw.trim() && !raw.contains('\\') && !raw.chars().any(char::is_control)
}

/// Server redirects and signed MCP links can contain short-lived CDN parameters.
/// These are used only by the transport, never in metadata or progress.
pub(crate) fn validated_endpoint(raw: &str) -> AppResult<Url> {
    if raw.len() > MAX_ENDPOINT_BYTES || !valid_url_text(raw) {
        return Err(AppError::Invalid("网页下载地址格式无效或过长".into()));
    }
    let url = validate_download_url(raw)?;
    if url.as_str().len() > MAX_ENDPOINT_BYTES {
        return Err(AppError::Invalid("网页下载地址过长".into()));
    }
    // Public HTTPS on the default port. The shared validator permits numeric
    // loopback HTTP only in unit-test builds, including ephemeral fixture ports.
    if url.scheme() == "https" && url.port().is_some() {
        return Err(AppError::Denied("网页归档只允许标准 HTTPS 端口".into()));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_archives_derive_a_safe_filename_without_using_remote_headers() {
        for (raw, expected, magic) in [
            (
                "https://example.com/releases/File%20Name.ZIP",
                "File Name.ZIP",
                &b"PK\x03\x04"[..],
            ),
            ("https://example.com/mod.JAR", "mod.JAR", &b"PK\x05\x06"[..]),
            (
                "https://example.com/%E6%A8%A1%E7%BB%84.7z",
                "\u{6a21}\u{7ec4}.7z",
                &b"7z\xbc\xaf\x27\x1c"[..],
            ),
            (
                "https://example.com/file.tar.gz",
                "file.tar.gz",
                &b"\x1f\x8b\x08"[..],
            ),
        ] {
            let request = WebDownload::parse(raw).unwrap();
            assert_eq!(request.filename(), expected);
            assert!(request.archive().matches(magic));
            assert!(!request.archive().matches(b"<html>"));
        }
    }

    #[test]
    fn mcp_archive_accepts_a_signed_opaque_path_without_relaxing_public_links() {
        let raw = "https://example.com/resolve/opaque?signature=marker-secret";
        let (url, archive) = parse_mcp_archive(raw, "Release 14.TGZ").unwrap();
        assert_eq!(url.as_str(), raw);
        assert!(archive.matches(b"\x1f\x8b\x08"));
        assert!(!archive.matches(b"PK\x03\x04"));
        assert!(WebDownload::parse(raw).is_err());
        assert!(
            WebDownload::parse("https://example.com/resolve.zip?signature=marker-secret").is_err()
        );
    }

    #[test]
    fn mcp_archive_rejects_unsafe_endpoints_and_names_without_echoing_them() {
        for raw in [
            "https://localhost/opaque?signature=marker-secret",
            "https://169.254.169.254/opaque?signature=marker-secret",
            "https://[::ffff:192.168.0.1]/opaque?signature=marker-secret",
            "https://me:marker-secret@example.com/opaque",
            "https://example.com:8443/opaque?signature=marker-secret",
            "https://example.com/opaque#marker-secret",
            "http://example.com/opaque?signature=marker-secret",
            "https://example.com/opaque?signature=marker-secret\n",
        ] {
            let error = parse_mcp_archive(raw, "release.zip").err().unwrap();
            let message = error.to_string();
            assert!(!message.contains(raw));
            assert!(!message.contains("marker-secret"));
        }
        let overlong = format!(
            "https://example.com/opaque?signature={}",
            "a".repeat(MAX_ENDPOINT_BYTES)
        );
        assert!(parse_mcp_archive(&overlong, "release.zip").is_err());

        let raw = "https://example.com/opaque?signature=marker-secret";
        for filename in [
            raw,
            "../marker-secret.zip",
            "CON.zip",
            "bad.zip:ads",
            "bad\u{202e}.zip",
            "file.exe",
        ] {
            let message = parse_mcp_archive(raw, filename).err().unwrap().to_string();
            assert!(!message.contains(filename));
            assert!(!message.contains("marker-secret"));
        }
    }

    #[test]
    fn rejects_private_and_nonarchive_urls_without_echoing_credentials() {
        for raw in [
            "https://localhost/a.zip",
            "https://192.168.1.12/a.zip",
            "https://[::ffff:169.254.169.254]/a.zip",
            "http://example.com/a.zip",
            "https://example.com:8443/a.zip",
            "file:///etc/a.zip",
            "https://me:marker-secret@example.com/a.zip",
            "https://example.com/a.zip?token=marker-secret",
            "https://example.com/a.zip#marker-secret",
            "https://example.com/a.zip?",
            "https://example.com/mods/download",
            "https://example.com/a.exe",
            "https://example.com/x%2Fa.zip",
            "https://example.com/x%5Ca.zip",
            "https://example.com/CON.zip",
            "https://example.com/.zip",
            "https://example.com/%FF.zip",
            "https://example.com/a%0A.zip",
            "https://example.com/a%E2%80%AE.zip",
        ] {
            let message = WebDownload::parse(raw)
                .err()
                .expect("invalid web URL")
                .to_string();
            assert!(!message.contains("marker-secret"));
        }
    }
}
