use crate::{
    error::{AppError, AppResult},
    safe_fs,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// Recursive byte size. Symlinks are counted but never followed.
pub fn path_size(path: &Path) -> u64 {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum()
}

pub fn copy_dir(src: &Path, dst: &Path) -> AppResult<()> {
    safe_fs::create_dir(dst)?;
    for entry in walkdir::WalkDir::new(src).follow_links(false) {
        let entry = entry?;
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| AppError::Other(e.to_string()))?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        let ft = entry.file_type();
        if ft.is_dir() {
            safe_fs::create_dir(&target)?;
        } else if ft.is_file() {
            atomic_copy(entry.path(), &target)?;
        } else {
            return Err(AppError::Denied(format!(
                "拒绝复制链接或特殊文件: {}",
                crate::paths::display_path(entry.path())
            )));
        }
    }
    Ok(())
}

/// Move without an overwrite on supported desktop OSes. Cross-volume errors are handled
/// by the file engine's journaled copy/retain protocol, never by unlinking here.
pub fn move_path(src: &Path, dst: &Path) -> AppResult<()> {
    safe_fs::rename_exclusive(src, dst)
}

/// Copy into an absent target. Transactions retain intent before this call.
pub fn atomic_copy(src: &Path, dst: &Path) -> AppResult<()> {
    let mut input = safe_fs::open_regular(src)?;
    let mut output = safe_fs::create_new(dst)?;
    std::io::copy(&mut input, &mut output)?;
    output.set_permissions(input.metadata()?.permissions())?;
    output.sync_all()?;
    Ok(())
}

pub fn atomic_write(dst: &Path, bytes: &[u8]) -> AppResult<()> {
    let tmp = dst.with_file_name(new_id(".metadata"));
    let mut f = safe_fs::create_new(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    if dst.exists() {
        let previous = dst.with_file_name(new_id(".previous"));
        move_path(dst, &previous)?;
    }
    move_path(&tmp, dst)?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> AppResult<String> {
    sha256_reader(&mut safe_fs::open_regular(path)?)
}

pub(crate) fn sha256_reader(reader: &mut impl Read) -> AppResult<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

pub fn list_dir_shallow(root: &Path, limit: usize) -> AppResult<Vec<DirEntryInfo>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    let mut entries: Vec<_> = fs::read_dir(root)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries.into_iter().take(limit) {
        let path = entry.path();
        let meta = entry.metadata().ok();
        out.push(DirEntryInfo {
            name: entry.file_name().to_string_lossy().to_string(),
            path: crate::paths::display_path(&path),
            is_dir: meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
            size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
            modified: meta
                .and_then(|m| m.modified().ok())
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                .unwrap_or_default(),
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DirEntryInfo {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
}

pub fn ensure_dirs(paths: &[PathBuf]) -> AppResult<()> {
    for p in paths {
        fs::create_dir_all(p)?;
    }
    Ok(())
}
