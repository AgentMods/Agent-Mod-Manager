//! Contract adapter for stock download tools. Only locally bound arguments and
//! the tool's own observed receipt may be used to admit bytes to this install.
use super::{
    scope::{bind, boundary},
    WorkKey,
};
use crate::{
    agent::tools::{ToolContext, ToolRegistry},
    error::{AppError, AppResult},
    paths,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub(super) fn permits(source: &str, tool: &str) -> bool {
    match source {
        "modio" | "curseforge" => matches!(tool, "list_mod_files" | "download_file"),
        "gamebanana" | "nexusmods" => matches!(tool, "list_files" | "download_file"),
        "thunderstore" => matches!(tool, "list_versions" | "download_version"),
        "glossmod_3dm" => matches!(tool, "get-mod-detail" | "get_mod_detail"),
        _ => false,
    }
}

pub(super) fn downloading(tool: &str) -> bool {
    matches!(tool, "download_file" | "download_version")
}

pub(super) fn number(value: &str) -> AppResult<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991 && n.to_string() == value)
        .ok_or_else(boundary)
}

pub(super) fn arguments(key: &WorkKey, remote: &str, args: &mut Value) -> AppResult<()> {
    let mut allowed = vec![];
    let download = downloading(remote);
    if key.source_id == "thunderstore" {
        let (namespace, name) = key.project_id.split_once('/').ok_or_else(boundary)?;
        bind(args, "namespace", json!(namespace))?;
        bind(args, "name", json!(name))?;
        allowed.extend(["namespace", "name"]);
        if download {
            let version = args["version"].as_str().ok_or_else(boundary)?;
            if version.len() > 64
                || !version
                    .split('.')
                    .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                || version.split('.').count() != 3
            {
                return Err(boundary());
            }
            allowed.push("version");
        } else {
            allowed.extend(["offset", "limit"]);
        }
    } else {
        let project = key.project_id.rsplit('/').next().ok_or_else(boundary)?;
        bind(args, "mod_id", json!(number(project)?))?;
        allowed.push("mod_id");
        match key.source_id.as_str() {
            "modio" => {
                bind(args, "game_id", json!(number(&key.provider_game_id)?))?;
                allowed.push("game_id");
                if !download {
                    allowed.extend(["offset", "limit"]);
                }
            }
            "nexusmods" => {
                bind(args, "game_domain", json!(key.provider_game_id))?;
                allowed.push("game_domain");
            }
            "curseforge" if !download => allowed.extend(["index", "page_size"]),
            _ => {}
        }
        if download {
            allowed.push("file_id");
            let file = args["file_id"].as_u64().ok_or_else(boundary)?;
            number(&file.to_string())?;
        }
    }
    let args = args.as_object().ok_or_else(boundary)?;
    for (name, value) in args {
        if !allowed.contains(&name.as_str()) {
            return Err(boundary());
        }
        if matches!(name.as_str(), "offset" | "index")
            && !value.as_u64().is_some_and(|v| v <= 10_000)
            || matches!(name.as_str(), "limit" | "page_size")
                && !value.as_u64().is_some_and(|v| (1..=100).contains(&v))
        {
            return Err(boundary());
        }
    }
    Ok(())
}

pub(super) fn result(value: &Value) -> AppResult<Value> {
    if value["isError"] == true || value["is_error"] == true {
        return Err(unavailable());
    }
    if let Some(value) = value
        .get("structuredContent")
        .or_else(|| value.get("structured_content"))
    {
        return Ok(value
            .get("result")
            .cloned()
            .unwrap_or_else(|| value.clone()));
    }
    if value.get("content").is_some() {
        let array = value["content"]
            .as_array()
            .filter(|a| a.len() == 1)
            .ok_or_else(unavailable)?;
        if array[0]["type"] != "text" {
            return Err(unavailable());
        }
        let text = array[0]["text"].as_str().ok_or_else(unavailable)?;
        // The verified stock Agent bridge redacts known credentials in both
        // the outer MCP document and a single decoded JSON text block.
        return serde_json::from_str(text).map_err(|_| unavailable());
    }
    Err(unavailable())
}

/// Keep URLs, tokens and unchecked provider strings out of the model transcript.
fn sanitized(value: Value) -> Value {
    use serde_json::Value::*;
    match value {
        Object(rows) => Object(
            rows.into_iter()
                .filter(|(k, _)| {
                    let key = k.to_ascii_lowercase();
                    ![
                        "url",
                        "link",
                        "uri",
                        "token",
                        "secret",
                        "credential",
                        "path",
                        "key",
                    ]
                    .iter()
                    .any(|word| key.contains(word))
                })
                .map(|(k, v)| (k, sanitized(v)))
                .collect(),
        ),
        Array(rows) => Array(rows.into_iter().take(100).map(sanitized).collect()),
        String(s) if s.contains("://") || s.len() > 600 => Null,
        v => v,
    }
}

pub(super) fn list_result(value: &Value) -> AppResult<Value> {
    Ok(sanitized(result(value)?))
}

#[derive(Clone)]
pub(super) struct StockReceipt {
    pub filename: String,
    path: PathBuf,
    pub size: u64,
    pub sha256: String,
    pub file_id: String,
}

impl StockReceipt {
    pub(super) fn import_args(&self, key: &WorkKey) -> Value {
        json!({"filename": self.filename, "gameId":key.game_id})
    }

    pub(super) fn matches_preview(&self, game: &str, value: &Value) -> bool {
        value["receiptFolder"] == "incoming"
            && value["gameId"] == game
            && value["filename"] == self.filename
            && value["sourceFingerprint"]["size"] == self.size
            && value["sourceFingerprint"]["sha256"]
                .as_str()
                .is_some_and(|v| v == self.sha256)
            && value["sourcePath"]
                .as_str()
                .is_some_and(|source| same_path(&self.path, Path::new(source)))
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    let a = paths::user_path(a);
    let b = paths::user_path(b);
    paths::contains_path(&a, &b) && paths::contains_path(&b, &a)
}

pub(super) fn unavailable() -> AppError {
    AppError::Denied("来源未返回可验证的下载；请在原作品页选择文件并手动导入".into())
}

fn hash(raw: &Value, size: usize) -> Option<String> {
    raw.as_str()
        .filter(|s| s.len() == size && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

fn check_id(value: &Value, expected: &Value) -> AppResult<()> {
    if value != expected {
        return Err(unavailable());
    }
    Ok(())
}

pub(super) async fn transfer_receipt(
    key: &WorkKey,
    args: &Value,
    value: &Value,
    context: &ToolContext,
    registry: &ToolRegistry,
) -> AppResult<StockReceipt> {
    let data = result(value)?;
    let mut sha = hash(&data["sha256"], 64);
    let mut sha1 = None;
    let mut md5 = None;
    let (size_key, file_id) = match key.source_id.as_str() {
        "modio" => {
            for name in ["game_id", "mod_id", "file_id"] {
                check_id(&data[name], &args[name])?;
            }
            ("bytes_written", args["file_id"].to_string())
        }
        "gamebanana" => {
            // The exact stock call first resolves this file against the requested mod's
            // file list; the saved receipt echoes the file it actually wrote.
            check_id(&data["file_id"], &args["file_id"])?;
            ("bytes", args["file_id"].to_string())
        }
        "curseforge" => {
            check_id(&data["modId"], &args["mod_id"])?;
            check_id(&data["fileId"], &args["file_id"])?;
            sha1 = hash(&data["verifiedHashes"]["sha1"], 40);
            md5 = hash(&data["verifiedHashes"]["md5"], 32);
            ("bytes", args["file_id"].to_string())
        }
        "nexusmods" => {
            for field in ["game_domain", "mod_id", "file_id"] {
                if let Some(observed) = data.get(field) {
                    check_id(observed, &args[field])?;
                }
            }
            // Stock implementation obtains metadata for the exact domain/mod/file,
            // validates the URI association, and returns local hashes, not a file ID.
            // Account/manual-link status objects have neither a path nor a hash.
            sha = hash(&data["hashes"]["sha256"], 64);
            ("bytes", args["file_id"].to_string())
        }
        "thunderstore" => {
            // The stock download method validates full_version_name before writing.
            // Some versions also return it; an explicit disagreement is never ignored.
            let version = args["version"].as_str().ok_or_else(unavailable)?;
            let expected = format!(
                "{}-{}-{version}",
                args["namespace"].as_str().ok_or_else(unavailable)?,
                args["name"].as_str().ok_or_else(unavailable)?
            );
            if data
                .get("full_version_name")
                .is_some_and(|v| v != &expected)
            {
                return Err(unavailable());
            }
            ("bytes_written", version.into())
        }
        _ => return Err(unavailable()),
    };
    if sha.is_none() && sha1.is_none() && md5.is_none() {
        return Err(unavailable());
    }
    let raw = data["path"]
        .as_str()
        .filter(|p| p.len() <= 4096 && !p.chars().any(char::is_control))
        .ok_or_else(unavailable)?;
    let path = PathBuf::from(raw);
    let folder = crate::storage::incoming_downloads(&context.engine.paths.root);
    if !path.is_absolute() || !path.parent().is_some_and(|p| same_path(p, &folder)) {
        return Err(unavailable());
    }
    let filename = path
        .file_name()
        .and_then(|p| p.to_str())
        .ok_or_else(unavailable)?
        .to_owned();
    crate::download::safe_filename(&filename)?;
    let size = data[size_key]
        .as_u64()
        .filter(|n| (1..=4 * 1024 * 1024 * 1024).contains(n))
        .ok_or_else(unavailable)?;
    let mut receipt = StockReceipt {
        filename,
        path,
        size,
        sha256: String::new(),
        file_id,
    };
    let prepared = registry
        .get("file_received_mod")?
        .prepare(context, &receipt.import_args(key))
        .await?;
    let captured_sha =
        hash(&prepared.preview["sourceFingerprint"]["sha256"], 64).ok_or_else(unavailable)?;
    if sha.is_some_and(|sha| sha != captured_sha) {
        return Err(unavailable());
    }
    receipt.sha256 = captured_sha;
    if !receipt.matches_preview(&key.game_id, &prepared.preview) {
        return Err(unavailable());
    }
    if sha1.is_some() || md5.is_some() {
        let path = receipt.path.clone();
        let digest = receipt.sha256.clone();
        let cancel = context.cancel.clone();
        tokio::task::spawn_blocking(move || check_legacy(&path, size, &digest, sha1, md5, &cancel))
            .await
            .map_err(|_| unavailable())??;
    }
    Ok(receipt)
}

fn check_legacy(
    path: &Path,
    size: u64,
    sha256: &str,
    sha1: Option<String>,
    md5: Option<String>,
    cancel: &tokio_util::sync::CancellationToken,
) -> AppResult<()> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = crate::safe_fs::open_regular(path)?;
    let (mut a, mut b, mut c) = (sha2::Sha256::new(), sha1::Sha1::new(), md5::Md5::new());
    let mut reader = (&mut f).take(size + 1);
    let mut buf = [0u8; 65536];
    let mut counted = 0;
    loop {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        counted += n as u64;
        a.update(&buf[..n]);
        b.update(&buf[..n]);
        c.update(&buf[..n]);
    }
    crate::safe_fs::verify_regular_at(&f, path)?;
    if counted != size
        || hex::encode(a.finalize()) != sha256
        || sha1.is_some_and(|sha1| hex::encode(b.finalize()) != sha1)
        || md5.is_some_and(|md5| hex::encode(c.finalize()) != md5)
    {
        return Err(unavailable());
    }
    Ok(())
}
